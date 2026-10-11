//! Tests for the Gemini Interactions codec: the client request parser, the upstream request
//! renderer, the upstream response parser, the client response renderers, and the JSON affinity
//! projection of the aggregated resource.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{UpstreamParser, json_affinity, request_parse};
use crate::{
    format::{ClientSseRenderer as _, ParsedRequest, RouteContext, UpstreamEventParser as _},
    ir::{
        Aggregator, InvalidRequest, IrAggregation, IrEvent, IrFinishReason, IrImage, IrMessage,
        IrPart, IrReasoningBlock, IrRequest, IrResponseFormat, IrRole, IrToolChoice, IrUsage,
        StateFamily, StateItem,
    },
    sse::SseFrame,
    vocabulary::ReasoningLevel,
};
use serde_json::{Value, json};
use std::error::Error as StdError;

/// The request boundary's parse outcome.
type Parsed = Result<ParsedRequest, InvalidRequest>;

/// The empty route context: `POST /interactions` routes no model from the URL.
fn body_route() -> RouteContext {
    RouteContext::default()
}

/// A route context carrying the `?alt=sse` streaming override.
fn override_route(is_streaming: bool) -> RouteContext {
    RouteContext {
        is_streaming: Some(is_streaming),
        ..RouteContext::default()
    }
}

/// Serializes the JSON fixture body and parses it as an Interactions request.
fn parse(body: Value, route: &RouteContext) -> Result<Parsed, Box<dyn StdError>> {
    let raw = serde_json::to_vec(&body)?;

    Ok(request_parse::parse_request(&raw, route))
}

/// Parses the JSON fixture into IR, failing the test when the request is rejected.
fn parsed(body: Value, route: &RouteContext) -> Result<ParsedRequest, Box<dyn StdError>> {
    Ok(parse(body, route)??)
}

/// Turns a parse result into its rejection, failing the test when the parse succeeded.
fn rejection(parsed: Parsed) -> Result<InvalidRequest, Box<dyn StdError>> {
    match parsed {
        Err(error) => Ok(error),

        Ok(_) => Err("expected the request to be rejected".into()),
    }
}

/// Unwraps an option the test-assertion way: a missing value fails the test through the result.
fn required<T>(value: Option<T>) -> Result<T, Box<dyn StdError>> {
    value.ok_or_else(|| "expected a value the codec must produce".into())
}

/// A minimal valid body over the given input: the model rides the body.
fn minimal_body(input: Value) -> Value {
    json!({
        "model": "gemini-3-pro",
        "input": input,
    })
}

/// An upstream frame carrying an `event:` line and a JSON payload.
fn event_frame(event: &str, payload: Value) -> SseFrame {
    SseFrame {
        data: payload.to_string(),
        event: Some(event.to_string()),
    }
}

/// An upstream frame discriminated only by its payload's `event_type` member.
fn typed_frame(event_type: &str, mut payload: Value) -> SseFrame {
    if let Some(object) = payload.as_object_mut() {
        object.insert("event_type".to_string(), json!(event_type));
    }

    SseFrame {
        data: payload.to_string(),
        event: None,
    }
}

/// Renders an IR request into its upstream JSON shape over the given upstream id.
fn rendered(
    ir: &IrRequest,
    upstream_id: &str,
    is_streaming: bool,
) -> Result<Value, Box<dyn StdError>> {
    let body = super::render_request_body(ir, upstream_id, is_streaming)?;

    Ok(serde_json::to_value(&body)?)
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

/// An assistant message carrying one reasoning block.
fn reasoning(text: &str, signature: Option<&str>, opaque: Value) -> IrMessage {
    IrMessage {
        role: IrRole::Assistant,
        parts: vec![IrPart::Reasoning {
            text: text.to_string(),
            signature: signature.map(str::to_string),
            opaque,
        }],
    }
}

/// The usage fixture every renderer test shares: reasoning and cache breakdowns reported.
fn usage_fixture() -> IrUsage {
    IrUsage {
        input_tokens: 10,
        output_tokens: 25,
        cached_tokens: Some(3),
        cache_write_tokens: None,
        reasoning_tokens: Some(5),
    }
}

/// Collects the events' text deltas.
fn text_deltas(events: &[IrEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            IrEvent::TextDelta(text) => Some(text.as_str()),

            _ => None,
        })
        .collect()
}

/// Collects the events' reasoning deltas.
fn reasoning_deltas(events: &[IrEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            IrEvent::ReasoningDelta(text) => Some(text.as_str()),

            _ => None,
        })
        .collect()
}

/// The stream's single finish reason, failing the test when absent or not unique.
fn finish_of(events: &[IrEvent]) -> Result<IrFinishReason, Box<dyn StdError>> {
    let finishes: Vec<IrFinishReason> = events
        .iter()
        .filter_map(|event| match event {
            IrEvent::Finish { reason, .. } => Some(*reason),

            _ => None,
        })
        .collect();

    let [reason] = finishes.as_slice() else {
        return Err(format!("expected exactly one finish, got {finishes:?}").into());
    };

    Ok(*reason)
}

/// The stream's single usage report, failing the test when absent or not unique.
fn usage_of(events: &[IrEvent]) -> Result<IrUsage, Box<dyn StdError>> {
    let usages: Vec<IrUsage> = events
        .iter()
        .filter_map(|event| match event {
            IrEvent::Usage(usage) => Some(*usage),

            _ => None,
        })
        .collect();

    let [usage] = usages.as_slice() else {
        return Err(format!("expected exactly one usage report, got {usages:?}").into());
    };

    Ok(*usage)
}

/// The stream's single opening identity, failing the test when absent or not first.
fn start_of(events: &[IrEvent]) -> Result<(String, String), Box<dyn StdError>> {
    let Some(IrEvent::MessageStart { id, model }) = events.first() else {
        return Err("expected the stream to open with MessageStart".into());
    };

    Ok((id.clone(), model.clone()))
}

/// One parsed rendered frame: its event name and its JSON payload.
type RenderedFrame = (Option<String>, Value);

/// Parses the rendered SSE frame strings into their event names and JSON payloads.
fn rendered_frames(frames: &[String]) -> Result<Vec<RenderedFrame>, Box<dyn StdError>> {
    frames
        .iter()
        .map(|frame| {
            let (event, rest) = match frame.strip_prefix("event: ") {
                Some(rest) => {
                    let (event, rest) = rest.split_once('\n').ok_or("frame lacks a data line")?;

                    (Some(event.to_string()), rest)
                }

                None => (None, frame.as_str()),
            };
            let data = rest.strip_prefix("data: ").ok_or("frame lacks a payload")?;
            let data = data
                .strip_suffix("\n\n")
                .ok_or("frame lacks a terminator")?;

            Ok((event, serde_json::from_str(data)?))
        })
        .collect()
}

/// Aggregates the event stream the gateway folds before aggregated client rendering.
fn aggregated(events: &[IrEvent]) -> IrAggregation {
    let mut aggregator = Aggregator::default();

    for event in events {
        aggregator.ingest(event);
    }

    aggregator.finish()
}

/// The IR events of one complete streamed completion: reasoning, answer, and a signature-bearing
/// tool call.
fn streamed_events() -> Vec<IrEvent> {
    vec![
        IrEvent::MessageStart {
            id: "up".to_string(),
            model: "m".to_string(),
        },
        IrEvent::ReasoningDelta("thinking".to_string()),
        IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("think-sig".to_string()),
            opaque: Value::Null,
        },
        IrEvent::TextDelta("answer".to_string()),
        IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("c1".to_string()),
            name: Some("f".to_string()),
            arguments_delta: r#"{"a":"#.to_string(),
        },
        IrEvent::ToolCallDone {
            index: 0,
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: Some("call-sig".to_string()),
        },
        IrEvent::Finish {
            reason: IrFinishReason::Stop,
            prompt_block_reason: None,
        },
        IrEvent::Done,
    ]
}

/// Verifies the model rides the body and the streaming precedence: the `?alt=sse` override wins
/// whenever present, and the body's `stream` member decides otherwise.
#[test]
fn parses_body_model_and_streaming_precedence() -> Result<(), Box<dyn StdError>> {
    let request = parsed(minimal_body(json!("hi")), &body_route())?;

    assert_eq!(request.ir.model, "gemini-3-pro");
    assert!(!request.is_streaming);

    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "stream": true,
    });

    assert!(parsed(body, &body_route())?.is_streaming);

    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "stream": false,
    });

    assert!(parsed(body, &override_route(true))?.is_streaming);

    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "stream": true,
    });

    assert!(!parsed(body, &override_route(false))?.is_streaming);

    Ok(())
}

