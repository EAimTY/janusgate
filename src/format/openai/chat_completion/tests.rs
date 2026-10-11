//! Tests for the Chat Completions codec: the client request parser, the upstream request renderer,
//! the upstream response parser, and the client response renderers, live and aggregated.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{UpstreamParser, client_render, request_parse};
use crate::{
    affinity::fingerprint::Projector,
    api::Error,
    format::{
        ClientFormat as _, ClientSseRenderer as _, ParsedRequest, UpstreamEventParser as _,
        UpstreamFormat,
    },
    ir::{
        InvalidRequest, IrAggregatedToolCall, IrAggregation, IrEvent, IrFinishReason, IrImage,
        IrMessage, IrPart, IrRequest, IrRole, IrTool, IrToolChoice, IrUsage, StateFamily,
        StateItem,
    },
    sse::SseFrame,
    test_support::at,
    vocabulary::ReasoningLevel,
};
use serde_json::{Value, json};
use std::error::Error as StdError;

/// Serializes the JSON fixture and parses it as a Chat Completions request.
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
        "messages": [{"role": "user", "content": "hello"}]
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

/// The upstream `[DONE]` sentinel frame.
fn done_frame() -> SseFrame {
    SseFrame {
        data: "[DONE]".to_string(),
        event: None,
    }
}

/// Splits one rendered SSE frame into its event name and JSON payload.
fn frame(raw: &str) -> Result<(Option<String>, Value), Box<dyn StdError>> {
    let (event, rest) = match raw.strip_prefix("event: ") {
        Some(rest) => {
            let (name, rest) = rest
                .split_once('\n')
                .ok_or("the frame carries an event line")?;

            (Some(name.to_string()), rest)
        }

        None => (None, raw),
    };

    let payload = rest
        .strip_prefix("data: ")
        .ok_or("the frame carries a data line")?;
    let payload = payload
        .strip_suffix("\n\n")
        .ok_or("the frame is terminated")?;

    Ok((event, serde_json::from_str(payload)?))
}

/// The first rendered frame of a batch, failing the test when the batch is empty.
fn first_frame(frames: &[String]) -> Result<&str, Box<dyn StdError>> {
    frames
        .first()
        .map(String::as_str)
        .ok_or("expected at least one frame".into())
}

/// Renders an IR request into its upstream JSON shape.
fn rendered(ir: &IrRequest, is_streaming: bool) -> Result<Value, Box<dyn StdError>> {
    let body = super::render_request_body(ir, is_streaming)?;

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

/// The minimal request parses to its IR defaults: no instructions, no tools, no streaming, and no
/// compaction facts.
#[test]
fn minimal_request_parses_to_ir_defaults() -> Result<(), Box<dyn StdError>> {
    let request = parsed(minimal_body())?;

    assert_eq!(request.ir.model, "public-model");
    assert_eq!(request.ir.instructions, None);
    assert_eq!(request.ir.messages, vec![user_text("hello")]);
    assert!(request.ir.tools.is_empty());
    assert_eq!(request.ir.tool_choice, None);
    assert_eq!(request.ir.temperature, None);
    assert_eq!(request.ir.top_p, None);
    assert_eq!(request.ir.max_output_tokens, None);
    assert_eq!(request.ir.response_format, None);
    assert_eq!(request.ir.reasoning_effort, None);
    assert!(!request.is_streaming);
    assert!(!request.should_include_usage);
    assert!(request.compaction.is_none());

    Ok(())
}

/// System and developer instructions concatenate in encounter order, and their array content
/// flattens to exactly what the equivalent plain-string form carries.
#[test]
fn instructions_concatenate_in_encounter_order() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [
            {"role": "system", "content": "first"},
            {"role": "developer", "content": [
                {"type": "text", "text": "second"},
                {"type": "text", "text": "third"}
            ]},
            {"role": "system", "content": "fourth"}
        ]
    }))?;

    assert_eq!(
        request.ir.instructions.unwrap_or_default(),
        "first\n\nsecond\n\nthird\n\nfourth"
    );
    assert!(request.ir.messages.is_empty());

    Ok(())
}

/// User content maps text and image parts, splitting data URLs into inline image data and keeping
/// remote references verbatim.
#[test]
fn user_content_maps_text_and_images() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "look"},
                {
                    "type": "image_url",
                    "image_url": {"url": "data:image/png;base64,QUJD"},
                    "detail": "high"
                },
                {"type": "image_url", "image_url": {"url": "https://example.com/cat.png"}}
            ]
        }]
    }))?;

    let message = request
        .ir
        .messages
        .first()
        .ok_or("the request carries one message")?;

    assert_eq!(
        *message,
        IrMessage {
            role: IrRole::User,
            parts: vec![
                IrPart::Text {
                    text: "look".to_string(),
                },
                IrPart::Image {
                    image: IrImage::Inline {
                        mime_type: "image/png".to_string(),
                        data_base64: "QUJD".to_string(),
                    },
                },
                IrPart::Image {
                    image: IrImage::Url("https://example.com/cat.png".to_string()),
                },
            ],
        }
    );

    Ok(())
}

/// An image reference of an unsupported scheme is rejected naming the image parameter.
#[test]
fn image_url_schemes_are_validated() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({
        "model": "m",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": "ftp://example.com/cat.png"}}
            ]
        }]
    }))?)?;

    assert_eq!(error.param.clone().unwrap_or_default(), "image_url");
    assert!(error.to_string().contains("data:, http:, or https:"));

    Ok(())
}

