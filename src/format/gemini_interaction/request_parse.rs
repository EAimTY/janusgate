//! Client-request parsing for the Gemini Interactions codec: the inbound `POST /interactions`
//! body deserialized into the IR request, speaking the wire vocabulary the parent module fixes.
//!
//! The endpoint handler lives in [`crate::api::gemini_v1beta::interactions`]; shared prefix
//! concerns (authentication, 404/405 bodies, body limit, accounting row id) live beside it.
//!
//! # Surface contract
//!
//! - The model rides the request **body** (`model`): `POST /interactions` has no URL model slot, so
//!   the surface addresses models in the body like the OpenAI surfaces do. The router threads
//!   [`RouteContext`](crate::format::RouteContext) into parsing carrying only the streaming
//!   override: `?alt=sse` forces streaming, otherwise the body's `stream` member decides.
//! - The body is parsed strictly: unknown members are 400s, and the known-but-unsupported controls
//!   each reject naming their field — `previous_interaction_id`, `background`, and `last_event_id`
//!   (the gateway never chains); `thinkingBudget` and `budget_tokens` (legacy numeric mechanisms);
//!   `stop` and `stop_sequences`; `seed` and `modalities`.
//! - Presentation-only controls are accepted and ignored: `store` (in whatever shape it arrives)
//!   and `thinking_summaries`.
//! - Reasoning enters the IR through `thought` steps, whose complete step JSON is stored verbatim
//!   as the block's opaque replay payload; cross-family blocks never originate here. Tool calls
//!   replay through `function_call` steps carrying their ids and signatures.
//! - `function_result` steps accept a plain string, a text-block array (image blocks are a 400 —
//!   the IR tool output has no media channel), or the tool's own result object, flattened to JSON
//!   text. There is no `is_error` member here: that flag belongs to the Anthropic pair alone.
//! - `tool_choice` accepts the `auto`/`any`/`none` modes as a bare string or as an object with a
//!   single `allowed_tools` entry; `validated` and multiple allowed tools are 400s, as is a choice
//!   without advertised tools. Built-in/server-side tools are 400s — function tools only.

