//! Tests for the Responses codec: the client request parser, the upstream request renderer, the
//! upstream response parser, the client response renderers, and the standalone compact endpoint's
//! wire behavior.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{
    CompactedResponseNative, SseRenderer, UpstreamParser, UpstreamUsage, client_render,
    compaction_item, parse_compact_request, parse_compact_response, render_compact_request_body,
    render_compacted_response, render_gateway_compacted_response,
    render_request_body_with_extensions, render_trigger_stream_frames, request_parse,
    window_affinity,
};
use crate::{
    affinity::fingerprint::Projector,
    api::Error,
    compaction_model::{identity::InstructionRole, sealing},
    format::{
        ClientFormat as _, ClientSseRenderer as _, CompactionBoundary, CompactionControl,
        NativeArtifact, ParsedInstructionItem, ParsedRequest, RenderOption,
        UpstreamEventParser as _, compaction_wire::Checkpoint,
    },
    ir::{
        InvalidRequest, IrAggregatedToolCall, IrAggregation, IrEvent, IrFinishReason, IrImage,
        IrMessage, IrPart, IrRequest, IrRole, IrToolChoice, IrUsage, StateFamily, StateItem,
        compaction::AutomaticEntry,
    },
    sse::SseFrame,
    test_support::at,
    vocabulary::ReasoningLevel,
};
use serde_json::{Value, json};
use std::error::Error as StdError;

/// Serializes the JSON fixture and parses it as a Responses request.
fn parse(body: Value) -> Result<Result<ParsedRequest, InvalidRequest>, Box<dyn StdError>> {
    let raw = serde_json::to_vec(&body)?;

    Ok(request_parse::parse_request(&raw))
}

/// Parses the JSON fixture into IR, failing the test when the request is rejected.
fn parsed(body: Value) -> Result<ParsedRequest, Box<dyn StdError>> {
    Ok(parse(body)??)
}

/// Turns a parse result into its rejection, failing the test when the parse succeeded.
fn rejection(
    parsed: Result<ParsedRequest, InvalidRequest>,
) -> Result<InvalidRequest, Box<dyn StdError>> {
    match parsed {
        Err(error) => Ok(error),

        Ok(_) => Err("expected the request to be rejected".into()),
    }
}

/// A minimal well-formed request body carrying one user turn.
fn minimal_body() -> Value {
    json!({
        "model": "public-model",
        "input": "hello"
    })
}

/// An IR request addressed to the upstream model, carrying nothing else.
fn ir_request() -> IrRequest {
    IrRequest {
        model: "upstream-model".to_string(),
        ..IrRequest::default()
    }
}

/// A user text message.
fn user_text(text: &str) -> IrMessage {
    IrMessage {
        role: IrRole::User,
        parts: vec![IrPart::Text {
            text: text.to_string(),
        }],
    }
}

/// An assistant message carrying one tool invocation.
fn assistant_call(call_id: &str, signature: Option<&str>) -> IrMessage {
    IrMessage {
        role: IrRole::Assistant,
        parts: vec![IrPart::ToolCall {
            call_id: call_id.to_string(),
            name: "find".to_string(),
            arguments: "{\"q\":1}".to_string(),
            thought_signature: signature.map(str::to_string),
        }],
    }
}

/// A tool message answering one invocation.
fn tool_output(call_id: &str, content: &str) -> IrMessage {
    IrMessage {
        role: IrRole::Tool,
        parts: vec![IrPart::ToolOutput {
            call_id: call_id.to_string(),
            content: content.to_string(),
            is_error: false,
        }],
    }
}

/// A bare upstream `data:` frame carrying the JSON payload.
fn data_frame(payload: Value) -> SseFrame {
    SseFrame {
        data: payload.to_string(),
        event: None,
    }
}

/// Splits one rendered SSE frame into its event name and JSON payload.
fn frame(raw: &str) -> Result<(String, Value), Box<dyn StdError>> {
    let (_, rest) = raw
        .split_once("event: ")
        .ok_or("the frame carries an event line")?;
    let (event, rest) = rest.split_once('\n').ok_or("the event line terminates")?;

    let payload = rest
        .strip_prefix("data: ")
        .ok_or("the frame carries a data line")?;
    let payload = payload
        .strip_suffix("\n\n")
        .ok_or("the frame is terminated")?;

    Ok((event.to_string(), serde_json::from_str(payload)?))
}

/// The first rendered frame of a batch, failing the test when the batch is empty.
fn first_frame(frames: &[String]) -> Result<&str, Box<dyn StdError>> {
    frames
        .first()
        .map(String::as_str)
        .ok_or("expected at least one frame".into())
}

/// Renders an IR request into its upstream JSON shape with no render options and no native
/// automatic controls.
fn rendered(ir: &IrRequest, is_streaming: bool) -> Result<Value, Box<dyn StdError>> {
    let body = render_request_body_with_extensions(ir, is_streaming, &[], None)?;

    Ok(serde_json::to_value(&body)?)
}

/// The usage observation the rendering tests carry.
fn usage() -> IrUsage {
    IrUsage {
        input_tokens: 10,
        output_tokens: 4,
        cached_tokens: Some(3),
        cache_write_tokens: Some(2),
        reasoning_tokens: Some(1),
    }
}

/// The gateway carrier marker the namespace-projection tests build fixtures from.
fn gateway_marker() -> String {
    format!("{}key.aGVsbG8", sealing::CARRIER_NAMESPACE)
}

/// One native reasoning state item carrying encrypted replay state.
fn encrypted_reasoning() -> Value {
    json!({
        "type": "reasoning",
        "id": "rs_1",
        "summary": [],
        "encrypted_content": "producer-cipher"
    })
}

/// The bare string input parses as one plain user turn with no compaction facts.
#[test]
fn bare_string_input_becomes_user_message() -> Result<(), Box<dyn StdError>> {
    let request = parsed(minimal_body())?;

    assert_eq!(request.ir.model, "public-model");
    assert_eq!(request.ir.messages, vec![user_text("hello")]);
    assert_eq!(request.ir.instructions, None);
    assert!(request.ir.state_items.is_empty());
    assert!(!request.ir.has_compaction_trigger);
    assert!(request.compaction.is_none());
    assert!(!request.is_streaming);
    assert!(!request.should_include_usage);

    Ok(())
}

/// The `instructions` field prepends the system/developer input items, and the parsed instruction
/// items keep their roles for the remote-compaction identity pass.
#[test]
fn instructions_prepend_message_instructions() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "instructions": "direct",
        "input": [
            {"type": "message", "role": "developer", "content": "dev"},
            {"type": "reasoning", "encrypted_content": "enc"},
            {"type": "message", "role": "system", "content": "sys"}
        ]
    }))?;

    assert_eq!(
        request.ir.instructions.unwrap_or_default(),
        "direct\n\ndev\n\nsys"
    );

    let compaction = request
        .compaction
        .as_ref()
        .ok_or("the state forces compaction facts")?;

    assert_eq!(compaction.control, None);
    assert_eq!(
        compaction.instruction_items,
        vec![
            ParsedInstructionItem {
                role: InstructionRole::System,
                text: "direct".to_string(),
            },
            ParsedInstructionItem {
                role: InstructionRole::Developer,
                text: "dev".to_string(),
            },
            ParsedInstructionItem {
                role: InstructionRole::System,
                text: "sys".to_string(),
            },
        ]
    );

    Ok(())
}

/// System and developer content is text-only: an image part is rejected naming the kind, because
/// silently dropping parts would lose request content.
#[test]
fn system_content_rejects_images() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "input": [{
            "type": "message",
            "role": "system",
            "content": [
                {"type": "input_text", "text": "a"},
                {"type": "input_image", "image_url": "https://example.com/x.png"}
            ]
        }]
    }))?)?;

    assert_eq!(error.param.clone().unwrap_or_default(), "content");
    assert!(error.to_string().contains("`input_image` is not supported"));

    Ok(())
}

/// The message roles map onto conversation turns, and any other role is a 400.
#[test]
fn message_roles_map_and_unknown_rejected() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "q"},
            {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "a", "annotations": [], "logprobs": []}
            ]}
        ]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![
            user_text("q"),
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Text {
                    text: "a".to_string(),
                }],
            },
        ]
    );

    let unknown = rejection(parse(json!({
        "model": "m",
        "input": [{"type": "message", "role": "tool", "content": "x"}]
    }))?)?;

    assert_eq!(unknown.param.clone().unwrap_or_default(), "role");
    assert!(
        unknown
            .to_string()
            .contains("Unknown input message role `tool`")
    );

    Ok(())
}

/// Function call items parse with their wire identity and replayable signature; their arguments
/// must encode a JSON object.
#[test]
fn function_call_items_parse() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "find",
                "arguments": "{\"q\":1}",
                "thought_signature": "sig",
                "id": "fc_1",
                "status": "completed"
            }
        ]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![assistant_call("call_1", Some("sig"))]
    );

    let non_object = rejection(parse(json!({
        "model": "m",
        "input": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": "find",
            "arguments": "[]"
        }]
    }))?)?;

    assert_eq!(non_object.param.clone().unwrap_or_default(), "arguments");

    Ok(())
}

