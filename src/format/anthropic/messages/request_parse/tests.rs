//! Inbound request parsing tests for the Anthropic Messages client surface: supported controls,
//! strict rejection of unknown members and unsupported semantic controls, content blocks, and the
//! compaction controls and blocks (`docs/reference/inference.md`).
#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section on every \
              test would document nothing its name does not"
)]

use crate::{
    format::{
        BoundaryPlacement, CompactionBoundary, CompactionControl, NativeArtifact, ParsedRequest,
        compaction_control::ThresholdEdit,
    },
    ir::{
        IrImage, IrMessage, IrPart, IrResponseFormat, IrRole, IrToolChoice, StateFamily, StateItem,
    },
    vocabulary::ReasoningLevel,
};
use serde_json::Value;
use std::error::Error as StdError;

/// The minimal valid request body every scenario starts from.
fn base_body() -> Value {
    serde_json::json!({
        "model": "claude-public",
        "max_tokens": 512,
        "messages": [{"role": "user", "content": "hello"}]
    })
}

/// Extends the base body with one top-level member, replacing an existing one.
#[expect(clippy::expect_used, reason = "the base fixture body is an object")]
fn with_member(member: &str, value: Value) -> Value {
    let mut body = base_body();

    body.as_object_mut()
        .expect("the base fixture body is an object")
        .insert(member.to_string(), value);

    body
}

/// Extends the base body with several top-level members, replacing existing ones.
#[expect(clippy::expect_used, reason = "the base fixture body is an object")]
fn with_members(members: &[(&str, Value)]) -> Value {
    let mut body = base_body();

    let object = body
        .as_object_mut()
        .expect("the base fixture body is an object");

    for (member, value) in members {
        object.insert((*member).to_string(), value.clone());
    }

    body
}

/// Replaces or inserts one top-level member of a fixture body.
#[expect(clippy::expect_used, reason = "fixture bodies are objects")]
fn set_member(mut body: Value, member: &str, value: Value) -> Value {
    body.as_object_mut()
        .expect("fixture bodies are objects")
        .insert(member.to_string(), value);

    body
}

/// Serializes a fixture body into the bytes the parser reads.
#[expect(
    clippy::expect_used,
    reason = "fixture bodies serialize by construction"
)]
fn wire_bytes(body: &Value) -> Vec<u8> {
    serde_json::to_vec(body).expect("fixture bodies serialize")
}

/// Parses a fixture body carrying the given `anthropic-beta` entries.
fn parse_with_betas(body: &Value, betas: &[&str]) -> Result<ParsedRequest, Box<dyn StdError>> {
    let betas = betas
        .iter()
        .map(|beta| (*beta).to_string())
        .collect::<Vec<_>>();
    let parsed = super::parse_inbound_request(&wire_bytes(body), &betas)?;

    Ok(parsed)
}

/// Parses a fixture body carrying no inbound beta entries.
fn parse(body: &Value) -> Result<ParsedRequest, Box<dyn StdError>> {
    parse_with_betas(body, &[])
}

/// Asserts a fixture body is rejected and returns the rendered rejection message.
#[expect(
    clippy::expect_used,
    reason = "the rejection under test is the expected outcome"
)]
fn rejection_message(body: &Value) -> String {
    let error = super::parse_inbound_request(&wire_bytes(body), &[])
        .expect_err("the fixture body must be rejected");

    error.to_string()
}

/// Asserts a fixture body is rejected naming `param` and returns the rendered message.
#[expect(
    clippy::expect_used,
    reason = "the rejection under test is the expected outcome"
)]
fn field_rejection_message(body: &Value, param: &str) -> String {
    let error = super::parse_inbound_request(&wire_bytes(body), &[])
        .expect_err("the fixture body must be rejected");

    assert_eq!(error.param.as_deref(), Some(param), "rejection: {error}");

    error.to_string()
}