/// Verifies every official `input` union member normalizes into one user turn: the plain string,
/// a single content block, a content-block array, and the Step array.
#[test]
fn normalizes_input_union_members() -> Result<(), Box<dyn StdError>> {
    for input in [
        json!("hello"),
        json!({ "type": "text", "text": "hello" }),
        json!([{ "type": "text", "text": "hello" }]),
        json!([{ "type": "user_input", "content": [{ "type": "text", "text": "hello" }] }]),
    ] {
        let request = parsed(minimal_body(input.clone()), &body_route())?;

        assert_eq!(
            request.ir.messages,
            vec![user_text("hello")],
            "input {input}"
        );
    }

    Ok(())
}

/// Verifies the Step walk: user, model-output, thought, function-call, and function-result steps
/// map to their IR parts, with consecutive same-role steps coalescing into one message and the
/// thought step replaying verbatim as the reasoning block's opaque payload.
#[test]
fn parses_steps_and_coalesces_runs() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "model": "gemini-3-pro",
        "input": [
            { "type": "user_input", "content": [{ "type": "text", "text": "q" }] },
            { "type": "model_output", "content": [{ "type": "text", "text": "ans" }] },
            {
                "type": "thought",
                "summary": [{ "type": "text", "text": "th1" }, { "type": "text", "text": "th2" }],
                "signature": "s",
            },
            { "type": "function_call", "id": "c1", "name": "f", "arguments": { "a": 1 } },
            {
                "type": "function_result",
                "call_id": "c1",
                "name": "ignored",
                "result": "ok",
                "is_error": true,
            },
        ],
    });

    let request = parsed(body, &body_route())?;

    assert_eq!(request.ir.messages.len(), 3);

    let assistant = required(request.ir.messages.get(1))?;

    assert_eq!(assistant.role, IrRole::Assistant);
    assert_eq!(
        assistant.parts.first(),
        Some(&IrPart::Text {
            text: "ans".to_string(),
        })
    );
    assert_eq!(
        assistant.parts.get(1),
        Some(&IrPart::Reasoning {
            text: "th1th2".to_string(),
            signature: Some("s".to_string()),
            opaque: json!({
                "type": "thought",
                "summary": [{ "type": "text", "text": "th1" }, { "type": "text", "text": "th2" }],
                "signature": "s",
            }),
        })
    );
    assert_eq!(
        assistant.parts.get(2),
        Some(&IrPart::ToolCall {
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: None,
        })
    );

    let tool = required(request.ir.messages.get(2))?;

    assert_eq!(
        tool.parts.first(),
        Some(&IrPart::ToolOutput {
            call_id: "c1".to_string(),
            content: "ok".to_string(),
            is_error: true,
        })
    );

    Ok(())
}

/// Verifies image blocks: the `uri` and inline spellings parse, while both-at-once, inline data
/// without a MIME type, and neither spelling reject naming `content`.
#[test]
fn parses_image_blocks_and_rejects_malformed() -> Result<(), Box<dyn StdError>> {
    let request = parsed(
        minimal_body(json!([
            { "type": "text", "text": "see" },
            { "type": "image", "uri": "https://example.com/x.png" },
            { "type": "image", "data": "AA", "mime_type": "image/png", "resolution": "low" },
        ])),
        &body_route(),
    )?;

    assert_eq!(
        request
            .ir
            .messages
            .first()
            .and_then(|message| message.parts.last()),
        Some(&IrPart::Image {
            image: IrImage::Inline {
                mime_type: "image/png".to_string(),
                data_base64: "AA".to_string(),
            },
        })
    );

    for image in [
        json!({ "type": "image", "uri": "https://x", "data": "AA", "mime_type": "image/png" }),
        json!({ "type": "image", "data": "AA" }),
        json!({ "type": "image" }),
    ] {
        let body = minimal_body(json!([image]));

        let error = rejection(parse(body, &body_route())?)?;

        assert_eq!(error.param.as_deref(), Some("content"), "image {image}");
    }

    Ok(())
}

/// Verifies image blocks inside `function_result` content reject: the IR tool output has no media
/// channel, so silently dropping one would lose request content.
#[test]
fn rejects_images_in_function_results() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "model": "gemini-3-pro",
        "input": [{
            "type": "function_result",
            "call_id": "c1",
            "result": [{ "type": "image", "uri": "https://x" }],
        }],
    });

    let error = rejection(parse(body, &body_route())?)?;

    assert_eq!(error.param.as_deref(), Some("result"));

    Ok(())
}

/// Verifies the `function_result` payload shapes: strings ride verbatim, text blocks join with
/// the blank-line separator, objects flatten to JSON text, and the optional `name` is ignored.
#[test]
fn flattens_function_result_payloads() -> Result<(), Box<dyn StdError>> {
    let result = |result: Value| {
        let body = json!({
            "model": "gemini-3-pro",
            "input": [{
                "type": "function_result",
                "call_id": "c1",
                "name": "ignored",
                "result": result,
            }],
        });

        parsed(body, &body_route())
    };

    let request = result(json!("plain"))?;
    assert_eq!(
        request
            .ir
            .messages
            .first()
            .and_then(|message| message.parts.first()),
        Some(&IrPart::ToolOutput {
            call_id: "c1".to_string(),
            content: "plain".to_string(),
            is_error: false,
        })
    );

    let request = result(json!([
        { "type": "text", "text": "a", "annotations": [{ "x": 1 }] },
        { "type": "text", "text": "b" },
    ]))?;
    assert_eq!(
        request
            .ir
            .messages
            .first()
            .and_then(|message| message.parts.first()),
        Some(&IrPart::ToolOutput {
            call_id: "c1".to_string(),
            content: "a\n\nb".to_string(),
            is_error: false,
        })
    );

    let request = result(json!({ "ok": true }))?;
    assert_eq!(
        request
            .ir
            .messages
            .first()
            .and_then(|message| message.parts.first()),
        Some(&IrPart::ToolOutput {
            call_id: "c1".to_string(),
            content: r#"{"ok":true}"#.to_string(),
            is_error: false,
        })
    );

    Ok(())
}

/// Verifies every known-but-unsupported top-level control rejects naming its own field.
#[test]
fn rejects_known_unsupported_controls() -> Result<(), Box<dyn StdError>> {
    for member in [
        "previous_interaction_id",
        "background",
        "cached_content",
        "webhook_config",
        "response_modalities",
        "response_mime_type",
        "safety_settings",
        "agent",
        "agent_config",
        "environment",
    ] {
        let mut body = minimal_body(json!("hi"));

        let object = body
            .as_object_mut()
            .ok_or("the fixture body is an object")?;

        object.insert(member.to_string(), json!(1));

        let error = rejection(parse(body, &body_route())?)?;

        assert_eq!(
            error.param.as_deref(),
            Some(member),
            "the rejection must name `{member}`"
        );
    }

    Ok(())
}

/// Verifies the presentation-only members parse and change nothing: `store`, `service_tier`,
/// `labels`, and `generation_config.thinking_summaries`.
#[test]
fn accepts_presentation_members() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "store": { "whatever": true },
        "service_tier": "pro",
        "labels": { "team": "x" },
        "generation_config": { "thinking_summaries": true },
    });

    let request = parsed(body, &body_route())?;

    assert_eq!(request.ir.messages, vec![user_text("hi")]);

    Ok(())
}

/// Verifies unknown members and the unsupported `generation_config` controls reject, each naming
/// its field — including the sampling members this surface has no slot for.
#[test]
fn rejects_unknown_and_sampling_members() -> Result<(), Box<dyn StdError>> {
    let mut body = minimal_body(json!("hi"));

    let object = body
        .as_object_mut()
        .ok_or("the fixture body is an object")?;

    object.insert("nope".to_string(), json!(1));

    let error = rejection(parse(body, &body_route())?)?;

    assert!(error.param.is_none());
    assert!(error.to_string().contains("nope"));

    for member in [
        "seed",
        "stop_sequences",
        "speech_config",
        "image_config",
        "video_config",
        "transcription_config",
    ] {
        let body = json!({
            "model": "gemini-3-pro",
            "input": "hi",
            "generation_config": { member: 1 },
        });

        let error = rejection(parse(body, &body_route())?)?;

        assert_eq!(
            error.param.as_deref(),
            Some(member),
            "the rejection must name `{member}`"
        );
    }

    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "generation_config": { "temperature": 1.0 },
    });

    let error = rejection(parse(body, &body_route())?)?;

    assert!(error.param.is_none());
    assert!(error.to_string().contains("temperature"));

    Ok(())
}

