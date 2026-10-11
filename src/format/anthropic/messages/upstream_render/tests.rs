//! Upstream request rendering tests for the Anthropic Messages codec: the IR → upstream body
//! mapping, turn alternation and merging, reasoning replay rules, and the compaction body
//! extensions (`docs/reference/inference.md`).
#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section on every \
              test would document nothing its name does not"
)]

use crate::{
    ir::{
        IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice,
        StateFamily, StateItem,
    },
    vocabulary::ReasoningLevel,
};
use serde_json::Value;
use std::error::Error as StdError;

/// The verbatim opaque payload of a signed same-family thinking block.
fn thinking_opaque(text: &str, signature: &str) -> Value {
    serde_json::json!({
        "type": "thinking",
        "thinking": text,
        "signature": signature
    })
}

/// The verbatim opaque payload of a redacted same-family block.
fn redacted_opaque(data: &str) -> Value {
    serde_json::json!({"type": "redacted_thinking", "data": data})
}

/// A user turn carrying one text part.
fn user_text(text: &str) -> IrMessage {
    IrMessage {
        role: IrRole::User,
        parts: vec![IrPart::Text {
            text: text.to_string(),
        }],
    }
}

/// An assistant turn carrying one text part.
fn assistant_text(text: &str) -> IrMessage {
    IrMessage {
        role: IrRole::Assistant,
        parts: vec![IrPart::Text {
            text: text.to_string(),
        }],
    }
}

/// A minimal IR request addressing the upstream model with one user turn.
fn base_ir() -> IrRequest {
    IrRequest {
        model: "claude-upstream".to_string(),
        messages: vec![user_text("hi")],
        max_output_tokens: Some(256),
        ..IrRequest::default()
    }
}

/// Renders an IR request into its upstream body.
#[expect(
    clippy::expect_used,
    reason = "fixture requests render; a rendering failure is the behavior under test"
)]
fn rendered_body(ir: &IrRequest) -> super::RequestBody<'_> {
    super::render_request_body(ir, false).expect("the fixture renders")
}

/// Renders an IR request into its serialized upstream body.
#[expect(
    clippy::expect_used,
    reason = "the rendered wire type serializes by construction"
)]
fn rendered(ir: &IrRequest) -> Value {
    serde_json::to_value(rendered_body(ir)).expect("rendered bodies serialize")
}

/// Asserts a fixture request fails its render, returning the rendered rejection message.
#[expect(
    clippy::expect_used,
    reason = "the rejection under test is the expected outcome"
)]
fn rejected(ir: &IrRequest) -> String {
    super::render_request_body(ir, false)
        .expect_err("the fixture request must fail its render")
        .to_string()
}