/// The full supported control surface maps onto the IR: envelope fields, a custom tool with its
/// ignored presentation members, auto tool choice, every user/assistant content block kind, and
/// the reasoning blocks' verbatim opaque replay payloads.
#[test]
fn full_supported_request_maps_to_ir() -> Result<(), Box<dyn StdError>> {
    let body = serde_json::json!({
        "model": "claude-public",
        "max_tokens": 1024,
        "stream": true,
        "temperature": 0.5,
        "top_p": 0.9,
        "system": "Be terse.",
        "tools": [{
            "name": "find",
            "description": "Search",
            "type": "custom",
            "input_schema": {"type": "object"},
            "strict": true,
            "cache_control": {"type": "ephemeral"}
        }],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
        "metadata": {"user_id": "u"},
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "Look", "cache_control": {"type": "ephemeral"}},
                {"type": "image", "source": {
                    "type": "base64", "media_type": "image/png", "data": "QUJD"
                }}
            ]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "ponder", "signature": "sig-1"},
                {"type": "redacted_thinking", "data": "cipher"},
                {"type": "text", "text": "Answer"},
                {"type": "tool_use", "id": "call_1", "name": "find", "input": {"q": "x"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_1", "is_error": true,
                 "content": [{"type": "text", "text": "boom"}]}
            ]}
        ]
    });

    let parsed = parse(&body)?;
    let request = parsed.ir;

    assert_eq!(request.model, "claude-public");
    assert_eq!(request.max_output_tokens, Some(1024));
    assert_eq!(request.temperature, Some(0.5));
    assert_eq!(request.top_p, Some(0.9));
    assert_eq!(request.instructions.as_deref(), Some("Be terse."));
    assert_eq!(request.reasoning_effort, None);
    assert!(parsed.is_streaming);
    assert!(!parsed.should_include_usage);
    assert_eq!(request.tool_choice, Some(IrToolChoice::Auto));
    assert_eq!(request.tools.len(), 1);

    assert_eq!(
        request.messages,
        vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![
                    IrPart::Text {
                        text: "Look".to_string()
                    },
                    IrPart::Image {
                        image: IrImage::Inline {
                            mime_type: "image/png".to_string(),
                            data_base64: "QUJD".to_string(),
                        },
                    },
                ],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::Reasoning {
                        text: "ponder".to_string(),
                        signature: Some("sig-1".to_string()),
                        opaque: serde_json::json!({
                            "type": "thinking",
                            "thinking": "ponder",
                            "signature": "sig-1"
                        }),
                    },
                    IrPart::Reasoning {
                        text: String::new(),
                        signature: None,
                        opaque: serde_json::json!({"type": "redacted_thinking", "data": "cipher"}),
                    },
                    IrPart::Text {
                        text: "Answer".to_string()
                    },
                    IrPart::ToolCall {
                        call_id: "call_1".to_string(),
                        name: "find".to_string(),
                        arguments: "{\"q\":\"x\"}".to_string(),
                        thought_signature: None,
                    },
                ],
            },
            IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id: "call_1".to_string(),
                    content: "boom".to_string(),
                    is_error: true,
                }],
            },
        ]
    );

    Ok(())
}

/// The `system` array form joins its text blocks with the blank-line separator and ignores the
/// per-block presentation members; the scalar string form carries one instruction item.
#[test]
fn system_forms_join_into_instructions() -> Result<(), Box<dyn StdError>> {
    let array_body = with_member(
        "system",
        serde_json::json!([
            {"type": "text", "text": "First", "citations": {"enabled": true}},
            {"type": "text", "text": "Second", "cache_control": {"type": "ephemeral"}}
        ]),
    );

    let parsed = parse(&array_body)?;

    assert_eq!(
        parsed.ir.instructions.as_deref(),
        Some("First\n\nSecond"),
        "array blocks join on the blank line"
    );

    let scalar_body = with_member("system", Value::String("Solo".to_string()));
    let parsed = parse(&scalar_body)?;

    assert_eq!(parsed.ir.instructions.as_deref(), Some("Solo"));

    Ok(())
}