/// Tool outputs accept the plain string and the text-part array forms; media parts are rejected
/// naming the kind.
#[test]
fn function_call_output_flattens_text_parts() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": [
            {"type": "function_call_output", "call_id": "call_1", "output": "plain"},
            {"type": "function_call_output", "call_id": "call_2", "output": [
                {"type": "input_text", "text": "a"},
                {"type": "input_text", "text": "b"}
            ]}
        ]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![
            tool_output("call_1", "plain"),
            tool_output("call_2", "a\n\nb")
        ]
    );

    let image = rejection(parse(json!({
        "model": "m",
        "input": [{
            "type": "function_call_output",
            "call_id": "call_1",
            "output": [{"type": "input_image", "image_url": "https://example.com/x.png"}]
        }]
    }))?)?;

    assert_eq!(image.param.clone().unwrap_or_default(), "output");
    assert!(image.to_string().contains("`input_image` is not supported"));

    let unknown = rejection(parse(json!({
        "model": "m",
        "input": [{
            "type": "function_call_output",
            "call_id": "call_1",
            "output": [{"type": "diagram"}]
        }]
    }))?)?;

    assert_eq!(unknown.param.clone().unwrap_or_default(), "output");
    assert_eq!(
        unknown.to_string(),
        "function_call_output output parts must be text"
    );

    Ok(())
}

/// A reasoning item carrying `encrypted_content` is opaque native state at its history position;
/// the replayed item re-renders its typed members and drops what it never typed.
#[test]
fn encrypted_reasoning_becomes_native_state() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "q"},
            {
                "type": "reasoning",
                "id": "rs_1",
                "summary": [],
                "encrypted_content": "producer-cipher",
                "vendor_extension": {"opaque": true}
            }
        ]
    }))?;

    assert_eq!(request.ir.messages.len(), 1);

    let state = request
        .ir
        .state_items
        .first()
        .ok_or("the item is native state")?;

    assert_eq!(state.offset, 1);
    assert!(!state.is_gateway);
    assert!(!state.is_compaction);
    assert_eq!(state.family, StateFamily::Responses);
    assert_eq!(at(&state.item, "/type"), &json!("reasoning"));
    assert_eq!(
        at(&state.item, "/encrypted_content"),
        &json!("producer-cipher")
    );
    assert_eq!(at(&state.item, "/id"), &json!("rs_1"));
    assert!(state.item.get("vendor_extension").is_none());

    Ok(())
}

/// A reasoning item without ciphertext carries no replayable state and is dropped.
#[test]
fn plain_reasoning_is_dropped() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "q"},
            {"type": "reasoning", "summary": [{"type": "summary_text", "text": "visible"}]}
        ]
    }))?;

    assert!(request.ir.state_items.is_empty());
    assert!(request.compaction.is_none());

    Ok(())
}

/// Compaction items split on the carrier namespace: a gateway marker is a gateway boundary and
/// gateway state, anything else is native compaction state.
#[test]
fn compaction_items_split_on_the_carrier_namespace() -> Result<(), Box<dyn StdError>> {
    let gateway = parsed(json!({
        "model": "m",
        "input": [{"type": "compaction", "encrypted_content": gateway_marker()}]
    }))?;

    assert_eq!(
        gateway.compaction.unwrap_or_default().boundaries,
        vec![CompactionBoundary::Gateway {
            offset: 0,
            carrier: gateway_marker(),
            visible_content: None,
        }]
    );

    let gateway_state = gateway
        .ir
        .state_items
        .first()
        .ok_or("the carrier is state")?;

    assert!(gateway_state.is_gateway);
    assert!(gateway_state.is_compaction);

    let native = parsed(json!({
        "model": "m",
        "input": [{"type": "compaction", "encrypted_content": "producer-cipher"}]
    }))?;

    assert_eq!(
        native.compaction.unwrap_or_default().boundaries,
        vec![CompactionBoundary::Native {
            offset: 0,
            artifact: NativeArtifact::ResponsesCompactionItem,
        }]
    );

    let native_state = native.ir.state_items.first().ok_or("the item is state")?;

    assert!(!native_state.is_gateway);
    assert!(native_state.is_compaction);
    assert_eq!(
        at(&native_state.item, "/encrypted_content"),
        &json!("producer-cipher")
    );

    Ok(())
}

/// The `compaction_summary` alias is accepted, an explicitly null id survives replay as null, and
/// an omitted id stays omitted.
#[test]
fn compaction_alias_and_null_identity_replay() -> Result<(), Box<dyn StdError>> {
    let aliased = parsed(json!({
        "model": "m",
        "input": [
            {"type": "compaction_summary", "id": "c1", "encrypted_content": "native"}
        ]
    }))?;

    assert_eq!(
        at(
            &aliased
                .ir
                .state_items
                .first()
                .ok_or("the aliased item is state")?
                .item,
            "/id"
        ),
        &json!("c1")
    );

    let nulled = parsed(json!({
        "model": "m",
        "input": [{"type": "compaction", "id": null, "encrypted_content": "native"}]
    }))?;

    assert_eq!(
        at(
            &nulled
                .ir
                .state_items
                .first()
                .ok_or("the item is state")?
                .item,
            "/id"
        ),
        &Value::Null
    );

    let absent = parsed(json!({
        "model": "m",
        "input": [{"type": "compaction", "encrypted_content": "native"}]
    }))?;

    assert!(
        absent
            .ir
            .state_items
            .first()
            .ok_or("the item is state")?
            .item
            .get("id")
            .is_none()
    );

    Ok(())
}

/// The explicit trigger must be the one and last input item, and it records the trigger control.
#[test]
fn trigger_must_be_last_and_unique() -> Result<(), Box<dyn StdError>> {
    let last = parsed(json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "q"},
            {"type": "compaction_trigger"}
        ]
    }))?;

    assert_eq!(
        last.compaction.unwrap_or_default().control,
        Some(CompactionControl::ResponsesTrigger)
    );
    assert!(!last.ir.has_compaction_trigger);

    let not_last = rejection(parse(json!({
        "model": "m",
        "input": [
            {"type": "compaction_trigger"},
            {"type": "message", "role": "user", "content": "q"}
        ]
    }))?)?;

    assert!(not_last.to_string().contains("must be the last input item"));

    let repeated = rejection(parse(json!({
        "model": "m",
        "input": [{"type": "compaction_trigger"}, {"type": "compaction_trigger"}]
    }))?)?;

    assert!(
        repeated
            .to_string()
            .contains("at most one compaction_trigger")
    );

    Ok(())
}

/// A trigger beside the automatic controls is ambiguous and rejected instead of interpreted.
#[test]
fn trigger_beside_context_management_is_ambiguous() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "q"},
            {"type": "compaction_trigger"}
        ],
        "context_management": [{"type": "compaction"}]
    }))?)?;

    assert!(error.to_string().contains("ambiguous"));

    Ok(())
}

/// The automatic compaction entries validate their closed kind and carry their thresholds.
#[test]
fn context_management_entries_parse() -> Result<(), Box<dyn StdError>> {
    let entries = parsed(json!({
        "model": "m",
        "input": "q",
        "context_management": [
            {"type": "compaction"},
            {"type": "compaction", "compact_threshold": 5000}
        ]
    }))?;

    assert_eq!(
        entries.compaction.unwrap_or_default().control,
        Some(CompactionControl::ResponsesAutomatic {
            entries: vec![
                AutomaticEntry {
                    compact_threshold: None,
                },
                AutomaticEntry {
                    compact_threshold: Some(5000),
                },
            ],
        })
    );

    let unknown_kind = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "context_management": [{"type": "summarize"}]
    }))?)?;

    assert_eq!(unknown_kind.param.clone().unwrap_or_default(), "type");
    assert!(
        unknown_kind
            .to_string()
            .contains("Unsupported context management entry type `summarize`")
    );

    Ok(())
}

/// The gateway is stateless: `store: true` is rejected while `store: false` parses.
#[test]
fn store_true_is_rejected() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "store": true
    }))?)?;

    assert_eq!(error.param.clone().unwrap_or_default(), "store");

    let stored_false = parsed(json!({
        "model": "m",
        "input": "q",
        "store": false
    }))?;

    assert!(!stored_false.is_streaming);

    Ok(())
}

/// The reasoning summary control only accepts `auto`.
#[test]
fn reasoning_summary_only_auto() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "reasoning": {"summary": "concise"}
    }))?)?;

    assert_eq!(error.param.clone().unwrap_or_default(), "summary");
    assert!(
        error
            .to_string()
            .contains("Unsupported reasoning summary `concise`")
    );

    let auto = parsed(json!({
        "model": "m",
        "input": "q",
        "reasoning": {"summary": "auto"}
    }))?;

    assert_eq!(auto.ir.reasoning_effort, None);

    Ok(())
}

/// The reasoning effort parses within the closed shared vocabulary; a foreign level names the
/// `reasoning` parameter.
#[test]
fn reasoning_effort_uses_the_closed_vocabulary() -> Result<(), Box<dyn StdError>> {
    let known = parsed(json!({
        "model": "m",
        "input": "q",
        "reasoning": {"effort": "xhigh"}
    }))?;

    assert_eq!(known.ir.reasoning_effort, Some(ReasoningLevel::XHigh));

    let unknown = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "reasoning": {"effort": "extreme"}
    }))?)?;

    assert_eq!(unknown.param.clone().unwrap_or_default(), "reasoning");
    assert!(
        unknown
            .to_string()
            .contains("Unknown reasoning effort `extreme`")
    );

    Ok(())
}

/// The stateless-idempotent envelope controls typed clients always carry are accepted and ignored.
#[test]
fn envelope_ignored_members_are_accepted() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": "q",
        "parallel_tool_calls": false,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": "cache-key",
        "stream_options": {"include_obfuscation": true},
        "service_tier": "auto",
        "client_metadata": {"origin": "codex"},
        "access_programs": ["codex"],
        "text": {"verbosity": "low"}
    }))?;

    assert_eq!(request.ir.messages, vec![user_text("q")]);

    Ok(())
}