/// The full control surface renders verbatim: envelope fields, tools with schemas, every tool
/// choice mode, sampling, the effort spelling, and the `none` level's native off spelling.
#[test]
fn controls_render_verbatim() -> Result<(), Box<dyn StdError>> {
    for (choice, expected) in [
        (IrToolChoice::Auto, serde_json::json!({"type": "auto"})),
        (IrToolChoice::None, serde_json::json!({"type": "none"})),
        (IrToolChoice::Required, serde_json::json!({"type": "any"})),
        (
            IrToolChoice::Function {
                name: "find".to_string(),
            },
            serde_json::json!({"type": "tool", "name": "find"}),
        ),
    ] {
        let ir = IrRequest {
            model: "claude-up".to_string(),
            instructions: Some("Be terse.".to_string()),
            temperature: Some(0.5),
            top_p: Some(0.9),
            max_output_tokens: Some(777),
            tools: vec![IrTool {
                name: "find".to_string(),
                description: Some("Search".to_string()),
                parameters: Some(serde_json::json!({"type": "object"})),
            }],
            tool_choice: Some(choice),
            reasoning_effort: Some(ReasoningLevel::XHigh),
            messages: vec![user_text("hi")],
            ..IrRequest::default()
        };

        let body = rendered(&ir);

        assert_eq!(crate::test_support::at(&body, "/model"), "claude-up");
        assert_eq!(crate::test_support::at(&body, "/max_tokens"), 777);
        assert_eq!(crate::test_support::at(&body, "/system"), "Be terse.");
        assert_eq!(crate::test_support::at(&body, "/temperature"), 0.5);
        assert_eq!(crate::test_support::at(&body, "/top_p"), 0.9);
        assert_eq!(crate::test_support::at(&body, "/stream"), false);
        assert_eq!(
            crate::test_support::at(&body, "/tools"),
            &serde_json::json!([{
                "name": "find",
                "description": "Search",
                "input_schema": {"type": "object"}
            }])
        );

        assert_eq!(crate::test_support::at(&body, "/tool_choice"), &expected);
        assert_eq!(
            crate::test_support::at(&body, "/output_config"),
            &serde_json::json!({"effort": "xhigh"})
        );
        assert!(
            body.get("thinking").is_none(),
            "a level renders verbatim, not as the off spelling"
        );
    }

    let none_level = IrRequest {
        model: "claude-up".to_string(),
        reasoning_effort: Some(ReasoningLevel::None),
        messages: vec![user_text("hi")],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    let body = rendered(&none_level);

    assert_eq!(
        crate::test_support::at(&body, "/thinking"),
        &serde_json::json!({"type": "disabled"})
    );

    assert!(
        body.get("output_config").is_none(),
        "the none level has no effort spelling to carry"
    );

    Ok(())
}

/// Members the request carries none of stay absent: a bare body is exactly `model`,
/// `max_tokens`, `messages`, and `stream` (in the serialized member order).
#[test]
fn absent_members_stay_absent() -> Result<(), Box<dyn StdError>> {
    let keys = rendered(&base_ir())
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>());

    assert_eq!(
        keys,
        Some(vec![
            "max_tokens".to_string(),
            "messages".to_string(),
            "model".to_string(),
            "stream".to_string(),
        ])
    );

    Ok(())
}

/// A request without an output budget synthesizes the documented 16384-token cap, because the
/// upstream requires the field.
#[test]
fn missing_budget_synthesizes_the_cap() -> Result<(), Box<dyn StdError>> {
    let mut ir = base_ir();

    ir.max_output_tokens = None;

    assert_eq!(
        crate::test_support::at(&rendered(&ir), "/max_tokens"),
        16_384
    );

    Ok(())
}

/// A JSON-schema response format renders inside `output_config.format` with its schema
/// preserved and the synthesized name dropped; free-form JSON has no Messages slot and fails
/// explicitly; plain text renders nothing.
#[test]
fn response_format_mapping() -> Result<(), Box<dyn StdError>> {
    let schema = serde_json::json!({"type": "object", "required": ["a"]});
    let mut schema_ir = base_ir();

    schema_ir.response_format = Some(IrResponseFormat::JsonSchema {
        name: "response".to_string(),
        schema: schema.clone(),
    });

    assert_eq!(
        crate::test_support::at(&rendered(&schema_ir), "/output_config"),
        &serde_json::json!({"format": {"type": "json_schema", "schema": schema}})
    );

    let mut both_ir = schema_ir;

    both_ir.reasoning_effort = Some(ReasoningLevel::Low);

    assert_eq!(
        crate::test_support::at(&rendered(&both_ir), "/output_config"),
        &serde_json::json!({
            "effort": "low",
            "format": {"type": "json_schema", "schema": {"type": "object", "required": ["a"]}}
        })
    );

    let mut json_ir = base_ir();

    json_ir.response_format = Some(IrResponseFormat::Json);

    let message = rejected(&json_ir);

    assert!(
        message.contains("JSON schema"),
        "free-form JSON fails naming the mismatch: {message}"
    );

    let mut text_ir = base_ir();

    text_ir.response_format = Some(IrResponseFormat::Text);

    assert!(rendered(&text_ir).get("output_config").is_none());

    Ok(())
}