/// A user turn mixing ordinary content with tool results chunks into consecutive user/tool IR
/// messages: same-side neighbours merge and sides alternate in encounter order.
#[test]
fn user_turn_mixed_content_chunks_into_role_runs() -> Result<(), Box<dyn StdError>> {
    let body = with_member(
        "messages",
        serde_json::json!([
            {"role": "user", "content": [
                {"type": "text", "text": "before"},
                {"type": "tool_result", "tool_use_id": "t1", "content": "r1"},
                {"type": "image", "source": {"type": "url", "url": "https://example.test/i.png"}},
                {"type": "tool_result", "tool_use_id": "t2", "content": "r2"},
                {"type": "tool_result", "tool_use_id": "t3", "content": "r3"},
                {"type": "text", "text": "after"}
            ]}
        ]),
    );

    let parsed = parse(&body)?;

    assert_eq!(
        parsed.ir.messages,
        vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "before".to_string()
                }],
            },
            IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id: "t1".to_string(),
                    content: "r1".to_string(),
                    is_error: false,
                }],
            },
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Image {
                    image: IrImage::Url("https://example.test/i.png".to_string())
                },],
            },
            IrMessage {
                role: IrRole::Tool,
                parts: vec![
                    IrPart::ToolOutput {
                        call_id: "t2".to_string(),
                        content: "r2".to_string(),
                        is_error: false,
                    },
                    IrPart::ToolOutput {
                        call_id: "t3".to_string(),
                        content: "r3".to_string(),
                        is_error: false,
                    },
                ],
            },
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "after".to_string()
                }],
            },
        ]
    );

    Ok(())
}

/// Tool-result content flattens under the shared text-part contract: the string form carries
/// verbatim, the array form joins text parts on the blank line with leading empties skipped, and
/// a missing `is_error` defaults to false.
#[test]
fn tool_result_content_flattens_text_parts() -> Result<(), Box<dyn StdError>> {
    let body = with_member(
        "messages",
        serde_json::json!([
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "plain"},
                {"type": "tool_result", "tool_use_id": "t2", "content": [
                    {"type": "text", "text": ""},
                    {"type": "text", "text": "a"},
                    {"type": "text", "text": "b"}
                ]},
                {"type": "tool_result", "tool_use_id": "t3", "content": ""}
            ]}
        ]),
    );

    let parsed = parse(&body)?;

    assert_eq!(
        parsed.ir.messages,
        vec![IrMessage {
            role: IrRole::Tool,
            parts: vec![
                IrPart::ToolOutput {
                    call_id: "t1".to_string(),
                    content: "plain".to_string(),
                    is_error: false,
                },
                IrPart::ToolOutput {
                    call_id: "t2".to_string(),
                    content: "a\n\nb".to_string(),
                    is_error: false,
                },
                IrPart::ToolOutput {
                    call_id: "t3".to_string(),
                    content: String::new(),
                    is_error: false,
                },
            ],
        }]
    );

    Ok(())
}

/// Strictness: unknown members on every parsed level, a missing required member, an empty
/// history, and misplaced blocks are all rejections, each naming or describing the offender.
#[test]
fn strictness_rejections() {
    let cases = vec![
        (
            with_member("bogus", serde_json::json!(true)),
            "bogus".to_string(),
        ),
        (
            with_member("messages", serde_json::json!([])),
            "at least one message".to_string(),
        ),
        (
            serde_json::json!({
                "model": "claude-public",
                "messages": [{"role": "user", "content": "hi"}]
            }),
            "max_tokens".to_string(),
        ),
        (
            with_member(
                "messages",
                serde_json::json!([
                    {"role": "user", "content": [
                        {"type": "text", "text": "x", "bogus": 1}
                    ]}
                ]),
            ),
            "bogus".to_string(),
        ),
        (
            with_member(
                "tools",
                serde_json::json!([{"name": "t", "input_schema": {}, "bogus": 1}]),
            ),
            "bogus".to_string(),
        ),
        (
            with_member(
                "messages",
                serde_json::json!([
                    {"role": "user", "content": "hi", "bogus": 1}
                ]),
            ),
            "bogus".to_string(),
        ),
        (
            with_member(
                "messages",
                serde_json::json!([
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "a"},
                        {"type": "tool_use", "id": "t", "name": "n", "input": [1]}
                    ]}
                ]),
            ),
            "must be a JSON object".to_string(),
        ),
    ];

    for (body, fragment) in cases {
        let message = rejection_message(&body);

        assert!(
            message.contains(&fragment),
            "rejection `{message}` must name `{fragment}`"
        );
    }
}