use super::ResponseFormatShape;
use crate::{
    format::{self, ParsedRequest, RouteContext},
    ir::{
        InvalidRequest, IrImage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat, IrRole,
        IrTool, IrToolChoice,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Strictly parsed Interactions request body.
///
/// Unknown members are rejected outright; the known-but-unsupported controls below are modelled
/// explicitly so their rejections name the offending field instead of rendering serde's generic
/// unknown-field wording.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model id to route; the Interactions API addresses models in the body.
    model: String,

    /// Conversation input as typed steps.
    input: Vec<StepIn>,

    /// System instructions.
    #[serde(default)]
    system_instruction: Option<String>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<ToolIn>>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<ToolChoiceIn>,

    /// Sampling temperature.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold.
    #[serde(default)]
    top_p: Option<f64>,

    /// Output token budget.
    #[serde(default)]
    max_output_tokens: Option<u64>,

    /// Reasoning level in the closed shared vocabulary.
    #[serde(default)]
    thinking_level: Option<String>,

    /// Response format restriction.
    #[serde(default)]
    response_format: Option<ResponseFormatShape>,

    /// Whether the client wants a streamed response; the `?alt=sse` query overrides it.
    #[serde(default)]
    stream: Option<bool>,

    /// Storage request; the gateway is stateless, so the member is accepted and ignored in
    /// whatever shape it arrives.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    store: Option<serde::de::IgnoredAny>,

    /// Summary verbosity hint; the IR has no summary channel, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<serde::de::IgnoredAny>,

    /// Chained interaction resume; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    previous_interaction_id: Option<serde::de::IgnoredAny>,

    /// Background execution; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    background: Option<serde::de::IgnoredAny>,

    /// Resume cursor; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    last_event_id: Option<serde::de::IgnoredAny>,

    /// Legacy numeric reasoning budget; a present member is a 400 naming the field. The member
    /// keeps its camel-case wire spelling through the rename.
    #[serde(default, rename = "thinkingBudget")]
    thinking_budget: Option<serde::de::IgnoredAny>,

    /// Legacy numeric reasoning budget under its alternate spelling; a present member is a 400
    /// naming the field.
    #[serde(default)]
    budget_tokens: Option<serde::de::IgnoredAny>,

    /// Stop sequences; known-unsupported, so a present member is a 400.
    #[serde(default)]
    stop: Option<serde::de::IgnoredAny>,

    /// Stop sequences under the plural spelling; a present member is a 400.
    #[serde(default)]
    stop_sequences: Option<serde::de::IgnoredAny>,

    /// Sampling seed; out of scope, so a present member is a 400.
    #[serde(default)]
    seed: Option<serde::de::IgnoredAny>,

    /// Modality configuration; out of scope, so a present member is a 400.
    #[serde(default)]
    modalities: Option<serde::de::IgnoredAny>,
}

/// One input step, tagged by `type`.
///
/// The shapes are strict: a member outside the step's own shape is rejected. The gateway's own
/// rendered responses replay through these same shapes — `output_text` and `thought` steps it
/// emitted, `function_call` steps carrying the ids and signatures it minted — so replaying them
/// verbatim parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum StepIn {
    /// A user turn.
    UserInput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlockIn>,
    },

    /// A model text turn, replayed from an earlier response.
    OutputText {
        /// The text content.
        text: String,
    },

    /// A model reasoning block: its summary text plus the signature terminating it. The complete
    /// step JSON becomes the block's opaque replay payload, so the step replays byte-exact to
    /// the Interactions upstream.
    Thought {
        /// Step identifier, round-tripped verbatim.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,

        /// Reasoning text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,

        /// Upstream signature terminating the block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// A past tool invocation, carrying its id and signature for cross-turn matching.
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// JSON-encoded arguments, or the arguments object itself.
        arguments: CallArgumentsIn,

        /// Upstream thought signature to reattach across turns.
        #[serde(default)]
        signature: Option<String>,
    },

    /// A tool result for a prior function call.
    FunctionResult {
        /// Name of the function being answered.
        name: String,

        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content: a plain string, a text-block array, or the tool's own result object
        /// flattened to JSON text.
        result: FunctionResultPayload,
    },
}

/// One content block of a `user_input` step or a `function_result` block array, tagged by `type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ContentBlockIn {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },

    /// Image content. The URL is optional in the shape so the conversion — not serde's
    /// shape check — owns the rejection: images are accepted in user turns and rejected in tool
    /// results, and both rejections must name their field.
    Image {
        /// Remote or data URL of the image; required in user turns.
        #[serde(default)]
        image_url: Option<String>,
    },
}

/// The two accepted spellings of function-call arguments: the JSON-encoded text form and the
/// arguments object itself. Both normalize to the encoded-text form the IR carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum CallArgumentsIn {
    /// The JSON-encoded text form.
    Text(String),

    /// The arguments object form.
    Object(Map<String, Value>),
}

impl CallArgumentsIn {
    /// Normalizes either spelling to the JSON-encoded text the IR carries, validating that the
    /// text encodes an object — in parity with the OpenAI surfaces, which validate tool-call
    /// arguments at parse time.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `arguments` for text that is not a JSON object.
    fn into_text(self) -> Result<String, InvalidRequest> {
        match self {
            Self::Text(text) => {
                format::arguments_as_object(&text)?;

                Ok(text)
            }

            Self::Object(map) => Ok(format::arguments_object_to_text(map)),
        }
    }
}

/// The three accepted shapes of a `function_result` payload: a plain string, a text-block array,
/// or the tool's own result object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum FunctionResultPayload {
    /// The plain-string form.
    Text(String),

    /// The block-array form.
    Blocks(Vec<ContentBlockIn>),

    /// The tool's own result object form, flattened to JSON text.
    Object(Map<String, Value>),
}