/// Consecutive same-role IR messages merge into one turn, tool results fold into user turns,
/// and tool arguments ride their protocol string field as the input object — the body opens
/// with a user turn and strictly alternates.
#[test]
fn turns_alternate_and_merge() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![
            user_text("one"),
            user_text("two"),
            assistant_text("draft"),
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::Text {
                        text: "answer".to_string(),
                    },
                    IrPart::ToolCall {
                        call_id: "t1".to_string(),
                        name: "find".to_string(),
                        arguments: "{\"q\":1}".to_string(),
                        thought_signature: None,
                    },
                ],
            },
            IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id: "t1".to_string(),
                    content: "result".to_string(),
                    is_error: false,
                }],
            },
        ],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&ir), "/messages"),
        &serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "one"},
                {"type": "text", "text": "two"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "draft"},
                {"type": "text", "text": "answer"},
                {"type": "tool_use", "id": "t1", "name": "find", "input": {"q": 1}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1",
                "content": "result"}]}
        ])
    );

    Ok(())
}

/// A tool result carries `is_error` only when the tool reported an error.
#[test]
fn is_error_renders_only_when_true() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![IrMessage {
            role: IrRole::Tool,
            parts: vec![
                IrPart::ToolOutput {
                    call_id: "ok".to_string(),
                    content: "fine".to_string(),
                    is_error: false,
                },
                IrPart::ToolOutput {
                    call_id: "bad".to_string(),
                    content: "boom".to_string(),
                    is_error: true,
                },
            ],
        }],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&ir), "/messages"),
        &serde_json::json!([
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "ok", "content": "fine"},
                {"type": "tool_result", "tool_use_id": "bad", "content": "boom",
                 "is_error": true}
            ]}
        ])
    );

    Ok(())
}

/// Same-family reasoning blocks hoist before their turn's text and tool calls and replay their
/// opaque payloads verbatim; foreign-family or signature-less thinking renders nothing, and a
/// message rendering nothing is transparent inside its merged turn.
#[test]
fn assistant_reasoning_hoists_and_filters() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::Text {
                        text: "answer".to_string(),
                    },
                    IrPart::Reasoning {
                        text: "why".to_string(),
                        signature: Some("sig".to_string()),
                        opaque: thinking_opaque("why", "sig"),
                    },
                    IrPart::ToolCall {
                        call_id: "t1".to_string(),
                        name: "find".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    },
                ],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::Reasoning {
                        text: "foreign".to_string(),
                        signature: None,
                        opaque: Value::Null,
                    },
                    IrPart::Reasoning {
                        text: "unsigned".to_string(),
                        signature: None,
                        opaque: serde_json::json!({"type": "thinking", "thinking": "unsigned"}),
                    },
                    IrPart::Text {
                        text: "kept".to_string(),
                    },
                ],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Reasoning {
                    text: "dropped".to_string(),
                    signature: None,
                    opaque: Value::Null,
                }],
            },
        ],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&ir), "/messages"),
        &serde_json::json!([
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "why", "signature": "sig"},
                {"type": "text", "text": "answer"},
                {"type": "tool_use", "id": "t1", "name": "find", "input": {}},
                {"type": "text", "text": "kept"}
            ]}
        ])
    );

    Ok(())
}

/// Redacted reasoning replays verbatim regardless of a signature, and user turns carry text and
/// images only.
#[test]
fn redacted_replays_and_user_content_maps() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![
                    IrPart::Image {
                        image: IrImage::Inline {
                            mime_type: "image/png".to_string(),
                            data_base64: "QUJD".to_string(),
                        },
                    },
                    IrPart::Image {
                        image: IrImage::Url("https://example.test/i.png".to_string()),
                    },
                ],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Reasoning {
                    text: String::new(),
                    signature: None,
                    opaque: redacted_opaque("blob"),
                }],
            },
        ],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&ir), "/messages"),
        &serde_json::json!([
            {"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                 "data": "QUJD"}},
                {"type": "image", "source": {"type": "url", "url": "https://example.test/i.png"}}
            ]},
            {"role": "assistant", "content": [
                {"type": "redacted_thinking", "data": "blob"}
            ]}
        ])
    );

    Ok(())
}

