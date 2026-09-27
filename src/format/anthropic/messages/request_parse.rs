//! Client-request parsing for the Anthropic Messages codec: the inbound `POST /messages` body
//! deserialized into the IR request.
//!
//! The surface parses request bodies strictly into IR (`max_tokens` required per the spec,
//! streaming via body `stream: true`): unknown members are 400s, known semantic controls without
//! gateway support are 400s, and presentation-only controls are accepted-and-ignored
//! (README §6.6).

use super::{TypedBlock, block_opaque};
use crate::{
    format::{ParsedRequest, RoleRun, StringOrArray},
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat,
        IrRole, IrTool, IrToolChoice,
    },
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::Value;

/// Strictly parsed `POST /messages` request body: unknown members are 400s, known semantic
/// controls without gateway support are 400s, and presentation-only controls are
/// accepted-and-ignored (README §6.6).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundRequest {
    /// Requested model, routed against the public model list.
    model: String,

    /// Output token budget, required per the spec.
    max_tokens: u64,

    /// Conversation turns.
    messages: Vec<InboundMessage>,

    /// System instructions: a plain string or an array of text blocks.
    #[serde(default)]
    system: Option<StringOrArray<InboundTextBlock>>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Sampling temperature, passed through verbatim; the upstream decides support.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, passed through verbatim like `temperature`.
    #[serde(default)]
    top_p: Option<f64>,

    /// Top-of-distribution truncation, known-but-unsupported: the IR has no top-k slot, so
    /// present is a 400 naming the field (README §6.6).
    #[serde(default)]
    top_k: Option<IgnoredAny>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<InboundTool>>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<InboundToolChoice>,

    /// Legacy thinking control: `disabled` parses into `none`, `enabled`/`adaptive` are
    /// accepted-and-ignored, and `budget_tokens` is a 400 naming the field.
    #[serde(default)]
    thinking: Option<InboundThinking>,

    /// Reasoning level and output-format container: `effort` parses into the closed shared
    /// vocabulary and `format` into the IR response format, each optional per the official
    /// contract.
    #[serde(default)]
    output_config: Option<InboundOutputConfig>,

    /// Summary control, accepted and ignored: summaries never enter the IR.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<IgnoredAny>,

    /// Request metadata (`metadata.user_id` foremost), accepted and ignored: the gateway is
    /// stateless and never persists request metadata (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    metadata: Option<IgnoredAny>,

    /// Capacity-tier selection, accepted and ignored: a stateless gateway has no tier to honor
    /// downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    service_tier: Option<IgnoredAny>,

    /// Container reuse identifier, accepted and ignored: the gateway is stateless and never
    /// chains containers (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    container: Option<IgnoredAny>,

    /// Top-level prompt-caching breakpoint, accepted and ignored: the stateless gateway renders
    /// no caching markers downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    cache_control: Option<IgnoredAny>,
}

/// One inbound conversation turn, tagged by `role`.
#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
enum InboundMessage {
    /// User turn carrying text, images, and tool results.
    User {
        /// Turn content: a plain string or an array of user blocks.
        content: StringOrArray<InboundUserBlock>,
    },

    /// Assistant turn carrying text, tool calls, and reasoning blocks.
    Assistant {
        /// Turn content: a plain string or an array of assistant blocks.
        content: StringOrArray<InboundAssistantBlock>,
    },
}

/// Content block of a user turn; a member outside the block's own shape is rejected. Placement
/// is enforced by the type itself: `tool_use` and thinking blocks are not user content, so they
/// fail as unknown variants rather than parsing into the wrong turn side.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundUserBlock {
    /// Text segment.
    Text {
        /// The text content.
        text: String,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,

        /// Citations replayed from a citations-enabled upstream turn, accepted and ignored:
        /// the stateless gateway renders no citations downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        citations: Option<IgnoredAny>,
    },

    /// Image segment.
    Image {
        /// Inline data or remote URL source.
        source: InboundImageSource,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },

    /// Tool result segment.
    ToolResult {
        /// Identifier of the invocation being answered.
        tool_use_id: String,

        /// Result content: a plain string or an array of result parts.
        content: StringOrArray<InboundToolResultPart>,

        /// Whether the tool reported an error.
        #[serde(default)]
        is_error: Option<bool>,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },
}