impl FunctionResultPayload {
    /// Flattens any shape into the tool-output text the IR carries: strings ride verbatim, text
    /// blocks join with the blank-line separator the other codecs use, and objects render as
    /// their JSON text. Image blocks are rejected: the IR tool output has no media channel, so
    /// silently dropping one would lose request content.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `result` for image blocks.
    fn into_text(self) -> Result<String, InvalidRequest> {
        match self {
            Self::Text(text) => Ok(text),

            Self::Blocks(blocks) => {
                let mut combined = String::new();

                for block in blocks {
                    match block {
                        ContentBlockIn::Text { text } => {
                            if !combined.is_empty() {
                                combined.push_str("\n\n");
                            }

                            combined.push_str(&text);
                        }

                        ContentBlockIn::Image { .. } => {
                            return Err(InvalidRequest::field(
                                "result",
                                "function_result content blocks must be text; \
                                 `image` is not supported",
                            ));
                        }
                    }
                }

                Ok(combined)
            }

            // Rendering a JSON object map cannot fail: the value holds finite numbers only.
            Self::Object(map) => Ok(Value::Object(map).to_string()),
        }
    }
}

/// Advertised function tool: function tools only.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIn {
    /// Tool kind; only `function` is supported.
    #[serde(rename = "type")]
    tool_type: String,

    /// Function name the model calls.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of the accepted arguments.
    #[serde(default)]
    parameters: Option<Value>,
}

/// Tool choice: a bare mode name, or an object pairing a mode with a single allowed function.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolChoiceIn {
    /// The bare mode form (`auto`, `any`, `none`).
    Mode(String),

    /// The object form: a mode with an optional single allowed function.
    Object {
        /// Selection mode.
        mode: String,

        /// The one function the model may call.
        #[serde(default)]
        allowed_tools: Option<Vec<String>>,
    },
}

/// Parses an Interactions request body into IR with the router's route context: the URL carries
/// no model on this surface, so the body must name it; only the streaming mode may arrive from
/// the route (`?alt=sse` overriding the body's `stream` member).
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed or unsupported requests.
///
/// # Panics
///
/// Panics if re-serializing a parsed thought step fails, which cannot occur for the strictly
/// parsed first-party shape.
pub(super) fn parse_request(
    body: &[u8],
    route: &RouteContext,
) -> Result<ParsedRequest, InvalidRequest> {
    let request: RequestBody = format::parse_client_request(body)?;

    reject_present_fields(&request)?;
    format::validate_sampling_ranges(request.temperature, request.top_p)?;

    // Consecutive same-role steps coalesce into one message per run, so a text turn followed by
    // its thought step and tool calls stays one assistant message.
    let mut run = format::RoleRun::default();

    for step in request.input {
        match step {
            StepIn::UserInput { content } => {
                let mut parts = Vec::with_capacity(content.len());

                for block in content {
                    match block {
                        ContentBlockIn::Text { text } => {
                            parts.push(IrPart::Text { text });
                        }

                        ContentBlockIn::Image { image_url } => {
                            let Some(image_url) = image_url else {
                                return Err(InvalidRequest::field(
                                    "content",
                                    "user_input image blocks require an `image_url`",
                                ));
                            };

                            parts.push(IrPart::Image {
                                image: IrImage::from_url(&image_url)?,
                            });
                        }
                    }
                }

                run.push(IrRole::User, parts);
            }

            StepIn::OutputText { text } => {
                run.push(IrRole::Assistant, vec![IrPart::Text { text }]);
            }

            StepIn::Thought {
                id,
                summary,
                signature,
            } => {
                // The opaque payload is the step's own complete JSON: strict parsing guarantees
                // no member escapes the typed shape, so re-serializing it preserves the value
                // exactly.
                #[expect(
                    clippy::expect_used,
                    reason = "the parsed step shape is a first-party type and serializes totally"
                )]
                let opaque = serde_json::to_value(StepIn::Thought {
                    id,
                    summary: summary.clone(),
                    signature: signature.clone(),
                })
                .expect("the parsed thought shape serializes");

                run.push(
                    IrRole::Assistant,
                    vec![IrPart::Reasoning {
                        text: summary.unwrap_or_default(),
                        signature,
                        opaque,
                    }],
                );
            }

            StepIn::FunctionCall {
                id,
                name,
                arguments,
                signature,
            } => {
                run.push(
                    IrRole::Assistant,
                    vec![IrPart::ToolCall {
                        call_id: id,
                        name,
                        arguments: arguments.into_text()?,
                        thought_signature: signature,
                    }],
                );
            }

            StepIn::FunctionResult {
                // The name is accepted for wire compatibility but not preserved: the IR tool
                // output carries no name, and the upstream render resolves it from the matching
                // call instead.
                name: _,
                call_id,
                result,
            } => {
                run.push(
                    IrRole::Tool,
                    vec![IrPart::ToolOutput {
                        call_id,
                        content: result.into_text()?,
                        is_error: false,
                    }],
                );
            }
        }
    }

    let messages = run.finish();

    let mut tools: Vec<IrTool> = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            format::reject_non_function_type(&tool.tool_type, "tool")?;

            tools.push(IrTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            });
        }
    }

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(parse_tool_choice(choice)?)
        }
    };

    let response_format = match request.response_format {
        None => None,
        Some(ResponseFormatShape::Text {}) => Some(IrResponseFormat::Text),
        Some(ResponseFormatShape::Json {}) => Some(IrResponseFormat::Json),

        Some(ResponseFormatShape::JsonSchema { name, schema }) => {
            Some(IrResponseFormat::JsonSchema { name, schema })
        }
    };

    let reasoning_effort = request
        .thinking_level
        .map(|level| IrReasoningLevel::parse(&level, "thinking_level"))
        .transpose()?;

    Ok(ParsedRequest {
        ir: IrRequest {
            // The model rides the body on this surface: `POST /interactions` has no URL model
            // slot, so the route context carries no public id and the body's member decides.
            model: request.model,
            instructions: request.system_instruction,
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_output_tokens,
            response_format,
            reasoning_effort,
        },
        // The `?alt=sse` query forces streaming; otherwise the body's `stream` member decides.
        is_streaming: route
            .is_streaming
            .unwrap_or(request.stream.unwrap_or(false)),
        // Usage is always included on this surface; only Chat Completions exposes the option.
        should_include_usage: false,
    })
}