/// The replay members of items — rendered identities, phases, statuses, annotations, logprobs,
/// and image detail hints — are accepted and ignored.
#[test]
fn item_replay_members_are_ignored() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": [
            {
                "type": "message",
                "id": "msg_1",
                "role": "user",
                "content": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/x.png",
                        "detail": "high"
                    }
                ],
                "phase": "answer",
                "status": "completed"
            }
        ]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![IrMessage {
            role: IrRole::User,
            parts: vec![IrPart::Image {
                image: IrImage::Url("https://example.com/x.png".to_string()),
            }],
        }]
    );

    Ok(())
}

/// Tools and tool choice parse in the flat Responses shape; non-function kinds and a choice
/// without tools are rejected.
#[test]
fn tools_and_tool_choice_flat_shapes() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "input": "q",
        "tools": [{
            "type": "function",
            "name": "find",
            "description": "Finds things.",
            "parameters": {"type": "object"},
            "strict": true
        }],
        "tool_choice": {"type": "function", "name": "find"}
    }))?;

    assert_eq!(request.ir.tools.len(), 1);
    assert_eq!(
        request.ir.tool_choice,
        Some(IrToolChoice::Function {
            name: "find".to_string(),
        })
    );

    let mode = parsed(json!({
        "model": "m",
        "input": "q",
        "tools": [{"type": "function", "name": "find"}],
        "tool_choice": "required"
    }))?;

    assert_eq!(mode.ir.tool_choice, Some(IrToolChoice::Required));

    let non_function = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "tools": [{"type": "web_search", "name": "search"}]
    }))?)?;

    assert_eq!(non_function.param.clone().unwrap_or_default(), "type");

    let without_tools = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "tool_choice": "auto"
    }))?)?;

    assert_eq!(
        without_tools.param.clone().unwrap_or_default(),
        "tool_choice"
    );

    Ok(())
}

/// The text output format maps all three kinds; the schema strictness flag is ignored.
#[test]
fn text_format_maps() -> Result<(), Box<dyn StdError>> {
    use crate::ir::IrResponseFormat;

    let schema = parsed(json!({
        "model": "m",
        "input": "q",
        "text": {
            "format": {
                "type": "json_schema",
                "name": "out",
                "schema": {"type": "object"},
                "strict": true
            }
        }
    }))?;

    assert_eq!(
        schema.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "out".to_string(),
            schema: json!({"type": "object"}),
        })
    );

    let object = parsed(json!({
        "model": "m",
        "input": "q",
        "text": {"format": {"type": "json_object"}}
    }))?;

    assert_eq!(object.ir.response_format, Some(IrResponseFormat::Json));

    Ok(())
}

/// The shared sampling ranges are validated and the output budget carries.
#[test]
fn sampling_ranges_and_budget_validated() -> Result<(), Box<dyn StdError>> {
    let hot = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "temperature": 3.0
    }))?)?;

    assert_eq!(hot.param.clone().unwrap_or_default(), "temperature");

    let carried = parsed(json!({
        "model": "m",
        "input": "q",
        "temperature": 0.5,
        "top_p": 0.9,
        "max_output_tokens": 128
    }))?;

    assert_eq!(carried.ir.temperature, Some(0.5));
    assert_eq!(carried.ir.top_p, Some(0.9));
    assert_eq!(carried.ir.max_output_tokens, Some(128));

    Ok(())
}

/// The strictly shaped body rejects members outside the ordinary Responses vocabulary.
#[test]
fn unknown_envelope_member_rejected() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "input": "q",
        "seed": 1
    }))?)?;

    assert!(error.to_string().contains("unknown field `seed`"));

    Ok(())
}

/// A configuration-update item carries durable controls the gateway cannot apply, so it is
/// rejected explicitly instead of being dropped.
#[test]
fn configuration_update_item_rejected() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "input": [
            {"type": "configuration_update", "control": {"anything": true}}
        ]
    }))?)?;

    assert!(error.to_string().contains("configuration_update"));

    Ok(())
}

/// The ordinary parser renders the IR into the flat upstream conversation: text, calls with
/// signatures, and outputs as typed items, with the stateless envelope members.
#[test]
fn render_basic_shape() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.instructions = Some("be helpful".to_string());
    ir.messages = vec![
        user_text("question"),
        IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::Text {
                text: "answer".to_string(),
            }],
        },
        assistant_call("call_1", Some("sig")),
        tool_output("call_1", "result"),
    ];

    let body = rendered(&ir, true)?;

    assert_eq!(at(&body, "/model"), &json!("upstream-model"));
    assert_eq!(at(&body, "/instructions"), &json!("be helpful"));
    assert_eq!(at(&body, "/stream"), &json!(true));
    assert_eq!(at(&body, "/store"), &json!(false));
    assert_eq!(at(&body, "/input").as_array().map(Vec::len), Some(4));
    assert_eq!(
        body.pointer("/input/0"),
        Some(&json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "question"}]
        }))
    );
    assert_eq!(
        body.pointer("/input/1"),
        Some(&json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "answer"}]
        }))
    );
    assert_eq!(
        body.pointer("/input/2"),
        Some(&json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "find",
            "arguments": "{\"q\":1}",
            "thought_signature": "sig"
        }))
    );
    assert_eq!(
        body.pointer("/input/3"),
        Some(&json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "result"
        }))
    );

    Ok(())
}

/// Native state items splice verbatim at their message offsets: at the head, between messages,
/// and beyond the last message.
#[test]
fn render_state_items_splice_at_offsets() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.messages = vec![user_text("u1"), user_text("u2")];

    let state_at = |offset: usize| StateItem {
        offset,
        item: encrypted_reasoning(),
        is_gateway: false,
        is_compaction: false,
        family: StateFamily::Responses,
    };

    ir.state_items = vec![state_at(1), state_at(2)];

    let body = rendered(&ir, false)?;

    assert_eq!(at(&body, "/input").as_array().map(Vec::len), Some(4));
    assert_eq!(body.pointer("/input/0/role"), Some(&json!("user")));
    assert_eq!(body.pointer("/input/1"), Some(&encrypted_reasoning()));
    assert_eq!(body.pointer("/input/2/role"), Some(&json!("user")));
    assert_eq!(body.pointer("/input/3"), Some(&encrypted_reasoning()));

    Ok(())
}

/// A gateway carrier never reaches an upstream, and foreign-family native state is inaccessible
/// to a Responses render — both fail instead of being dropped.
#[test]
fn render_rejects_gateway_carriers_and_foreign_family() -> Result<(), Box<dyn StdError>> {
    let mut carrier = ir_request();

    carrier.state_items = vec![StateItem {
        offset: 0,
        item: json!({"type": "compaction", "encrypted_content": gateway_marker()}),
        is_gateway: true,
        is_compaction: true,
        family: StateFamily::Responses,
    }];

    let error = render_request_body_with_extensions(&carrier, false, &[], None)
        .err()
        .ok_or("the unprepared carrier must fail the render")?;

    assert!(error.to_string().contains("without preparation"));

    let mut foreign = ir_request();

    foreign.state_items = vec![StateItem {
        offset: 0,
        item: json!({"type": "compaction", "signature": "native"}),
        is_gateway: false,
        is_compaction: true,
        family: StateFamily::Messages,
    }];

    let error = render_request_body_with_extensions(&foreign, false, &[], None)
        .err()
        .ok_or("the foreign-family state must fail the render")?;

    assert!(error.to_string().contains("cannot replay"));

    Ok(())
}

/// The trigger control rides the render as the terminal input item when the executor set it.
#[test]
fn render_trigger_appends_the_control_item() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.messages = vec![user_text("u1")];
    ir.has_compaction_trigger = true;

    let body = rendered(&ir, false)?;

    assert_eq!(
        body.pointer("/input/0"),
        Some(&json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "u1"}]
        }))
    );
    assert_eq!(
        body.pointer("/input/1"),
        Some(&json!({"type": "compaction_trigger"}))
    );

    Ok(())
}

/// The generation controls render into their flat slots and every absent control stays absent.
#[test]
fn render_generation_controls_and_omissions() -> Result<(), Box<dyn StdError>> {
    use crate::ir::{IrResponseFormat, IrTool};

    let mut ir = ir_request();

    ir.temperature = Some(0.5);
    ir.top_p = Some(0.9);
    ir.max_output_tokens = Some(256);
    ir.response_format = Some(IrResponseFormat::JsonSchema {
        name: "out".to_string(),
        schema: json!({"type": "object"}),
    });
    ir.reasoning_effort = Some(ReasoningLevel::High);
    ir.tools = vec![IrTool {
        name: "find".to_string(),
        description: None,
        parameters: None,
    }];
    ir.tool_choice = Some(IrToolChoice::Function {
        name: "find".to_string(),
    });

    let body = rendered(&ir, false)?;

    assert_eq!(at(&body, "/temperature"), &json!(0.5));
    assert_eq!(at(&body, "/top_p"), &json!(0.9));
    assert_eq!(at(&body, "/max_output_tokens"), &json!(256));
    assert_eq!(
        at(&body, "/text"),
        &json!({"format": {"type": "json_schema", "name": "out", "schema": {"type": "object"}}})
    );
    assert_eq!(at(&body, "/reasoning"), &json!({"effort": "high"}));
    assert_eq!(
        at(&body, "/tools"),
        &json!([{"type": "function", "name": "find", "description": null, "parameters": null}])
    );
    assert_eq!(
        at(&body, "/tool_choice"),
        &json!({"type": "function", "name": "find"})
    );

    let bare = rendered(&ir_request(), false)?;

    for absent in [
        "instructions",
        "tools",
        "tool_choice",
        "temperature",
        "top_p",
        "max_output_tokens",
        "text",
        "reasoning",
        "context_management",
    ] {
        assert!(bare.get(absent).is_none(), "{absent} must stay absent");
    }

    Ok(())
}