/// Roles whose array content admits only text parts reject media and unknown part kinds, naming
/// the recognized kind.
#[test]
fn text_only_roles_reject_media_parts() -> Result<(), Box<dyn StdError>> {
    let assistant_image = rejection(parse(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "content": [
                {"type": "text", "text": "a"},
                {"type": "image_url", "image_url": {"url": "https://example.com/x.png"}}
            ]
        }]
    }))?)?;

    assert_eq!(assistant_image.param.clone().unwrap_or_default(), "content");
    assert!(
        assistant_image
            .to_string()
            .contains("assistant message content parts must be text; `image_url` is not supported")
    );

    let tool_image = rejection(parse(json!({
        "model": "m",
        "messages": [
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": [
                {"type": "image_url", "image_url": {"url": "https://example.com/x.png"}}
            ]}
        ]
    }))?)?;

    assert_eq!(tool_image.param.clone().unwrap_or_default(), "content");
    assert!(
        tool_image
            .to_string()
            .contains("tool message content parts must be text")
    );

    let unknown_kind = rejection(parse(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "content": [{"type": "diagram", "data": {}}]
        }]
    }))?)?;

    assert_eq!(unknown_kind.param.clone().unwrap_or_default(), "content");
    assert_eq!(
        unknown_kind.to_string(),
        "assistant message content parts must be text"
    );

    Ok(())
}

/// Tool invocations parse with their wire identity, arguments, and replayable thought signature.
#[test]
fn assistant_tool_calls_parse_with_signatures() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "content": "calling",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "find", "arguments": "{\"q\":1}"},
                "thought_signature": "sig"
            }]
        }]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![IrMessage {
            role: IrRole::Assistant,
            parts: vec![
                IrPart::Text {
                    text: "calling".to_string(),
                },
                IrPart::ToolCall {
                    call_id: "call_1".to_string(),
                    name: "find".to_string(),
                    arguments: "{\"q\":1}".to_string(),
                    thought_signature: Some("sig".to_string()),
                },
            ],
        }]
    );

    Ok(())
}

/// Replayed tool-call arguments must encode a JSON object: a non-object fails the request at the
/// parse boundary for both the modern and the legacy protocol.
#[test]
fn non_object_arguments_are_rejected() -> Result<(), Box<dyn StdError>> {
    let modern = rejection(parse(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "f", "arguments": "[1,2]"}
                }
            ]
        }]
    }))?)?;

    assert_eq!(modern.param.clone().unwrap_or_default(), "arguments");
    assert!(modern.to_string().contains("must encode a JSON object"));

    let legacy = rejection(parse(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "function_call": {"name": "f", "arguments": "{"}
        }]
    }))?)?;

    assert_eq!(legacy.param.clone().unwrap_or_default(), "arguments");

    Ok(())
}

/// The legacy `function_call` normalizes to a single tool call under the stable synthesized id of
/// its assembly slot, and the round-trip renders that id into the modern shape.
#[test]
fn legacy_function_call_normalizes_to_synthetic_call() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "function_call": {"name": "legacy", "arguments": "{\"a\":1}"}
        }]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::ToolCall {
                call_id: "call_0".to_string(),
                name: "legacy".to_string(),
                arguments: "{\"a\":1}".to_string(),
                thought_signature: None,
            }],
        }]
    );

    let body = rendered(&request.ir, false)?;

    assert_eq!(body.pointer("/messages/0/role"), Some(&json!("assistant")));
    assert_eq!(
        body.pointer("/messages/0/tool_calls/0/id"),
        Some(&json!("call_0"))
    );
    assert_eq!(
        body.pointer("/messages/0/tool_calls/0/type"),
        Some(&json!("function"))
    );

    Ok(())
}

/// Modern `tool_calls` take precedence: a message carrying both carries only the modern calls.
#[test]
fn modern_tool_calls_take_precedence_over_function_call() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "modern", "arguments": "{}"}
                }
            ],
            "function_call": {"name": "legacy", "arguments": "{}"}
        }]
    }))?;

    let message = request
        .ir
        .messages
        .first()
        .ok_or("the request carries one message")?;

    assert_eq!(message.parts.len(), 1);

    let IrPart::ToolCall { call_id, name, .. } = message
        .parts
        .first()
        .ok_or("the message carries one part")?
    else {
        return Err("the part must be a tool call".into());
    };

    assert_eq!(call_id, "call_1");
    assert_eq!(name, "modern");

    Ok(())
}

/// The assistant replay members the reference SDKs echo verbatim are accepted and ignored.
#[test]
fn assistant_replay_members_are_accepted_and_ignored() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [{
            "role": "assistant",
            "content": "answer",
            "reasoning": {"anything": true},
            "reasoning_content": "invisible",
            "refusal": null,
            "name": "turn-name",
            "audio": {"id": "audio-1"},
            "annotations": [{"type": "url_citation"}]
        }]
    }))?;

    assert_eq!(
        request.ir.messages,
        vec![IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::Text {
                text: "answer".to_string(),
            }],
        }]
    );

    Ok(())
}

/// The output budgets are mutually exclusive, and the modern budget is carried when given.
#[test]
fn budgets_are_mutually_exclusive_and_prefer_modern() -> Result<(), Box<dyn StdError>> {
    let both = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 10,
        "max_completion_tokens": 20
    }))?)?;

    assert_eq!(both.param.clone().unwrap_or_default(), "max_tokens");
    assert!(both.to_string().contains("mutually exclusive"));

    let modern = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "max_completion_tokens": 20
    }))?;

    assert_eq!(modern.ir.max_output_tokens, Some(20));

    let legacy = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 10
    }))?;

    assert_eq!(legacy.ir.max_output_tokens, Some(10));

    Ok(())
}

/// Every response format kind maps onto its IR form; the strictness flag is ignored.
#[test]
fn response_format_maps_all_three_kinds() -> Result<(), Box<dyn StdError>> {
    use crate::ir::IrResponseFormat;

    let text = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "response_format": {"type": "text"}
    }))?;

    assert_eq!(text.ir.response_format, Some(IrResponseFormat::Text));

    let object = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "response_format": {"type": "json_object"}
    }))?;

    assert_eq!(object.ir.response_format, Some(IrResponseFormat::Json));

    let schema = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": "out", "schema": {"type": "object"}, "strict": true}
        }
    }))?;

    assert_eq!(
        schema.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "out".to_string(),
            schema: json!({"type": "object"}),
        })
    );

    Ok(())
}