/// Rejects every known-but-unsupported control the request carries, each naming its own field.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first present control.
fn reject_present_fields(request: &RequestBody) -> Result<(), InvalidRequest> {
    // Each control rejects with the wording its disposition row documents: chaining controls name
    // the statelessness invariant, numeric budgets name the level vocabulary that replaces them.
    if request.previous_interaction_id.is_some() {
        return Err(InvalidRequest::field(
            "previous_interaction_id",
            "The gateway is stateless; previous_interaction_id is not supported",
        ));
    }

    if request.background.is_some() {
        return Err(InvalidRequest::field(
            "background",
            "The gateway is stateless; background is not supported",
        ));
    }

    if request.last_event_id.is_some() {
        return Err(InvalidRequest::field(
            "last_event_id",
            "The gateway is stateless; last_event_id is not supported",
        ));
    }

    if request.thinking_budget.is_some() {
        return Err(InvalidRequest::field(
            "thinkingBudget",
            "thinkingBudget is not supported; use thinking_level",
        ));
    }

    if request.budget_tokens.is_some() {
        return Err(InvalidRequest::field(
            "budget_tokens",
            "budget_tokens is not supported; use thinking_level",
        ));
    }

    if request.stop.is_some() {
        return Err(InvalidRequest::field("stop", "stop is not supported"));
    }

    if request.stop_sequences.is_some() {
        return Err(InvalidRequest::field(
            "stop_sequences",
            "stop_sequences is not supported",
        ));
    }

    if request.seed.is_some() {
        return Err(InvalidRequest::field("seed", "seed is not supported"));
    }

    if request.modalities.is_some() {
        return Err(InvalidRequest::field(
            "modalities",
            "modalities is not supported",
        ));
    }

    Ok(())
}