/// Known-but-unsupported semantic controls are rejected naming their field, never approximated
/// (`docs/reference/inference.md`).
#[test]
fn unsupported_semantic_controls_reject_by_name() {
    let cases = vec![
        (with_member("top_k", serde_json::json!(5)), "top_k"),
        (
            with_member("diagnostics", serde_json::json!({})),
            "diagnostics",
        ),
        (
            with_member("inference_geo", serde_json::json!({})),
            "inference_geo",
        ),
        (
            with_member(
                "tools",
                serde_json::json!([{
                    "name": "web",
                    "type": "web_search_20250305",
                    "input_schema": {}
                }]),
            ),
            "type",
        ),
        (
            with_member(
                "messages",
                serde_json::json!([
                    {"role": "assistant", "content": [
                        {"type": "document", "source": {"type": "url"}}
                    ]}
                ]),
            ),
            "content",
        ),
        (
            with_member(
                "messages",
                serde_json::json!([
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "t", "content": [
                            {"type": "image", "source": {"type": "url", "url": "https://x.test/a"}}
                        ]}
                    ]}
                ]),
            ),
            "content",
        ),
        (
            with_member(
                "system",
                serde_json::json!([{"type": "search", "text": "x"}]),
            ),
            "system",
        ),
    ];

    for (body, param) in cases {
        field_rejection_message(&body, param);
    }
}

/// Presentation-only controls are accepted and ignored: request metadata, tiers, containers,
/// caching breakpoints, citations, tool `strict`, the parallel-use switch, and the legacy
/// thinking display member never reach the IR.
#[test]
fn ignored_presentation_members_are_accepted() -> Result<(), Box<dyn StdError>> {
    let body = serde_json::json!({
        "model": "claude-public",
        "max_tokens": 64,
        "metadata": {"user_id": "u"},
        "service_tier": "standard",
        "container": {"id": "c"},
        "cache_control": {"type": "ephemeral"},
        "thinking_summaries": {"enabled": true},
        "tools": [{
            "name": "find",
            "input_schema": {"type": "object"},
            "strict": true
        }],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "q", "cache_control": {"type": "ephemeral"},
                 "citations": {"enabled": true}}
            ]},
            {"role": "assistant", "content": [
                {"type": "text", "text": "a", "cache_control": {"type": "ephemeral"},
                 "citations": {"enabled": true}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_1", "content": "r",
                 "cache_control": {"type": "ephemeral"}}
            ]}
        ]
    });

    let parsed = parse(&body)?;

    assert_eq!(parsed.ir.messages.len(), 3);

    assert_eq!(parsed.ir.tools.len(), 1);

    Ok(())
}

/// The legacy thinking control maps by variant: `disabled` parses into the `none` level,
/// `enabled` and `adaptive` are accepted-and-ignored metadata, and an explicit `none` effort
/// beside the disabled control is the documented conflict.
#[test]
fn thinking_control_semantics() -> Result<(), Box<dyn StdError>> {
    let disabled = with_member("thinking", serde_json::json!({"type": "disabled"}));

    assert_eq!(
        parse(&disabled)?.ir.reasoning_effort,
        Some(ReasoningLevel::None)
    );

    let enabled = with_member(
        "thinking",
        serde_json::json!({"type": "enabled", "display": "raw"}),
    );

    assert_eq!(parse(&enabled)?.ir.reasoning_effort, None);

    let adaptive = with_member("thinking", serde_json::json!({"type": "adaptive"}));

    assert_eq!(parse(&adaptive)?.ir.reasoning_effort, None);

    // The legacy numeric budget mechanism is the one rejected `enabled` member.
    let budget = with_member(
        "thinking",
        serde_json::json!({"type": "enabled", "budget_tokens": 1024}),
    );

    field_rejection_message(&budget, "budget_tokens");

    // A disabled legacy control contradicts an explicit effort level; an ignored one beside it
    // is simply redundant.
    let conflict = with_members(&[
        ("thinking", serde_json::json!({"type": "disabled"})),
        ("output_config", serde_json::json!({"effort": "high"})),
    ]);

    field_rejection_message(&conflict, "thinking");

    let redundant = with_members(&[
        ("thinking", serde_json::json!({"type": "enabled"})),
        ("output_config", serde_json::json!({"effort": "high"})),
    ]);

    assert_eq!(
        parse(&redundant)?.ir.reasoning_effort,
        Some(ReasoningLevel::High)
    );

    Ok(())
}