/// The reasoning effort parses within the closed shared vocabulary, and a foreign level is a 400
/// naming the parameter.
#[test]
fn reasoning_effort_uses_the_closed_vocabulary() -> Result<(), Box<dyn StdError>> {
    let known = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "reasoning_effort": "high"
    }))?;

    assert_eq!(known.ir.reasoning_effort, Some(ReasoningLevel::High));

    let unknown = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "reasoning_effort": "extreme"
    }))?)?;

    assert_eq!(
        unknown.param.clone().unwrap_or_default(),
        "reasoning_effort"
    );
    assert!(
        unknown
            .to_string()
            .contains("Unknown reasoning effort `extreme`")
    );

    Ok(())
}

/// The tool choice modes and the forced function parse; a choice without tools, an unknown mode,
/// and a non-function forced kind are rejected.
#[test]
fn tool_choice_modes_and_forced_function() -> Result<(), Box<dyn StdError>> {
    let tool = json!({"type": "function", "function": {"name": "find", "parameters": {}}});

    for (mode, expected) in [
        ("auto", IrToolChoice::Auto),
        ("none", IrToolChoice::None),
        ("required", IrToolChoice::Required),
    ] {
        let request = parsed(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [tool],
            "tool_choice": mode
        }))?;

        assert_eq!(request.ir.tool_choice, Some(expected));
    }

    let forced = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [tool],
        "tool_choice": {"type": "function", "function": {"name": "find"}}
    }))?;

    assert_eq!(
        forced.ir.tool_choice,
        Some(IrToolChoice::Function {
            name: "find".to_string(),
        })
    );

    let without_tools = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tool_choice": "auto"
    }))?)?;

    assert_eq!(
        without_tools.param.clone().unwrap_or_default(),
        "tool_choice"
    );

    let unknown = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [tool],
        "tool_choice": "mandatory"
    }))?)?;

    assert_eq!(unknown.param.clone().unwrap_or_default(), "tool_choice");

    let non_function = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [tool],
        "tool_choice": {"type": "builtin", "function": {"name": "find"}}
    }))?)?;

    assert_eq!(non_function.param.clone().unwrap_or_default(), "type");

    Ok(())
}

/// The shared sampling ranges are validated at the surface: an out-of-range temperature is a 400
/// naming the parameter, an in-range pair is carried.
#[test]
fn sampling_ranges_are_validated() -> Result<(), Box<dyn StdError>> {
    let hot = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "temperature": 2.5
    }))?)?;

    assert_eq!(hot.param.clone().unwrap_or_default(), "temperature");

    let carried = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "temperature": 0.5,
        "top_p": 1.0
    }))?;

    assert_eq!(carried.ir.temperature, Some(0.5));
    assert_eq!(carried.ir.top_p, Some(1.0));

    Ok(())
}

/// Function tools parse with their schema; a non-function tool kind and the strictness flag are
/// rejected and ignored respectively.
#[test]
fn tools_parse_with_strict_ignored() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "find",
                "description": "Finds things.",
                "parameters": {"type": "object"},
                "strict": true
            }
        }]
    }))?;

    let tool = request
        .ir
        .tools
        .first()
        .ok_or("the request carries one tool")?;

    assert_eq!(
        *tool,
        IrTool {
            name: "find".to_string(),
            description: Some("Finds things.".to_string()),
            parameters: Some(json!({"type": "object"})),
        }
    );

    let non_function = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{"type": "web_search", "function": {"name": "search"}}]
    }))?)?;

    assert_eq!(non_function.param.clone().unwrap_or_default(), "type");

    Ok(())
}

/// The streaming flags parse: `stream` selects the streamed response and `stream_options`
/// requests the trailing usage chunk.
#[test]
fn stream_and_usage_flags_parse() -> Result<(), Box<dyn StdError>> {
    let plain = parsed(minimal_body())?;

    assert!(!plain.is_streaming);
    assert!(!plain.should_include_usage);

    let streaming = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true,
        "stream_options": {"include_usage": true}
    }))?;

    assert!(streaming.is_streaming);
    assert!(streaming.should_include_usage);

    let stream_without_usage = parsed(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    }))?;

    assert!(stream_without_usage.is_streaming);
    assert!(!stream_without_usage.should_include_usage);

    Ok(())
}

/// Unknown members of the body and of a message are rejected, per the strict request shape.
#[test]
fn unknown_members_are_rejected() -> Result<(), Box<dyn StdError>> {
    let top_level = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "seed": 1
    }))?)?;

    assert_eq!(top_level.param, None);
    assert!(top_level.to_string().contains("unknown field `seed`"));

    let stop_reason = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "stop": ["."]
    }))?)?;

    assert!(stop_reason.to_string().contains("unknown field `stop`"));

    let in_message = rejection(parse(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi", "surprise": true}]
    }))?)?;

    assert!(in_message.to_string().contains("unknown field `surprise`"));

    Ok(())
}

/// Parsed requests record the Chat Completions source format for the affinity projection.
#[test]
fn parsed_requests_record_the_chat_affinity_format() -> Result<(), Box<dyn StdError>> {
    let request = parsed(json!({
        "model": "m",
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "ok"}
        ]
    }))?;

    assert_eq!(
        request.affinity.client_format(),
        UpstreamFormat::OpenAiChatCompletion
    );

    Ok(())
}

