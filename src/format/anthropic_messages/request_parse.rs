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
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningLevel, IrRequest, IrRole, IrTool,
        IrToolChoice,
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

    /// Reasoning level container: `effort` parses into the closed shared vocabulary.
    #[serde(default)]
    output_config: Option<InboundOutputConfig>,

    /// Summary control, accepted and ignored: summaries never enter the IR.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<IgnoredAny>,
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
    },

    /// Image segment.
    Image {
        /// Inline data or remote URL source.
        source: InboundImageSource,
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
    },

    /// Tool invocation segment.
    ToolUse {
        /// Invocation identifier.
        id: String,

        /// Invoked function name.
        name: String,

        /// JSON arguments object; the fragment's shape is the caller's to define.
        input: Value,
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
    },

    /// Image segment, known-but-unsupported in tool results.
    Image {
        /// Image source, ignored: the part is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Advertised function tool: function tools only — a server-side tool shape fails on its unknown
/// members.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTool {
    /// Function name the model calls.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of the accepted arguments.
    input_schema: Value,
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

/// Reasoning level container.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundOutputConfig {
    /// Reasoning effort in the closed shared vocabulary.
    effort: String,

    /// Display control, accepted and ignored like its `thinking` sibling.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    display: Option<IgnoredAny>,
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
        .map(|output_config| IrReasoningLevel::parse(&output_config.effort, "output_config"))
        .transpose()?;

    // A disabled legacy control contradicts an explicit effort level; an ignored legacy control
    // beside one is simply redundant.
    if thinking_effort.is_some() && output_effort.is_some() {
        return Err(InvalidRequest::field(
            "thinking",
            "thinking and output_config are mutually exclusive",
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
            response_format: None,
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
        InboundUserBlock::Text { text } => Ok(IrPart::Text { text: text.clone() }),

        InboundUserBlock::Image { source } => Ok(IrPart::Image {
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
        } => {
            let text = match content {
                StringOrArray::Text(text) => text.clone(),

                StringOrArray::Array(parts) => {
                    let mut combined = String::new();

                    for part in parts {
                        match part {
                            InboundToolResultPart::Text { text } => {
                                if !combined.is_empty() {
                                    combined.push_str("\n\n");
                                }

                                combined.push_str(text);
                            }

                            InboundToolResultPart::Image { .. } => {
                                return Err(InvalidRequest::field(
                                    "content",
                                    "Image content inside tool results is not supported",
                                ));
                            }
                        }
                    }

                    combined
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
        InboundAssistantBlock::Text { text } => Ok(IrPart::Text { text: text.clone() }),

        InboundAssistantBlock::ToolUse { id, name, input } => {
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

    /// Rejects unsupported and malformed inbound controls exactly where the contract pins them:
    /// `stop_sequences`, `budget_tokens` (naming the field), `document` blocks, images inside
    /// tool results, a missing `max_tokens`, and unknown members — while `thinking: disabled`
    /// parses into `none` and `enabled`/`adaptive` plus `display` controls are accepted and
    /// ignored.
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
                json!({"model": "m", "max_tokens": 16,
                       "thinking": {"type": "enabled", "budget_tokens": 1000},
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("budget_tokens"),
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