/// Verifies the `generation_config` controls the surface reads: the output cap and the thinking
/// level in the closed vocabulary, whose unknown values reject naming `thinking_level`.
#[test]
fn parses_generation_config_controls() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "generation_config": { "max_output_tokens": 256, "thinking_level": "xhigh" },
    });

    let request = parsed(body, &body_route())?;

    assert_eq!(request.ir.max_output_tokens, Some(256));
    assert_eq!(request.ir.reasoning_effort, Some(ReasoningLevel::XHigh));

    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "generation_config": { "thinking_level": "bogus" },
    });

    let error = rejection(parse(body, &body_route())?)?;

    assert_eq!(error.param.as_deref(), Some("thinking_level"));

    Ok(())
}

/// Verifies the `response_format` table: the absent member is no format, `text` is free-form text
/// or JSON (named-schema beside a schema), and the media members, a schema without the JSON MIME
/// type, and foreign MIME types reject naming `response_format`.
#[test]
fn parses_response_format_members() -> Result<(), Box<dyn StdError>> {
    let format = |response_format: Value| {
        let body = json!({
            "model": "gemini-3-pro",
            "input": "hi",
            "response_format": response_format,
        });

        parse(body, &body_route())
    };

    let request = parsed(minimal_body(json!("hi")), &body_route())?;
    assert_eq!(request.ir.response_format, None);

    let request = parsed(
        json!({
            "model": "gemini-3-pro",
            "input": "hi",
            "response_format": { "type": "text" },
        }),
        &body_route(),
    )?;
    assert_eq!(request.ir.response_format, Some(IrResponseFormat::Text));

    let request = format(json!({ "type": "text", "mime_type": "application/json" }))??;
    assert_eq!(request.ir.response_format, Some(IrResponseFormat::Json));

    let request = format(json!({
        "type": "text",
        "mime_type": "application/json",
        "schema": { "type": "object" },
    }))??;
    assert_eq!(
        request.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "response".to_string(),
            schema: json!({ "type": "object" }),
        })
    );

    for response_format in [
        json!({ "type": "text", "mime_type": "text/plain", "schema": {} }),
        json!({ "type": "audio" }),
        json!({ "type": "image" }),
        json!({ "type": "video" }),
        json!({ "type": "text", "mime_type": "application/xml" }),
    ] {
        let error = rejection(format(response_format.clone())?)?;

        assert_eq!(
            error.param.as_deref(),
            Some("response_format"),
            "format {response_format}"
        );
    }

    Ok(())
}

/// Verifies the `tool_choice` forms: bare modes, the official object form whose mode defaults to
/// `any` and forces one function, and the rejections (`validated`, multiple allowed tools,
/// allowed tools beside `auto`/`none`, and any choice without advertised tools).
#[test]
fn parses_tool_choice_forms() -> Result<(), Box<dyn StdError>> {
    let tools = json!([{ "type": "function", "name": "f" }]);
    let choice = |tool_choice: Value, tools: Value| {
        let body = json!({
            "model": "gemini-3-pro",
            "input": "hi",
            "tools": tools,
            "generation_config": { "tool_choice": tool_choice },
        });

        parse(body, &body_route())
    };

    for (tool_choice, expected) in [
        (json!("auto"), Some(IrToolChoice::Auto)),
        (json!("any"), Some(IrToolChoice::Required)),
        (json!("none"), Some(IrToolChoice::None)),
        (
            json!({ "allowed_tools": { "tools": ["f"] } }),
            Some(IrToolChoice::Function {
                name: "f".to_string(),
            }),
        ),
    ] {
        let request = parsed(
            json!({
                "model": "gemini-3-pro",
                "input": "hi",
                "tools": tools.clone(),
                "generation_config": { "tool_choice": tool_choice },
            }),
            &body_route(),
        )?;

        assert_eq!(request.ir.tool_choice, expected, "choice {tool_choice}");
    }

    for tool_choice in [
        json!("validated"),
        json!("whatever"),
        json!({ "allowed_tools": { "mode": "any", "tools": ["a", "b"] } }),
        json!({ "allowed_tools": { "mode": "auto", "tools": ["f"] } }),
        json!({ "allowed_tools": { "mode": "none", "tools": ["f"] } }),
    ] {
        let error = rejection(choice(tool_choice.clone(), tools.clone())?)?;

        assert_eq!(
            error.param.as_deref(),
            Some("tool_choice"),
            "choice {tool_choice}"
        );
    }

    let error = rejection(choice(json!("auto"), json!([]))?)?;

    assert_eq!(error.param.as_deref(), Some("tool_choice"));

    Ok(())
}

/// Verifies advertised tools: function tools parse with their schema, and a foreign tool type
/// rejects naming `type`.
#[test]
fn parses_tools() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "tools": [{
            "type": "function",
            "name": "f",
            "description": "d",
            "parameters": { "type": "object" },
        }],
    });

    let request = parsed(body, &body_route())?;

    let tool = required(request.ir.tools.first())?;

    assert_eq!(tool.name, "f");
    assert_eq!(tool.description.as_deref(), Some("d"));
    assert_eq!(tool.parameters.as_ref(), Some(&json!({ "type": "object" })));

    let body = json!({
        "model": "gemini-3-pro",
        "input": "hi",
        "tools": [{ "type": "web_search", "name": "f" }],
    });

    let error = rejection(parse(body, &body_route())?)?;

    assert_eq!(error.param.as_deref(), Some("type"));

    Ok(())
}

/// Verifies the stateless envelope: the upstream id rides `model`, `store` is always false, the
/// streaming flag rides through, and instructions are omitted when absent.
#[test]
fn renders_stateless_envelope() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "public".to_string(),
        instructions: Some("be terse".to_string()),
        messages: vec![user_text("u1")],
        ..IrRequest::default()
    };

    let payload = rendered(&ir, "upstream-model", true)?;

    assert_eq!(required(payload.get("model"))?, &json!("upstream-model"));
    assert_eq!(required(payload.get("store"))?, &json!(false));
    assert_eq!(required(payload.get("stream"))?, &json!(true));
    assert_eq!(
        required(payload.get("system_instruction"))?,
        &json!("be terse")
    );
    assert!(payload.get("previous_interaction_id").is_none());

    let payload = rendered(
        &IrRequest {
            model: "public".to_string(),
            messages: vec![user_text("u1")],
            ..IrRequest::default()
        },
        "upstream-model",
        false,
    )?;

    assert!(payload.get("system_instruction").is_none());
    assert_eq!(required(payload.get("stream"))?, &json!(false));

    Ok(())
}

/// Verifies the input replay: consecutive user messages coalesce into one `user_input` step,
/// assistant messages render one step per part, a user run whose parts all drop renders no step,
/// and tool outputs render as `function_result` steps with the resolved call name.
#[test]
fn renders_input_steps_and_coalesces_user_runs() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "public".to_string(),
        messages: vec![
            user_text("u1"),
            user_text("u2"),
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::Text {
                        text: "m1".to_string(),
                    },
                    IrPart::Image {
                        image: IrImage::Url("https://example.com/x.png".to_string()),
                    },
                    IrPart::ToolOutput {
                        call_id: "stray".to_string(),
                        content: "misplaced".to_string(),
                        is_error: false,
                    },
                ],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Text {
                    text: "m2".to_string(),
                }],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::ToolCall {
                    call_id: "c1".to_string(),
                    name: "f".to_string(),
                    arguments: "{}".to_string(),
                    thought_signature: None,
                }],
            },
            tool_output("c1", "ok"),
            user_text("u3"),
        ],
        tools: vec![crate::ir::IrTool {
            name: "f".to_string(),
            description: None,
            parameters: None,
        }],
        ..IrRequest::default()
    };

    let payload = rendered(&ir, "upstream-model", false)?;
    let input = required(payload.get("input"))?.as_array().cloned();

    let input = required(input)?;

    assert_eq!(
        input,
        vec![
            json!({
                "type": "user_input",
                "content": [{ "type": "text", "text": "u1" }, { "type": "text", "text": "u2" }],
            }),
            json!({ "type": "model_output", "content": [{ "type": "text", "text": "m1" }] }),
            json!({ "type": "model_output", "content": [{ "type": "text", "text": "m2" }] }),
            json!({ "type": "function_call", "id": "c1", "name": "f", "arguments": {} }),
            json!({
                "type": "function_result",
                "name": "f",
                "call_id": "c1",
                "result": [{ "type": "text", "text": "ok" }],
            }),
            json!({ "type": "user_input", "content": [{ "type": "text", "text": "u3" }] }),
        ]
    );

    Ok(())
}

