//! Upstream-request rendering for the Gemini Interactions codec: renders an IR request into the
//! `gemini_interaction` upstream payload, `POST {base}/v1beta/interactions` carrying the resolved
//! upstream id as the body `model`.

use super::{ContentBlock, RenderStep, ResponseFormatShape, TypedStep, render_thought};
use crate::{
    format,
    ir::{InvalidRequest, IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice},
};
use serde::Serialize;
use serde_json::Value;

/// Renders the upstream request URL and payload: `POST {base}/v1beta/interactions` carrying the
/// resolved `upstream_id` as the body `model`, the stateless full-history `input` replay, and
/// always `store: false`.
/// # Errors
///
/// Returns [`InvalidRequest`] when a tool output answers an unknown call id, or when tool call
/// arguments are not a JSON object.
pub(crate) fn render_request(
    ir: &IrRequest,
    base_url: &str,
    upstream_id: &str,
    is_streaming: bool,
) -> Result<(String, RequestBody), InvalidRequest> {
    let url = format!("{}/v1beta/interactions", base_url.trim_end_matches('/'));

    let body = RequestBody {
        model: upstream_id.to_string(),
        system_instruction: ir.instructions.clone(),
        input: render_input(ir)?,
        tools: render_tools(ir),
        generation_config: render_generation_config(ir),
        response_format: render_response_format(ir),
        stream: is_streaming,
        // The gateway never stores or chains: every call replays the full history statelessly.
        store: false,
    };

    Ok((url, body))
}

/// The upstream Interactions request payload.
///
/// The body owns its strings: synthesized thought steps carry gateway-minted ids, so borrowing the
/// IR alone cannot cover every member.
#[derive(Debug, Serialize)]
pub(crate) struct RequestBody {
    /// Upstream model name, rendered verbatim from the resolved `upstream_id`.
    model: String,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<String>,

    /// Stateless full-history replay: every turn so far, oldest first.
    input: Vec<RenderStep>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<UpstreamTool>,

    /// Sampling, reasoning, and tool-choice configuration, omitted when it would be empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,

    /// Response format restriction, omitted for free-form text.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormatShape>,

    /// Whether the upstream streams its response as SSE.
    stream: bool,

    /// Always `false`: the gateway is stateless.
    store: bool,
}
/// One advertised function tool in the typed Interactions shape: function tools only.
#[derive(Debug, Serialize)]
struct UpstreamTool {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// Function name the model calls.
    name: String,

    /// Human-readable description, omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,

    /// JSON Schema of the accepted arguments, omitted for a function that takes none.
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

/// Sampling, reasoning, and tool-choice configuration of an upstream request.
#[derive(Debug, Serialize)]
struct GenerationConfig {
    /// Sampling temperature, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,

    /// Output token budget, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,

    /// Reasoning level rendered verbatim from the closed vocabulary, omitted when the client sent
    /// none — then the upstream's own default applies. A token budget is never rendered: reasoning
    /// control reaches only models that accept levels, never a numeric budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,

    /// Tool selection restriction, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<UpstreamToolChoice>,
}

impl GenerationConfig {
    /// Whether every field is absent, in which case the whole configuration is omitted from the
    /// request.
    fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.max_output_tokens.is_none()
            && self.thinking_level.is_none()
            && self.tool_choice.is_none()
    }
}

/// Tool selection restriction in the upstream shape: a mode with an optional single allowed
/// function.
#[derive(Debug, Serialize)]
struct UpstreamToolChoice {
    /// Selection mode.
    mode: ToolChoiceMode,

    /// The one function the model may call, present only for a forced single function.
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_tools: Option<Vec<String>>,
}