/// `output_config` parses the effort vocabulary and the JSON-schema output format independently:
/// each member is optional, explicit nulls mean absent, and malformed format shapes reject
/// naming `format`.
#[test]
fn output_config_semantics() -> Result<(), Box<dyn StdError>> {
    let effort = with_member("output_config", serde_json::json!({"effort": "xhigh"}));
    let parsed = parse(&effort)?;

    assert_eq!(parsed.ir.reasoning_effort, Some(ReasoningLevel::XHigh));
    assert_eq!(parsed.ir.response_format, None);

    let unknown_effort = with_member("output_config", serde_json::json!({"effort": "ultra"}));

    field_rejection_message(&unknown_effort, "output_config");

    let schema = with_member(
        "output_config",
        serde_json::json!({
            "effort": "low",
            "format": {"type": "json_schema", "schema": {"type": "object"}}
        }),
    );
    let parsed = parse(&schema)?;

    assert_eq!(parsed.ir.reasoning_effort, Some(ReasoningLevel::Low));

    assert_eq!(
        parsed.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "response".to_string(),
            schema: serde_json::json!({"type": "object"}),
        })
    );

    // The synthesized schema name is the Gemini surfaces' stand-in, dropped again by the
    // upstream render; the effort stays readable beside it.
    let format_only = with_member(
        "output_config",
        serde_json::json!({"format": {"type": "json_schema", "schema": {}}}),
    );

    assert_eq!(
        parse(&format_only)?.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "response".to_string(),
            schema: serde_json::json!({}),
        })
    );

    let null_members = with_member(
        "output_config",
        serde_json::json!({"effort": null, "format": null}),
    );
    let parsed = parse(&null_members)?;

    assert_eq!(parsed.ir.reasoning_effort, None);
    assert_eq!(parsed.ir.response_format, None);

    for malformed in [
        serde_json::json!({"format": {"type": "json"}}),
        serde_json::json!({"format": {"type": "json_schema"}}),
        serde_json::json!({"format": {"type": "json_schema", "schema": [1]}}),
    ] {
        let body = with_member("output_config", malformed);

        field_rejection_message(&body, "format");
    }

    Ok(())
}

/// Tool choice validates against the advertised tools: it requires tools to restrict, the
/// `tool` form requires its function name, and unknown kinds are rejected.
#[test]
fn tool_choice_semantics() -> Result<(), Box<dyn StdError>> {
    let no_tools = with_member("tool_choice", serde_json::json!({"type": "auto"}));

    field_rejection_message(&no_tools, "tool_choice");

    let unnamed = with_member("tool_choice", serde_json::json!({"type": "tool"}));

    field_rejection_message(&unnamed, "tool_choice");

    let unknown = with_member("tool_choice", serde_json::json!({"type": "bogus"}));

    field_rejection_message(&unknown, "tool_choice");

    let body_with_tools = with_member(
        "tools",
        serde_json::json!([{"name": "find", "input_schema": {}}]),
    );

    for (choice, expected) in [
        (serde_json::json!({"type": "auto"}), IrToolChoice::Auto),
        (serde_json::json!({"type": "none"}), IrToolChoice::None),
        (serde_json::json!({"type": "any"}), IrToolChoice::Required),
        (
            serde_json::json!({"type": "tool", "name": "find"}),
            IrToolChoice::Function {
                name: "find".to_string(),
            },
        ),
    ] {
        let body = set_member(body_with_tools.clone(), "tool_choice", choice);

        assert_eq!(parse(&body)?.ir.tool_choice, Some(expected));
    }

    Ok(())
}