/// Verifies the thought-step rendering: a same-family step replays byte-exact (foreign members
/// preserved), a foreign or text-only block synthesizes from text and signature, a signature-only
/// block omits the summary, and an empty block renders the bare thought tag.
#[test]
fn replays_thought_steps() -> Result<(), Box<dyn StdError>> {
    let verbatim = json!({
        "type": "thought",
        "id": "step-1",
        "summary": [{ "type": "text", "text": "t", "annotations": [{ "x": 1 }] }],
        "signature": "s",
        "producer_extra": 7,
    });

    let ir = IrRequest {
        model: "public".to_string(),
        messages: vec![
            reasoning("t", Some("s"), verbatim.clone()),
            reasoning("th", Some("sig"), Value::Null),
            reasoning("", Some("sig"), Value::Null),
            reasoning("", None, Value::Null),
            reasoning(
                "x",
                None,
                json!({ "type": "redacted_thinking", "data": "d" }),
            ),
        ],
        ..IrRequest::default()
    };

    let payload = rendered(&ir, "upstream-model", false)?;
    let input = required(payload.get("input"))?.as_array().cloned();

    let input = required(input)?;

    assert_eq!(required(input.first())?, &verbatim, "verbatim replay");
    assert_eq!(
        required(input.get(1))?,
        &json!({
            "type": "thought",
            "summary": [{ "type": "text", "text": "th" }],
            "signature": "sig",
        })
    );
    assert_eq!(
        required(input.get(2))?,
        &json!({ "type": "thought", "signature": "sig" })
    );
    assert_eq!(required(input.get(3))?, &json!({ "type": "thought" }));
    assert_eq!(
        required(input.get(4))?,
        &json!({
            "type": "thought",
            "summary": [{ "type": "text", "text": "x" }],
        }),
        "a foreign opaque payload drops and synthesizes from text"
    );

    Ok(())
}

/// Verifies function-call steps (arguments as the official JSON object, no signature member),
/// the tool-choice mapping, and the generation config: thinking level and output cap render,
/// sampling has no wire slot and is dropped, and an all-absent config is omitted.
#[test]
fn renders_calls_results_tool_choice_and_config() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "public".to_string(),
        messages: vec![
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::ToolCall {
                    call_id: "c1".to_string(),
                    name: "f".to_string(),
                    arguments: r#"{"a":1}"#.to_string(),
                    thought_signature: Some("s".to_string()),
                }],
            },
            tool_output("c1", "ok"),
        ],
        tools: vec![crate::ir::IrTool {
            name: "f".to_string(),
            description: None,
            parameters: None,
        }],
        tool_choice: Some(IrToolChoice::Function {
            name: "f".to_string(),
        }),
        max_output_tokens: Some(256),
        reasoning_effort: Some(ReasoningLevel::High),
        temperature: Some(0.7),
        top_p: Some(0.9),
        ..IrRequest::default()
    };

    let payload = rendered(&ir, "upstream-model", false)?;
    let input = required(payload.get("input"))?.as_array().cloned();

    let input = required(input)?;

    assert_eq!(
        required(input.first())?,
        &json!({ "type": "function_call", "id": "c1", "name": "f", "arguments": { "a": 1 } })
    );
    assert_eq!(
        required(input.get(1))?,
        &json!({
            "type": "function_result",
            "name": "f",
            "call_id": "c1",
            "result": [{ "type": "text", "text": "ok" }],
        })
    );
    assert_eq!(
        required(payload.get("generation_config"))?,
        &json!({
            "max_output_tokens": 256,
            "thinking_level": "high",
            "tool_choice": { "allowed_tools": { "mode": "any", "tools": ["f"] } },
        })
    );
    assert!(
        !payload.to_string().contains("temperature"),
        "sampling has no Interactions wire slot"
    );

    let bare = |choice: IrToolChoice| {
        let ir = IrRequest {
            model: "public".to_string(),
            messages: vec![user_text("u1")],
            tools: vec![crate::ir::IrTool {
                name: "f".to_string(),
                description: None,
                parameters: None,
            }],
            tool_choice: Some(choice),
            ..IrRequest::default()
        };

        rendered(&ir, "upstream-model", false)
    };

    assert_eq!(
        required(required(bare(IrToolChoice::Auto)?.get("generation_config"))?.get("tool_choice"))?,
        &json!("auto")
    );
    assert_eq!(
        required(
            required(bare(IrToolChoice::Required)?.get("generation_config"))?.get("tool_choice")
        )?,
        &json!("any")
    );
    assert_eq!(
        required(required(bare(IrToolChoice::None)?.get("generation_config"))?.get("tool_choice"))?,
        &json!("none")
    );

    let empty = rendered(
        &IrRequest {
            model: "public".to_string(),
            messages: vec![user_text("u1")],
            ..IrRequest::default()
        },
        "upstream-model",
        false,
    )?;

    assert!(empty.get("generation_config").is_none());
    assert!(empty.get("tools").is_none());

    Ok(())
}

/// Verifies the response-format rendering: free-form text renders no member, JSON-constrained
/// output rides the text member with the JSON MIME type, and a schema rides beside it with its
/// OpenAI-style name dropped.
#[test]
fn renders_response_format_members() -> Result<(), Box<dyn StdError>> {
    let with_format = |response_format: Option<IrResponseFormat>| {
        let ir = IrRequest {
            model: "public".to_string(),
            messages: vec![user_text("u1")],
            response_format,
            ..IrRequest::default()
        };

        rendered(&ir, "upstream-model", false)
    };

    assert!(
        with_format(None)?.get("response_format").is_none()
            && with_format(Some(IrResponseFormat::Text))?
                .get("response_format")
                .is_none()
    );

    assert_eq!(
        required(with_format(Some(IrResponseFormat::Json))?.get("response_format"))?,
        &json!({ "type": "text", "mime_type": "application/json" })
    );
    assert_eq!(
        required(
            with_format(Some(IrResponseFormat::JsonSchema {
                name: "dropped".to_string(),
                schema: json!({ "type": "object" }),
            }))?
            .get("response_format")
        )?,
        &json!({
            "type": "text",
            "mime_type": "application/json",
            "schema": { "type": "object" },
        })
    );

    Ok(())
}

/// Verifies image rendering: a URL image rides `uri`, an inline image rides `data` beside its
/// MIME type.
#[test]
fn renders_images() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "public".to_string(),
        messages: vec![IrMessage {
            role: IrRole::User,
            parts: vec![
                IrPart::Image {
                    image: IrImage::Url("https://example.com/x.png".to_string()),
                },
                IrPart::Image {
                    image: IrImage::Inline {
                        mime_type: "image/png".to_string(),
                        data_base64: "AA".to_string(),
                    },
                },
            ],
        }],
        ..IrRequest::default()
    };

    let payload = rendered(&ir, "upstream-model", false)?;

    assert_eq!(
        required(
            required(payload.get("input"))?
                .get(0)
                .and_then(|step| step.get("content"))
        )?,
        &json!([
            { "type": "image", "uri": "https://example.com/x.png" },
            { "type": "image", "data": "AA", "mime_type": "image/png" },
        ])
    );

    Ok(())
}