/// Content block of an assistant turn; placement is enforced by the type itself like the user
/// blocks. `document` blocks parse only to be rejected as known-unsupported (README §6.6).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundAssistantBlock {
    /// Text segment.
    Text {
        /// The text content.
        text: String,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,

        /// Citations replayed from a citations-enabled upstream turn, accepted and ignored:
        /// the stateless gateway renders no citations downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        citations: Option<IgnoredAny>,
    },

    /// Tool invocation segment.
    ToolUse {
        /// Invocation identifier.
        id: String,

        /// Invoked function name.
        name: String,

        /// JSON arguments object; the fragment's shape is the caller's to define.
        input: Value,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },

    /// Reasoning segment with its upstream signature.
    Thinking {
        /// Reasoning text.
        thinking: String,

        /// Upstream signature terminating the block.
        #[serde(default)]
        signature: Option<String>,
    },

    /// Opaque redacted reasoning segment.
    RedactedThinking {
        /// Redacted payload, replayed verbatim on same-family upstreams.
        data: Value,
    },

    /// Document segment, known-but-unsupported: the conversion rejects it, and any member
    /// beyond `source` fails even earlier as an unknown field.
    Document {
        /// Document source, ignored: the block is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Text block of the `system` array form.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTextBlock {
    /// Block type tag, always `text` for system content.
    #[serde(rename = "type")]
    kind: String,

    /// The instruction text.
    text: String,

    /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no caching
    /// markers downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    cache_control: Option<IgnoredAny>,

    /// Citations replayed from a citations-enabled upstream turn, accepted and ignored: the
    /// stateless gateway renders no citations downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    citations: Option<IgnoredAny>,
}

/// Image source of an inbound image block.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundImageSource {
    /// Base64-encoded inline data.
    Base64 {
        /// MIME type of the image data.
        media_type: String,

        /// Base64-encoded image bytes.
        data: String,
    },

    /// Remote image referenced by URL.
    Url {
        /// Remote URL.
        url: String,
    },
}

/// Content part of a `tool_result` array: text flattens into the tool output, image content is
/// a 400 (the IR tool output has no media channel).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundToolResultPart {
    /// Text segment.
    Text {
        /// The text content.
        text: String,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,

        /// Citations replayed from a citations-enabled upstream turn, accepted and ignored:
        /// the stateless gateway renders no citations downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        citations: Option<IgnoredAny>,
    },

    /// Image segment, known-but-unsupported in tool results.
    Image {
        /// Image source, ignored: the part is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Advertised function tool: function tools only — a server-side tool discriminator is a 400
/// naming `type`, and a server-side tool shape fails on its unknown members.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTool {
    /// Function name the model calls.
    name: String,

    /// Tool discriminator: `"custom"` marks a function tool; any other value names a
    /// server-side tool and is a 400 naming `type` (README §6.6).
    #[serde(default, rename = "type")]
    kind: Option<String>,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of the accepted arguments.
    input_schema: Value,

    /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no caching
    /// markers downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    cache_control: Option<IgnoredAny>,

    /// Schema-validation guarantee, accepted and ignored: the gateway exposes no such control
    /// downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    strict: Option<bool>,
}

/// Tool choice restriction: `disable_parallel_tool_use` is accepted and ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundToolChoice {
    /// Choice type: `auto`, `any`, `tool`, or `none`.
    #[serde(rename = "type")]
    kind: String,

    /// Required function name for the `tool` type.
    #[serde(default)]
    name: Option<String>,

    /// Parallel tool-use switch, accepted and ignored: the gateway exposes no such control.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    disable_parallel_tool_use: Option<bool>,
}

/// Legacy thinking control.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundThinking {
    /// Control type: `disabled` parses into `none`, `enabled`/`adaptive` are accepted and
    /// ignored.
    #[serde(rename = "type")]
    kind: String,

    /// Legacy numeric budget mechanism, rejected naming the field like Gemini `thinkingBudget`.
    #[serde(default)]
    budget_tokens: Option<IgnoredAny>,

    /// Display control, accepted and ignored: display never enters the IR.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    display: Option<IgnoredAny>,
}

/// The synthesized name of a schema-bearing Anthropic response format: the Messages wire carries
/// no schema-name slot in `output_config.format`, so the IR's named-schema form carries this
/// stand-in — the Gemini surfaces' posture — and the Anthropic upstream render drops it again.
const ANTHROPIC_RESPONSE_SCHEMA_NAME: &str = "response";

/// Reasoning level container — the official `OutputConfig`, whose `effort` and `format` members
/// are each optional.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundOutputConfig {
    /// Reasoning effort in the closed shared vocabulary; optional per the official contract, so
    /// the container may carry a format alone.
    #[serde(default)]
    effort: Option<String>,

    /// Structured-outputs schema, parsed into the IR response format.
    #[serde(default)]
    format: Option<InboundOutputFormat>,

    /// Display control, accepted and ignored like its `thinking` sibling.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    display: Option<IgnoredAny>,
}

/// The inbound `output_config.format` member — the official `JSONOutputFormat`, whose only
/// variant is `json_schema` with both members required. Malformed shapes are validated at the
/// conversion so the rejections name `format` instead of rendering serde's generic wording.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundOutputFormat {
    /// Format discriminator: `"json_schema"` is the official union's only member.
    #[serde(default, rename = "type")]
    kind: Option<String>,

    /// The JSON Schema of the response.
    #[serde(default)]
    schema: Option<Value>,
}