/// The on-demand compaction control parses with normalized instructions — under its required
/// inbound beta: absent and whitespace-only mean none, the documented character cap is enforced
/// naming `instructions`, and an unknown operation type is rejected naming `type`.
#[test]
fn on_demand_control_semantics() -> Result<(), Box<dyn StdError>> {
    let bare = with_member("compaction", serde_json::json!({"type": "summarize"}));
    let parsed = parse_with_betas(&bare, &["compact-2026-09-04"])?;

    assert_eq!(
        parsed.compaction.ok_or("the control parses")?.control,
        Some(CompactionControl::MessagesOnDemand { instructions: None })
    );

    for (supplied, expected) in [
        (None, None),
        (Some("   \n\t".to_string()), None),
        (
            Some("keep decisions".to_string()),
            Some("keep decisions".to_string()),
        ),
    ] {
        let mut value = serde_json::json!({"type": "summarize"});

        if let Some(instructions) = supplied.clone() {
            value = set_member(value, "instructions", Value::String(instructions));
        }

        let body = with_member("compaction", value);
        let parsed = parse_with_betas(&body, &["compact-2026-09-04"])?;

        assert_eq!(
            parsed.compaction.ok_or("the control parses")?.control,
            Some(CompactionControl::MessagesOnDemand {
                instructions: expected
            }),
            "null and whitespace-only instructions mean none"
        );
    }

    // The instruction bounds validate before the inbound beta requirement.
    let capped = with_member(
        "compaction",
        serde_json::json!({"type": "summarize", "instructions": "x".repeat(16_385)}),
    );

    field_rejection_message(&capped, "instructions");

    let at_cap = with_member(
        "compaction",
        serde_json::json!({"type": "summarize", "instructions": "x".repeat(16_384)}),
    );

    assert!(
        parse_with_betas(&at_cap, &["compact-2026-09-04"]).is_ok(),
        "the documented cap itself is valid"
    );

    let unknown = with_member("compaction", serde_json::json!({"type": "archive"}));

    field_rejection_message(&unknown, "type");

    Ok(())
}

/// The on-demand contract requires history and excludes forced tool use and output formats, and
/// a history ending in an unresolved tool call is rejected instead of summarized.
#[test]
fn on_demand_context_contract() {
    let empty = serde_json::json!({
        "model": "claude-public",
        "max_tokens": 64,
        "messages": [],
        "compaction": {"type": "summarize"}
    });

    let message = field_rejection_message(&empty, "messages");

    assert!(
        message.contains("nothing to summarize"),
        "empty-history wording names the summarization: {message}"
    );

    let forced_any = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "tools",
            serde_json::json!([{"name": "find", "input_schema": {}}]),
        ),
        ("tool_choice", serde_json::json!({"type": "any"})),
    ]);

    field_rejection_message(&forced_any, "tool_choice");

    let forced_tool = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "tools",
            serde_json::json!([{"name": "find", "input_schema": {}}]),
        ),
        (
            "tool_choice",
            serde_json::json!({"type": "tool", "name": "find"}),
        ),
    ]);

    field_rejection_message(&forced_tool, "tool_choice");

    let with_format = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "output_config",
            serde_json::json!({"format": {"type": "json_schema", "schema": {}}}),
        ),
    ]);

    field_rejection_message(&with_format, "format");

    // Auto tool choice beside the control is valid: only forced use contradicts the contract.
    let auto_choice = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "tools",
            serde_json::json!([{"name": "find", "input_schema": {}}]),
        ),
        ("tool_choice", serde_json::json!({"type": "auto"})),
    ]);

    assert!(
        parse_with_betas(&auto_choice, &["compact-2026-09-04"]).is_ok(),
        "auto tool choice stays valid"
    );

    let unresolved = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "working"},
                    {"type": "tool_use", "id": "t1", "name": "find", "input": {}}
                ]}
            ]),
        ),
    ]);

    field_rejection_message(&unresolved, "messages");

    // A matching result resolves the call wherever it sits in the history.
    let resolved = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "find", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "r"}
                ]}
            ]),
        ),
    ]);

    assert!(
        parse_with_betas(&resolved, &["compact-2026-09-04"]).is_ok(),
        "a resolved call is summarizable"
    );
}

/// The threshold compaction control parses its `compact_20260112` edits — under its required
/// inbound beta: the default and custom triggers, the pause switch, and instructions; invalid
/// triggers are rejected, never clamped.
#[test]
fn threshold_control_semantics() -> Result<(), Box<dyn StdError>> {
    let default_edit = with_member(
        "context_management",
        serde_json::json!({"edits": [{"type": "compact_20260112"}]}),
    );
    let parsed = parse_with_betas(&default_edit, &["compact-2026-01-12"])?;

    assert_eq!(
        parsed.compaction.ok_or("the control parses")?.control,
        Some(CompactionControl::MessagesThreshold {
            edits: vec![ThresholdEdit {
                trigger: None,
                pause_after_compaction: false,
                instructions: None,
            }],
        })
    );

    let custom = with_member(
        "context_management",
        serde_json::json!({"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 75_000},
            "pause_after_compaction": true,
            "instructions": "keep decisions"
        }]}),
    );
    let parsed = parse_with_betas(&custom, &["compact-2026-01-12"])?;

    assert_eq!(
        parsed.compaction.ok_or("the control parses")?.control,
        Some(CompactionControl::MessagesThreshold {
            edits: vec![ThresholdEdit {
                trigger: Some(75_000),
                pause_after_compaction: true,
                instructions: Some("keep decisions".to_string()),
            }],
        })
    );

    let below_minimum = with_member(
        "context_management",
        serde_json::json!({"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 49_999}
        }]}),
    );

    field_rejection_message(&below_minimum, "trigger");

    let wrong_trigger = with_member(
        "context_management",
        serde_json::json!({"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "output_tokens", "value": 50_000}
        }]}),
    );

    field_rejection_message(&wrong_trigger, "trigger.type");

    let unsupported_edit = with_member(
        "context_management",
        serde_json::json!({"edits": [{"type": "clear_thinking_20251015"}]}),
    );

    field_rejection_message(&unsupported_edit, "type");

    Ok(())
}