/// Verifies the render rejections: opaque compaction state from another surface and non-object
/// tool-call arguments each fail the render.
#[test]
fn rejects_foreign_state_and_bad_arguments() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        model: "public".to_string(),
        state_items: vec![StateItem {
            offset: 0,
            item: json!({ "opaque": true }),
            is_gateway: false,
            is_compaction: true,
            family: StateFamily::Responses,
        }],
        ..IrRequest::default()
    };

    assert!(super::render_request_body(&ir, "upstream-model", false).is_err());

    let ir = IrRequest {
        model: "public".to_string(),
        messages: vec![IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::ToolCall {
                call_id: "c1".to_string(),
                name: "f".to_string(),
                arguments: "not json".to_string(),
                thought_signature: None,
            }],
        }],
        ..IrRequest::default()
    };

    assert!(super::render_request_body(&ir, "upstream-model", false).is_err());

    Ok(())
}

/// Verifies the complete-body parse: output, thought, and function-call steps deliver at once,
/// request-echo and future step kinds are ignored, and non-text output blocks drop.
#[test]
fn parses_body_steps() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();
    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "id": "i9",
        "model": "m9",
        "status": "completed",
        "steps": [
            {
                "type": "model_output",
                "content": [
                    { "type": "text", "text": "hi" },
                    { "type": "image", "uri": "https://x" },
                ],
            },
            {
                "type": "thought",
                "id": "s1",
                "summary": [{ "type": "text", "text": "th" }],
                "signature": "sig",
            },
            { "type": "function_call", "id": "c1", "name": "f", "arguments": { "a": 1 } },
            { "type": "user_input", "content": [] },
            { "type": "function_result", "call_id": "c1", "result": [] },
            { "type": "server_tool_call", "x": 1 },
        ],
        "usage": {
            "total_input_tokens": 10,
            "total_output_tokens": 20,
            "total_thought_tokens": 5,
            "total_cached_tokens": 3,
            "total_tokens": 99,
            "total_tool_use_tokens": 7,
        },
    }))?)?;

    assert_eq!(start_of(&events)?, ("i9".to_string(), "m9".to_string()));
    assert_eq!(text_deltas(&events), vec!["hi"]);
    assert_eq!(reasoning_deltas(&events), Vec::<&str>::new());
    assert_eq!(
        events.get(2),
        Some(&IrEvent::ReasoningDone {
            text: "th".to_string(),
            signature: Some("sig".to_string()),
            opaque: json!({
                "type": "thought",
                "id": "s1",
                "summary": [{ "type": "text", "text": "th" }],
                "signature": "sig",
            }),
        })
    );

    let call_delta = required(events.get(3))?;

    match call_delta {
        IrEvent::ToolCallDelta {
            index,
            call_id,
            name,
            arguments_delta,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(call_id.as_deref(), Some("c1"));
            assert_eq!(name.as_deref(), Some("f"));
            assert_eq!(arguments_delta, r#"{"a":1}"#);
        }

        other => return Err(format!("expected ToolCallDelta, got {other:?}").into()),
    }
    assert_eq!(finish_of(&events)?, IrFinishReason::Stop);
    assert_eq!(
        usage_of(&events)?,
        IrUsage {
            input_tokens: 10,
            output_tokens: 25,
            cached_tokens: Some(3),
            cache_write_tokens: None,
            reasoning_tokens: Some(5),
        }
    );

    Ok(())
}

/// Verifies the status mapping: the terminal statuses map onto the canonical finishes,
/// `cancelled` degrades to a clean stop, the deprecated budget status maps to length, `failed`
/// fails the payload, and an absent status degrades to a stop.
#[test]
fn maps_statuses() -> Result<(), Box<dyn StdError>> {
    for (status, expected) in [
        ("completed", IrFinishReason::Stop),
        ("requires_action", IrFinishReason::ToolCall),
        ("incomplete", IrFinishReason::Length),
        ("budget_exceeded", IrFinishReason::Length),
        ("cancelled", IrFinishReason::Stop),
        ("in_progress", IrFinishReason::Stop),
        ("queued", IrFinishReason::Stop),
        ("some_future_status", IrFinishReason::Stop),
    ] {
        let mut parser = UpstreamParser::new();
        let body = serde_json::to_vec(&json!({ "status": status }))?;

        assert_eq!(
            finish_of(&parser.parse_json_response(&body)?)?,
            expected,
            "status {status}"
        );
    }

    let mut parser = UpstreamParser::new();

    assert!(
        parser
            .parse_json_response(&serde_json::to_vec(&json!({ "status": "failed" }))?)
            .is_err()
    );

    let mut parser = UpstreamParser::new();
    let body = serde_json::to_vec(&json!({}))?;

    assert_eq!(
        finish_of(&parser.parse_json_response(&body)?)?,
        IrFinishReason::Stop
    );

    Ok(())
}

/// Verifies the streamed step lifecycle: `step.start` records, deltas deliver under the stable
/// call index, `step.stop` assembles the closing events — the thought stop synthesizing one
/// summary block per delta into the opaque replay payload.
#[test]
fn parses_streamed_step_lifecycle() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&event_frame(
        "interaction.created",
        json!({ "interaction": { "id": "i1", "model": "m1" } }),
    ))?;

    assert_eq!(start_of(&events)?, ("i1".to_string(), "m1".to_string()));

    let events = parser.ingest_frame(&event_frame(
        "step.start",
        json!({ "index": 2, "step": { "type": "function_call", "id": "c1", "name": "f" } }),
    ))?;

    assert!(events.is_empty(), "a call start records without delivering");

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 2, "delta": { "type": "arguments_delta", "arguments": "{\"a\":" } }),
    ))?;

    assert_eq!(
        events,
        vec![IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("c1".to_string()),
            name: Some("f".to_string()),
            arguments_delta: "{\"a\":".to_string(),
        }]
    );

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 2, "delta": { "type": "arguments_delta", "arguments": "1}" } }),
    ))?;

    assert_eq!(
        events.first(),
        Some(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("c1".to_string()),
            name: Some("f".to_string()),
            arguments_delta: "1}".to_string(),
        })
    );

    let events = parser.ingest_frame(&event_frame("step.stop", json!({ "index": 2 })))?;

    assert_eq!(
        events,
        vec![IrEvent::ToolCallDone {
            index: 0,
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: "{\"a\":1}".to_string(),
            thought_signature: None,
        }]
    );

    parser.ingest_frame(&event_frame(
        "step.start",
        json!({ "index": 3, "step": { "type": "thought" } }),
    ))?;

    for text in ["s1", "s2"] {
        let events = parser.ingest_frame(&event_frame(
            "step.delta",
            json!({
                "index": 3,
                "delta": { "type": "thought_summary", "content": { "type": "text", "text": text } },
            }),
        ))?;

        assert_eq!(reasoning_deltas(&events), vec![text]);
    }

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({
            "index": 3,
            "delta": { "type": "thought_signature", "signature": "sig" },
        }),
    ))?;

    assert!(events.is_empty(), "the signature rides the closing done");

    let events = parser.ingest_frame(&event_frame("step.stop", json!({ "index": 3 })))?;

    assert_eq!(
        events,
        vec![IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("sig".to_string()),
            opaque: json!({
                "type": "thought",
                "summary": [
                    { "type": "text", "text": "s1" },
                    { "type": "text", "text": "s2" },
                ],
                "signature": "sig",
            }),
        }]
    );

    let events = parser.ingest_frame(&event_frame(
        "step.start",
        json!({ "index": 0, "step": { "type": "model_output" } }),
    ))?;

    assert!(events.is_empty());

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 0, "delta": { "type": "text", "text": "a" } }),
    ))?;

    assert_eq!(text_deltas(&events), vec!["a"]);

    let events = parser.ingest_frame(&event_frame("step.stop", json!({ "index": 0 })))?;

    assert!(events.is_empty(), "an output stop closes silently");

    Ok(())
}