/// The `omit_max_output_tokens` render option drops the client-requested cap for upstreams that
/// reject the parameter.
#[test]
fn omit_max_output_tokens_render_option() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.max_output_tokens = Some(256);

    let kept = render_request_body_with_extensions(&ir, false, &[], None)?;
    let kept = serde_json::to_value(&kept)?;

    assert_eq!(at(&kept, "/max_output_tokens"), &json!(256));

    let omitted = render_request_body_with_extensions(
        &ir,
        false,
        &[RenderOption::OmitMaxOutputTokens],
        None,
    )?;
    let omitted = serde_json::to_value(&omitted)?;

    assert!(omitted.get("max_output_tokens").is_none());

    Ok(())
}

/// Native automatic controls pass through verbatim under `context_management`.
#[test]
fn context_management_passthrough() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    let body = render_request_body_with_extensions(
        &ir,
        false,
        &[],
        Some(&[AutomaticEntry {
            compact_threshold: Some(5000),
        }]),
    )?;
    let body = serde_json::to_value(&body)?;

    assert_eq!(
        at(&body, "/context_management"),
        &json!([{"type": "compaction", "compact_threshold": 5000}])
    );

    ir.has_compaction_trigger = false;

    let bare = rendered(&ir, false)?;

    assert!(bare.get("context_management").is_none());

    Ok(())
}

/// The compact upstream body carries only the model, the spliced input, and the instructions — no
/// stream member, so the framing is always non-streaming.
#[test]
fn compact_body_renders_the_narrow_shape() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.instructions = Some("Summarize.".to_string());
    ir.messages = vec![user_text("u1")];
    ir.max_output_tokens = Some(256);
    ir.has_compaction_trigger = false;

    let body = render_compact_request_body(&ir)?;
    let body = serde_json::to_value(&body)?;

    assert_eq!(
        body,
        json!({
            "model": "upstream-model",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "u1"}]
                }
            ],
            "instructions": "Summarize."
        })
    );

    Ok(())
}

/// The opening events carry the upstream identity exactly once, first.
#[test]
fn stream_opens_once_from_created() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.created",
        "response": {"id": "up-1", "model": "upstream-model"}
    })))?;

    assert_eq!(
        events,
        vec![IrEvent::MessageStart {
            id: "up-1".to_string(),
            model: "upstream-model".to_string(),
        }]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.in_progress",
        "response": {"id": "up-1", "model": "upstream-model"}
    })))?;

    assert!(events.is_empty());

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.created",
        "response": {"id": "again"}
    })))?;

    assert!(events.is_empty());

    Ok(())
}

/// The live delta events deliver text, reasoning, and refusal onto their channels, open calls with
/// their identity, and deliver nothing when the payload is absent.
#[test]
fn live_deltas_deliver_text_reasoning_and_calls() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_item.added",
        "output_index": 3,
        "item": {"type": "function_call", "call_id": "call_1", "name": "find", "arguments": ""}
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ToolCallDelta {
                index: 3,
                call_id: Some("call_1".to_string()),
                name: Some("find".to_string()),
                arguments_delta: String::new(),
            },
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.function_call_arguments.delta",
        "output_index": 3,
        "delta": "{\"q\":1}"
    })))?;

    assert_eq!(
        events,
        vec![IrEvent::ToolCallDelta {
            index: 3,
            call_id: None,
            name: None,
            arguments_delta: "{\"q\":1}".to_string(),
        }]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_text.delta",
        "delta": "answer"
    })))?;

    assert_eq!(events, vec![IrEvent::TextDelta("answer".to_string())]);

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.refusal.delta",
        "delta": "never"
    })))?;

    assert_eq!(events, vec![IrEvent::TextDelta("never".to_string())]);

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.reasoning_summary_text.delta",
        "delta": "think"
    })))?;

    assert_eq!(events, vec![IrEvent::ReasoningDelta("think".to_string())]);

    // A delta-typed event without a payload delivered nothing.
    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_text.delta"
    })))?;

    assert!(events.is_empty());

    Ok(())
}

/// A static-key stream that carried no live events recovers every kind exactly once from the
/// terminal output array, and the recovered calls decide the tool-call finish.
#[test]
fn terminal_walk_recovers_undelivered_kinds_once() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "id": "up-1",
            "model": "upstream-model",
            "status": "completed",
            "output": [
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "answer"}
                ]},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "think"}]},
                {"type": "function_call", "call_id": "call_1", "name": "find", "arguments": "{}"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 4}
        }
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: "up-1".to_string(),
                model: "upstream-model".to_string(),
            },
            IrEvent::TextDelta("answer".to_string()),
            IrEvent::ReasoningDelta("think".to_string()),
            IrEvent::ToolCallDone {
                index: 2,
                call_id: "call_1".to_string(),
                name: "find".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            },
            IrEvent::finish(IrFinishReason::ToolCall),
            IrEvent::Usage(IrUsage {
                input_tokens: 10,
                output_tokens: 4,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        ]
    );

    Ok(())
}

/// Live delta delivery exhausts the terminal output array's copy of the same kind.
#[test]
fn live_delivery_exhausts_terminal_fallback() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&data_frame(json!({
        "type": "response.output_text.delta",
        "delta": "answer"
    })))?;

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [{"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "answer"}
            ]}]
        }
    })))?;

    assert_eq!(events, vec![IrEvent::finish(IrFinishReason::Stop)]);

    Ok(())
}

/// A streamed `output_item.done` item is content-bearing delivery: the terminal array's copy of
/// the same index is skipped, and a duplicated done never re-completes its call.
#[test]
fn streamed_done_items_count_as_delivery() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let done_item = json!({
        "type": "function_call",
        "call_id": "call_1",
        "name": "find",
        "arguments": "{}",
        "thought_signature": "sig"
    });

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_item.done",
        "output_index": 1,
        "item": done_item
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ToolCallDone {
                index: 1,
                call_id: "call_1".to_string(),
                name: "find".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig".to_string()),
            },
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [
                {"type": "message", "role": "assistant", "content": []},
                done_item
            ]
        }
    })))?;

    assert_eq!(events, vec![IrEvent::finish(IrFinishReason::ToolCall)]);

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_item.done",
        "output_index": 1,
        "item": done_item
    })))?;

    assert!(events.is_empty());

    Ok(())
}

/// A bare `output_item.added` opening carries the item skeleton without argument bytes, so it
/// never exhausts the terminal output array's copy of the call.
#[test]
fn bare_added_does_not_deliver_call_content() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_item.added",
        "output_index": 0,
        "item": {"type": "function_call", "call_id": "call_1", "name": "find", "arguments": ""}
    })))?;

    assert!(matches!(
        &events[..],
        [
            IrEvent::MessageStart { .. },
            IrEvent::ToolCallDelta {
                arguments_delta,
                ..
            }
        ] if arguments_delta.is_empty()
    ));

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "call_1", "name": "find", "arguments": "{}"}
            ]
        }
    })))?;

    // The terminal recovery delivers the complete call as one done, exactly the recovery class
    // whose done the streaming renderer opens the item from.
    assert_eq!(
        events,
        vec![
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_1".to_string(),
                name: "find".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            },
            IrEvent::finish(IrFinishReason::ToolCall),
        ]
    );

    Ok(())
}

/// The incomplete status maps through its reason: a content filter is the content-filter finish,
/// every other reason is the length finish.
#[test]
fn incomplete_reason_mapping() -> Result<(), Box<dyn StdError>> {
    let terminal = |reason: Value| -> Result<IrFinishReason, Box<dyn StdError>> {
        let mut parser = UpstreamParser::new();

        let events = parser.ingest_frame(&data_frame(json!({
            "type": "response.incomplete",
            "response": {
                "status": "incomplete",
                "incomplete_details": {"reason": reason}
            }
        })))?;

        let reason = events
            .into_iter()
            .find_map(|event| match event {
                IrEvent::Finish { reason, .. } => Some(reason),

                _ => None,
            })
            .ok_or("the terminal event finishes")?;

        Ok(reason)
    };

    assert_eq!(
        terminal(json!("content_filter"))?,
        IrFinishReason::ContentFilter
    );
    assert_eq!(
        terminal(json!("max_output_tokens"))?,
        IrFinishReason::Length
    );
    assert_eq!(terminal(json!("something-else"))?, IrFinishReason::Length);

    Ok(())
}