/// The two compaction controls are mutually exclusive, and each requires its own inbound
/// `anthropic-beta` entry spelled exactly.
#[test]
fn compaction_controls_and_betas() {
    let both = with_members(&[
        ("compaction", serde_json::json!({"type": "summarize"})),
        (
            "context_management",
            serde_json::json!({"edits": [{"type": "compact_20260112"}]}),
        ),
    ]);

    field_rejection_message(&both, "compaction");

    let on_demand = with_member("compaction", serde_json::json!({"type": "summarize"}));

    field_rejection_message(&on_demand, "anthropic-beta");

    assert!(parse_with_betas(&on_demand, &["compact-2026-09-04"]).is_ok());

    let threshold = with_member(
        "context_management",
        serde_json::json!({"edits": [{"type": "compact_20260112"}]}),
    );

    field_rejection_message(&threshold, "anthropic-beta");

    assert!(parse_with_betas(&threshold, &["compact-2026-01-12"]).is_ok());

    let on_demand_history = with_member(
        "messages",
        serde_json::json!([
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "s", "signature": "native"}
            ]}
        ]),
    );

    field_rejection_message(&on_demand_history, "anthropic-beta");

    assert!(parse_with_betas(&on_demand_history, &["compact-2026-09-04"]).is_ok());

    let historical_block = with_member(
        "messages",
        serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "s", "signature": "native"}
            ]}
        ]),
    );

    field_rejection_message(&historical_block, "anthropic-beta");

    assert!(parse_with_betas(&historical_block, &["compact-2026-01-12"]).is_ok());
}

/// Native compaction blocks record their boundary beside the history conversion: the historical
/// and leading placements, the null-content no-op, and the threshold rule that a native block
/// cannot coexist with a `compact_20260112` edit while a gateway carrier can.
#[test]
fn native_compaction_blocks_record_boundaries() -> Result<(), Box<dyn StdError>> {
    let historical = parse_with_betas(
        &with_member(
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "a"},
                    {"type": "compaction", "content": "s", "signature": "native"}
                ]}
            ]),
        ),
        &["compact-2026-01-12"],
    )?;

    assert_eq!(
        historical.compaction.ok_or("the block parses")?.boundaries,
        vec![CompactionBoundary::Native {
            offset: 1,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: false,
                placement: BoundaryPlacement::Historical,
            },
        }]
    );

    let leading = parse_with_betas(
        &with_member(
            "messages",
            serde_json::json!([
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "s", "signature": "native"}
                ]}
            ]),
        ),
        &["compact-2026-09-04"],
    )?;

    assert_eq!(
        leading.compaction.ok_or("the block parses")?.boundaries,
        vec![CompactionBoundary::Native {
            offset: 0,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: false,
                placement: BoundaryPlacement::Leading,
            },
        }]
    );

    let no_op = parse_with_betas(
        &with_member(
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "signature": "native"}
                ]}
            ]),
        ),
        &["compact-2026-01-12"],
    )?;

    assert_eq!(
        no_op.compaction.ok_or("the block parses")?.boundaries,
        vec![CompactionBoundary::Native {
            offset: 1,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: true,
                placement: BoundaryPlacement::Historical,
            },
        }]
    );

    // A threshold edit cannot run on a request carrying a native block.
    let conflict = with_members(&[
        (
            "context_management",
            serde_json::json!({"edits": [{"type": "compact_20260112"}]}),
        ),
        (
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "s", "signature": "native"}
                ]}
            ]),
        ),
    ]);

    field_rejection_message(&conflict, "context_management");

    // A gateway carrier is not native state and does not trigger the rule.
    let carrier = with_members(&[
        (
            "context_management",
            serde_json::json!({"edits": [{"type": "compact_20260112"}]}),
        ),
        (
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "s",
                     "signature": "janusgate.compaction.v1.primary.ABCD"}
                ]}
            ]),
        ),
    ]);

    assert!(parse_with_betas(&carrier, &["compact-2026-01-12"]).is_ok());

    Ok(())
}