/// Verifies the implicit-open and contradiction rules: a content delta without a start opens the
/// matching kind, a non-text summary block and a payload-less signature delta open nothing, a
/// delta contradicting the open step kind drops, and a repeated start never consumes an index.
#[test]
fn implicitly_opens_and_drops_contradictions() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&event_frame(
        "interaction.created",
        json!({ "interaction": {} }),
    ))?;

    assert_eq!(start_of(&events)?, (String::new(), String::new()));

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 7, "delta": { "type": "text", "text": "no start" } }),
    ))?;

    assert_eq!(text_deltas(&events), vec!["no start"]);

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({
            "index": 8,
            "delta": { "type": "thought_summary", "content": { "type": "image", "uri": "x" } },
        }),
    ))?;

    assert!(events.is_empty(), "a non-text summary block is a no-op");

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 8, "delta": { "type": "text", "text": "opened output" } }),
    ))?;

    assert_eq!(text_deltas(&events), vec!["opened output"]);

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 9, "delta": { "type": "thought_signature" } }),
    ))?;

    assert!(
        events.is_empty(),
        "a payload-less signature delta opens nothing"
    );

    parser.ingest_frame(&event_frame(
        "step.start",
        json!({ "index": 10, "step": { "type": "thought" } }),
    ))?;

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 10, "delta": { "type": "text", "text": "contradiction" } }),
    ))?;

    assert!(events.is_empty(), "text on a thought step drops");

    for _ in 0..2 {
        parser.ingest_frame(&event_frame(
            "step.start",
            json!({ "index": 11, "step": { "type": "function_call", "id": "c9", "name": "f" } }),
        ))?;
    }

    let events = parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 11, "delta": { "type": "arguments_delta", "arguments": "{}" } }),
    ))?;

    assert_eq!(
        events.len(),
        1,
        "a repeated start never mints a second call"
    );

    Ok(())
}

/// Verifies frame discrimination: the `event:` line wins, the payload's `event_type` member
/// substitutes, status updates and unknown events are heartbeats, and malformed or type-invalid
/// payloads fail.
#[test]
fn discriminates_frames_by_line_and_member() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let delta = json!({ "index": 0, "delta": { "type": "text", "text": "a" } });

    let by_line = parser.ingest_frame(&event_frame("step.delta", delta.clone()))?;
    let by_member = parser.ingest_frame(&typed_frame("step.delta", delta))?;

    assert_eq!(text_deltas(&by_line), vec!["a"]);
    assert_eq!(text_deltas(&by_member), vec!["a"]);

    let ignored = parser.ingest_frame(&event_frame(
        "interaction.status_update",
        json!({ "interaction": { "status": "in_progress" } }),
    ))?;

    assert!(ignored.is_empty());

    let ignored =
        parser.ingest_frame(&event_frame("brand.new.event", json!({ "surprise": true })))?;

    assert!(ignored.is_empty());

    let heartbeat = parser.ingest_frame(&SseFrame {
        data: json!({ "no": "discriminator" }).to_string(),
        event: None,
    })?;

    assert!(heartbeat.is_empty());

    let malformed = parser.ingest_frame(&SseFrame {
        data: "{".to_string(),
        event: Some("step.delta".to_string()),
    });

    assert!(malformed.is_err());

    let type_invalid = parser.ingest_frame(&SseFrame {
        data: json!({ "event_type": 42 }).to_string(),
        event: None,
    });

    assert!(type_invalid.is_err());

    Ok(())
}

/// Verifies the error veto: any frame bearing a failure member aborts whatever its event name,
/// and a `failed` terminal status surfaces as an upstream failure.
#[test]
fn vetoes_embedded_errors() -> Result<(), Box<dyn StdError>> {
    for frame in [
        event_frame(
            "interaction.created",
            json!({ "error": { "message": "boom" } }),
        ),
        typed_frame("step.delta", json!({ "error": { "message": "boom" } })),
        event_frame(
            "interaction.completed",
            json!({
                "interaction": { "status": "failed" },
            }),
        ),
    ] {
        let mut parser = UpstreamParser::new();
        let failure = parser
            .ingest_frame(&frame)
            .err()
            .ok_or("an embedded failure must abort")?;

        assert!(failure.to_string().contains("boom") || failure.to_string().contains("failed"));
    }

    Ok(())
}

/// Verifies the terminal-frame recovery: steps the live events never delivered are recovered
/// exactly once from `interaction.completed`, text recovery answers to the text-delivery record,
/// call recovery to the call-delivery record alone.
#[test]
fn recovers_undelivered_steps_from_completed_frame() -> Result<(), Box<dyn StdError>> {
    let completed = json!({
        "interaction": {
            "id": "i2",
            "model": "m2",
            "status": "completed",
            "steps": [
                { "type": "model_output", "content": [{ "type": "text", "text": "t" }] },
                {
                    "type": "thought",
                    "summary": [{ "type": "text", "text": "s" }],
                    "signature": "sg",
                },
                { "type": "function_call", "id": "c1", "name": "f", "arguments": { "a": 1 } },
            ],
        },
    });

    let mut parser = UpstreamParser::new();
    let events = parser.ingest_frame(&event_frame("interaction.completed", completed.clone()))?;

    assert_eq!(text_deltas(&events), vec!["t"]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, IrEvent::ReasoningDone { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, IrEvent::ToolCallDone { .. }))
            .count(),
        1
    );
    assert_eq!(finish_of(&events)?, IrFinishReason::Stop);

    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 0, "delta": { "type": "text", "text": "live" } }),
    ))?;

    let events = parser.ingest_frame(&event_frame("interaction.completed", completed.clone()))?;

    assert_eq!(
        text_deltas(&events),
        Vec::<&str>::new(),
        "delivered text silences the terminal text and thought copies"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, IrEvent::ToolCallDone { .. }))
            .count(),
        1,
        "delivered text never silences an undelivered call"
    );

    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&event_frame(
        "step.start",
        json!({ "index": 5, "step": { "type": "function_call", "id": "c1", "name": "f" } }),
    ))?;
    parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 5, "delta": { "type": "arguments_delta", "arguments": "{}" } }),
    ))?;
    parser.ingest_frame(&event_frame("step.stop", json!({ "index": 5 })))?;

    let events = parser.ingest_frame(&event_frame("interaction.completed", completed.clone()))?;

    assert_eq!(text_deltas(&events), Vec::<&str>::new());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, IrEvent::ToolCallDone { .. })),
        "a call delivered with payload bytes silences every terminal copy"
    );

    // A zero-argument call delivered live carries no payload bytes: the call-delivery record
    // alone silences the terminal call copy, never the undelivered text.
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&event_frame(
        "step.start",
        json!({ "index": 5, "step": { "type": "function_call", "id": "c1", "name": "f" } }),
    ))?;
    parser.ingest_frame(&event_frame("step.stop", json!({ "index": 5 })))?;

    let events = parser.ingest_frame(&event_frame("interaction.completed", completed))?;

    assert_eq!(text_deltas(&events), vec!["t"]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, IrEvent::ReasoningDone { .. })),
        "a delivered call never silences undelivered text"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, IrEvent::ToolCallDone { .. })),
        "the terminal call copy stays silenced"
    );

    Ok(())
}

/// Verifies a stream ending without `interaction.completed` degrades to a clean stop exactly
/// once.
#[test]
fn degrades_missing_completed_to_stop() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&event_frame(
        "interaction.created",
        json!({ "interaction": { "id": "i1" } }),
    ))?;
    parser.ingest_frame(&event_frame(
        "step.delta",
        json!({ "index": 0, "delta": { "type": "text", "text": "a" } }),
    ))?;

    assert_eq!(finish_of(&parser.finish())?, IrFinishReason::Stop);
    assert!(parser.finish().is_empty());

    Ok(())
}

/// Verifies the created frame: emitted exactly once, first, carrying the minted interaction id,
/// the public model, and the in-progress status.
#[test]
fn emits_created_frame_once_first() -> Result<(), Box<dyn StdError>> {
    let mut renderer = super::client_render::SseRenderer::new("pub-model".to_string());

    let frames = renderer.render(&IrEvent::MessageStart {
        id: "up".to_string(),
        model: "m".to_string(),
    });

    assert_eq!(frames.len(), 1);

    let parsed = rendered_frames(&frames)?;
    let (event, payload) = required(parsed.first())?;

    assert_eq!(event.as_deref(), Some("interaction.created"));

    let interaction = required(payload.get("interaction"))?;

    assert_eq!(required(interaction.get("model"))?, &json!("pub-model"));
    assert_eq!(required(interaction.get("status"))?, &json!("in_progress"));
    assert!(
        interaction
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("int_"))
    );

    let frames = renderer.render(&IrEvent::TextDelta("a".to_string()));

    assert_eq!(frames.len(), 2, "a delta opens its step, no second created");

    let parsed = rendered_frames(&frames)?;

    assert_eq!(
        parsed.first().and_then(|(event, _)| event.as_deref()),
        Some("step.start")
    );

    Ok(())
}