/// Failure events and frames — `response.failed`, the bare `error` event, and any frame carrying
/// an error object — fail the stream; malformed frames fail as malformed payloads.
#[test]
fn failed_and_error_events_fail() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let error = parser
        .ingest_frame(&data_frame(json!({
            "type": "response.failed",
            "response": {"error": {"message": "upstream broke"}}
        })))
        .err()
        .ok_or("the failed response must fail")?;

    assert_eq!(error.to_string(), "upstream broke");

    let error = parser
        .ingest_frame(&data_frame(json!({
            "type": "error",
            "message": "stream broke"
        })))
        .err()
        .ok_or("the bare error event must fail")?;

    assert_eq!(error.to_string(), "stream broke");

    let error = parser
        .ingest_frame(&data_frame(json!({
            "type": "response.output_text.delta",
            "error": {"message": "mid-stream"}
        })))
        .err()
        .ok_or("any frame with an error object must fail")?;

    assert_eq!(error.to_string(), "mid-stream");

    let error = parser
        .ingest_frame(&SseFrame {
            data: "not json".to_string(),
            event: None,
        })
        .err()
        .ok_or("the malformed frame must fail")?;

    assert_eq!(error.to_string(), "malformed upstream payload in SSE frame");

    Ok(())
}

/// A stream without a terminal event degrades to exactly one stop finish at the clean end.
#[test]
fn missing_terminal_degrades_to_stop() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&data_frame(json!({
        "type": "response.output_text.delta",
        "delta": "answer"
    })))?;

    assert_eq!(parser.finish(), vec![IrEvent::finish(IrFinishReason::Stop)]);
    assert!(parser.finish().is_empty());

    Ok(())
}

/// The Responses usage object carries totals and detail breakdowns into the IR.
#[test]
fn usage_breakdowns_ride_into_ir() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 4,
                "input_tokens_details": {"cached_tokens": 3, "cache_write_tokens": 2},
                "output_tokens_details": {"reasoning_tokens": 1}
            }
        }
    })))?;

    assert!(matches!(
        &events[..],
        [IrEvent::MessageStart { .. }, IrEvent::Finish { .. }, IrEvent::Usage(actual)]
            if *actual == usage()
    ));

    Ok(())
}

/// Native compaction items deliver at their arrival position, and a conformant stream's terminal
/// copies deduplicate by identity while distinct items deliver.
#[test]
fn compaction_items_deliver_once_by_identity() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let item = json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "enc"});

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.output_item.done",
        "item": item
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::CompactionState {
                payload: item.clone(),
            },
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [item]
        }
    })))?;

    assert_eq!(events, vec![IrEvent::finish(IrFinishReason::Stop)]);

    let distinct = json!({"type": "compaction", "id": "cmp_2", "encrypted_content": "enc-2"});

    let events = parser.ingest_frame(&data_frame(json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [distinct]
        }
    })))?;

    assert!(matches!(
        &events[..],
        [
            IrEvent::CompactionState { .. },
            IrEvent::Finish {
                reason: IrFinishReason::Stop,
                ..
            }
        ]
    ));

    Ok(())
}

/// The complete JSON body parses compaction items at the output head ahead of the ordinary items,
/// and a failed status or error member fails the body.
#[test]
fn json_body_full_parse() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "id": "up-1",
        "model": "upstream-model",
        "status": "completed",
        "output": [
            {"type": "compaction", "id": "cmp_1", "encrypted_content": "enc"},
            {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "answer"}
            ]},
            {"type": "function_call", "call_id": "call_1", "name": "find", "arguments": "{}"}
        ],
        "usage": {"input_tokens": 10, "output_tokens": 4}
    }))?)?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: "up-1".to_string(),
                model: "upstream-model".to_string(),
            },
            IrEvent::CompactionState {
                payload: json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "enc"}),
            },
            IrEvent::TextDelta("answer".to_string()),
            IrEvent::ToolCallDelta {
                index: 2,
                call_id: Some("call_1".to_string()),
                name: Some("find".to_string()),
                arguments_delta: "{}".to_string(),
            },
            IrEvent::ToolCallDone {
                index: 2,
                call_id: "call_1".to_string(),
                name: "find".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            },
            IrEvent::finish(IrFinishReason::ToolCall),
            IrEvent::Usage(IrUsage {
                input_tokens: 10,
                output_tokens: 4,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        ]
    );

    let mut parser = UpstreamParser::new();

    let error = parser
        .parse_json_response(&serde_json::to_vec(&json!({
            "status": "failed",
            "error": {"message": "the model refused"}
        }))?)
        .err()
        .ok_or("the failed status must fail the body")?;

    assert_eq!(error.to_string(), "the model refused");

    let mut parser = UpstreamParser::new();

    let error = parser
        .parse_json_response(&serde_json::to_vec(&json!({
            "error": {"message": "attached"}
        }))?)
        .err()
        .ok_or("the error member must fail the body")?;

    assert_eq!(error.to_string(), "attached");

    Ok(())
}

/// The compact upstream response must carry a non-empty output window with exactly one compaction
/// item, and an embedded error member is a failed operation.
#[test]
fn compact_response_parsing() -> Result<(), Box<dyn StdError>> {
    let native = parse_compact_response(&serde_json::to_vec(&json!({
        "id": "resp_1",
        "created_at": 1700000000,
        "output": [
            {"type": "message", "role": "user", "content": "kept"},
            {"type": "compaction", "encrypted_content": "enc"}
        ],
        "usage": {"input_tokens": 10, "output_tokens": 4, "dropped_message_count": 2}
    }))?)?;

    assert_eq!(native.id.unwrap_or_default(), "resp_1");
    assert_eq!(native.created_at, Some(1_700_000_000));
    assert_eq!(native.output.len(), 2);
    assert_eq!(
        native.usage.map(|usage| usage.dropped_message_count),
        Some(Some(2))
    );

    for broken in [
        json!({"output": []}),
        json!({"output": [{"type": "message", "role": "user", "content": "x"}]}),
        json!({"output": [
            {"type": "compaction", "encrypted_content": "a"},
            {"type": "compaction", "encrypted_content": "b"}
        ]}),
    ] {
        let error = parse_compact_response(&serde_json::to_vec(&broken)?)
            .err()
            .ok_or("the unusable window must fail")?;

        assert!(error.to_string().contains("compaction") || error.to_string().contains("output"));
    }

    let error = parse_compact_response(&serde_json::to_vec(&json!({
        "output": [{"type": "compaction", "encrypted_content": "enc"}],
        "error": {"message": "nope"}
    }))?)
    .err()
    .ok_or("the embedded error must fail")?;

    assert_eq!(error.to_string(), "nope");

    Ok(())
}

/// The aggregated body carries the official Response shape: statics describing the gateway,
/// zero-filled usage details, and the checkpoint-before-answer output ordering.
#[test]
fn aggregated_body_carries_the_official_shape() -> Result<(), Box<dyn StdError>> {
    let aggregation = IrAggregation {
        text: "answer".to_string(),
        reasoning: "think".to_string(),
        tool_calls: vec![IrAggregatedToolCall {
            call_id: "call_1".to_string(),
            name: "find".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }],
        finish: Some(IrFinishReason::Stop),
        usage: Some(usage()),
        compaction_states: vec![json!({"type": "compaction", "encrypted_content": "native"})],
        ..IrAggregation::default()
    };

    let body = serde_json::to_value(client_render::render_json_response(
        "public-model",
        &aggregation,
    ))?;

    assert_eq!(at(&body, "/object"), &json!("response"));
    assert_eq!(at(&body, "/model"), &json!("public-model"));
    assert_eq!(at(&body, "/status"), &json!("completed"));
    assert_eq!(at(&body, "/parallel_tool_calls"), &json!(false));
    assert_eq!(at(&body, "/tool_choice"), &json!("auto"));
    assert_eq!(at(&body, "/tools"), &json!([]));
    assert_eq!(at(&body, "/output").as_array().map(Vec::len), Some(4));
    assert_eq!(
        body.pointer("/output/0"),
        Some(&json!({"type": "compaction", "encrypted_content": "native"}))
    );
    assert_eq!(body.pointer("/output/1/type"), Some(&json!("reasoning")));
    assert_eq!(
        body.pointer("/output/1/summary/0/text"),
        Some(&json!("think"))
    );
    assert_eq!(body.pointer("/output/2/type"), Some(&json!("message")));
    assert_eq!(body.pointer("/output/2/status"), Some(&json!("completed")));
    assert_eq!(
        body.pointer("/output/2/content/0/text"),
        Some(&json!("answer"))
    );
    assert_eq!(
        body.pointer("/output/2/content/0/annotations"),
        Some(&json!([]))
    );
    assert_eq!(
        body.pointer("/output/2/content/0/logprobs"),
        Some(&json!([]))
    );
    assert_eq!(
        body.pointer("/output/3/type"),
        Some(&json!("function_call"))
    );
    assert_eq!(body.pointer("/output/3/call_id"), Some(&json!("call_1")));
    assert_eq!(body.pointer("/output/3/status"), Some(&json!("completed")));
    assert_eq!(
        at(&body, "/usage"),
        &json!({
            "input_tokens": 10,
            "input_tokens_details": {"cached_tokens": 3, "cache_write_tokens": 2},
            "output_tokens": 4,
            "output_tokens_details": {"reasoning_tokens": 1},
            "total_tokens": 14
        })
    );
    assert!(body.get("incomplete_details").is_none());
    assert!(body.pointer("/usage/dropped_message_count").is_none());

    Ok(())
}