/// Parses the `tool_choice` member into IR: the `auto`/`any`/`none` modes, with `any` plus one
/// allowed tool forcing that function. `validated` and multiple allowed tools are 400s.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` for unknown or unsupported choices.
fn parse_tool_choice(choice: &ToolChoiceIn) -> Result<IrToolChoice, InvalidRequest> {
    let (mode, allowed_tools) = match choice {
        ToolChoiceIn::Mode(mode) => (mode.as_str(), None),

        ToolChoiceIn::Object {
            mode,
            allowed_tools,
        } => (mode.as_str(), allowed_tools.as_ref()),
    };

    match mode {
        "auto" => {
            reject_allowed_tools(allowed_tools, "auto")?;

            Ok(IrToolChoice::Auto)
        }

        "any" => match allowed_tools {
            None => Ok(IrToolChoice::Required),

            Some(names) => match names.as_slice() {
                [] => Ok(IrToolChoice::Required),

                [name] => Ok(IrToolChoice::Function { name: name.clone() }),

                _ => Err(InvalidRequest::field(
                    "tool_choice",
                    "tool_choice allows at most one entry in allowed_tools",
                )),
            },
        },

        "none" => {
            reject_allowed_tools(allowed_tools, "none")?;

            Ok(IrToolChoice::None)
        }

        "validated" => Err(InvalidRequest::field(
            "tool_choice",
            "tool_choice `validated` is not supported",
        )),

        other => Err(InvalidRequest::field(
            "tool_choice",
            format!("Unknown tool_choice `{other}`"),
        )),
    }
}