/// Verifies the streamed step events: output and thought steps open once and delta under their
/// minted indexes, a signature-bearing done streams the signature then closes, the finish closes
/// the open output step, and the completed frame carries the terminal status.
#[test]
fn streams_output_thought_and_call_steps() -> Result<(), Box<dyn StdError>> {
    let mut renderer = super::client_render::SseRenderer::new("pub-model".to_string());
    let mut frames = Vec::new();

    frames.extend(renderer.render(&IrEvent::MessageStart {
        id: "up".to_string(),
        model: "m".to_string(),
    }));
    frames.extend(renderer.render(&IrEvent::TextDelta("a".to_string())));
    frames.extend(renderer.render(&IrEvent::TextDelta("b".to_string())));
    frames.extend(renderer.render(&IrEvent::ReasoningDelta("r".to_string())));
    frames.extend(renderer.render(&IrEvent::ReasoningDone {
        text: String::new(),
        signature: Some("sig".to_string()),
        opaque: Value::Null,
    }));
    frames.extend(renderer.render(&IrEvent::Finish {
        reason: IrFinishReason::Stop,
        prompt_block_reason: None,
    }));
    frames.extend(renderer.render(&IrEvent::Done));

    let parsed = rendered_frames(&frames)?;

    let step_event = |parsed: &[(Option<String>, Value)],
                      position: usize|
     -> Result<(Option<String>, Value), Box<dyn StdError>> {
        let (event, payload) = required(parsed.get(position))?;

        Ok((event.clone(), payload.clone()))
    };

    let (_, payload) = step_event(&parsed, 1)?;

    assert_eq!(
        payload,
        json!({
            "event_type": "step.start",
            "index": 0,
            "step": { "type": "model_output" },
        })
    );

    let (_, payload) = step_event(&parsed, 2)?;

    assert_eq!(
        payload,
        json!({ "event_type": "step.delta", "index": 0, "delta": { "type": "text", "text": "a" } })
    );

    let (_, payload) = step_event(&parsed, 3)?;

    assert_eq!(
        payload,
        json!({ "event_type": "step.delta", "index": 0, "delta": { "type": "text", "text": "b" } })
    );

    let (_, payload) = step_event(&parsed, 4)?;

    assert_eq!(
        payload,
        json!({ "event_type": "step.start", "index": 1, "step": { "type": "thought" } })
    );

    let (_, payload) = step_event(&parsed, 5)?;

    assert_eq!(
        payload,
        json!({
            "event_type": "step.delta",
            "index": 1,
            "delta": {
                "type": "thought_summary",
                "content": { "type": "text", "text": "r" },
            },
        })
    );

    let (_, payload) = step_event(&parsed, 6)?;

    assert_eq!(
        payload,
        json!({
            "event_type": "step.delta",
            "index": 1,
            "delta": { "type": "thought_signature", "signature": "sig" },
        })
    );

    let (_, payload) = step_event(&parsed, 7)?;

    assert_eq!(payload, json!({ "event_type": "step.stop", "index": 1 }));

    let (_, payload) = step_event(&parsed, 8)?;

    assert_eq!(payload, json!({ "event_type": "step.stop", "index": 0 }));

    let (event, payload) = step_event(&parsed, 9)?;

    assert_eq!(event, Some("interaction.completed".to_string()));
    assert_eq!(
        required(payload.get("interaction"))?.get("status"),
        Some(&json!("completed"))
    );
    assert!(required(payload.get("interaction"))?.get("usage").is_none());

    Ok(())
}

/// Verifies done-only reasoning: a done carrying full text opens and closes its block, a
/// signature-only done reopens the block for the replayable signature, and an empty done with no
/// open block closes nothing.
#[test]
fn renders_done_only_reasoning_and_signatures() -> Result<(), Box<dyn StdError>> {
    let mut renderer = super::client_render::SseRenderer::new("pub-model".to_string());

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: "full".to_string(),
        signature: None,
        opaque: Value::Null,
    });

    let parsed = rendered_frames(&frames)?;

    assert_eq!(parsed.len(), 4, "created, start, summary delta, stop");
    assert_eq!(
        required(required(parsed.get(2))?.1.get("delta"))?,
        &json!({
            "type": "thought_summary",
            "content": { "type": "text", "text": "full" },
        })
    );

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: String::new(),
        signature: Some("s".to_string()),
        opaque: Value::Null,
    });

    let parsed = rendered_frames(&frames)?;

    assert_eq!(parsed.len(), 3, "start, signature delta, stop");
    assert_eq!(
        required(required(parsed.get(1))?.1.get("delta"))?,
        &json!({ "type": "thought_signature", "signature": "s" })
    );

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: String::new(),
        signature: None,
        opaque: Value::Null,
    });

    assert!(frames.is_empty());

    Ok(())
}

/// Verifies tool-call streaming: deltas open the call and stream argument bytes, an empty
/// keep-alive delta renders nothing, the done reconciles with the streamed prefix, a done-only
/// call opens and completes in one go, and a signature-bearing done streams the signature-only
/// thought step ahead of the call.
#[test]
fn reconciles_tool_call_dones_with_deltas() -> Result<(), Box<dyn StdError>> {
    let mut renderer = super::client_render::SseRenderer::new("pub-model".to_string());
    let mut frames = Vec::new();

    frames.extend(renderer.render(&IrEvent::ToolCallDelta {
        index: 0,
        call_id: Some("c1".to_string()),
        name: Some("f".to_string()),
        arguments_delta: "{\"a\":".to_string(),
    }));

    let parsed = rendered_frames(&frames)?;

    assert_eq!(parsed.len(), 3, "created, start, argument delta");
    assert_eq!(
        required(parsed.get(1))?.1.get("step"),
        Some(&json!({ "type": "function_call", "id": "c1", "name": "f" }))
    );

    frames.extend(renderer.render(&IrEvent::ToolCallDelta {
        index: 0,
        call_id: None,
        name: None,
        arguments_delta: String::new(),
    }));

    assert_eq!(frames.len(), 3, "an empty keep-alive renders nothing");

    frames.extend(renderer.render(&IrEvent::ToolCallDelta {
        index: 0,
        call_id: None,
        name: None,
        arguments_delta: "1}".to_string(),
    }));

    assert_eq!(frames.len(), 4);

    frames.extend(renderer.render(&IrEvent::ToolCallDone {
        index: 0,
        call_id: "c1".to_string(),
        name: "f".to_string(),
        arguments: "{\"a\":1}".to_string(),
        thought_signature: None,
    }));

    assert_eq!(
        frames.len(),
        5,
        "the deltas delivered everything, only a stop"
    );

    frames.extend(renderer.render(&IrEvent::ToolCallDone {
        index: 1,
        call_id: "c2".to_string(),
        name: "g".to_string(),
        arguments: "{\"b\":2}".to_string(),
        thought_signature: None,
    }));

    let parsed = rendered_frames(&frames)?;
    let (event, payload) = required(parsed.get(5))?;

    assert_eq!(event.as_deref(), Some("step.start"));
    assert_eq!(
        required(payload.get("step"))?,
        &json!({ "type": "function_call", "id": "c2", "name": "g" })
    );
    assert_eq!(
        required(required(parsed.get(6))?.1.get("delta"))?,
        &json!({ "type": "arguments_delta", "arguments": "{\"b\":2}" })
    );
    assert_eq!(
        required(parsed.get(7))?.1,
        json!({ "event_type": "step.stop", "index": 1 })
    );

    frames.extend(renderer.render(&IrEvent::ToolCallDone {
        index: 2,
        call_id: "c3".to_string(),
        name: "h".to_string(),
        arguments: "{}".to_string(),
        thought_signature: Some("sig".to_string()),
    }));

    let parsed = rendered_frames(&frames)?;

    assert_eq!(
        required(parsed.get(8))?.1.get("step"),
        Some(&json!({ "type": "thought" })),
        "the signature rides its own thought step"
    );
    assert_eq!(
        required(required(parsed.get(9))?.1.get("delta"))?,
        &json!({ "type": "thought_signature", "signature": "sig" })
    );
    assert_eq!(
        required(required(parsed.get(11))?.1.get("step"))?,
        &json!({ "type": "function_call", "id": "c3", "name": "h" })
    );
    assert_eq!(
        required(required(parsed.get(12))?.1.get("delta"))?,
        &json!({ "type": "arguments_delta", "arguments": "{}" })
    );

    Ok(())
}