/// A Length or content-filter finish renders the incomplete status with its official reason.
#[test]
fn aggregated_incomplete_mapping() -> Result<(), Box<dyn StdError>> {
    let truncated = client_render::render_json_response(
        "public-model",
        &IrAggregation {
            text: "partial".to_string(),
            finish: Some(IrFinishReason::Length),
            usage: Some(IrUsage::default()),
            ..IrAggregation::default()
        },
    );

    let body = serde_json::to_value(&truncated)?;

    assert_eq!(at(&body, "/status"), &json!("incomplete"));
    assert_eq!(
        at(&body, "/incomplete_details"),
        &json!({"reason": "max_output_tokens"})
    );

    let filtered = client_render::render_json_response(
        "public-model",
        &IrAggregation {
            finish: Some(IrFinishReason::ContentFilter),
            usage: Some(IrUsage::default()),
            ..IrAggregation::default()
        },
    );

    let body = serde_json::to_value(&filtered)?;

    assert_eq!(
        at(&body, "/incomplete_details"),
        &json!({"reason": "content_filter"})
    );
    assert_eq!(
        at(&body, "/usage"),
        &json!({
            "input_tokens": 0,
            "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0},
            "output_tokens": 0,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": 0
        })
    );

    Ok(())
}

/// The checkpoint-before-answer ordering holds for the aggregated compaction flows: scheduled
/// prefix items prepend the derived output.
#[test]
fn prefix_items_prepend_to_aggregated_output() -> Result<(), Box<dyn StdError>> {
    let checkpoint = compaction_item(
        Some("cmp_1".to_string()),
        "sealed".to_string(),
        Some("gateway".to_string()),
    );

    let body = serde_json::to_value(client_render::render_json_response_with_prefix_items(
        "public-model",
        &IrAggregation {
            text: "answer".to_string(),
            usage: Some(IrUsage::default()),
            ..IrAggregation::default()
        },
        vec![checkpoint],
    ))?;

    assert_eq!(at(&body, "/output").as_array().map(Vec::len), Some(2));
    assert_eq!(body.pointer("/output/0/type"), Some(&json!("compaction")));
    assert_eq!(body.pointer("/output/0/id"), Some(&json!("cmp_1")));
    assert_eq!(
        body.pointer("/output/0/encrypted_content"),
        Some(&json!("sealed"))
    );
    assert_eq!(
        body.pointer("/output/0/created_by"),
        Some(&json!("gateway"))
    );
    assert_eq!(body.pointer("/output/1/type"), Some(&json!("message")));

    Ok(())
}

/// The live stream opens with `response.created` and `response.in_progress` carrying matching
/// event names and monotonically increasing sequence numbers, exactly once.
#[test]
fn stream_lifecycle_and_sequence_numbers() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    let frames = renderer.render(&IrEvent::MessageStart {
        id: "up-1".to_string(),
        model: "upstream-model".to_string(),
    });

    assert_eq!(frames.len(), 2);

    let (created_event, created) = frame(first_frame(&frames)?)?;

    assert_eq!(created_event, "response.created");
    assert_eq!(at(&created, "/type"), &json!("response.created"));
    assert_eq!(at(&created, "/sequence_number"), &json!(0));
    assert_eq!(
        at(at(&created, "/response"), "/status"),
        &json!("in_progress")
    );
    assert_eq!(
        at(at(&created, "/response"), "/model"),
        &json!("public-model")
    );
    assert!(
        at(at(&created, "/response"), "/id")
            .as_str()
            .is_some_and(|id| id.starts_with("resp_"))
    );

    let (in_progress_event, in_progress) = frame(
        frames
            .get(1)
            .map(String::as_str)
            .ok_or("the in_progress frame renders")?,
    )?;

    assert_eq!(in_progress_event, "response.in_progress");
    assert_eq!(at(&in_progress, "/sequence_number"), &json!(1));

    let frames = renderer.render(&IrEvent::MessageStart {
        id: "again".to_string(),
        model: String::new(),
    });

    assert!(frames.is_empty());

    Ok(())
}

/// A text delta opens its message item once and accumulates; closing renders the done sequence
/// and the terminal response object carries the completed items in output order.
#[test]
fn text_delta_lifecycle() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    let frames = renderer.render(&IrEvent::TextDelta("Hel".to_string()));

    assert_eq!(frames.len(), 3);

    let mut events = frames.iter();

    let (added_event, added) = frame(events.next().ok_or("the item opens")?)?;

    assert_eq!(added_event, "response.output_item.added");
    assert_eq!(at(at(&added, "/item"), "/type"), &json!("message"));
    assert_eq!(at(at(&added, "/item"), "/status"), &json!("in_progress"));
    assert_eq!(at(at(&added, "/item"), "/content"), &json!([]));
    assert_eq!(at(&added, "/output_index"), &json!(0));

    let (part_event, part) = frame(events.next().ok_or("the content part opens")?)?;

    assert_eq!(part_event, "response.content_part.added");
    assert_eq!(at(&part, "/item_id"), at(at(&added, "/item"), "/id"));

    let (delta_event, delta) = frame(events.next().ok_or("the delta renders")?)?;

    assert_eq!(delta_event, "response.output_text.delta");
    assert_eq!(at(&delta, "/delta"), &json!("Hel"));
    assert_eq!(at(&delta, "/logprobs"), &json!([]));

    let frames = renderer.render(&IrEvent::TextDelta("lo".to_string()));

    assert_eq!(frames.len(), 1);

    let (_, continuation) = frame(first_frame(&frames)?)?;

    assert_eq!(at(&continuation, "/delta"), &json!("lo"));

    let frames = renderer.render(&IrEvent::finish(IrFinishReason::Stop));

    // The finish closes the open message: text done, content part done, item done.
    assert_eq!(frames.len(), 3);

    let mut closing = frames.iter();

    let (text_done_event, text_done) = frame(closing.next().ok_or("the text done renders")?)?;

    assert_eq!(text_done_event, "response.output_text.done");
    assert_eq!(at(&text_done, "/text"), &json!("Hello"));

    let (part_done_event, _) = frame(closing.next().ok_or("the part done renders")?)?;

    assert_eq!(part_done_event, "response.content_part.done");

    let (item_done_event, item_done) = frame(closing.next().ok_or("the item done renders")?)?;

    assert_eq!(item_done_event, "response.output_item.done");
    assert_eq!(at(at(&item_done, "/item"), "/status"), &json!("completed"));

    let frames = renderer.render(&IrEvent::Done);

    assert_eq!(frames.len(), 1);

    let (terminal_event, terminal) = frame(first_frame(&frames)?)?;

    assert_eq!(terminal_event, "response.completed");
    assert_eq!(
        at(at(&terminal, "/response"), "/status"),
        &json!("completed")
    );
    assert_eq!(
        at(at(&terminal, "/response"), "/output")
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        terminal.pointer("/response/output/0/id"),
        Some(at(at(&item_done, "/item"), "/id"))
    );
    assert_eq!(
        terminal.pointer("/response/usage/total_tokens"),
        Some(&json!(0))
    );

    Ok(())
}

/// Reasoning renders through its own item and summary-part lifecycle, a done carrying text opens
/// the block when no deltas preceded it, and an empty done closes nothing.
#[test]
fn reasoning_lifecycle() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    let frames = renderer.render(&IrEvent::ReasoningDelta("deep ".to_string()));

    assert_eq!(frames.len(), 3);

    let mut events = frames.iter();

    let (added_event, added) = frame(events.next().ok_or("the reasoning item opens")?)?;

    assert_eq!(added_event, "response.output_item.added");
    assert_eq!(at(at(&added, "/item"), "/type"), &json!("reasoning"));

    let (part_event, part) = frame(events.next().ok_or("the summary part opens")?)?;

    assert_eq!(part_event, "response.reasoning_summary_part.added");
    assert_eq!(at(&part, "/summary_index"), &json!(0));

    let (delta_event, delta) = frame(events.next().ok_or("the reasoning delta renders")?)?;

    assert_eq!(delta_event, "response.reasoning_summary_text.delta");
    assert_eq!(at(&delta, "/delta"), &json!("deep "));

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: "thoughts".to_string(),
        signature: None,
        opaque: Value::Null,
    });

    assert_eq!(frames.len(), 1);

    let (_, continuation) = frame(first_frame(&frames)?)?;

    assert_eq!(at(&continuation, "/delta"), &json!("thoughts"));

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: String::new(),
        signature: None,
        opaque: Value::Null,
    });

    assert!(frames.is_empty());

    Ok(())
}

/// A tool call opens from its first delta, skips empty argument bytes, and the done event stamps
/// the authoritative identity onto the completed item; a done for an unopened call recovers it.
#[test]
fn tool_call_lifecycle() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    let frames = renderer.render(&IrEvent::ToolCallDelta {
        index: 0,
        call_id: Some("call_1".to_string()),
        name: Some("find".to_string()),
        arguments_delta: String::new(),
    });

    assert_eq!(frames.len(), 1);

    let (_, opening) = frame(first_frame(&frames)?)?;

    assert_eq!(at(at(&opening, "/item"), "/type"), &json!("function_call"));
    assert_eq!(at(at(&opening, "/item"), "/call_id"), &json!("call_1"));
    assert_eq!(at(at(&opening, "/item"), "/status"), &json!("in_progress"));

    let frames = renderer.render(&IrEvent::ToolCallDelta {
        index: 0,
        call_id: None,
        name: None,
        arguments_delta: "{\"q\":1}".to_string(),
    });

    assert_eq!(frames.len(), 1);

    let (_, arguments) = frame(first_frame(&frames)?)?;

    assert_eq!(
        at(&arguments, "/type"),
        &json!("response.function_call_arguments.delta")
    );
    assert_eq!(at(&arguments, "/delta"), &json!("{\"q\":1}"));

    let frames = renderer.render(&IrEvent::ToolCallDone {
        index: 0,
        call_id: "call_1".to_string(),
        name: "find".to_string(),
        arguments: "{\"q\":1}".to_string(),
        thought_signature: Some("sig".to_string()),
    });

    assert_eq!(frames.len(), 2);

    let mut events = frames.iter();

    let (arguments_done_event, arguments_done) =
        frame(events.next().ok_or("the arguments done renders")?)?;

    assert_eq!(
        arguments_done_event,
        "response.function_call_arguments.done"
    );
    assert_eq!(at(&arguments_done, "/arguments"), &json!("{\"q\":1}"));

    let (_, item_done) = frame(events.next().ok_or("the item done renders")?)?;

    assert_eq!(at(at(&item_done, "/item"), "/status"), &json!("completed"));
    assert_eq!(
        at(at(&item_done, "/item"), "/thought_signature"),
        &json!("sig")
    );

    // A done for a call the stream never opened recovers the complete call.
    let frames = renderer.render(&IrEvent::ToolCallDone {
        index: 9,
        call_id: "call_recovered".to_string(),
        name: "other".to_string(),
        arguments: "{}".to_string(),
        thought_signature: None,
    });

    assert_eq!(frames.len(), 3);

    let (_, recovered_opening) = frame(first_frame(&frames)?)?;

    assert_eq!(
        at(at(&recovered_opening, "/item"), "/type"),
        &json!("function_call")
    );
    assert_eq!(at(&recovered_opening, "/output_index"), &json!(1));

    Ok(())
}