/// Parses a `POST /messages` request body into IR.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON and for malformed, unknown, or unsupported
/// members and values.
pub(super) fn parse_inbound_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: InboundRequest = crate::format::parse_client_request(body)?;

    if request.messages.is_empty() {
        return Err(InvalidRequest::field(
            "messages",
            "messages must contain at least one message",
        ));
    }

    // Known-but-unsupported sampling control: the IR carries no top-k slot, so present is a
    // 400 naming the field rather than the generic unknown-field rejection (README §6.6).
    if request.top_k.is_some() {
        return Err(InvalidRequest::field("top_k", "top_k is not supported"));
    }

    let instructions = match &request.system {
        None => None,

        Some(StringOrArray::Text(system)) => Some(system.clone()),

        Some(StringOrArray::Array(blocks)) => {
            for block in blocks {
                if block.kind != "text" {
                    return Err(InvalidRequest::field(
                        "system",
                        "system blocks must be text",
                    ));
                }
            }

            let combined = blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");

            Some(combined)
        }
    };

    let mut messages: Vec<IrMessage> = Vec::new();

    for message in &request.messages {
        match message {
            InboundMessage::User { content } => {
                // A user turn may mix text/images with tool results; the IR separates the sides,
                // so consecutive same-side parts chunk into one IR message each, in order.
                let mut run = RoleRun::default();

                let parts = match content {
                    StringOrArray::Text(text) => vec![IrPart::Text { text: text.clone() }],

                    StringOrArray::Array(blocks) => {
                        let mut converted = Vec::with_capacity(blocks.len());

                        for block in blocks {
                            converted.push(user_block_to_part(block)?);
                        }

                        converted
                    }
                };

                for part in parts {
                    let role = if matches!(part, IrPart::ToolOutput { .. }) {
                        IrRole::Tool
                    } else {
                        IrRole::User
                    };

                    run.push(role, vec![part]);
                }

                messages.extend(run.finish());
            }

            InboundMessage::Assistant { content } => {
                let parts = match content {
                    StringOrArray::Text(text) => vec![IrPart::Text { text: text.clone() }],

                    StringOrArray::Array(blocks) => {
                        let mut converted = Vec::with_capacity(blocks.len());

                        for block in blocks {
                            converted.push(assistant_block_to_part(block)?);
                        }

                        converted
                    }
                };

                messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts,
                });
            }
        }
    }

    let mut tools = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            // A non-`custom` discriminator names a server-side tool: function tools only, and
            // the rejection names `type` (README §6.6).
            if let Some(other) = tool.kind.as_deref().filter(|kind| *kind != "custom") {
                return Err(InvalidRequest::field(
                    "type",
                    format!("Unsupported tool type `{other}`; function tools only"),
                ));
            }

            if !tool.input_schema.is_object() {
                return Err(InvalidRequest::field(
                    "input_schema",
                    "Tool input_schema must be a JSON object",
                ));
            }

            tools.push(IrTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(tool.input_schema.clone()),
            });
        }
    }

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            crate::format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice.kind.as_str() {
                "auto" => IrToolChoice::Auto,
                "none" => IrToolChoice::None,
                "any" => IrToolChoice::Required,

                "tool" => match &choice.name {
                    Some(name) => IrToolChoice::Function { name: name.clone() },

                    None => {
                        return Err(InvalidRequest::field(
                            "tool_choice",
                            "tool_choice `tool` requires a `name`",
                        ));
                    }
                },

                other => {
                    return Err(InvalidRequest::field(
                        "tool_choice",
                        format!("Unknown tool_choice `{other}`"),
                    ));
                }
            })
        }
    };

    let thinking_effort = match &request.thinking {
        None => None,

        Some(thinking) => {
            if thinking.budget_tokens.is_some() {
                return Err(InvalidRequest::field(
                    "budget_tokens",
                    "budget_tokens is not supported; use output_config.effort",
                ));
            }

            match thinking.kind.as_str() {
                "disabled" => Some(IrReasoningLevel::None),

                // Accepted and ignored: no replayable state, no IR slot.
                "enabled" | "adaptive" => None,

                other => {
                    return Err(InvalidRequest::field(
                        "thinking",
                        format!("Unknown thinking type `{other}`"),
                    ));
                }
            }
        }
    };

    let output_effort = request
        .output_config
        .as_ref()
        .and_then(|output_config| output_config.effort.as_deref())
        .map(|effort| IrReasoningLevel::parse(effort, "output_config"))
        .transpose()?;

    // The structured-outputs member parses into the IR response format; a shape outside the
    // official `JSONOutputFormat` union — any other `type`, a missing or non-object `schema` —
    // is a 400 naming `format` (README §6.6).
    let response_format = match &request.output_config {
        None => None,

        Some(output_config) => match &output_config.format {
            None => None,

            Some(format) => {
                if format.kind.as_deref() != Some("json_schema") {
                    return Err(InvalidRequest::field(
                        "format",
                        "output format requires type `json_schema`",
                    ));
                }

                let Some(schema) = format.schema.as_ref() else {
                    return Err(InvalidRequest::field(
                        "format",
                        "output format `json_schema` requires a `schema`",
                    ));
                };

                if !schema.is_object() {
                    return Err(InvalidRequest::field(
                        "format",
                        "output format schema must be a JSON object",
                    ));
                }

                Some(IrResponseFormat::JsonSchema {
                    name: ANTHROPIC_RESPONSE_SCHEMA_NAME.to_string(),
                    schema: schema.clone(),
                })
            }
        },
    };

    // A disabled legacy control contradicts an explicit effort level; an ignored legacy control
    // beside one is simply redundant.
    if thinking_effort.is_some() && output_effort.is_some() {
        return Err(InvalidRequest::field(
            "thinking",
            "thinking and output_config.effort are mutually exclusive",
        ));
    }

    Ok(ParsedRequest {
        ir: IrRequest {
            model: request.model,
            instructions,
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: Some(request.max_tokens),
            response_format,
            reasoning_effort: output_effort.or(thinking_effort),
        },
        is_streaming: request.stream.unwrap_or(false),
        // Usage rides every Anthropic response unconditionally; no opt-in chunk exists.
        should_include_usage: false,
    })
}