/// Verifies stream completion: the terminal frame closes any still-open steps and carries the
/// mapped status and usage.
#[test]
fn completes_stream_with_status_and_usage() -> Result<(), Box<dyn StdError>> {
    let mut renderer = super::client_render::SseRenderer::new("pub-model".to_string());
    let mut frames = Vec::new();

    frames.extend(renderer.render(&IrEvent::ReasoningDelta("t".to_string())));

    let created = frames.remove(0);

    assert!(created.starts_with("event: interaction.created"));

    frames.extend(renderer.render(&IrEvent::ToolCallDone {
        index: 0,
        call_id: "c1".to_string(),
        name: "f".to_string(),
        arguments: "{}".to_string(),
        thought_signature: None,
    }));
    frames.extend(renderer.render(&IrEvent::Finish {
        reason: IrFinishReason::ToolCall,
        prompt_block_reason: None,
    }));
    frames.extend(renderer.render(&IrEvent::Usage(usage_fixture())));
    frames.extend(renderer.render(&IrEvent::Done));

    let parsed = rendered_frames(&frames)?;
    let stops: Vec<&Value> = parsed
        .iter()
        .filter(|(event, _)| event.as_deref() == Some("step.stop"))
        .map(|(_, payload)| payload)
        .collect();

    assert_eq!(stops.len(), 2, "the open thought and call steps close");

    let (_, completed) = required(parsed.last())?;

    let interaction = required(completed.get("interaction"))?;

    assert_eq!(
        required(interaction.get("status"))?,
        &json!("requires_action")
    );
    assert_eq!(
        required(interaction.get("usage"))?,
        &json!({
            "total_input_tokens": 10,
            "total_output_tokens": 20,
            "total_tokens": 35,
            "total_thought_tokens": 5,
            "total_cached_tokens": 3,
        })
    );
    assert!(
        interaction
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("int_"))
    );

    Ok(())
}

/// Verifies the aggregated Interaction resource: thought steps replay verbatim or synthesize,
/// steps render thoughts-then-output-then-calls, a signature-bearing call synthesizes its
/// signature-only thought step, and the usage splits reasoning back out.
#[test]
fn renders_aggregated_resource() -> Result<(), Box<dyn StdError>> {
    let verbatim = json!({
        "type": "thought",
        "summary": [{ "type": "text", "text": "think" }],
        "signature": "s",
    });

    let aggregation = IrAggregation {
        text: "out".to_string(),
        reasoning_blocks: vec![IrReasoningBlock {
            text: "think".to_string(),
            signature: Some("s".to_string()),
            opaque: verbatim.clone(),
        }],
        tool_calls: vec![crate::ir::IrAggregatedToolCall {
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: Some("cs".to_string()),
        }],
        finish: Some(IrFinishReason::ToolCall),
        usage: Some(usage_fixture()),
        ..IrAggregation::default()
    };

    let payload = serde_json::to_value(super::client_render::render_json_response(
        "pub-model",
        &aggregation,
    ))?;

    assert_eq!(
        required(payload.get("steps"))?,
        &json!([
            verbatim,
            { "type": "model_output", "content": [{ "type": "text", "text": "out" }] },
            { "type": "function_call", "id": "c1", "name": "f", "arguments": { "a": 1 } },
            { "type": "thought", "signature": "cs" },
        ])
    );
    assert_eq!(required(payload.get("status"))?, &json!("requires_action"));
    assert_eq!(
        required(payload.get("usage"))?,
        &json!({
            "total_input_tokens": 10,
            "total_output_tokens": 20,
            "total_tokens": 35,
            "total_thought_tokens": 5,
            "total_cached_tokens": 3,
        })
    );
    assert!(
        required(payload.get("id"))?
            .as_str()
            .is_some_and(|id| id.starts_with("int_"))
    );

    let bare = IrAggregation::default();
    let payload = serde_json::to_value(super::client_render::render_json_response(
        "pub-model",
        &bare,
    ))?;

    assert_eq!(required(payload.get("status"))?, &json!("completed"));
    assert!(payload.get("usage").is_none());
    assert!(
        required(payload.get("steps"))?
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    Ok(())
}

/// Verifies the finish-to-status vocabulary: stop completes, tool calls require action, and a
/// length or content-filter cutoff reports incomplete.
#[test]
fn maps_finish_status_vocabulary() -> Result<(), Box<dyn StdError>> {
    for (finish, expected) in [
        (None, "completed"),
        (Some(IrFinishReason::Stop), "completed"),
        (Some(IrFinishReason::ToolCall), "requires_action"),
        (Some(IrFinishReason::Length), "incomplete"),
        (Some(IrFinishReason::ContentFilter), "incomplete"),
    ] {
        let aggregation = IrAggregation {
            finish,
            ..IrAggregation::default()
        };

        let payload = serde_json::to_value(super::client_render::render_json_response(
            "pub-model",
            &aggregation,
        ))?;

        assert_eq!(
            required(payload.get("status"))?,
            &json!(expected),
            "finish {finish:?}"
        );
    }

    Ok(())
}

/// Verifies the JSON affinity projection fingerprints identically to the streamed render-point
/// projection of the same completion: both transports of one response are one history.
#[test]
fn aggregated_and_streamed_projections_fingerprint_identically() -> Result<(), Box<dyn StdError>> {
    let events = streamed_events();

    let mut streamed = super::client_render::SseRenderer::new("pub-model".to_string());

    streamed.begin_projection(crate::affinity::fingerprint::Projector::new());

    for event in &events {
        streamed.render(event);
    }

    let streamed_projector = required(streamed.take_projection())?;

    let response = super::client_render::render_json_response("pub-model", &aggregated(&events));
    let mut aggregated_projector = crate::affinity::fingerprint::Projector::new();

    assert!(json_affinity::project_json_response(
        &response,
        &mut aggregated_projector
    ));

    let streamed_identity = streamed_projector
        .evidence()
        .retry
        .map(|retry| retry.digest);
    let aggregated_identity = aggregated_projector
        .evidence()
        .retry
        .map(|retry| retry.digest);

    assert!(
        streamed_identity == aggregated_identity,
        "both transports of one response fingerprint one history"
    );

    Ok(())
}

/// Verifies anchor withholding: a stored thought step whose summary shape contradicts the
/// accepted replay vocabulary is unreplayable, while a well-formed step projects.
#[test]
fn withholds_anchor_for_malformed_verbatim_thought() -> Result<(), Box<dyn StdError>> {
    let project = |opaque: Value| {
        let aggregation = IrAggregation {
            reasoning_blocks: vec![IrReasoningBlock {
                text: "t".to_string(),
                signature: None,
                opaque,
            }],
            ..IrAggregation::default()
        };

        let response = super::client_render::render_json_response("pub-model", &aggregation);
        let mut projector = crate::affinity::fingerprint::Projector::new();

        json_affinity::project_json_response(&response, &mut projector)
    };

    assert!(!project(
        json!({ "type": "thought", "summary": "not-blocks" })
    ));
    assert!(project(json!({ "type": "thought", "summary": [] })));

    Ok(())
}

/// Verifies the projection digests the verbatim payload's annotation fragments: two steps
/// differing only inside opaque annotations fingerprint differently.
#[test]
fn digests_ride_verbatim_annotations() -> Result<(), Box<dyn StdError>> {
    let identity = |annotations: Value| {
        let aggregation = IrAggregation {
            reasoning_blocks: vec![IrReasoningBlock {
                text: String::new(),
                signature: None,
                opaque: json!({
                    "type": "thought",
                    "summary": [{ "type": "text", "text": "t", "annotations": annotations }],
                }),
            }],
            ..IrAggregation::default()
        };

        let response = super::client_render::render_json_response("pub-model", &aggregation);
        let mut projector = crate::affinity::fingerprint::Projector::new();

        assert!(json_affinity::project_json_response(
            &response,
            &mut projector
        ));

        projector.evidence().retry.map(|retry| retry.digest)
    };

    assert!(
        identity(json!([{ "title": "a" }])) != identity(json!([{ "title": "b" }])),
        "opaque annotation fragments ride the digest"
    );

    Ok(())
}