/// Tool arguments must encode a JSON object: an empty string degrades to the empty object while
/// a non-object or malformed encoding fails the render naming `arguments`.
#[test]
fn tool_arguments_must_encode_objects() -> Result<(), Box<dyn StdError>> {
    let ir_with = |arguments: &str| IrRequest {
        model: "claude-up".to_string(),
        messages: vec![IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::ToolCall {
                call_id: "t1".to_string(),
                name: "find".to_string(),
                arguments: arguments.to_string(),
                thought_signature: None,
            }],
        }],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&ir_with("")), "/messages"),
        &serde_json::json!([
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "find", "input": {}}
            ]}
        ])
    );

    assert_eq!(
        crate::test_support::at(&rendered(&ir_with("  ")), "/messages"),
        &serde_json::json!([
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "find", "input": {}}
            ]}
        ])
    );

    for arguments in ["nope", "[1]"] {
        let message = rejected(&ir_with(arguments));

        assert!(
            message.contains("arguments"),
            "rejection `{message}` must name `arguments`"
        );
    }

    Ok(())
}

/// A tool declared without a parameter schema advertises the empty object schema.
#[test]
fn tool_without_schema_gets_the_empty_object() -> Result<(), Box<dyn StdError>> {
    let mut ir = base_ir();

    ir.tools = vec![IrTool {
        name: "find".to_string(),
        description: None,
        parameters: None,
    }];

    assert_eq!(
        crate::test_support::at(&rendered(&ir), "/tools"),
        &serde_json::json!([{"name": "find", "input_schema": {"type": "object"}}])
    );

    Ok(())
}

/// Compaction state splices into the upstream turns: a leading block opens the first turn, a
/// historical block rides the assistant turn at its history position, and a trailing block
/// rides one trailing assistant turn — while gateway carriers and foreign-family state are
/// invariant violations.
#[test]
fn state_items_splice_into_turns() -> Result<(), Box<dyn StdError>> {
    let native_block = serde_json::json!({
        "type": "compaction",
        "content": "s",
        "signature": "native"
    });

    let state = |offset: usize| StateItem {
        offset,
        item: native_block.clone(),
        is_gateway: false,
        is_compaction: true,
        family: StateFamily::Messages,
    };

    let leading = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![user_text("q")],
        state_items: vec![state(0)],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&leading), "/messages"),
        &serde_json::json!([
            {"role": "assistant", "content": [native_block]},
            {"role": "user", "content": [{"type": "text", "text": "q"}]}
        ])
    );

    let historical = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![user_text("q"), assistant_text("a")],
        state_items: vec![state(1)],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&historical), "/messages"),
        &serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "q"}]},
            {"role": "assistant", "content": [native_block, {"type": "text", "text": "a"}]}
        ])
    );

    let trailing = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![user_text("q")],
        state_items: vec![state(5)],
        max_output_tokens: Some(64),
        ..IrRequest::default()
    };

    assert_eq!(
        crate::test_support::at(&rendered(&trailing), "/messages"),
        &serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "q"}]},
            {"role": "assistant", "content": [native_block]}
        ])
    );

    let mut gateway = leading.clone();

    gateway.state_items = vec![StateItem {
        is_gateway: true,
        ..state(0)
    }];

    let message = rejected(&gateway);

    assert!(
        message.contains("preparation"),
        "gateway carriers are preparation invariant violations: {message}"
    );

    let mut foreign = leading.clone();

    foreign.state_items = vec![StateItem {
        family: StateFamily::Responses,
        ..state(0)
    }];

    let message = rejected(&foreign);

    assert!(
        message.contains("cannot replay"),
        "foreign-family state is inaccessible: {message}"
    );

    Ok(())
}