/// The upstream render orders system instructions first, joins assistant text, and emits one tool
/// message per tool output.
#[test]
fn render_orders_system_user_assistant_and_tool() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.instructions = Some("be helpful".to_string());
    ir.messages = vec![
        user_text("question"),
        IrMessage {
            role: IrRole::Assistant,
            parts: vec![
                IrPart::Text {
                    text: "part one".to_string(),
                },
                IrPart::Text {
                    text: "part two".to_string(),
                },
            ],
        },
        IrMessage {
            role: IrRole::Assistant,
            parts: Vec::new(),
        },
        tool_output("call_1", "result"),
    ];

    let body = rendered(&ir, false)?;

    assert_eq!(at(&body, "/messages").as_array().map(Vec::len), Some(5));
    assert_eq!(
        body.pointer("/messages/0"),
        Some(&json!({"role": "system", "content": "be helpful"}))
    );
    assert_eq!(
        body.pointer("/messages/1"),
        Some(&json!({"role": "user", "content": [{"type": "text", "text": "question"}]}))
    );
    assert_eq!(
        body.pointer("/messages/2/content"),
        Some(&json!("part one\n\npart two"))
    );
    assert_eq!(
        body.pointer("/messages/3"),
        Some(&json!({"role": "assistant", "content": null}))
    );
    assert_eq!(
        body.pointer("/messages/4"),
        Some(&json!({"role": "tool", "tool_call_id": "call_1", "content": "result"}))
    );

    Ok(())
}

/// User parts render as typed content, synthesizing the data URL of an inline image and borrowing
/// a remote URL verbatim.
#[test]
fn render_user_parts_and_image_urls() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.messages = vec![IrMessage {
        role: IrRole::User,
        parts: vec![
            IrPart::Text {
                text: "look".to_string(),
            },
            IrPart::Image {
                image: IrImage::Inline {
                    mime_type: "image/png".to_string(),
                    data_base64: "QUJD".to_string(),
                },
            },
            IrPart::Image {
                image: IrImage::Url("https://example.com/cat.png".to_string()),
            },
        ],
    }];

    let body = rendered(&ir, false)?;

    assert_eq!(
        body.pointer("/messages/0/content/0"),
        Some(&json!({"type": "text", "text": "look"}))
    );
    assert_eq!(
        body.pointer("/messages/0/content/1"),
        Some(&json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,QUJD"}}))
    );
    assert_eq!(
        body.pointer("/messages/0/content/2"),
        Some(&json!({"type": "image_url", "image_url": {"url": "https://example.com/cat.png"}}))
    );

    Ok(())
}

/// Assistant tool calls render with the function kind and carry the thought signature, and the
/// advertised tools render with their schema.
#[test]
fn render_tool_calls_and_tools() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.messages = vec![assistant_call("call_1", Some("sig"))];
    ir.tools = vec![IrTool {
        name: "find".to_string(),
        description: Some("Finds things.".to_string()),
        parameters: Some(json!({"type": "object"})),
    }];

    let body = rendered(&ir, false)?;

    assert_eq!(
        body.pointer("/messages/0/tool_calls/0"),
        Some(&json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "find", "arguments": "{\"q\":1}"},
            "thought_signature": "sig"
        }))
    );
    assert_eq!(
        body.pointer("/tools/0"),
        Some(&json!({
            "type": "function",
            "function": {
                "name": "find",
                "description": "Finds things.",
                "parameters": {"type": "object"}
            }
        }))
    );

    Ok(())
}

/// The tool choice renders as the bare mode string or the nested named function.
#[test]
fn render_tool_choice_shapes() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.tool_choice = Some(IrToolChoice::Required);

    let body = rendered(&ir, false)?;

    assert_eq!(at(&body, "/tool_choice"), &json!("required"));

    ir.tool_choice = Some(IrToolChoice::Function {
        name: "find".to_string(),
    });

    let body = rendered(&ir, false)?;

    assert_eq!(
        at(&body, "/tool_choice"),
        &json!({"type": "function", "function": {"name": "find"}})
    );

    ir.tool_choice = Some(IrToolChoice::None);

    let body = rendered(&ir, false)?;

    assert_eq!(at(&body, "/tool_choice"), &json!("none"));

    Ok(())
}

/// The generation controls render into their upstream slots and every absent control stays
/// absent.
#[test]
fn render_generation_controls() -> Result<(), Box<dyn StdError>> {
    use crate::ir::IrResponseFormat;

    let mut ir = ir_request();

    ir.temperature = Some(0.5);
    ir.top_p = Some(0.9);
    ir.max_output_tokens = Some(256);
    ir.response_format = Some(IrResponseFormat::JsonSchema {
        name: "out".to_string(),
        schema: json!({"type": "object"}),
    });
    ir.reasoning_effort = Some(ReasoningLevel::XHigh);

    let body = rendered(&ir, false)?;

    assert_eq!(at(&body, "/temperature"), &json!(0.5));
    assert_eq!(at(&body, "/top_p"), &json!(0.9));
    assert_eq!(at(&body, "/max_completion_tokens"), &json!(256));
    assert!(body.get("max_tokens").is_none());
    assert_eq!(
        at(&body, "/response_format"),
        &json!({"type": "json_schema", "json_schema": {"name": "out", "schema": {"type": "object"}}})
    );
    assert_eq!(at(&body, "/reasoning_effort"), &json!("xhigh"));

    let bare = rendered(&ir_request(), false)?;

    for absent in [
        "temperature",
        "top_p",
        "max_completion_tokens",
        "response_format",
        "reasoning_effort",
        "tools",
        "tool_choice",
    ] {
        assert!(bare.get(absent).is_none(), "{absent} must stay absent");
    }

    Ok(())
}