/// Converts one user content block into its IR part.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for image content inside tool results.
fn user_block_to_part(block: &InboundUserBlock) -> Result<IrPart, InvalidRequest> {
    match block {
        InboundUserBlock::Text { text, .. } => Ok(IrPart::Text { text: text.clone() }),

        InboundUserBlock::Image { source, .. } => Ok(IrPart::Image {
            image: match source {
                InboundImageSource::Base64 { media_type, data } => IrImage::Inline {
                    mime_type: media_type.clone(),
                    data_base64: data.clone(),
                },

                InboundImageSource::Url { url } => IrImage::Url(url.clone()),
            },
        }),

        InboundUserBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            let text = match content {
                StringOrArray::Text(text) => text.clone(),

                StringOrArray::Array(parts) => {
                    let texts = parts
                        .iter()
                        .map(|part| match part {
                            InboundToolResultPart::Text { text, .. } => Ok(text.as_str()),

                            InboundToolResultPart::Image { .. } => Err(InvalidRequest::field(
                                "content",
                                "Image content inside tool results is not supported",
                            )),
                        })
                        .collect::<Result<Vec<_>, _>>()?;

                    // The separator only ever separates text the accumulator already holds, so
                    // a leading run of empty parts carries none: skip it, then join the rest
                    // with the blank-line separator.
                    texts
                        .iter()
                        .skip_while(|text| text.is_empty())
                        .copied()
                        .collect::<Vec<&str>>()
                        .join("\n\n")
                }
            };

            Ok(IrPart::ToolOutput {
                call_id: tool_use_id.clone(),
                content: text,
                is_error: is_error.unwrap_or(false),
            })
        }
    }
}