/// A native compaction state item closes the preceding ordinary item, renders its own added/done
/// pair, and pushes the following answer onto a new output position.
#[test]
fn compaction_state_closes_items_and_advances_index() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    renderer.render(&IrEvent::TextDelta("covered".to_string()));

    let state = json!({"type": "compaction", "encrypted_content": "native"});

    let frames = renderer.render(&IrEvent::CompactionState {
        payload: state.clone(),
    });

    // The open message closes (text done, part done, item done), then the state's pair renders.
    assert_eq!(frames.len(), 5);

    let mut events = frames.iter();

    let (text_done_event, _) = frame(events.next().ok_or("the covered item closes")?)?;

    assert_eq!(text_done_event, "response.output_text.done");

    // The content part and the item close without further assertions.
    assert!(events.next().is_some());
    assert!(events.next().is_some());

    let (state_added_event, state_added) = frame(events.next().ok_or("the state item opens")?)?;

    assert_eq!(state_added_event, "response.output_item.added");
    assert_eq!(at(&state_added, "/output_index"), &json!(1));
    assert_eq!(at(&state_added, "/item"), &state);

    let frames = renderer.render(&IrEvent::TextDelta("after".to_string()));

    let (_, reopened) = frame(first_frame(&frames)?)?;

    assert_eq!(at(&reopened, "/output_index"), &json!(2));
    assert_ne!(at(at(&reopened, "/item"), "/id"), &json!(null));

    Ok(())
}

/// The finish reason decides the terminal event: truncation renders `response.incomplete` with
/// its reason, a clean stop renders `response.completed`.
#[test]
fn finish_decides_the_terminal_event() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    renderer.render(&IrEvent::finish(IrFinishReason::Length));

    let frames = renderer.render(&IrEvent::Usage(usage()));

    assert!(frames.is_empty());

    let frames = renderer.render(&IrEvent::Done);

    let (terminal_event, terminal) = frame(first_frame(&frames)?)?;

    assert_eq!(terminal_event, "response.incomplete");
    assert_eq!(
        at(at(&terminal, "/response"), "/status"),
        &json!("incomplete")
    );
    assert_eq!(
        at(at(&terminal, "/response"), "/incomplete_details"),
        &json!({"reason": "max_output_tokens"})
    );
    assert_eq!(
        terminal.pointer("/response/usage/total_tokens"),
        Some(&json!(14))
    );
    assert_eq!(
        terminal.pointer("/response/usage/input_tokens_details"),
        Some(&json!({"cached_tokens": 3, "cache_write_tokens": 2}))
    );

    let mut renderer = SseRenderer::new("public-model".to_string());

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    renderer.render(&IrEvent::finish(IrFinishReason::Stop));

    let frames = renderer.render(&IrEvent::Done);

    let (terminal_event, terminal) = frame(first_frame(&frames)?)?;

    assert_eq!(terminal_event, "response.completed");
    assert_eq!(
        at(at(&terminal, "/response"), "/status"),
        &json!("completed")
    );
    assert!(
        at(&terminal, "/response")
            .get("incomplete_details")
            .is_none()
    );

    Ok(())
}

/// A mid-stream error renders the classified failure as the terminal `response.failed` event
/// carrying the full required response shell.
#[test]
fn error_frame_renders_failed_response() -> Result<(), Box<dyn StdError>> {
    let mut renderer = SseRenderer::new("public-model".to_string());

    let raw = renderer.error_frame(&Error::UpstreamUnavailable {
        message: "connection reset".to_string(),
    });

    let (event, body) = frame(&raw)?;

    assert_eq!(event, "response.failed");
    assert_eq!(at(at(&body, "/response"), "/status"), &json!("failed"));
    assert_eq!(at(at(&body, "/response"), "/model"), &json!("public-model"));
    assert_eq!(
        at(at(at(&body, "/response"), "/error"), "/message"),
        &json!("Upstream request failed: connection reset")
    );
    assert_eq!(
        at(at(at(&body, "/response"), "/error"), "/type"),
        &json!("api_error")
    );

    Ok(())
}

/// Scheduled prefix items drain after the response opens, ahead of every IR-derived item, and the
/// following answer continues at the next output position.
#[test]
fn prefix_frames_drain_after_created() -> Result<(), Box<dyn StdError>> {
    let checkpoint = compaction_item(Some("cmp_1".to_string()), "sealed".to_string(), None);
    let mut renderer =
        SseRenderer::new("public-model".to_string()).with_prefix_items(vec![checkpoint]);

    let frames = renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    // created, in_progress, then the prefix item's added/done pair.
    assert_eq!(frames.len(), 4);

    let (prefix_event, prefix) = frame(
        frames
            .get(2)
            .map(String::as_str)
            .ok_or("the prefix item opens")?,
    )?;

    assert_eq!(prefix_event, "response.output_item.added");
    assert_eq!(at(&prefix, "/output_index"), &json!(0));
    assert_eq!(at(at(&prefix, "/item"), "/type"), &json!("compaction"));
    assert_eq!(at(at(&prefix, "/item"), "/id"), &json!("cmp_1"));

    let (done_event, _) = frame(
        frames
            .get(3)
            .map(String::as_str)
            .ok_or("the prefix item completes")?,
    )?;

    assert_eq!(done_event, "response.output_item.done");

    assert_eq!(renderer.take_prefix_boundary(), Some(3));
    assert_eq!(renderer.take_prefix_boundary(), None);

    let frames = renderer.render(&IrEvent::TextDelta("answer".to_string()));

    let (_, message_opening) = frame(first_frame(&frames)?)?;

    assert_eq!(at(&message_opening, "/output_index"), &json!(1));

    Ok(())
}

/// The trigger lifecycle's frame sequence is complete: the response opens, every prefix item
/// renders its pair, and the completed terminal carries the usage.
#[test]
fn trigger_stream_frames_complete_sequence() -> Result<(), Box<dyn StdError>> {
    let checkpoint = compaction_item(None, "sealed".to_string(), None);

    let frames = render_trigger_stream_frames(
        "public-model".to_string(),
        vec![checkpoint],
        IrUsage {
            input_tokens: 5,
            output_tokens: 2,
            ..IrUsage::default()
        },
    );

    let parsed: Vec<(String, Value)> = frames
        .iter()
        .map(|raw| frame(raw))
        .collect::<Result<Vec<_>, _>>()?;

    let names: Vec<&str> = parsed.iter().map(|(event, _)| event.as_str()).collect();

    assert_eq!(
        names,
        vec![
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.output_item.done",
            "response.completed",
        ]
    );

    let (_, terminal) = parsed.last().ok_or("the lifecycle terminates")?;
    let response = at(terminal, "/response");

    assert_eq!(at(response, "/status"), &json!("completed"));
    assert_eq!(at(response, "/usage/input_tokens"), &json!(5));
    assert_eq!(
        terminal.pointer("/response/usage/total_tokens"),
        Some(&json!(7))
    );
    assert_eq!(
        terminal.pointer("/response/output/0/type"),
        Some(&json!("compaction"))
    );

    Ok(())
}