/// The streaming decision renders with the trailing usage options only for a streamed request.
#[test]
fn render_streaming_shape() -> Result<(), Box<dyn StdError>> {
    let streaming = rendered(&ir_request(), true)?;

    assert_eq!(at(&streaming, "/stream"), &json!(true));
    assert_eq!(
        at(&streaming, "/stream_options"),
        &json!({"include_usage": true})
    );

    let buffered = rendered(&ir_request(), false)?;

    assert_eq!(at(&buffered, "/stream"), &json!(false));
    assert!(buffered.get("stream_options").is_none());

    Ok(())
}

/// The Chat Completions upstream has no compaction state channel, so a request carrying state
/// items fails the render instead of silently dropping opaque replay state.
#[test]
fn render_rejects_state_items() -> Result<(), Box<dyn StdError>> {
    let mut ir = ir_request();

    ir.state_items = vec![StateItem {
        offset: 0,
        item: json!({"type": "compaction", "encrypted_content": "native"}),
        is_gateway: false,
        is_compaction: true,
        family: StateFamily::Responses,
    }];

    let error = super::render_request_body(&ir, false)
        .err()
        .ok_or("the state-carrying request must be rejected")?;

    assert!(error.to_string().contains("cannot replay"));

    Ok(())
}

/// The streamed chunks open exactly once and order reasoning, content, and refusal onto the text
/// channels in the shared contract's order.
#[test]
fn stream_opens_once_and_orders_text_channels() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "id": "up-1",
        "model": "upstream-model",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}}]
    })))?;

    assert_eq!(
        events,
        vec![IrEvent::MessageStart {
            id: "up-1".to_string(),
            model: "upstream-model".to_string(),
        }]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"index": 0, "delta": {"reasoning_content": "think"}}]
    })))?;

    assert_eq!(events, vec![IrEvent::ReasoningDelta("think".to_string())]);

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"index": 0, "delta": {"content": "Hel"}}]
    })))?;

    assert_eq!(events, vec![IrEvent::TextDelta("Hel".to_string())]);

    // The refusal rides the ordinary text channel after any content text of the same object.
    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"index": 0, "delta": {"content": "lo", "refusal": "or not"}}]
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::TextDelta("lo".to_string()),
            IrEvent::TextDelta("or not".to_string()),
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    })))?;

    assert_eq!(events, vec![IrEvent::finish(IrFinishReason::Stop)]);

    Ok(())
}

/// When both reasoning keys are present `reasoning` wins; empty payloads deliver nothing.
#[test]
fn reasoning_key_preference() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{
            "delta": {"reasoning": "from-reasoning", "reasoning_content": "from-deepseek"}
        }]
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ReasoningDelta("from-reasoning".to_string()),
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"content": "", "reasoning_content": ""}}]
    })))?;

    assert!(events.is_empty());

    Ok(())
}

/// Tool-call deltas accumulate per index and complete at the finish chunk; the last delta entry
/// carrying a signature wins.
#[test]
fn tool_call_deltas_accumulate_and_complete_at_finish() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"tool_calls": [
            {
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": {"name": "find", "arguments": "{\"q\""}
            },
            {
                "index": 1,
                "id": "call_2",
                "function": {"name": "other", "arguments": "{}"},
                "thought_signature": "sig-a"
            }
        ]}}]
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call_1".to_string()),
                name: Some("find".to_string()),
                arguments_delta: "{\"q\"".to_string(),
            },
            IrEvent::ToolCallDelta {
                index: 1,
                call_id: Some("call_2".to_string()),
                name: Some("other".to_string()),
                arguments_delta: "{}".to_string(),
            },
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": ":1}"}, "thought_signature": "sig-b"}
        ]}}]
    })))?;

    assert_eq!(
        events,
        vec![IrEvent::ToolCallDelta {
            index: 0,
            call_id: None,
            name: None,
            arguments_delta: ":1}".to_string(),
        }]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {}, "finish_reason": "tool_calls"}]
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_1".to_string(),
                name: "find".to_string(),
                arguments: "{\"q\":1}".to_string(),
                thought_signature: Some("sig-b".to_string()),
            },
            IrEvent::ToolCallDone {
                index: 1,
                call_id: "call_2".to_string(),
                name: "other".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig-a".to_string()),
            },
            IrEvent::finish(IrFinishReason::ToolCall),
        ]
    );

    Ok(())
}

/// Calls opened after the finish chunk complete at the `[DONE]` sentinel, and a delta reopening an
/// already-completed index never re-completes it.
#[test]
fn late_calls_complete_at_done_sentinel() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}]
    })))?;

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "call_late", "function": {"name": "f", "arguments": "{\"a\""}}
        ]}}]
    })))?;

    assert!(matches!(
        &events[..],
        [IrEvent::ToolCallDelta { index: 0, .. }]
    ));

    // A reopened index keeps streaming deltas but is never completed twice.
    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": ":1}"}}
        ]}}]
    })))?;

    assert!(matches!(
        &events[..],
        [IrEvent::ToolCallDelta { index: 0, .. }]
    ));

    let events = parser.ingest_frame(&done_frame())?;

    assert_eq!(
        events,
        vec![IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_late".to_string(),
            name: "f".to_string(),
            arguments: "{\"a\":1}".to_string(),
            thought_signature: None,
        }]
    );

    let again = parser.ingest_frame(&done_frame())?;

    assert!(again.is_empty());

    Ok(())
}

/// A clean upstream end-of-stream without a finish degrades to exactly one stop finish.
#[test]
fn missing_finish_degrades_to_stop_once() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"content": "hi"}}]
    })))?;

    assert_eq!(parser.finish(), vec![IrEvent::finish(IrFinishReason::Stop)]);
    assert!(parser.finish().is_empty());

    Ok(())
}