/// Converts one assistant content block into its IR part: thinking blocks keep their verbatim
/// replay payload so same-family upstreams receive them byte-exact.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for `document` blocks and for tool arguments that do not encode a
/// JSON object.
fn assistant_block_to_part(block: &InboundAssistantBlock) -> Result<IrPart, InvalidRequest> {
    match block {
        InboundAssistantBlock::Text { text, .. } => Ok(IrPart::Text { text: text.clone() }),

        InboundAssistantBlock::ToolUse {
            id, name, input, ..
        } => {
            if !input.is_object() {
                return Err(InvalidRequest::field(
                    "input",
                    "Tool use input must be a JSON object",
                ));
            }

            // The input is a proven object, so its rendered arguments are one by construction;
            // no re-validation of the rendered text.
            let arguments = crate::format::render_json(input);

            Ok(IrPart::ToolCall {
                call_id: id.clone(),
                name: name.clone(),
                arguments,
                thought_signature: None,
            })
        }

        InboundAssistantBlock::Thinking {
            thinking,
            signature,
        } => {
            let block = TypedBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            };

            Ok(IrPart::Reasoning {
                text: thinking.clone(),
                signature: signature.clone(),
                opaque: block_opaque(&block),
            })
        }

        InboundAssistantBlock::RedactedThinking { data } => {
            let block = TypedBlock::RedactedThinking { data: data.clone() };

            Ok(IrPart::Reasoning {
                text: String::new(),
                signature: None,
                opaque: block_opaque(&block),
            })
        }

        InboundAssistantBlock::Document { .. } => Err(InvalidRequest::field(
            "content",
            "document blocks are not supported",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Reads one parsed message part by position.
    ///
    /// # Panics
    ///
    /// Panics if the position is absent: a missing part is the failure under test.
    fn part(parts: &[IrPart], index: usize) -> &IrPart {
        #[expect(
            clippy::expect_used,
            reason = "parsed messages are under test; a missing part is the failure"
        )]
        parts
            .get(index)
            .expect("the parsed message carries the expected part")
    }
    /// Sets one member of a JSON object fixture body.
    ///
    /// # Panics
    ///
    /// Panics if the body is not an object, which cannot occur for the fixtures.
    fn set_member(body: &mut Value, key: &str, value: Value) {
        #[expect(
            clippy::expect_used,
            reason = "fixture bodies are objects by construction"
        )]
        body.as_object_mut()
            .expect("fixture bodies are JSON objects")
            .insert(key.to_string(), value);
    }
    /// Parses one inbound body covering every content shape: system string, text and both image
    /// sources, tool calls with object inputs, error tool results, and thinking blocks whose
    /// opaque payload replays byte-exact upstream.
    ///
    /// # Panics
    ///
    /// Panics if the parsed IR diverges from the documented mapping.
    #[test]
    fn parses_every_inbound_content_shape() {
        let body = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "stream": true,
            "system": "be terse",
            "temperature": 0.7,
            "top_p": 0.9,
            "output_config": {"effort": "high"},
            "tools": [{
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
            }],
            "tool_choice": {"type": "tool", "name": "get_weather"},
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "image", "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "aGVsbG8=",
                    }},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                    }},
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "check", "signature": "sig"},
                    {"type": "redacted_thinking", "data": "enc"},
                    {"type": "text", "text": "looking"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": "sunny", "is_error": true},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed =
            parse_inbound_request(body.to_string().as_bytes()).expect("every content shape parses");

        assert!(parsed.is_streaming);
        assert_eq!(parsed.ir.model, "claude-test");
        assert_eq!(parsed.ir.max_output_tokens, Some(128));
        assert_eq!(parsed.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(parsed.ir.temperature, Some(0.7));
        assert_eq!(parsed.ir.top_p, Some(0.9));
        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningLevel::High),);
        assert_eq!(
            parsed.ir.tools,
            vec![IrTool {
                name: "get_weather".to_string(),
                description: Some("City weather".to_string()),
                parameters: Some(json!({"type": "object"})),
            }],
        );
        assert_eq!(
            parsed.ir.tool_choice,
            Some(IrToolChoice::Function {
                name: "get_weather".to_string(),
            }),
        );

        assert_eq!(parsed.ir.messages.len(), 3);

        #[expect(clippy::expect_used, reason = "the fixture carries three messages")]
        let user = parsed.ir.messages.first().expect("the user message parses");

        assert_eq!(user.role, IrRole::User);
        assert_eq!(user.parts.len(), 3);
        assert!(matches!(
            part(&user.parts, 1),
            IrPart::Image {
                image: IrImage::Inline { .. },
            },
        ));
        assert_eq!(
            part(&user.parts, 2),
            &IrPart::Image {
                image: IrImage::Url("https://example.com/i.png".to_string()),
            },
        );

        #[expect(
            clippy::expect_used,
            reason = "the fixture carries an assistant message"
        )]
        let assistant = parsed
            .ir
            .messages
            .get(1)
            .expect("the assistant message parses");

        assert_eq!(assistant.parts.len(), 4);
        assert_eq!(
            part(&assistant.parts, 2),
            &IrPart::Text {
                text: "looking".to_string(),
            },
        );
        assert_eq!(
            part(&assistant.parts, 3),
            &IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments: r#"{"city":"Paris"}"#.to_string(),
                thought_signature: None,
            },
        );

        // The thinking block keeps its verbatim replay payload: the opaque value equals the
        // inbound block exactly.
        assert_eq!(
            part(&assistant.parts, 0),
            &IrPart::Reasoning {
                text: "check".to_string(),
                signature: Some("sig".to_string()),
                opaque: json!({"type": "thinking", "thinking": "check",
                                "signature": "sig"}),
            },
        );

        #[expect(clippy::expect_used, reason = "the fixture carries a tool message")]
        let tool = parsed.ir.messages.get(2).expect("the tool message parses");

        assert_eq!(tool.role, IrRole::Tool);
        assert_eq!(
            tool.parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "sunny".to_string(),
                is_error: true,
            }],
        );
    }

    /// A `tool_result` content-parts array flattens into the tool output with the blank-line
    /// separator: parts join in order, an empty part between texts keeps both its separators,
    /// and a leading run of empty parts carries none (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed tool outputs diverge from the documented flattening.
    #[test]
    fn tool_result_text_part_arrays_flatten_into_the_tool_output() {
        let between_empty = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": [
                        {"type": "text", "text": "a"},
                        {"type": "text", "text": ""},
                        {"type": "text", "text": "b"},
                    ]},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the between-empty parts form is constructed to parse"
        )]
        let parsed = parse_inbound_request(between_empty.to_string().as_bytes())
            .expect("tool_result content parts must parse");

        #[expect(clippy::expect_used, reason = "the body parses to one message")]
        let tool = parsed.ir.messages.first().expect("the tool message parses");

        assert_eq!(tool.role, IrRole::Tool);

        assert_eq!(
            part(&tool.parts, 0),
            &IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "a\n\n\n\nb".to_string(),
                is_error: false,
            },
        );

        let leading_empty = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": [
                        {"type": "text", "text": ""},
                        {"type": "text", "text": "c"},
                    ]},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the leading-empty parts form is constructed to parse"
        )]
        let parsed = parse_inbound_request(leading_empty.to_string().as_bytes())
            .expect("tool_result content parts must parse");

        #[expect(clippy::expect_used, reason = "the body parses to one message")]
        let tool = parsed.ir.messages.first().expect("the tool message parses");

        assert_eq!(
            part(&tool.parts, 0),
            &IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "c".to_string(),
                is_error: false,
            },
        );
    }

    /// Official envelope and caching controls parse to the same IR as their absence: `metadata`
    /// (`user_id` foremost), `service_tier`, `container`, the top-level `cache_control`,
    /// `cache_control` on tools and on text/image blocks in user/assistant/system positions and
    /// on `tool_use`/`tool_result` blocks and `tool_result` text parts, and the tool `strict`
    /// flag are accepted-and-ignored (README §6.6) and never enter the IR, and the official
    /// `type: "custom"` tool discriminator parses like its absence.
    ///
    /// # Panics
    ///
    /// Panics if either body fails to parse or the two IRs diverge.
    #[test]
    fn accepts_and_ignores_official_envelope_and_caching_controls() {
        let plain = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "system": [{"type": "text", "text": "be terse"}],
            "tools": [{
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
            }],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                    }},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny"}]},
                ]},
            ],
        });

        let decorated = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "metadata": {"user_id": "user_abc"},
            "service_tier": "auto",
            "container": {"id": "cntr_1", "skills": []},
            "cache_control": {"type": "ephemeral"},
            "system": [{"type": "text", "text": "be terse",
                        "cache_control": {"type": "ephemeral"}}],
            "tools": [{
                "type": "custom",
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
                "cache_control": {"type": "ephemeral"},
                "strict": true,
            }],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?", "cache_control": {"type": "ephemeral"}},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                     }, "cache_control": {"type": "ephemeral"}},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun", "cache_control": {"type": "ephemeral"}},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}, "cache_control": {"type": "ephemeral"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny",
                                  "cache_control": {"type": "ephemeral"}}],
                     "cache_control": {"type": "ephemeral"}},
                ]},
            ],
        });

        #[expect(clippy::expect_used, reason = "the plain body is constructed to parse")]
        let plain =
            parse_inbound_request(plain.to_string().as_bytes()).expect("the plain body parses");

        #[expect(
            clippy::expect_used,
            reason = "the decorated body is constructed to parse"
        )]
        let decorated = parse_inbound_request(decorated.to_string().as_bytes())
            .expect("the decorated body parses");

        // The decoration carries no semantics, so both parses land on the same IR, byte for
        // byte across every IR member.
        assert_eq!(plain.ir.model, decorated.ir.model);
        assert_eq!(plain.ir.instructions, decorated.ir.instructions);
        assert_eq!(plain.ir.messages, decorated.ir.messages);
        assert_eq!(plain.ir.tools, decorated.ir.tools);
        assert_eq!(plain.ir.tool_choice, decorated.ir.tool_choice);
        assert_eq!(plain.ir.temperature, decorated.ir.temperature);
        assert_eq!(plain.ir.top_p, decorated.ir.top_p);
        assert_eq!(plain.ir.max_output_tokens, decorated.ir.max_output_tokens);
        assert_eq!(plain.ir.response_format, decorated.ir.response_format);
        assert_eq!(plain.ir.reasoning_effort, decorated.ir.reasoning_effort);
        assert_eq!(plain.is_streaming, decorated.is_streaming);

        // Sanity: the shared system array and tool list parse to their expected IR values, so
        // the parity above compares real content rather than two empty shells.
        assert_eq!(plain.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(
            plain.ir.tools,
            vec![IrTool {
                name: "get_weather".to_string(),
                description: Some("City weather".to_string()),
                parameters: Some(json!({"type": "object"})),
            }],
        );
    }

    /// Text-block `citations` — the official replay member on every request text shape —
    /// parse to the same IR as their absence on all four shapes (user text, assistant text,
    /// system text block, `tool_result` text part): typed clients replay citations-bearing
    /// history verbatim, and the stateless gateway renders no citations downstream
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if either body fails to parse or the two IRs diverge.
    #[test]
    fn accepts_and_ignores_text_block_citations() {
        let plain = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "system": [{"type": "text", "text": "be terse"}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny"}]},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun"},
                ]},
            ],
        });

        let citations = json!([{
            "type": "char_location",
            "cited_text": "the sky is blue",
            "document_index": 0,
            "document_title": "weather",
            "start_char_index": 0,
            "end_char_index": 15,
        }]);

        let decorated = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "system": [{"type": "text", "text": "be terse", "citations": citations}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?", "citations": citations},
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny",
                                  "citations": citations}]},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun", "citations": citations},
                ]},
            ],
        });

        #[expect(clippy::expect_used, reason = "the plain body is constructed to parse")]
        let plain =
            parse_inbound_request(plain.to_string().as_bytes()).expect("the plain body parses");

        #[expect(
            clippy::expect_used,
            reason = "the decorated body is constructed to parse"
        )]
        let decorated = parse_inbound_request(decorated.to_string().as_bytes())
            .expect("the decorated body parses");

        // The citations carry no semantics, so both parses land on the same IR, byte for
        // byte across every IR member.
        assert_eq!(plain.ir.model, decorated.ir.model);
        assert_eq!(plain.ir.instructions, decorated.ir.instructions);
        assert_eq!(plain.ir.messages, decorated.ir.messages);
        assert_eq!(plain.ir.tools, decorated.ir.tools);
        assert_eq!(plain.ir.tool_choice, decorated.ir.tool_choice);
        assert_eq!(plain.ir.temperature, decorated.ir.temperature);
        assert_eq!(plain.ir.top_p, decorated.ir.top_p);
        assert_eq!(plain.ir.max_output_tokens, decorated.ir.max_output_tokens);
        assert_eq!(plain.ir.response_format, decorated.ir.response_format);
        assert_eq!(plain.ir.reasoning_effort, decorated.ir.reasoning_effort);
        assert_eq!(plain.is_streaming, decorated.is_streaming);

        // Sanity: the shared system array and the text/tool-output/assistant runs parse to
        // their expected IR values, so the parity above compares real content rather than
        // two empty shells.
        assert_eq!(plain.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(plain.ir.messages.len(), 3);

        #[expect(clippy::expect_used, reason = "the body carries a tool message")]
        let tool = plain.ir.messages.get(1).expect("the tool message parses");

        assert_eq!(
            part(&tool.parts, 0),
            &IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "sunny".to_string(),
                is_error: false,
            },
        );
    }

    /// `output_config.format` parses into the IR JSON-schema response format — the official
    /// `JSONOutputFormat` union carries no schema-name slot, so the IR `name` synthesizes the
    /// surface stand-in — while `effort` is optional beside it (the official `OutputConfig`
    /// carries both its members optional), `format: null` and an empty container parse as no
    /// constraint, and the malformed shapes — a non-`json_schema` `type`, a missing `schema`,
    /// a non-object `schema` — are 400s naming `format` (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if any parse diverges from the documented mapping or a rejection lands outside
    /// its documented wording.
    #[test]
    fn parses_output_config_format_and_optional_effort() {
        let base = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            })
        };

        let schema = json!({"type": "object", "properties": {"city": {"type": "string"}}});

        let expected_format = |schema: Value| {
            Some(IrResponseFormat::JsonSchema {
                name: ANTHROPIC_RESPONSE_SCHEMA_NAME.to_string(),
                schema,
            })
        };

        for (output_config, expected_format, expected_effort) in [
            (
                json!({"effort": "high",
                       "format": {"type": "json_schema", "schema": schema.clone()}}),
                expected_format(schema.clone()),
                Some(IrReasoningLevel::High),
            ),
            (
                json!({"format": {"type": "json_schema", "schema": schema.clone()}}),
                expected_format(schema),
                None,
            ),
            (json!({"format": null}), None, None),
            (json!({}), None, None),
        ] {
            let mut body = base();

            set_member(&mut body, "output_config", output_config);

            #[expect(
                clippy::expect_used,
                reason = "each output_config shape is constructed to parse"
            )]
            let parsed = parse_inbound_request(body.to_string().as_bytes())
                .expect("the output_config shape parses");

            assert_eq!(parsed.ir.response_format, expected_format);
            assert_eq!(parsed.ir.reasoning_effort, expected_effort);
        }

        // The malformed shapes are 400s naming `format` (README §6.6).
        for format in [
            json!({"type": "text"}),
            json!({"type": "json_schema"}),
            json!({"type": "json_schema", "schema": "x"}),
            json!({"type": "unknown", "schema": {}}),
        ] {
            let mut body = base();

            set_member(&mut body, "output_config", json!({"format": format}));

            #[expect(
                clippy::expect_used,
                reason = "each malformed format is constructed to be rejected"
            )]
            let error = parse_inbound_request(body.to_string().as_bytes())
                .expect_err("the malformed format must be rejected");

            assert_eq!(error.param.as_deref(), Some("format"));
        }
    }

    /// Rejects unsupported and malformed inbound controls exactly where the contract pins them:
    /// `stop_sequences`, `budget_tokens` and `top_k` (naming the field), `document` blocks,
    /// images inside tool results, a missing `max_tokens`, a non-`custom` tool `type`, and
    /// unknown members — while `thinking: disabled` parses into `none` and
    /// `enabled`/`adaptive` plus `display` controls are accepted and ignored.
    ///
    /// # Panics
    ///
    /// Panics if any rejection lands outside its documented wording.
    #[test]
    fn rejects_unsupported_controls_and_honors_thinking_dispositions() {
        let base = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            })
        };

        let cases: Vec<(Value, Option<&str>)> = vec![
            (
                json!({"stop_sequences": ["x"], "model": "m", "max_tokens": 16,
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16, "tools": [
                    {"type": "web_search_20250305", "name": "ws",
                     "input_schema": {"type": "object"}},
                ], "messages": [{"role": "user", "content": "hi"}]}),
                Some("type"),
            ),
            (
                json!({"model": "m", "max_tokens": 16,
                       "thinking": {"type": "enabled", "budget_tokens": 1000},
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("budget_tokens"),
            ),
            (
                json!({"top_k": 40, "model": "m", "max_tokens": 16,
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("top_k"),
            ),
            (
                json!({"model": "m", "max_tokens": 16, "messages": [
                    {"role": "assistant", "content": [
                        {"type": "document", "source": {"type": "text", "data": "x"}},
                    ]},
                ]}),
                Some("content"),
            ),
            (
                json!({"model": "m", "max_tokens": 16, "messages": [
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "c",
                         "content": [{"type": "image",
                                      "source": {"type": "url",
                                                 "url": "https://example.com/i.png"}}]},
                    ]},
                ]}),
                Some("content"),
            ),
            (
                json!({"model": "m",
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16, "bogus": true,
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
        ];

        for (mut body, param) in cases {
            // Every case starts from the valid base shape unless it already states its own.
            if body.get("messages").is_none() {
                body = base();
            }

            #[expect(
                clippy::expect_used,
                reason = "each control is constructed to be rejected"
            )]
            let error = parse_inbound_request(body.to_string().as_bytes())
                .expect_err("the unsupported control must be rejected");

            assert_eq!(error.param.as_deref(), param, "wrong param for {error}");
        }

        // The `top_k` rejection is the named known-unsupported row, not the generic
        // unknown-field accident: the 400 pins the field and its fixed message (README §6.6).
        let mut body = base();

        set_member(&mut body, "top_k", json!(40));

        #[expect(clippy::expect_used, reason = "top_k is constructed to be rejected")]
        let error =
            parse_inbound_request(body.to_string().as_bytes()).expect_err("top_k must be rejected");

        assert_eq!(error.to_string(), "top_k is not supported");

        // `thinking: disabled` parses into `none`; `enabled`/`adaptive` and both `display`
        // controls parse without entering the IR.
        for (thinking, output_config, expected) in [
            (
                Some(json!({"type": "disabled"})),
                None,
                Some(IrReasoningLevel::None),
            ),
            (Some(json!({"type": "enabled"})), None, None),
            (Some(json!({"type": "adaptive"})), None, None),
            (
                Some(json!({"type": "enabled", "display": "summary"})),
                None,
                None,
            ),
            (
                None,
                Some(json!({"effort": "low", "display": "none"})),
                Some(IrReasoningLevel::Low),
            ),
        ] {
            let mut body = base();

            if let Some(thinking) = thinking {
                set_member(&mut body, "thinking", thinking);
            }

            if let Some(output_config) = output_config {
                set_member(&mut body, "output_config", output_config);
            }

            set_member(&mut body, "thinking_summaries", json!(["summary"]));

            #[expect(
                clippy::expect_used,
                reason = "each thinking disposition is constructed to parse"
            )]
            let parsed = parse_inbound_request(body.to_string().as_bytes())
                .expect("the thinking disposition parses");

            assert_eq!(parsed.ir.reasoning_effort, expected);
        }

        // An unknown effort string is a 400 naming `output_config`.
        let mut body = base();

        set_member(&mut body, "output_config", json!({"effort": "ultra"}));

        #[expect(
            clippy::expect_used,
            reason = "the unknown effort is constructed to be rejected"
        )]
        let error = parse_inbound_request(body.to_string().as_bytes())
            .expect_err("an unknown effort must be rejected");

        assert_eq!(error.param.as_deref(), Some("output_config"));
    }
}