/// The compact endpoint parses only its narrow body: the route is the control, the stateless
/// continuation id and every generation control are rejected, and the caching conventions are
/// accepted and ignored.
#[test]
fn compact_request_parses_strictly() -> Result<(), Box<dyn StdError>> {
    let request = parse_compact_request(&serde_json::to_vec(&json!({
        "model": "m",
        "input": [{"type": "message", "role": "user", "content": "summarize this"}],
        "instructions": "Keep the facts.",
        "prompt_cache_key": "cache-key",
        "prompt_cache_options": {"mode": "extended", "ttl": "24h"},
        "prompt_cache_retention": "24h",
        "service_tier": "auto"
    }))?)?;

    assert_eq!(request.ir.model, "m");
    assert_eq!(
        request.ir.instructions.unwrap_or_default(),
        "Keep the facts."
    );
    assert!(!request.is_streaming);
    assert!(!request.should_include_usage);

    let compaction = request
        .compaction
        .as_ref()
        .ok_or("the route is the control")?;

    assert_eq!(
        compaction.control,
        Some(CompactionControl::ResponsesStandalone)
    );

    let stateful = parse_compact_request(&serde_json::to_vec(&json!({
        "model": "m",
        "input": "q",
        "previous_response_id": "resp_1"
    }))?)
    .err()
    .ok_or("the stateful continuation must be rejected")?;

    assert_eq!(
        stateful.param.clone().unwrap_or_default(),
        "previous_response_id"
    );

    let trigger = parse_compact_request(&serde_json::to_vec(&json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "q"},
            {"type": "compaction_trigger"}
        ]
    }))?)
    .err()
    .ok_or("the trigger belongs to the ordinary endpoint")?;

    assert!(
        trigger
            .to_string()
            .contains("not valid on the compact endpoint")
    );

    for generation_control in [
        json!({"stream": false}),
        json!({"tools": []}),
        json!({"temperature": 0.5}),
        json!({"reasoning": {"effort": "low"}}),
        json!({"text": {"format": {"type": "text"}}}),
    ] {
        let mut body = minimal_compact_body();

        let object = body.as_object_mut().ok_or("the fixture is an object")?;

        match &generation_control {
            Value::Object(members) => {
                for (name, value) in members {
                    object.insert(name.clone(), value.clone());
                }
            }

            _ => return Err("the control fixtures are objects".into()),
        }

        let error = parse_compact_request(&serde_json::to_vec(&body)?)
            .err()
            .ok_or("the generation-only control must be rejected")?;

        assert!(error.to_string().contains("unknown field"));
    }

    Ok(())
}

/// The minimal compact body the strictness fixtures build on.
fn minimal_compact_body() -> Value {
    json!({
        "model": "m",
        "input": "q"
    })
}

/// The gateway-generated checkpoint builds a Responses carrier under a minted or carried id.
#[test]
fn compaction_item_builds_the_responses_carrier() -> Result<(), Box<dyn StdError>> {
    let checkpoint = compaction_item(None, "sealed".to_string(), None);

    let Checkpoint::Responses(item) = checkpoint else {
        return Err("the checkpoint must be a Responses carrier".into());
    };

    assert!(item.id.starts_with("cmp_"));
    assert_eq!(item.encrypted_content, "sealed");
    assert_eq!(item.created_by, None);

    let checkpoint = compaction_item(
        Some("cmp_native".to_string()),
        "sealed".to_string(),
        Some("producer".to_string()),
    );

    let Checkpoint::Responses(item) = checkpoint else {
        return Err("the checkpoint must be a Responses carrier".into());
    };

    assert_eq!(item.id, "cmp_native");
    assert_eq!(item.created_by.unwrap_or_default(), "producer");

    Ok(())
}

/// The native passthrough carries the upstream's identity, window, and usage through — including
/// the vendor's `dropped_message_count` and unknown members — with defaults only when absent.
#[test]
fn render_compacted_response_preserves_the_native_window() -> Result<(), Box<dyn StdError>> {
    let native = CompactedResponseNative {
        id: Some("resp_upstream".to_string()),
        created_at: Some(1_700_000_000),
        output: vec![json!({
            "type": "compaction",
            "id": null,
            "encrypted_content": "producer-cipher",
            "created_by": "producer",
            "vendor_extension": true
        })],
        usage: Some(UpstreamUsage {
            input_tokens: 10,
            output_tokens: 4,
            dropped_message_count: Some(2),
            ..UpstreamUsage::default()
        }),
    };

    let body = serde_json::to_value(render_compacted_response(native))?;

    assert_eq!(at(&body, "/id"), &json!("resp_upstream"));
    assert_eq!(at(&body, "/object"), &json!("response.compaction"));
    assert_eq!(at(&body, "/created_at"), &json!(1_700_000_000));
    assert_eq!(
        body.pointer("/output/0"),
        Some(&json!({
            "type": "compaction",
            "id": null,
            "encrypted_content": "producer-cipher",
            "created_by": "producer",
            "vendor_extension": true
        }))
    );
    assert_eq!(
        body.pointer("/usage/dropped_message_count"),
        Some(&json!(2))
    );
    assert_eq!(body.pointer("/usage/total_tokens"), Some(&json!(14)));

    let defaulted = render_compacted_response(CompactedResponseNative {
        id: None,
        created_at: None,
        output: Vec::new(),
        usage: None,
    });

    let body = serde_json::to_value(&defaulted)?;

    assert!(
        at(&body, "/id")
            .as_str()
            .is_some_and(|id| id.starts_with("resp_"))
    );
    assert!(body.pointer("/usage/dropped_message_count").is_none());

    Ok(())
}

/// The gateway-produced compact result carries exactly one sealed compaction item beside the
/// summary call's zero-filled usage, and its sealer identity anchors the artifact.
#[test]
fn render_gateway_compacted_response_carries_one_carrier() -> Result<(), Box<dyn StdError>> {
    let response = render_gateway_compacted_response(
        gateway_marker(),
        "artifact-1".to_string(),
        IrUsage {
            input_tokens: 5,
            output_tokens: 2,
            ..IrUsage::default()
        },
    );

    let body = serde_json::to_value(&response)?;

    assert_eq!(at(&body, "/output").as_array().map(Vec::len), Some(1));
    assert_eq!(body.pointer("/output/0/type"), Some(&json!("compaction")));
    assert_eq!(
        body.pointer("/output/0/encrypted_content"),
        Some(&json!(gateway_marker()))
    );
    assert!(
        body.pointer("/output/0/id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("cmp_"))
    );
    assert_eq!(body.pointer("/usage/input_tokens"), Some(&json!(5)));

    let projector = Projector::new();

    assert!(response.artifact_anchor(&projector).is_some());

    Ok(())
}

/// Artifact identity gates on provenance: a native compaction item anchors by its exact bytes,
/// while a reflected gateway marker — never ours — anchors to nothing.
#[test]
fn artifact_anchors_gate_on_provenance() -> Result<(), Box<dyn StdError>> {
    let projector = Projector::new();

    let native = json!({"type": "compaction", "encrypted_content": "producer-cipher"});

    assert!(super::compaction::native_artifact_anchor(&native, &projector).is_some());
    assert!(
        super::compaction::native_artifact_anchor(
            &json!({"type": "compaction", "encrypted_content": gateway_marker()}),
            &projector
        )
        .is_none()
    );

    // The same gate holds for the rendered compacted body.
    let reflected = render_compacted_response(CompactedResponseNative {
        id: None,
        created_at: None,
        output: vec![json!({"type": "compaction", "encrypted_content": gateway_marker()})],
        usage: None,
    });

    assert!(reflected.artifact_anchor(&projector).is_none());

    Ok(())
}

/// The rendered window's history anchors exist exactly when the window is fully replayable: an
/// ordinary turn window anchors, a window with an unrecognizable role does not.
#[test]
fn window_anchors_require_a_replayable_window() -> Result<(), Box<dyn StdError>> {
    let usable = render_compacted_response(CompactedResponseNative {
        id: None,
        created_at: None,
        output: vec![
            json!({"type": "message", "role": "user", "content": "q"}),
            json!({"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "a"}
            ]}),
        ],
        usage: None,
    });

    assert!(!usable.window_anchors().is_empty());

    let unusable = render_compacted_response(CompactedResponseNative {
        id: None,
        created_at: None,
        output: vec![json!({"type": "message", "role": "moderator", "content": "x"})],
        usage: None,
    });

    assert!(unusable.window_anchors().is_empty());

    Ok(())
}

/// The window projection gates a gateway marker on sealer provenance: without it the window is
/// not projectable, with it the carrier anchors as gateway state.
#[test]
fn window_projection_gates_gateway_markers_on_provenance() -> Result<(), Box<dyn StdError>> {
    let marker_window = vec![Checkpoint::from(json!({
        "type": "compaction",
        "encrypted_content": gateway_marker()
    }))];

    assert!(window_affinity::project(&marker_window, false, false).is_none());
    assert!(window_affinity::project(&marker_window, true, false).is_some());

    Ok(())
}

/// The aggregated-body projection fingerprints the answer fields and native state, declines a
/// Responses checkpoint among the output, and the checkpoint channel skips one prefix item.
#[test]
fn aggregated_projection_covers_state_and_answers() -> Result<(), Box<dyn StdError>> {
    let answer = IrAggregation {
        text: "answer".to_string(),
        tool_calls: vec![IrAggregatedToolCall {
            call_id: "call_1".to_string(),
            name: "find".to_string(),
            arguments: "{\"q\":1}".to_string(),
            thought_signature: None,
        }],
        compaction_states: vec![encrypted_reasoning()],
        usage: Some(IrUsage::default()),
        ..IrAggregation::default()
    };

    let response = client_render::render_json_response("public-model", &answer);

    let mut projector = Projector::new();

    assert!(super::Format::project_json_response(
        &response,
        &mut projector
    ));

    let with_prefix = client_render::render_json_response_with_prefix_items(
        "public-model",
        &IrAggregation {
            text: "answer".to_string(),
            usage: Some(IrUsage::default()),
            ..IrAggregation::default()
        },
        vec![compaction_item(
            Some("cmp_1".to_string()),
            "sealed".to_string(),
            None,
        )],
    );

    let mut ordinary = Projector::new();

    assert!(!super::Format::project_json_response(
        &with_prefix,
        &mut ordinary
    ));

    let mut checkpointed = Projector::new();

    assert!(super::Format::project_json_checkpoint(
        &with_prefix,
        &mut checkpointed
    ));

    Ok(())
}