/// The primary choice wins by index zero, and an upstream that omits indexes falls back to the
/// first entry.
#[test]
fn primary_choice_selection() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [
            {"index": 1, "delta": {"content": "wrong"}},
            {"index": 0, "delta": {"content": "right"}}
        ]
    })))?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::TextDelta("right".to_string()),
        ]
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "choices": [{"delta": {"content": "first-of-indexless"}}]
    })))?;

    assert_eq!(
        events,
        vec![IrEvent::TextDelta("first-of-indexless".to_string())]
    );

    Ok(())
}

/// An embedded upstream error fails the frame (and the body) carrying its text and code.
#[test]
fn embedded_errors_fail_frames_and_bodies() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let error = parser
        .ingest_frame(&data_frame(json!({
            "choices": [],
            "error": {"message": "boom", "code": 42}
        })))
        .err()
        .ok_or("the embedded error must fail the frame")?;

    assert_eq!(error.to_string(), "boom (upstream error code: 42)");

    let error = parser
        .parse_json_response(&serde_json::to_vec(&json!({
            "choices": [],
            "error": "plain failure"
        }))?)
        .err()
        .ok_or("the string error must fail the body")?;

    assert_eq!(error.to_string(), "plain failure");

    Ok(())
}

/// The complete body parses text, reasoning, and complete calls as the canonical delta-plus-done
/// pair, and consults the legacy `function_call` only when no modern entry exists.
#[test]
fn body_parses_complete_calls_and_legacy_function_call() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "id": "up-1",
        "model": "upstream-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "answer",
                "reasoning_content": "think",
                "tool_calls": [{
                    "id": "call_9",
                    "type": "function",
                    "function": {"name": "find", "arguments": "{}"},
                    "thought_signature": "sig"
                }],
                "function_call": {"name": "ignored", "arguments": "{}"}
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 4,
            "prompt_tokens_details": {"cached_tokens": 3, "cache_write_tokens": 2},
            "completion_tokens_details": {"reasoning_tokens": 1}
        }
    }))?)?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: "up-1".to_string(),
                model: "upstream-model".to_string(),
            },
            IrEvent::ReasoningDelta("think".to_string()),
            IrEvent::TextDelta("answer".to_string()),
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call_9".to_string()),
                name: Some("find".to_string()),
                arguments_delta: "{}".to_string(),
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_9".to_string(),
                name: "find".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig".to_string()),
            },
            IrEvent::finish(IrFinishReason::ToolCall),
            IrEvent::Usage(usage()),
        ]
    );

    let mut parser = UpstreamParser::new();

    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "choices": [{
            "message": {
                "function_call": {"name": "legacy", "arguments": "{\"a\":1}"}
            },
            "finish_reason": "function_call"
        }]
    }))?)?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call_0".to_string()),
                name: Some("legacy".to_string()),
                arguments_delta: "{\"a\":1}".to_string(),
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_0".to_string(),
                name: "legacy".to_string(),
                arguments: "{\"a\":1}".to_string(),
                thought_signature: None,
            },
            IrEvent::finish(IrFinishReason::ToolCall),
        ]
    );

    Ok(())
}

/// The finish reason strings map onto the canonical vocabulary, degrading unknown values to stop.
#[test]
fn finish_reason_mapping() -> Result<(), Box<dyn StdError>> {
    assert_eq!(
        super::finish_reason_from_openai("length"),
        IrFinishReason::Length
    );
    assert_eq!(
        super::finish_reason_from_openai("tool_calls"),
        IrFinishReason::ToolCall
    );
    assert_eq!(
        super::finish_reason_from_openai("function_call"),
        IrFinishReason::ToolCall
    );
    assert_eq!(
        super::finish_reason_from_openai("content_filter"),
        IrFinishReason::ContentFilter
    );
    assert_eq!(
        super::finish_reason_from_openai("stop"),
        IrFinishReason::Stop
    );
    assert_eq!(
        super::finish_reason_from_openai("whatever"),
        IrFinishReason::Stop
    );

    let mut parser = UpstreamParser::new();

    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "choices": [{"message": {"content": "partial"}, "finish_reason": "length"}]
    }))?)?;

    assert!(matches!(
        &events[..],
        [
            ..,
            IrEvent::Finish {
                reason: IrFinishReason::Length,
                ..
            }
        ]
    ));

    Ok(())
}

/// Usage totals and the official detail breakdowns ride into the IR; absent details stay absent.
#[test]
fn usage_breakdowns_ride_into_ir() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "choices": [],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 4,
            "prompt_tokens_details": {"cached_tokens": 3},
            "completion_tokens_details": {}
        }
    }))?)?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::finish(IrFinishReason::Stop),
            IrEvent::Usage(IrUsage {
                input_tokens: 10,
                output_tokens: 4,
                cached_tokens: Some(3),
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        ]
    );

    Ok(())
}

/// A body without choices or a finish reason still opens and terminates with the degraded stop.
#[test]
fn body_without_choices_defaults_to_stop() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.parse_json_response(b"{}")?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::finish(IrFinishReason::Stop),
        ]
    );

    Ok(())
}