/// A gateway carrier's namespaced signature records a gateway boundary binding its visible
/// content, with no inbound beta requirement of its own.
#[test]
fn gateway_carrier_block_records_a_gateway_boundary() -> Result<(), Box<dyn StdError>> {
    let carrier = "janusgate.compaction.v1.primary.ABCD";
    let body = with_member(
        "messages",
        serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "visible", "signature": carrier}
            ]}
        ]),
    );

    let parsed = parse(&body)?;

    assert_eq!(
        parsed.compaction.ok_or("the block parses")?.boundaries,
        vec![CompactionBoundary::Gateway {
            offset: 1,
            carrier: carrier.to_string(),
            visible_content: Some("visible".to_string()),
        }]
    );

    Ok(())
}

/// Exactly one compaction block is accepted per request, and a second one is rejected wherever
/// it appears: two blocks in one turn, or one block in each of two turns.
#[test]
fn compaction_block_count_and_placement() {
    let doubled = with_member(
        "messages",
        serde_json::json!([
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "s", "signature": "one"},
                {"type": "compaction", "content": "t", "signature": "two"}
            ]}
        ]),
    );

    field_rejection_message(&doubled, "content");

    let split = with_member(
        "messages",
        serde_json::json!([
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "s", "signature": "one"}
            ]},
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "t", "signature": "two"}
            ]}
        ]),
    );

    field_rejection_message(&split, "content");
}

/// The recorded state item preserves the vendor block verbatim: the same members with the same
/// presence, so an absent member is never rendered as null.
#[test]
fn state_items_preserve_block_member_presence() -> Result<(), Box<dyn StdError>> {
    let full = parse_with_betas(
        &with_member(
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "s", "signature": "sig",
                     "encrypted_content": "enc", "tool_changes": [{"name": "t"}]}
                ]}
            ]),
        ),
        &["compact-2026-01-12"],
    )?;

    assert_eq!(
        full.ir.state_items,
        vec![StateItem {
            offset: 1,
            item: serde_json::json!({
                "type": "compaction",
                "content": "s",
                "signature": "sig",
                "encrypted_content": "enc",
                "tool_changes": [{"name": "t"}]
            }),
            is_gateway: false,
            is_compaction: true,
            family: StateFamily::Messages,
        }]
    );

    let sparse = parse_with_betas(
        &with_member(
            "messages",
            serde_json::json!([
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "s", "signature": "sig"}
                ]}
            ]),
        ),
        &["compact-2026-01-12"],
    )?;

    assert_eq!(
        sparse.ir.state_items,
        vec![StateItem {
            offset: 1,
            item: serde_json::json!({"type": "compaction", "content": "s", "signature": "sig"}),
            is_gateway: false,
            is_compaction: true,
            family: StateFamily::Messages,
        }]
    );

    Ok(())
}

/// A compaction-only replay turn yields no IR message: the state rides beside the loop at the
/// offset of the messages converted before it.
#[test]
fn compaction_only_turn_yields_no_ir_message() -> Result<(), Box<dyn StdError>> {
    let body = with_member(
        "messages",
        serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": [
                {"type": "compaction", "content": "s", "signature": "native"}
            ]}
        ]),
    );

    let parsed = parse_with_betas(&body, &["compact-2026-01-12"])?;

    assert_eq!(
        parsed.ir.messages,
        vec![IrMessage {
            role: IrRole::User,
            parts: vec![IrPart::Text {
                text: "q".to_string()
            }],
        }]
    );

    assert_eq!(
        parsed.compaction.map(|compaction| compaction.boundaries),
        Some(vec![CompactionBoundary::Native {
            offset: 1,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: false,
                placement: BoundaryPlacement::Historical,
            },
        }])
    );

    Ok(())
}