/// Rejects an `allowed_tools` list paired with a mode that takes none.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` when the list is present and non-empty.
fn reject_allowed_tools(
    allowed_tools: Option<&Vec<String>>,
    mode: &str,
) -> Result<(), InvalidRequest> {
    if allowed_tools.is_some_and(|names| !names.is_empty()) {
        return Err(InvalidRequest::field(
            "tool_choice",
            format!("tool_choice `{mode}` takes no allowed_tools"),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses one request body given as JSON with an empty route context.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes(), &RouteContext::empty())
    }

    /// A full request parses into the documented IR: the model from the body, instructions,
    /// coalesced messages carrying text, a thought step with its verbatim opaque, a tool call
    /// with its signature, and the matching tool output, plus tools, tool choice, sampling, the
    /// reasoning level, the response format, and the streaming flag.
    ///
    /// # Panics
    ///
    /// Panics if the parsed request diverges from the documented IR.
    #[test]
    fn parses_a_full_request() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "public-model",
            "system_instruction": "be terse",
            "input": [
                {"type": "user_input", "content": [{"type": "text", "text": "hi"}]},
                {"type": "output_text", "text": "hello"},
                {"type": "thought", "id": "th_1", "summary": "considering", "signature": "sig-1"},
                {"type": "function_call", "id": "call_1", "name": "search", "arguments": {
                    "q": "hi",
                }, "signature": "sig-call"},
                {"type": "function_result", "name": "search", "call_id": "call_1", "result": [
                    {"type": "text", "text": "found"},
                ]},
            ],
            "tools": [{
                "type": "function",
                "name": "search",
                "description": "Searches.",
                "parameters": {"type": "object"},
            }],
            "tool_choice": {"mode": "any", "allowed_tools": ["search"]},
            "temperature": 0.5,
            "top_p": 0.9,
            "max_output_tokens": 100,
            "thinking_level": "high",
            "response_format": {"type": "json"},
            "stream": true,
        }))
        .expect("the fixture body must parse");

        assert_eq!(parsed.ir.model, "public-model");
        assert_eq!(parsed.ir.instructions.as_deref(), Some("be terse"));
        assert!(parsed.is_streaming);

        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningLevel::High),);
        assert_eq!(parsed.ir.response_format, Some(IrResponseFormat::Json),);
        assert_eq!(
            parsed.ir.tool_choice,
            Some(IrToolChoice::Function {
                name: "search".to_string(),
            }),
        );
        assert_eq!(parsed.ir.tools.len(), 1);
        assert_eq!(parsed.ir.temperature, Some(0.5));

        assert_eq!(parsed.ir.messages.len(), 3);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees three messages"
        )]
        let thought = parsed.ir.messages[1]
            .parts
            .iter()
            .find_map(|part| match part {
                IrPart::Reasoning {
                    text,
                    signature,
                    opaque,
                } => Some((text, signature, opaque)),

                _ => None,
            });

        #[expect(clippy::expect_used, reason = "the fixture carries the thought step")]
        let thought = thought.expect("the assistant message carries the thought step");

        assert_eq!(thought.0, "considering");
        assert_eq!(thought.1, &Some("sig-1".to_string()));
        assert_eq!(
            thought.2,
            &serde_json::json!({
                "type": "thought",
                "id": "th_1",
                "summary": "considering",
                "signature": "sig-1",
            }),
        );
    }

    /// Consecutive same-role steps coalesce into one message while role changes split them: two
    /// user turns merge, and the tool outputs of one turn ride one tool message.
    ///
    /// # Panics
    ///
    /// Panics if the coalescing diverges from the documented rule.
    #[test]
    fn coalesces_consecutive_same_role_steps() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "user_input", "content": [{"type": "text", "text": "a"}]},
                {"type": "user_input", "content": [{"type": "text", "text": "b"}]},
                {"type": "function_result", "name": "f", "call_id": "c1", "result": "one"},
                {"type": "function_result", "name": "f", "call_id": "c2", "result": "two"},
            ],
        }))
        .expect("the fixture body must parse");

        assert_eq!(parsed.ir.messages.len(), 2);
        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees two messages"
        )]
        {
            assert_eq!(parsed.ir.messages[0].role, IrRole::User);
            assert_eq!(parsed.ir.messages[0].parts.len(), 2);
            assert_eq!(parsed.ir.messages[1].role, IrRole::Tool);
            assert_eq!(parsed.ir.messages[1].parts.len(), 2);
        }
    }

    /// Every disposition-table 400 rejects naming its field: the chaining controls, the numeric
    /// budgets, stop, seed, modalities, the `validated` tool choice, multiple allowed tools, a
    /// choice without tools, built-in tool types, images in tool results, and unknown members —
    /// while `store` and `thinking_summaries` stay accepted and ignored.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented field or wording.
    #[test]
    fn rejects_known_unsupported_controls_naming_the_field() {
        let base = || {
            serde_json::json!({
                "model": "m",
                "input": [{"type": "user_input", "content": [{"type": "text", "text": "hi"}]}],
            })
        };

        for (field, value) in [
            ("previous_interaction_id", serde_json::json!("int_1")),
            ("background", serde_json::json!(true)),
            ("last_event_id", serde_json::json!("evt_1")),
            ("thinkingBudget", serde_json::json!(100)),
            ("budget_tokens", serde_json::json!(100)),
            ("stop", serde_json::json!(["x"])),
            ("stop_sequences", serde_json::json!(["x"])),
            ("seed", serde_json::json!(7)),
            ("modalities", serde_json::json!(["image"])),
        ] {
            let mut body = base();

            #[expect(
                clippy::indexing_slicing,
                reason = "the fixture object carries every rejected member slot"
            )]
            {
                body[field] = value;
            }

            #[expect(
                clippy::expect_used,
                reason = "each control is constructed to be rejected"
            )]
            let error = parse(body).expect_err("a known-unsupported control must be rejected");

            assert_eq!(
                error.param.as_deref(),
                Some(field),
                "the rejection must name `{field}`",
            );
        }

        for body in [
            serde_json::json!({
                "model": "m",
                "input": [],
                "tool_choice": "validated",
                "tools": [{"type": "function", "name": "f"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "tool_choice": {"mode": "any", "allowed_tools": ["a", "b"]},
                "tools": [{"type": "function", "name": "a"}, {"type": "function", "name": "b"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "tool_choice": "auto",
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "tools": [{"type": "google_search", "name": "search"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [
                    {"type": "function_result", "name": "f", "call_id": "c",
                     "result": [{"type": "image", "image_url": "https://example.com/i.png"}]},
                ],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "thinking_level": "ultra",
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "unknown_member": true,
            }),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each body is constructed to be rejected"
            )]
            let error = parse(body).expect_err("an unsupported request must be rejected");

            assert!(
                error.param.is_some() || !error.to_string().is_empty(),
                "the rejection must carry its cause: {error}",
            );
        }

        // The presentation-only controls parse: `store` and `thinking_summaries` never affect
        // the IR.
        #[expect(
            clippy::expect_used,
            reason = "the ignored-controls body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [{"type": "user_input", "content": [{"type": "text", "text": "hi"}]}],
            "store": true,
            "thinking_summaries": "auto",
        }))
        .expect("accepted-and-ignored controls must parse");

        assert_eq!(parsed.ir.messages.len(), 1);
    }

    /// The `function_result` shapes agree: a plain string, a text-block array joining with the
    /// blank-line separator, and the tool's own result object flattened to JSON text all carry
    /// the same content class, while a non-object arguments string is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the shapes diverge from the documented flattening.
    #[test]
    fn flattens_tool_result_shapes() {
        for (result, expected) in [
            (serde_json::json!("plain"), "plain".to_string()),
            (
                serde_json::json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]),
                "a\n\nb".to_string(),
            ),
            (
                serde_json::json!({"passed": true}),
                r#"{"passed":true}"#.to_string(),
            ),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each result shape is constructed to parse"
            )]
            let parsed = parse(serde_json::json!({
                "model": "m",
                "input": [
                    {"type": "function_result", "name": "f", "call_id": "c", "result": result},
                ],
            }))
            .expect("each result shape must parse");

            assert_eq!(parsed.ir.messages.len(), 1);

            #[expect(
                clippy::expect_used,
                reason = "the preceding length assertion guarantees one message with one part"
            )]
            let output = parsed
                .ir
                .messages
                .first()
                .expect("the result parses to one message")
                .parts
                .first()
                .expect("the message carries one part");
            assert!(
                matches!(output, IrPart::ToolOutput { .. }),
                "a function_result step must parse to a tool output",
            );

            if let IrPart::ToolOutput { content, .. } = output {
                assert_eq!(content, &expected);
            }
        }

        #[expect(
            clippy::expect_used,
            reason = "the malformed arguments are constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call", "id": "c", "name": "f", "arguments": "[1, 2]"},
            ],
        }))
        .expect_err("non-object arguments must be rejected");

        assert_eq!(error.param.as_deref(), Some("arguments"));
    }

    /// Streaming follows the body-then-query rule: the body's `stream` member decides unless the
    /// route carries the `?alt=sse` override, which forces streaming either way.
    ///
    /// # Panics
    ///
    /// Panics if the streaming decision diverges from the convention.
    #[test]
    fn decides_streaming_from_body_unless_alt_sse_overrides() {
        let body = serde_json::json!({
            "model": "m",
            "input": [],
            "stream": true,
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture bodies are constructed to parse"
        )]
        let streaming = parse_request(body.to_string().as_bytes(), &RouteContext::empty())
            .expect("the streaming body must parse");

        assert!(streaming.is_streaming);

        #[expect(
            clippy::expect_used,
            reason = "the fixture bodies are constructed to parse"
        )]
        let forced = parse_request(
            serde_json::json!({"model": "m", "input": []})
                .to_string()
                .as_bytes(),
            &RouteContext {
                public_id: None,
                is_streaming: Some(true),
            },
        )
        .expect("the overridden body must parse");

        assert!(forced.is_streaming);

        #[expect(
            clippy::expect_used,
            reason = "the fixture bodies are constructed to parse"
        )]
        let buffered = parse_request(
            serde_json::json!({"model": "m", "input": []})
                .to_string()
                .as_bytes(),
            &RouteContext::empty(),
        )
        .expect("the buffered body must parse");

        assert!(!buffered.is_streaming);
    }
}