/// The aggregated body carries the official non-streaming shape: required-nullable members stay
/// present, the finish reason defaults to stop, and usage totals are rendered.
#[test]
fn aggregated_body_carries_the_official_shape() -> Result<(), Box<dyn StdError>> {
    let aggregation = IrAggregation {
        text: "answer".to_string(),
        reasoning: "think".to_string(),
        tool_calls: vec![IrAggregatedToolCall {
            call_id: "call_1".to_string(),
            name: "find".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("sig".to_string()),
        }],
        finish: Some(IrFinishReason::ToolCall),
        usage: Some(usage()),
        ..IrAggregation::default()
    };

    let body = serde_json::to_value(client_render::render_json_response(
        "public-model",
        &aggregation,
    ))?;

    assert_eq!(at(&body, "/object"), &json!("chat.completion"));
    assert_eq!(at(&body, "/model"), &json!("public-model"));
    assert_eq!(at(&body, "/choices").as_array().map(Vec::len), Some(1));
    assert_eq!(body.pointer("/choices/0/index"), Some(&json!(0)));
    assert_eq!(
        body.pointer("/choices/0/finish_reason"),
        Some(&json!("tool_calls"))
    );
    assert_eq!(body.pointer("/choices/0/logprobs"), Some(&Value::Null));
    assert_eq!(
        body.pointer("/choices/0/message/role"),
        Some(&json!("assistant"))
    );
    assert_eq!(
        body.pointer("/choices/0/message/content"),
        Some(&json!("answer"))
    );
    assert_eq!(
        body.pointer("/choices/0/message/refusal"),
        Some(&Value::Null)
    );
    assert_eq!(
        body.pointer("/choices/0/message/reasoning_content"),
        Some(&json!("think"))
    );
    assert_eq!(
        body.pointer("/choices/0/message/tool_calls/0"),
        Some(&json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "find", "arguments": "{}"},
            "thought_signature": "sig"
        }))
    );
    assert_eq!(
        at(&body, "/usage"),
        &json!({"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14})
    );

    Ok(())
}

/// A turn that produced no text or reasoning renders null content with no reasoning member, and a
/// finish the upstream never reported degrades to stop.
#[test]
fn aggregated_empty_turn_renders_defaults() -> Result<(), Box<dyn StdError>> {
    let body = serde_json::to_value(client_render::render_json_response(
        "public-model",
        &IrAggregation::default(),
    ))?;

    assert_eq!(
        body.pointer("/choices/0/finish_reason"),
        Some(&json!("stop"))
    );
    assert_eq!(
        body.pointer("/choices/0/message/content"),
        Some(&Value::Null)
    );
    assert!(
        body.pointer("/choices/0/message/reasoning_content")
            .is_none()
    );
    assert!(body.pointer("/choices/0/message/tool_calls").is_none());
    assert_eq!(
        at(&body, "/usage"),
        &json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0})
    );

    Ok(())
}

/// The live stream renders the role chunk exactly once, then one chunk per text or reasoning
/// delta, all sharing one completion identity.
#[test]
fn stream_renders_role_text_and_reasoning_chunks() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    let frames = renderer.render(&IrEvent::MessageStart {
        id: "up-1".to_string(),
        model: "upstream-model".to_string(),
    });

    assert_eq!(frames.len(), 1);

    let (event, role_chunk) = frame(first_frame(&frames)?)?;

    assert_eq!(event, None);
    assert_eq!(at(&role_chunk, "/object"), &json!("chat.completion.chunk"));
    assert_eq!(at(&role_chunk, "/model"), &json!("public-model"));
    assert_eq!(role_chunk.pointer("/choices/0/index"), Some(&json!(0)));
    assert_eq!(
        role_chunk.pointer("/choices/0/finish_reason"),
        Some(&Value::Null)
    );
    assert_eq!(
        role_chunk.pointer("/choices/0/delta"),
        Some(&json!({"role": "assistant", "content": ""}))
    );

    // A repeated message start never re-opens the stream.
    let frames = renderer.render(&IrEvent::MessageStart {
        id: "again".to_string(),
        model: String::new(),
    });

    assert!(frames.is_empty());

    let frames = renderer.render(&IrEvent::TextDelta("Hel".to_string()));

    let (_, text_chunk) = frame(first_frame(&frames)?)?;

    assert_eq!(
        text_chunk.pointer("/choices/0/delta"),
        Some(&json!({"content": "Hel"}))
    );

    let frames = renderer.render(&IrEvent::ReasoningDelta("think".to_string()));

    let (_, reasoning_chunk) = frame(first_frame(&frames)?)?;

    assert_eq!(
        reasoning_chunk.pointer("/choices/0/delta"),
        Some(&json!({"reasoning_content": "think"}))
    );

    Ok(())
}

/// A tool call renders its opening delta with the identity fragments, and its done renders only
/// the argument suffix the deltas did not stream — with the signature on its own chunk.
#[test]
fn tool_deltas_render_opening_then_residual_suffix() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    let frames = renderer.render(&IrEvent::ToolCallDelta {
        index: 0,
        call_id: Some("call_1".to_string()),
        name: Some("find".to_string()),
        arguments_delta: "{\"q\"".to_string(),
    });

    assert_eq!(frames.len(), 1);

    let (_, opening) = frame(first_frame(&frames)?)?;

    assert_eq!(
        opening.pointer("/choices/0/delta/tool_calls/0"),
        Some(&json!({
            "index": 0,
            "id": "call_1",
            "type": "function",
            "function": {"name": "find", "arguments": "{\"q\""}
        }))
    );

    let frames = renderer.render(&IrEvent::ToolCallDone {
        index: 0,
        call_id: "call_1".to_string(),
        name: "find".to_string(),
        arguments: "{\"q\":1}".to_string(),
        thought_signature: Some("sig".to_string()),
    });

    assert_eq!(frames.len(), 2);

    let mut chunks = frames.iter();

    let (_, residual) = frame(chunks.next().ok_or("the residual chunk renders")?)?;

    assert_eq!(
        residual.pointer("/choices/0/delta/tool_calls/0"),
        Some(&json!({"index": 0, "function": {"arguments": ":1}"}}))
    );

    let (_, signature) = frame(chunks.next().ok_or("the signature chunk renders")?)?;

    assert_eq!(
        signature.pointer("/choices/0/delta/tool_calls/0"),
        Some(&json!({"index": 0, "thought_signature": "sig"}))
    );

    // A done whose arguments the deltas already streamed in full renders no argument chunk.
    renderer.render(&IrEvent::ToolCallDelta {
        index: 5,
        call_id: Some("call_2".to_string()),
        name: Some("other".to_string()),
        arguments_delta: "{}".to_string(),
    });

    let frames = renderer.render(&IrEvent::ToolCallDone {
        index: 5,
        call_id: "call_2".to_string(),
        name: "other".to_string(),
        arguments: "{}".to_string(),
        thought_signature: None,
    });

    assert!(frames.is_empty());

    Ok(())
}