/// The on-demand body adds the summarize control with its optional instructions and overrides
/// the output budget; tool definitions stay advertised but their selection renders `none`.
#[test]
fn on_demand_body_extends_the_base_render() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![user_text("q")],
        tools: vec![IrTool {
            name: "find".to_string(),
            description: None,
            parameters: Some(serde_json::json!({"type": "object"})),
        }],
        tool_choice: Some(IrToolChoice::Auto),
        max_output_tokens: Some(4096),
        ..IrRequest::default()
    };

    let body = serde_json::to_value(super::render_on_demand_body(
        &ir,
        false,
        Some("keep decisions"),
        2048,
    )?)?;

    assert_eq!(
        crate::test_support::at(&body, "/compaction"),
        &serde_json::json!({"type": "summarize", "instructions": "keep decisions"})
    );

    assert_eq!(crate::test_support::at(&body, "/max_tokens"), 2048);

    assert_eq!(
        crate::test_support::at(&body, "/tool_choice"),
        &serde_json::json!({"type": "none"}),
        "a compaction turn must never answer by calling a tool"
    );

    let bare_ir = IrRequest {
        model: "claude-up".to_string(),
        messages: vec![user_text("q")],
        max_output_tokens: Some(4096),
        ..IrRequest::default()
    };

    let body = serde_json::to_value(super::render_on_demand_body(&bare_ir, false, None, 2048)?)?;

    assert_eq!(
        crate::test_support::at(&body, "/compaction"),
        &serde_json::json!({"type": "summarize"})
    );

    assert!(body.get("tool_choice").is_none());

    Ok(())
}

/// The threshold body carries the caller's vendor-shaped `compact_20260112` edit objects
/// verbatim, omitting the members the caller left absent.
#[test]
fn threshold_body_renders_vendor_edits() -> Result<(), Box<dyn StdError>> {
    use crate::ir::compaction::{InputTokenTrigger, ThresholdEdit};

    let edits = vec![
        ThresholdEdit {
            trigger: Some(InputTokenTrigger { value: 75_000 }),
            should_pause_after_compaction: Some(true),
            instructions: Some("keep decisions".to_string()),
        },
        ThresholdEdit {
            trigger: None,
            should_pause_after_compaction: None,
            instructions: None,
        },
    ];

    let body = serde_json::to_value(super::render_threshold_body(&base_ir(), false, &edits)?)?;

    assert_eq!(
        crate::test_support::at(&body, "/context_management"),
        &serde_json::json!({"edits": [
            {"type": "compact_20260112", "trigger": {"type": "input_tokens", "value": 75000},
             "pause_after_compaction": true, "instructions": "keep decisions"},
            {"type": "compact_20260112"}
        ]})
    );

    Ok(())
}

/// The token-count body renders only the count endpoint's shape: model, system, messages, and
/// tools, with no generation-only control.
#[test]
fn count_tokens_body_strips_generation_controls() -> Result<(), Box<dyn StdError>> {
    let mut ir = base_ir();

    ir.instructions = Some("Be terse.".to_string());
    ir.tools = vec![IrTool {
        name: "find".to_string(),
        description: None,
        parameters: None,
    }];
    ir.max_output_tokens = Some(4096);
    ir.reasoning_effort = Some(ReasoningLevel::High);

    let body = serde_json::to_value(super::render_count_tokens_request(&ir)?)?;

    let keys = body
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>());

    assert_eq!(
        keys,
        Some(vec![
            "messages".to_string(),
            "model".to_string(),
            "system".to_string(),
            "tools".to_string(),
        ])
    );

    Ok(())
}