/// The upstream `tool_choice.mode` values.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolChoiceMode {
    /// The model decides freely.
    Auto,

    /// The model must call a tool.
    Any,

    /// The model may not call tools.
    None,
}
/// Renders the stateless full-history `input` replay: every message in order, with consecutive
/// user messages coalesced into one `user_input` step whose blocks concatenate — the several
/// turns of plain chat then ride as one step per message, exactly the 1:1 shape.
///
/// A message whose parts all drop renders nothing and is transparent: it contributes no step and
/// cannot split a user run, since it produces no step to alternate against.
///
/// # Errors
///
/// Returns [`InvalidRequest`] per [`format::resolve_tool_output_name`] and for non-object tool call
/// arguments.
fn render_input(ir: &IrRequest) -> Result<Vec<RenderStep>, InvalidRequest> {
    let mut steps: Vec<RenderStep> = Vec::new();
    let mut user_run: Vec<ContentBlock> = Vec::new();

    // Pushes the open user run as one step; a no-op when the run is empty.
    let flush_user_run = |steps: &mut Vec<RenderStep>, run: &mut Vec<ContentBlock>| {
        if !run.is_empty() {
            steps.push(RenderStep::Typed(TypedStep::UserInput {
                content: std::mem::take(run),
            }));
        }
    };

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            user_run.push(ContentBlock::Text { text: text.clone() })
                        }

                        IrPart::Image { image } => user_run.push(ContentBlock::Image {
                            image_url: image.to_url(),
                        }),

                        // A user turn has no slot for calls, outputs, or reasoning: the IR type
                        // system never routes them here, and a misplaced part is dropped rather
                        // than rendered into a step whose role would misattribute it.
                        IrPart::ToolCall { .. }
                        | IrPart::ToolOutput { .. }
                        | IrPart::Reasoning { .. } => {}
                    }
                }
            }

            IrRole::Assistant => {
                flush_user_run(&mut steps, &mut user_run);

                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            steps.push(RenderStep::Typed(TypedStep::OutputText {
                                text: text.clone(),
                            }));
                        }

                        IrPart::Image { .. } | IrPart::ToolOutput { .. } => {
                            // An assistant turn has no image or tool-output slot on this family's
                            // wire: images are request content and outputs answer calls, so a
                            // misplaced part is dropped rather than rendered into a foreign step.
                        }

                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            format::arguments_as_object(arguments)?;

                            steps.push(RenderStep::Typed(TypedStep::FunctionCall {
                                id: call_id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                signature: thought_signature.clone(),
                            }));
                        }

                        IrPart::Reasoning {
                            text,
                            signature,
                            opaque,
                        } => steps.push(render_thought(text, signature.clone(), opaque)),
                    }
                }
            }

            IrRole::Tool => {
                flush_user_run(&mut steps, &mut user_run);

                for part in &message.parts {
                    if let IrPart::ToolOutput {
                        call_id, content, ..
                    } = part
                    {
                        let name = format::resolve_tool_output_name(call_id, ir)?;

                        steps.push(RenderStep::Typed(TypedStep::FunctionResult {
                            name,
                            call_id: call_id.clone(),
                            result: vec![ContentBlock::Text {
                                text: content.clone(),
                            }],
                        }));
                    }
                }
            }
        }
    }

    flush_user_run(&mut steps, &mut user_run);

    Ok(steps)
}

/// Renders the advertised function tools in the typed Interactions shape.
fn render_tools(ir: &IrRequest) -> Vec<UpstreamTool> {
    ir.tools
        .iter()
        .map(|tool| UpstreamTool {
            tool_type: "function",
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        })
        .collect()
}

/// Renders the sampling, reasoning, and tool-choice configuration; `None` when every field is
/// absent, so the whole member is omitted from the request.
fn render_generation_config(ir: &IrRequest) -> Option<GenerationConfig> {
    let tool_choice = ir
        .tool_choice
        .as_ref()
        .map(|tool_choice| match tool_choice {
            IrToolChoice::Auto => UpstreamToolChoice {
                mode: ToolChoiceMode::Auto,
                allowed_tools: None,
            },

            IrToolChoice::None => UpstreamToolChoice {
                mode: ToolChoiceMode::None,
                allowed_tools: None,
            },

            IrToolChoice::Required => UpstreamToolChoice {
                mode: ToolChoiceMode::Any,
                allowed_tools: None,
            },

            IrToolChoice::Function { name } => UpstreamToolChoice {
                mode: ToolChoiceMode::Any,
                allowed_tools: Some(vec![name.clone()]),
            },
        });

    let config = GenerationConfig {
        temperature: ir.temperature,
        top_p: ir.top_p,
        max_output_tokens: ir.max_output_tokens,
        thinking_level: ir.reasoning_effort.as_ref().map(|level| level.as_str()),
        tool_choice,
    };

    (!config.is_empty()).then_some(config)
}