/// A done for an index no delta opened carries the complete call and renders as one full delta
/// chunk, exactly the shape a delta would have produced.
#[test]
fn done_without_deltas_renders_the_full_call() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    let frames = renderer.render(&IrEvent::ToolCallDone {
        index: 3,
        call_id: "call_recovered".to_string(),
        name: "find".to_string(),
        arguments: "{\"q\":1}".to_string(),
        thought_signature: None,
    });

    assert_eq!(frames.len(), 1);

    let (_, recovery) = frame(first_frame(&frames)?)?;

    assert_eq!(
        recovery.pointer("/choices/0/delta/tool_calls/0"),
        Some(&json!({
            "index": 0,
            "id": "call_recovered",
            "type": "function",
            "function": {"name": "find", "arguments": "{\"q\":1}"}
        }))
    );

    Ok(())
}

/// The dense zero-based outward numbering holds however the upstream numbered its calls.
#[test]
fn ir_indexes_renumber_to_dense_outward_ordinals() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    let frames = renderer.render(&IrEvent::ToolCallDelta {
        index: 7,
        call_id: Some("call_7".to_string()),
        name: None,
        arguments_delta: String::new(),
    });

    let (_, first) = frame(first_frame(&frames)?)?;

    assert_eq!(
        first.pointer("/choices/0/delta/tool_calls/0/index"),
        Some(&json!(0))
    );

    let frames = renderer.render(&IrEvent::ToolCallDone {
        index: 2,
        call_id: "call_2".to_string(),
        name: "other".to_string(),
        arguments: "{}".to_string(),
        thought_signature: None,
    });

    let (_, second) = frame(first_frame(&frames)?)?;

    assert_eq!(
        second.pointer("/choices/0/delta/tool_calls/0"),
        Some(&json!({
            "index": 1,
            "id": "call_2",
            "type": "function",
            "function": {"name": "other", "arguments": "{}"}
        }))
    );

    Ok(())
}

/// The trailing usage chunk renders once, terminally, only when requested and observed.
#[test]
fn usage_chunk_renders_once_when_requested() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), true);

    assert!(renderer.render(&IrEvent::Usage(usage())).is_empty());

    let frames = renderer.render(&IrEvent::Done);

    assert_eq!(frames.len(), 2);

    let mut chunks = frames.iter();

    let (_, usage_chunk) = frame(chunks.next().ok_or("the usage chunk renders")?)?;

    assert_eq!(at(&usage_chunk, "/choices"), &json!([]));
    assert_eq!(
        at(&usage_chunk, "/usage"),
        &json!({"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14})
    );
    assert_eq!(chunks.next().map(String::as_str), Some("data: [DONE]\n\n"));

    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), true);

    assert_eq!(renderer.render(&IrEvent::Done), vec!["data: [DONE]\n\n"]);

    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    renderer.render(&IrEvent::Usage(usage()));

    assert_eq!(renderer.render(&IrEvent::Done), vec!["data: [DONE]\n\n"]);

    Ok(())
}

/// Finish reasons render terminally under their OpenAI spellings.
#[test]
fn finish_reasons_render_terminally() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    let frames = renderer.render(&IrEvent::finish(IrFinishReason::Length));

    assert_eq!(frames.len(), 1);

    let (_, finish) = frame(first_frame(&frames)?)?;

    assert_eq!(
        finish.pointer("/choices/0/finish_reason"),
        Some(&json!("length"))
    );
    assert_eq!(finish.pointer("/choices/0/delta"), Some(&json!({})));

    let frames = renderer.render(&IrEvent::finish(IrFinishReason::ToolCall));

    let (_, finish) = frame(first_frame(&frames)?)?;

    assert_eq!(
        finish.pointer("/choices/0/finish_reason"),
        Some(&json!("tool_calls"))
    );

    Ok(())
}

/// A mid-stream error renders the shared classification in the OpenAI error envelope.
#[test]
fn error_frame_renders_the_openai_envelope() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("public-model".to_string(), false);

    let raw = renderer.error_frame(&Error::MalformedRequest {
        param: Some("temperature".to_string()),
        message: "out of range".to_string(),
    });

    let (event, body) = frame(&raw)?;

    assert_eq!(event, None);
    assert_eq!(
        body,
        json!({"error": {
            "message": "out of range",
            "type": "invalid_request_error",
            "param": "temperature",
            "code": null
        }})
    );

    Ok(())
}

/// The aggregated-body projection fingerprints the actual answer fields, and declines a body
/// whose tool arguments are not a JSON object.
#[test]
fn aggregated_projection_gates_on_argument_objects() -> Result<(), Box<dyn StdError>> {
    let answer = IrAggregation {
        text: "answer".to_string(),
        tool_calls: vec![IrAggregatedToolCall {
            call_id: "call_1".to_string(),
            name: "find".to_string(),
            arguments: "{\"q\":1}".to_string(),
            thought_signature: None,
        }],
        ..IrAggregation::default()
    };

    let response = client_render::render_json_response("public-model", &answer);

    let mut projector = Projector::new();

    assert!(super::Format::project_json_response(
        &response,
        &mut projector
    ));

    let malformed = IrAggregation {
        tool_calls: vec![IrAggregatedToolCall {
            call_id: "call_1".to_string(),
            name: "find".to_string(),
            arguments: "[1,2]".to_string(),
            thought_signature: None,
        }],
        ..IrAggregation::default()
    };

    let response = client_render::render_json_response("public-model", &malformed);

    let mut declined = Projector::new();

    assert!(!super::Format::project_json_response(
        &response,
        &mut declined
    ));

    Ok(())
}