/// Renders the response format restriction; free-form text renders no member at all.
fn render_response_format(ir: &IrRequest) -> Option<ResponseFormatShape> {
    match &ir.response_format {
        None | Some(IrResponseFormat::Text) => None,

        Some(IrResponseFormat::Json) => Some(ResponseFormatShape::Json {}),

        Some(IrResponseFormat::JsonSchema { name, schema }) => {
            Some(ResponseFormatShape::JsonSchema {
                name: name.clone(),
                schema: schema.clone(),
            })
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes a rendered upstream body into the JSON value the tests assert against.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize, which cannot occur for the rendered wire
    /// types.
    #[expect(
        clippy::expect_used,
        reason = "the rendered body is under test; a failing render is the failure"
    )]
    fn body_json(body: &RequestBody) -> Value {
        serde_json::to_value(body).expect("the rendered body serializes")
    }

    /// Renders one upstream body for the given IR.
    ///
    /// # Panics
    ///
    /// Panics if the IR fails to render, which the fixture prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture IR is constructed to render"
    )]
    fn render(ir: &IrRequest) -> (String, RequestBody) {
        render_request(ir, "https://upstream.example/", "upstream-model", false)
            .expect("the fixture IR renders")
    }

    /// Verifies the request envelope: the URL pins the Interactions action, the body carries the
    /// upstream id as `model`, `store` is always false, and `stream` follows the requested mode.
    ///
    /// # Panics
    ///
    /// Panics if the envelope diverges from the contract.
    #[test]
    fn renders_the_request_envelope() {
        let ir = IrRequest {
            model: "upstream-model".to_string(),
            ..IrRequest::default()
        };

        let (url, _) = render(&ir);

        assert_eq!(url, "https://upstream.example/v1beta/interactions");

        #[expect(
            clippy::expect_used,
            reason = "the fixture IR is constructed to render"
        )]
        let (_, streaming) = render_request(&ir, "https://upstream.example", "m", true)
            .expect("the fixture IR renders");

        let rendered = body_json(&streaming);

        assert_eq!(rendered.pointer("/model"), Some(&serde_json::json!("m")));
        assert_eq!(rendered.pointer("/store"), Some(&serde_json::json!(false)));
        assert_eq!(rendered.pointer("/stream"), Some(&serde_json::json!(true)));
        assert_eq!(rendered.pointer("/input"), Some(&serde_json::json!([])));
    }

    /// Verifies the thinking slot of the closed-vocabulary contract: every vocabulary value renders
    /// into `generation_config.thinking_level` verbatim, an absent effort renders no thinking
    /// level at all, and no rendered body ever carries a token budget.
    ///
    /// # Panics
    ///
    /// Panics if a rendered slot diverges from the contract.
    #[test]
    fn renders_thinking_level_verbatim_and_never_a_budget() {
        use crate::ir::IrReasoningLevel;

        for (level, thinking_level) in [
            (IrReasoningLevel::None, "none"),
            (IrReasoningLevel::Minimal, "minimal"),
            (IrReasoningLevel::Low, "low"),
            (IrReasoningLevel::Medium, "medium"),
            (IrReasoningLevel::High, "high"),
            (IrReasoningLevel::XHigh, "xhigh"),
            (IrReasoningLevel::Max, "max"),
        ] {
            let ir = IrRequest {
                model: "m".to_string(),
                reasoning_effort: Some(level),
                ..IrRequest::default()
            };

            let (_, body) = render(&ir);
            let rendered = body_json(&body);

            assert_eq!(
                rendered
                    .pointer("/generation_config/thinking_level")
                    .and_then(Value::as_str),
                Some(thinking_level),
                "level `{thinking_level}` must render verbatim",
            );
            assert!(
                !rendered.to_string().contains("thinkingBudget"),
                "a token budget must never render",
            );
        }

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);
        let rendered = body_json(&body);

        assert!(
            rendered
                .get("generation_config")
                .is_none_or(|config| config.get("thinking_level").is_none()),
            "an absent effort renders no thinking level",
        );
        assert!(!rendered.to_string().contains("thinkingBudget"));
    }

    /// Verifies the tool-choice mapping into `generation_config`: auto, none, required, and a
    /// forced single function (the `any` mode with one allowed tool); an absent choice omits the
    /// member.
    ///
    /// # Panics
    ///
    /// Panics if a rendered choice diverges from the mapping.
    #[test]
    fn renders_tool_choice_modes() {
        for (choice, mode, allowed) in [
            (IrToolChoice::Auto, "auto", None),
            (IrToolChoice::None, "none", None),
            (IrToolChoice::Required, "any", None),
            (
                IrToolChoice::Function {
                    name: "search".to_string(),
                },
                "any",
                Some(vec!["search"]),
            ),
        ] {
            let ir = IrRequest {
                model: "m".to_string(),
                tool_choice: Some(choice),
                ..IrRequest::default()
            };

            let (_, body) = render(&ir);
            let rendered = body_json(&body);

            let tool_choice = rendered.pointer("/generation_config/tool_choice");

            assert_eq!(
                tool_choice
                    .and_then(|choice| choice.get("mode"))
                    .and_then(Value::as_str),
                Some(mode),
            );

            match allowed {
                Some(names) => assert_eq!(
                    tool_choice.and_then(|choice| choice.get("allowed_tools")),
                    Some(&serde_json::json!(names)),
                ),

                None => assert!(
                    tool_choice.is_none_or(|choice| choice.get("allowed_tools").is_none()),
                    "a mode without a forced function renders no allowed tools",
                ),
            }
        }

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert!(
            body_json(&body).pointer("/generation_config").is_none(),
            "an empty configuration is omitted entirely",
        );
    }

    /// Verifies the response-format mapping: free-form text renders no member, JSON renders the
    /// `json` shape, and a named schema rides through with its name and schema.
    ///
    /// # Panics
    ///
    /// Panics if a rendered format diverges from the mapping.
    #[test]
    fn renders_response_format() {
        let ir = IrRequest {
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::JsonSchema {
                name: "answer".to_string(),
                schema: serde_json::json!({"type": "object"}),
            }),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert_eq!(
            body_json(&body).pointer("/response_format"),
            Some(&serde_json::json!({
                "type": "json_schema",
                "name": "answer",
                "schema": {"type": "object"},
            })),
        );

        let ir = IrRequest {
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::Json),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert_eq!(
            body_json(&body).pointer("/response_format"),
            Some(&serde_json::json!({"type": "json"})),
        );

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert!(
            body_json(&body).get("response_format").is_none(),
            "free-form text renders no response format",
        );
    }

    /// Verifies the stateless full-history replay: consecutive user turns coalesce into one
    /// `user_input` step, a same-family thought step replays its stored value exactly, a
    /// text-only reasoning block synthesizes a fresh thought step carrying its text and
    /// signature, tool calls keep their ids and signatures, and tool outputs resolve their
    /// function names into `function_result` steps.
    ///
    /// # Panics
    ///
    /// Panics if the rendered steps diverge from the pinned wire shape.
    #[test]
    fn replays_full_history_through_typed_steps() {
        use crate::ir::{IrMessage, IrRole};

        let stored_thought = serde_json::json!({
            "type": "thought",
            "id": "th_1",
            "summary": "stored reasoning",
            "signature": "sig-stored",
        });

        let ir = IrRequest {
            model: "m".to_string(),
            instructions: Some("be terse".to_string()),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "first".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "second".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::Reasoning {
                            text: "stored reasoning".to_string(),
                            signature: Some("sig-stored".to_string()),
                            opaque: stored_thought.clone(),
                        },
                        IrPart::Reasoning {
                            text: "fresh reasoning".to_string(),
                            signature: Some("sig-fresh".to_string()),
                            opaque: Value::Null,
                        },
                        IrPart::Text {
                            text: "hello".to_string(),
                        },
                        IrPart::ToolCall {
                            call_id: "call_1".to_string(),
                            name: "search".to_string(),
                            arguments: r#"{"q":"hi"}"#.to_string(),
                            thought_signature: Some("sig-call".to_string()),
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "call_1".to_string(),
                        content: "found it".to_string(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![crate::ir::IrTool {
                name: "search".to_string(),
                description: Some("Searches.".to_string()),
                parameters: Some(serde_json::json!({"type": "object"})),
            }],
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);
        let rendered = body_json(&body);

        assert_eq!(
            rendered.pointer("/system_instruction"),
            Some(&serde_json::json!("be terse")),
        );
        assert_eq!(
            rendered.pointer("/tools"),
            Some(&serde_json::json!([{
                "type": "function",
                "name": "search",
                "description": "Searches.",
                "parameters": {"type": "object"},
            }])),
        );

        // Consecutive user turns coalesce into one step.
        assert_eq!(
            rendered.pointer("/input/0"),
            Some(&serde_json::json!({
                "type": "user_input",
                "content": [{"type": "text", "text": "first"}, {"type": "text", "text": "second"}],
            })),
        );

        // The same-family thought step replays its stored value exactly.
        assert_eq!(rendered.pointer("/input/1"), Some(&stored_thought));

        // A text-only block synthesizes a fresh step carrying its text and signature.
        assert_eq!(
            rendered.pointer("/input/2/type"),
            Some(&serde_json::json!("thought"))
        );
        assert_eq!(
            rendered.pointer("/input/2/summary"),
            Some(&serde_json::json!("fresh reasoning"))
        );
        assert_eq!(
            rendered.pointer("/input/2/signature"),
            Some(&serde_json::json!("sig-fresh"))
        );

        assert_eq!(
            rendered.pointer("/input/3"),
            Some(&serde_json::json!({"type": "output_text", "text": "hello"})),
        );
        assert_eq!(
            rendered.pointer("/input/4"),
            Some(&serde_json::json!({
                "type": "function_call",
                "id": "call_1",
                "name": "search",
                "arguments": r#"{"q":"hi"}"#,
                "signature": "sig-call",
            })),
        );
        assert_eq!(
            rendered.pointer("/input/5"),
            Some(&serde_json::json!({
                "type": "function_result",
                "name": "search",
                "call_id": "call_1",
                "result": [{"type": "text", "text": "found it"}],
            })),
        );
        assert!(
            rendered.pointer("/input/6").is_none(),
            "the replay renders exactly six steps",
        );
    }

    /// Verifies the family-membership rule: a cross-family opaque payload never reaches the wire —
    /// the block synthesizes a fresh thought step from its text instead — while the signature
    /// still rides along.
    ///
    /// # Panics
    ///
    /// Panics if a foreign payload leaks onto the wire.
    #[test]
    fn drops_cross_family_opaque_payloads() {
        use crate::ir::{IrMessage, IrRole};

        let foreign = serde_json::json!({
            "type": "thinking",
            "thinking": "another family's block",
            "signature": "sig-foreign",
        });

        let ir = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Reasoning {
                    text: "reasoned".to_string(),
                    signature: None,
                    opaque: foreign,
                }],
            }],
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);
        let rendered = body_json(&body);

        assert_eq!(
            rendered.pointer("/input/0/type"),
            Some(&serde_json::json!("thought"))
        );
        assert_eq!(
            rendered.pointer("/input/0/summary"),
            Some(&serde_json::json!("reasoned"))
        );
        assert_eq!(
            rendered.pointer("/input/0/thinking"),
            None,
            "a foreign payload must never leak onto the wire",
        );
        assert!(
            rendered.pointer("/input/1").is_none(),
            "the block renders exactly one step",
        );
    }

    /// A tool output answering an unknown call id cannot render: the function name is unresolvable
    /// and the request fails before dispatch.
    ///
    /// # Panics
    ///
    /// Panics if the rejection lands outside its documented wording.
    #[test]
    fn rejects_tool_outputs_without_a_matching_call() {
        use crate::ir::{IrMessage, IrRole};

        let ir = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id: "call_missing".to_string(),
                    content: "orphan".to_string(),
                    is_error: false,
                }],
            }],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the orphan output is constructed to be rejected"
        )]
        let error = render_request(&ir, "https://upstream.example", "m", false)
            .expect_err("an orphan tool output must fail rendering");

        assert_eq!(error.param.as_deref(), Some("messages"));
        assert!(error.to_string().contains("call_missing"));
    }
}
