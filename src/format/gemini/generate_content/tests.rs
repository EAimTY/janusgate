//! Tests for the Gemini GenerateContent codec: the client request parser, the upstream request
//! renderer, the upstream response parser, the client response renderers, and the aggregated
//! body's affinity projection.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{UpstreamParser, client_render, json_affinity, request_parse};
use crate::{
    affinity::fingerprint::{Projector, Role},
    api::Error,
    format::{ClientSseRenderer as _, ParsedRequest, RouteContext, UpstreamEventParser as _},
    ir::{
        Aggregator, InvalidRequest, IrAggregatedToolCall, IrAggregation, IrEvent, IrFinishReason,
        IrImage, IrMessage, IrPart, IrReasoningBlock, IrRequest, IrResponseFormat, IrRole, IrTool,
        IrToolChoice, IrUsage, StateFamily, StateItem,
    },
    sse::SseFrame,
    vocabulary::ReasoningLevel,
};
use serde_json::{Value, json};
use std::error::Error as StdError;

/// The request boundary's parse outcome.
type Parsed = Result<ParsedRequest, InvalidRequest>;

/// A route context addressing `public_id`, with the URL-owned streaming decision when the URL
/// action carries one.
fn route(public_id: &str, is_streaming: Option<bool>) -> RouteContext {
    RouteContext::with_model(public_id.to_string(), is_streaming)
}

/// Serializes the JSON fixture body and parses it as a GenerateContent request.
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

/// A bare upstream `data:` frame carrying the JSON payload.
fn data_frame(payload: Value) -> SseFrame {
    SseFrame {
        data: payload.to_string(),
        event: None,
    }
}

/// Renders an IR request into its upstream JSON shape.
fn rendered(ir: &IrRequest) -> Result<Value, Box<dyn StdError>> {
    let request = super::render_generate_content_request(ir)?;

    Ok(serde_json::to_value(&request)?)
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

/// The stream's single opening identity, failing the test when absent, not unique, or not first.
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

/// A minimal valid body: one user text turn.
fn minimal_body() -> Value {
    json!({
        "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
    })
}

/// Verifies a fully specified request parses through its camelCase wire spellings into every IR
/// control: instructions, tools, tool choice, sampling, thinking level, and response format.
#[test]
fn parses_full_request_via_camel_case_aliases() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "systemInstruction": { "parts": [{ "text": "be terse" }] },
        "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
        "tools": [{ "functionDeclarations": [{
            "name": "lookup",
            "description": "finds things",
            "parameters": { "type": "object" },
        }] }],
        "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
        "generationConfig": {
            "temperature": 0.5,
            "topP": 0.9,
            "maxOutputTokens": 256,
            "thinkingConfig": { "thinkingLevel": "high" },
            "responseMimeType": "application/json",
            "responseSchema": { "type": "object" },
        },
    });

    let request = parsed(body, &route("gemini-3-pro", Some(true)))?;

    assert_eq!(request.ir.model, "gemini-3-pro");
    assert!(request.is_streaming);
    assert!(!request.should_include_usage);
    assert_eq!(request.ir.instructions.as_deref(), Some("be terse"));
    assert_eq!(request.ir.temperature, Some(0.5));
    assert_eq!(request.ir.top_p, Some(0.9));
    assert_eq!(request.ir.max_output_tokens, Some(256));
    assert_eq!(request.ir.reasoning_effort, Some(ReasoningLevel::High));
    assert_eq!(
        request.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "response".to_string(),
            schema: json!({ "type": "object" }),
        })
    );
    assert_eq!(request.ir.tool_choice, Some(IrToolChoice::Auto));

    let tool = required(request.ir.tools.first())?;

    assert_eq!(tool.name, "lookup");
    assert_eq!(tool.description.as_deref(), Some("finds things"));
    assert_eq!(tool.parameters.as_ref(), Some(&json!({ "type": "object" })));

    Ok(())
}

/// Verifies the body carries no `model` and no `stream` member: both fail as unknown fields,
/// because the model and the streaming mode ride the URL alone.
#[test]
fn rejects_body_model_and_stream_members() -> Result<(), Box<dyn StdError>> {
    for member in ["model", "stream"] {
        let mut body = minimal_body();

        let object = body
            .as_object_mut()
            .ok_or("the fixture body is an object")?;

        object.insert(member.to_string(), json!("whatever"));

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert!(error.param.is_none());
        assert!(
            error.to_string().contains(member),
            "the rejection must name `{member}`: {error}"
        );
    }

    Ok(())
}

/// Verifies a request whose URL carries no model rejects naming `model`.
#[test]
fn rejects_route_without_public_model() -> Result<(), Box<dyn StdError>> {
    let mut route = route("pub", None);

    route.public_id = None;

    let error = rejection(parse(minimal_body(), &route)?)?;

    assert_eq!(error.param.as_deref(), Some("model"));

    Ok(())
}

/// Verifies every known-but-unsupported top-level control rejects naming its own field.
#[test]
fn rejects_unsupported_top_level_controls() -> Result<(), Box<dyn StdError>> {
    for member in [
        "stop",
        "stopSequences",
        "seed",
        "speechConfig",
        "audioConfig",
        "videoConfig",
    ] {
        let mut body = minimal_body();

        let object = body
            .as_object_mut()
            .ok_or("the fixture body is an object")?;

        object.insert(member.to_string(), json!(1));

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert_eq!(
            error.param.as_deref(),
            Some(member),
            "the rejection must name `{member}`"
        );
    }

    Ok(())
}

/// Verifies every known-but-unsupported `generationConfig` control rejects naming its own field,
/// including the numeric thinking budget in both of its slots.
#[test]
fn rejects_unsupported_generation_config_controls() -> Result<(), Box<dyn StdError>> {
    for member in [
        "topK",
        "stop",
        "stopSequences",
        "seed",
        "speechConfig",
        "audioConfig",
        "videoConfig",
    ] {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": { member: 1 },
        });

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert_eq!(
            error.param.as_deref(),
            Some(member),
            "the rejection must name `{member}`"
        );
    }

    for slot in ["thinkingBudget", "thinkingConfig"] {
        let inner = match slot {
            "thinkingBudget" => json!({ "thinkingLevel": "high", "thinkingBudget": 1024 }),

            _ => json!({
                "thinkingLevel": "high",
                "thinkingConfig": { "thinkingLevel": "low", "thinkingBudget": 1024 },
            }),
        };
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": inner,
        });

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert_eq!(error.param.as_deref(), Some("thinkingBudget"));
    }

    Ok(())
}

/// Verifies empty contents and a foreign content role both reject naming `contents`.
#[test]
fn rejects_empty_contents_and_unknown_roles() -> Result<(), Box<dyn StdError>> {
    let error = rejection(parse(json!({ "contents": [] }), &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("contents"));

    let body = json!({
        "contents": [
            { "role": "user", "parts": [{ "text": "hi" }] },
            { "role": "tool", "parts": [{ "text": "nope" }] },
        ],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("contents"));
    assert!(error.to_string().contains("tool"));

    Ok(())
}

/// Verifies a part must carry exactly one payload member and that a `thoughtSignature` is valid
/// only beside a `functionCall`.
#[test]
fn enforces_single_payload_parts_and_signature_placement() -> Result<(), Box<dyn StdError>> {
    for parts in [
        json!([{}]),
        json!([{ "text": "a", "inlineData": { "mimeType": "image/png", "data": "AA" } }]),
        json!([{ "thought": true }]),
    ] {
        let body = json!({ "contents": [{ "role": "user", "parts": parts }] });

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert_eq!(error.param.as_deref(), Some("contents"));
    }

    let body = json!({
        "contents": [{ "role": "model", "parts": [{ "text": "a", "thoughtSignature": "s" }] }],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("thoughtSignature"));

    Ok(())
}

/// Verifies role placement: `functionCall` belongs to `model` contents alone and
/// `functionResponse` to `user` contents alone.
#[test]
fn rejects_misplaced_call_and_response_parts() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [
            { "role": "user", "parts": [{ "functionCall": { "name": "f" } }] },
        ],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("contents"));

    let body = json!({
        "contents": [
            { "role": "model", "parts": [{ "functionResponse": { "name": "f", "response": {} } }] },
        ],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("contents"));

    Ok(())
}

/// Verifies a user turn parses into the tool message its results produce followed by the user
/// message carrying its media, with data URLs decoded and result strings riding verbatim.
#[test]
fn parses_user_media_and_tool_results() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": "look" },
                { "inlineData": { "mimeType": "image/png", "data": "AAEC" } },
                { "fileData": { "fileUri": "https://example.com/x.png", "mimeType": "image/png" } },
                { "fileData": { "fileUri": "data:image/jpeg;base64,Zm9v" } },
                { "functionResponse": { "name": "f", "response": "the result", "id": "c1" } },
            ],
        }],
    });

    let request = parsed(body, &route("pub", None))?;

    assert_eq!(request.ir.messages.len(), 2);

    let tool = required(request.ir.messages.first())?;

    assert_eq!(tool.role, IrRole::Tool);

    let output = match required(tool.parts.first())? {
        IrPart::ToolOutput {
            call_id,
            content,
            is_error,
        } => {
            assert_eq!(call_id, "c1");
            assert_eq!(content, "the result");
            assert!(!*is_error);

            true
        }

        _ => false,
    };
    assert!(output, "the tool message carries its output");

    let user = required(request.ir.messages.last())?;

    assert_eq!(user.role, IrRole::User);
    assert_eq!(user.parts.len(), 4);
    assert_eq!(
        user.parts.first(),
        Some(&IrPart::Text {
            text: "look".to_string(),
        })
    );

    let images: Vec<&IrImage> = user
        .parts
        .iter()
        .filter_map(|part| match part {
            IrPart::Image { image } => Some(image),

            _ => None,
        })
        .collect();

    assert_eq!(
        images,
        vec![
            &IrImage::Inline {
                mime_type: "image/png".to_string(),
                data_base64: "AAEC".to_string(),
            },
            &IrImage::Url("https://example.com/x.png".to_string()),
            &IrImage::Inline {
                mime_type: "image/jpeg".to_string(),
                data_base64: "Zm9v".to_string(),
            },
        ]
    );

    Ok(())
}

/// Verifies a `fileData` URL of an unsupported scheme rejects naming `fileData`.
#[test]
fn rejects_file_data_with_unsupported_scheme() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [{
            "role": "user",
            "parts": [{ "fileData": { "fileUri": "ftp://example.com/x.png" } }],
        }],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("fileData"));

    Ok(())
}

/// Verifies media inside a `functionResponse.parts` member rejects naming `parts`, the IR tool
/// output having no media channel.
#[test]
fn rejects_media_in_function_response() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [{
            "role": "user",
            "parts": [{
                "functionResponse": { "name": "f", "response": {}, "parts": [] },
            }],
        }],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("parts"));

    Ok(())
}

/// Verifies an id-less `functionResponse` pairs with the most recent unmatched call of the same
/// name, in order, and an exhausted registry mints a fresh id.
#[test]
fn pairs_idless_responses_with_latest_unmatched_call() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [
            { "role": "model", "parts": [
                { "functionCall": { "name": "f" } },
                { "functionCall": { "name": "f" } },
            ] },
            { "role": "user", "parts": [
                { "functionResponse": { "name": "f", "response": "first" } },
                { "functionResponse": { "name": "f", "response": "second" } },
                { "functionResponse": { "name": "f", "response": "third" } },
            ] },
        ],
    });

    let request = parsed(body, &route("pub", None))?;

    let assistant = required(request.ir.messages.first())?;

    let call_ids: Vec<String> = assistant
        .parts
        .iter()
        .filter_map(|part| match part {
            IrPart::ToolCall { call_id, .. } => Some(call_id.clone()),

            _ => None,
        })
        .collect();

    let [first_call, second_call] = call_ids.as_slice() else {
        return Err("expected exactly two parsed calls".into());
    };

    let outputs: Vec<(&str, &str)> = request
        .ir
        .messages
        .iter()
        .filter_map(|message| match required(message.parts.first()).ok()? {
            IrPart::ToolOutput {
                call_id, content, ..
            } => Some((call_id.as_str(), content.as_str())),

            _ => None,
        })
        .collect();

    let paired: Vec<(&str, &str)> = outputs.iter().take(2).copied().collect();

    assert_eq!(
        paired,
        vec![
            (second_call.as_str(), "first"),
            (first_call.as_str(), "second"),
        ]
    );

    let orphan = required(outputs.get(2))?.0;

    assert!(orphan.starts_with("call_"));
    assert_ne!(orphan, first_call.as_str());
    assert_ne!(orphan, second_call.as_str());

    Ok(())
}

/// Verifies `functionCall` argument shapes: absent args render `{}`, an object rides its JSON
/// text, an explicit id is kept, and a non-object args rejects naming `functionCall`.
#[test]
fn parses_function_call_argument_shapes() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [
            { "role": "model", "parts": [
                { "functionCall": { "name": "no_args" } },
                { "functionCall": { "name": "with_args", "args": { "a": 1 }, "id": "c9" } },
            ] },
        ],
    });

    let request = parsed(body, &route("pub", None))?;

    let assistant = required(request.ir.messages.first())?;

    let calls: Vec<(&str, &str, Option<&str>)> = assistant
        .parts
        .iter()
        .filter_map(|part| match part {
            IrPart::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => Some((name.as_str(), arguments.as_str(), Some(call_id.as_str()))),

            _ => None,
        })
        .collect();

    assert_eq!(calls.len(), 2);

    let [without_args, with_args] = calls.as_slice() else {
        return Err("expected exactly two parsed calls".into());
    };

    assert_eq!(without_args.1, "{}");
    assert!(without_args.2.is_some_and(|id| id.starts_with("call_")));
    assert_eq!(with_args.0, "with_args");
    assert_eq!(with_args.1, r#"{"a":1}"#);
    assert_eq!(with_args.2, Some("c9"));

    let body = json!({
        "contents": [
            { "role": "model", "parts": [{ "functionCall": { "name": "f", "args": [1] } }] },
        ],
    });

    let error = rejection(parse(body, &route("pub", None))?)?;

    assert_eq!(error.param.as_deref(), Some("functionCall"));

    Ok(())
}

/// Verifies system instruction parts join with the shared blank-line separator, a leading run of
/// empty parts carries none, and an all-empty instruction renders no instruction at all.
#[test]
fn flattens_system_instruction_parts() -> Result<(), Box<dyn StdError>> {
    let instruction = |parts: Value| {
        let body = json!({
            "system_instruction": { "parts": parts },
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
        });

        parsed(body, &route("pub", None))
    };

    let request = instruction(json!([{ "text": "a" }, { "text": "b" }]))?;

    assert_eq!(request.ir.instructions.as_deref(), Some("a\n\nb"));

    let request = instruction(json!([{ "text": "" }, { "text": "" }, { "text": "x" }]))?;

    assert_eq!(request.ir.instructions.as_deref(), Some("x"));

    let request = instruction(json!([{ "text": "" }]))?;

    assert_eq!(request.ir.instructions, None);

    Ok(())
}

/// Verifies the `toolConfig` mode table: `AUTO` and `NONE` forbid allowed names, `ANY` maps by
/// allowed-name count, unknown modes reject, and a config without tools rejects.
#[test]
fn parses_tool_config_modes() -> Result<(), Box<dyn StdError>> {
    let tool = json!({ "functionDeclarations": [{ "name": "f" }] });
    let configured = |tool_config: Value, tools: Value| {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "tools": tools,
            "toolConfig": tool_config,
        });

        parse(body, &route("pub", None))
    };

    for (mode, expected) in [
        ("AUTO", Some(IrToolChoice::Auto)),
        ("NONE", Some(IrToolChoice::None)),
        ("ANY", Some(IrToolChoice::Required)),
    ] {
        let request = parsed(
            json!({
                "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
                "tools": [tool.clone()],
                "toolConfig": { "functionCallingConfig": { "mode": mode } },
            }),
            &route("pub", None),
        )?;

        assert_eq!(request.ir.tool_choice, expected, "mode {mode}");
    }

    let request = parsed(
        json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "tools": [tool],
            "toolConfig": {
                "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["f"] },
            },
        }),
        &route("pub", None),
    )?;

    assert_eq!(
        request.ir.tool_choice,
        Some(IrToolChoice::Function {
            name: "f".to_string(),
        })
    );

    for config in [
        json!({ "functionCallingConfig": { "mode": "AUTO", "allowedFunctionNames": ["f"] } }),
        json!({ "functionCallingConfig": { "mode": "NONE", "allowedFunctionNames": ["f"] } }),
        json!({
            "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["a", "b"] },
        }),
        json!({ "functionCallingConfig": { "mode": "WAT" } }),
    ] {
        let error = rejection(configured(
            config.clone(),
            json!([{ "functionDeclarations": [{ "name": "f" }] }]),
        )?)?;

        assert_eq!(
            error.param.as_deref(),
            Some("toolConfig"),
            "config {config}"
        );
    }

    let error = rejection(configured(
        json!({ "functionCallingConfig": { "mode": "AUTO" } }),
        json!([]),
    )?)?;

    assert_eq!(error.param.as_deref(), Some("toolConfig"));

    Ok(())
}

/// Verifies tool declarations: a non-function tool group, an empty declaration list, and an
/// unnamed declaration each reject naming `tools`.
#[test]
fn rejects_invalid_tool_declarations() -> Result<(), Box<dyn StdError>> {
    for tools in [
        json!([{}]),
        json!([{ "functionDeclarations": [] }]),
        json!([{ "functionDeclarations": [{ "name": "" }] }]),
    ] {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "tools": tools,
        });

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert_eq!(error.param.as_deref(), Some("tools"), "tools {tools}");
    }

    Ok(())
}

/// Verifies out-of-range sampling rejects naming its parameter while in-range values parse.
#[test]
fn validates_sampling_ranges() -> Result<(), Box<dyn StdError>> {
    for (member, value, param) in [("temperature", 2.5, "temperature"), ("topP", 1.5, "topP")] {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": { member: value },
        });

        let error = rejection(parse(body, &route("pub", None))?)?;

        assert_eq!(
            error.param.as_deref(),
            Some(param),
            "the rejection must name `{param}`"
        );
    }

    let request = parsed(
        json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": { "temperature": 2.0, "topP": 0.0 },
        }),
        &route("pub", None),
    )?;

    assert_eq!(request.ir.temperature, Some(2.0));
    assert_eq!(request.ir.top_p, Some(0.0));

    Ok(())
}

/// Verifies the two `thinkingLevel` slots: either parses, both at once reject, and an unknown
/// level rejects naming `thinkingLevel`.
#[test]
fn parses_thinking_level_slots() -> Result<(), Box<dyn StdError>> {
    let level = |generation_config: Value| {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": generation_config,
        });

        parse(body, &route("pub", None))
    };

    let request = parsed(
        json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": { "thinkingLevel": "xhigh" },
        }),
        &route("pub", None),
    )?;
    assert_eq!(request.ir.reasoning_effort, Some(ReasoningLevel::XHigh));

    let request = level(json!({ "thinkingConfig": { "thinkingLevel": "none" } }))??;
    assert_eq!(request.ir.reasoning_effort, Some(ReasoningLevel::None));

    let error = rejection(level(json!({
        "thinkingLevel": "high",
        "thinkingConfig": { "thinkingLevel": "low" },
    }))?)?;

    assert_eq!(error.param.as_deref(), Some("thinkingLevel"));

    let error = rejection(level(json!({ "thinkingLevel": "bogus" }))?)?;

    assert_eq!(error.param.as_deref(), Some("thinkingLevel"));

    Ok(())
}

/// Verifies the response-format decision table: absent members mean no format, `text/plain` is
/// free-form text, `application/json` is JSON or named-schema, and every other cell rejects
/// naming the member that caused it.
#[test]
fn parses_response_format_table() -> Result<(), Box<dyn StdError>> {
    let config = |generation_config: Value| {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": generation_config,
        });

        parse(body, &route("pub", None))
    };

    let request = parsed(
        json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": {},
        }),
        &route("pub", None),
    )?;
    assert_eq!(request.ir.response_format, None);

    let request = config(json!({ "responseMimeType": "text/plain" }))??;
    assert_eq!(request.ir.response_format, Some(IrResponseFormat::Text));

    let request = config(json!({ "responseMimeType": "application/json" }))??;
    assert_eq!(request.ir.response_format, Some(IrResponseFormat::Json));

    let request = config(json!({
        "responseMimeType": "application/json",
        "responseSchema": { "type": "string" },
    }))??;
    assert_eq!(
        request.ir.response_format,
        Some(IrResponseFormat::JsonSchema {
            name: "response".to_string(),
            schema: json!({ "type": "string" }),
        })
    );

    for (config_value, param) in [
        (
            json!({ "responseSchema": { "type": "string" } }),
            "responseMimeType",
        ),
        (
            json!({ "responseMimeType": "text/plain", "responseSchema": {} }),
            "responseSchema",
        ),
        (
            json!({ "responseMimeType": "text/html" }),
            "responseMimeType",
        ),
    ] {
        let error = rejection(config(config_value.clone())?)?;

        assert_eq!(error.param.as_deref(), Some(param), "config {config_value}");
    }

    Ok(())
}

/// Verifies the replay companions the gateway itself emits parse back: the `thought` flag on
/// text is plain text, a signature rides a `functionCall`, and the ignored `functionResponse`
/// companions (`scheduling`, `willContinue`) parse.
#[test]
fn accepts_gateway_replay_companions() -> Result<(), Box<dyn StdError>> {
    let body = json!({
        "contents": [
            { "role": "model", "parts": [
                { "text": "reasoning", "thought": true },
                { "functionCall": { "name": "f", "id": "c1" }, "thoughtSignature": "sig" },
            ] },
            { "role": "user", "parts": [{
                "functionResponse": {
                    "name": "f",
                    "response": { "ok": true },
                    "id": "c1",
                    "scheduling": "WHEN_IDLE",
                    "willContinue": false,
                },
            }] },
        ],
    });

    let request = parsed(body, &route("pub", None))?;

    let assistant = required(request.ir.messages.first())?;

    assert_eq!(
        assistant.parts.first(),
        Some(&IrPart::Text {
            text: "reasoning".to_string(),
        })
    );

    assert_eq!(
        assistant.parts.get(1),
        Some(&IrPart::ToolCall {
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("sig".to_string()),
        })
    );

    Ok(())
}

/// Verifies role runs coalesce: consecutive same-role messages, and tool messages beside user
/// messages, merge into one content entry, preserving part order.
#[test]
fn coalesces_role_runs_in_contents() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![
            user_text("u1"),
            user_text("u2"),
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::Text {
                        text: "m1".to_string(),
                    },
                    IrPart::ToolCall {
                        call_id: "c1".to_string(),
                        name: "f1".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    },
                    IrPart::ToolCall {
                        call_id: "c2".to_string(),
                        name: "f2".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    },
                ],
            },
            tool_output("c1", r#"{"ok":true}"#),
            tool_output("c2", "plain"),
            user_text("u3"),
        ],
        ..IrRequest::default()
    };

    let payload = rendered(&ir)?;
    let contents = required(payload.get("contents"))?.as_array().cloned();

    let contents = required(contents)?;
    let roles: Vec<&str> = contents
        .iter()
        .filter_map(|content| content.get("role").and_then(Value::as_str))
        .collect();

    assert_eq!(roles, vec!["user", "model", "user"]);

    let user_parts = required(
        required(contents.first())?
            .get("parts")
            .and_then(Value::as_array),
    )?;

    assert_eq!(
        user_parts,
        &[json!({ "text": "u1" }), json!({ "text": "u2" })]
    );

    let model_parts = required(
        required(contents.get(1))?
            .get("parts")
            .and_then(Value::as_array),
    )?;

    assert_eq!(
        model_parts,
        &[
            json!({ "text": "m1" }),
            json!({ "functionCall": { "name": "f1", "args": {}, "id": "c1" } }),
            json!({ "functionCall": { "name": "f2", "args": {}, "id": "c2" } }),
        ][..]
    );

    let tool_parts = required(
        required(contents.get(2))?
            .get("parts")
            .and_then(Value::as_array),
    )?;

    assert_eq!(
        tool_parts,
        &[
            json!({ "functionResponse": { "name": "f1", "response": { "ok": true }, "id": "c1" } }),
            json!({
                "functionResponse": { "name": "f2", "response": { "result": "plain" }, "id": "c2" },
            }),
            json!({ "text": "u3" }),
        ][..]
    );

    Ok(())
}

/// Verifies the envelope members: system instructions render as one text part, tools render as a
/// single declaration group, and the tool-choice table maps onto the `functionCallingConfig`
/// modes.
#[test]
fn renders_envelope_tools_and_tool_config() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        instructions: Some("be terse".to_string()),
        messages: vec![user_text("u1")],
        tools: vec![IrTool {
            name: "f".to_string(),
            description: None,
            parameters: None,
        }],
        tool_choice: Some(IrToolChoice::Function {
            name: "f".to_string(),
        }),
        ..IrRequest::default()
    };

    let payload = rendered(&ir)?;

    assert_eq!(
        required(payload.get("systemInstruction"))?,
        &json!({ "parts": [{ "text": "be terse" }] })
    );
    assert_eq!(
        required(payload.get("tools"))?,
        &json!([{ "functionDeclarations": [{ "name": "f" }] }])
    );
    assert_eq!(
        required(payload.get("toolConfig"))?,
        &json!({
            "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["f"] },
        })
    );

    let bare_mode = |choice: IrToolChoice| {
        let ir = IrRequest {
            messages: vec![user_text("u1")],
            tools: vec![IrTool {
                name: "f".to_string(),
                description: None,
                parameters: None,
            }],
            tool_choice: Some(choice),
            ..IrRequest::default()
        };

        rendered(&ir)
    };

    assert_eq!(
        required(bare_mode(IrToolChoice::Auto)?.get("toolConfig"))?,
        &json!({ "functionCallingConfig": { "mode": "AUTO" } })
    );
    assert_eq!(
        required(bare_mode(IrToolChoice::Required)?.get("toolConfig"))?,
        &json!({ "functionCallingConfig": { "mode": "ANY" } })
    );
    assert_eq!(
        required(bare_mode(IrToolChoice::None)?.get("toolConfig"))?,
        &json!({ "functionCallingConfig": { "mode": "NONE" } })
    );

    let payload = rendered(&IrRequest {
        messages: vec![user_text("u1")],
        ..IrRequest::default()
    })?;

    assert!(payload.get("tools").is_none());
    assert!(payload.get("toolConfig").is_none());
    assert!(payload.get("systemInstruction").is_none());

    Ok(())
}

/// Verifies the generation-config rendering: sampling and the output cap ride their members, the
/// reasoning level rides `thinkingConfig.thinkingLevel` verbatim, the response formats map onto
/// `responseMimeType` (plus `responseSchema`), and an all-absent config is omitted.
#[test]
fn renders_generation_config_and_response_format() -> Result<(), Box<dyn StdError>> {
    let render_with = |mutate: fn(&mut IrRequest)| {
        let mut ir = IrRequest {
            messages: vec![user_text("u1")],
            ..IrRequest::default()
        };

        mutate(&mut ir);

        rendered(&ir)
    };

    let payload = render_with(|ir| {
        ir.temperature = Some(0.5);
        ir.top_p = Some(0.9);
        ir.max_output_tokens = Some(256);
        ir.reasoning_effort = Some(ReasoningLevel::XHigh);
        ir.response_format = Some(IrResponseFormat::JsonSchema {
            name: "irrelevant".to_string(),
            schema: json!({ "type": "object" }),
        });
    })?;

    assert_eq!(
        required(payload.get("generationConfig"))?,
        &json!({
            "temperature": 0.5,
            "topP": 0.9,
            "maxOutputTokens": 256,
            "responseMimeType": "application/json",
            "responseSchema": { "type": "object" },
            "thinkingConfig": { "thinkingLevel": "xhigh" },
        })
    );

    let payload = render_with(|ir| {
        ir.response_format = Some(IrResponseFormat::Json);
    })?;

    assert_eq!(
        required(required(payload.get("generationConfig"))?.get("responseMimeType"))?,
        &json!("application/json")
    );
    assert!(
        required(payload.get("generationConfig"))?
            .get("responseSchema")
            .is_none()
    );

    let payload = render_with(|ir| {
        ir.response_format = Some(IrResponseFormat::Text);
    })?;

    assert!(payload.get("generationConfig").is_none());

    Ok(())
}

/// Verifies part rendering: text, inline and URL images, and tool calls with their parsed
/// argument objects, ids, and signatures.
#[test]
fn renders_parts() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![
                    IrPart::Text {
                        text: "see".to_string(),
                    },
                    IrPart::Image {
                        image: IrImage::Inline {
                            mime_type: "image/png".to_string(),
                            data_base64: "AA".to_string(),
                        },
                    },
                    IrPart::Image {
                        image: IrImage::Url("https://example.com/x.png".to_string()),
                    },
                ],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::ToolCall {
                    call_id: "c1".to_string(),
                    name: "f".to_string(),
                    arguments: r#"{"a":1}"#.to_string(),
                    thought_signature: Some("sig".to_string()),
                }],
            },
        ],
        ..IrRequest::default()
    };

    let payload = rendered(&ir)?;
    let contents = required(payload.get("contents"))?.as_array().cloned();

    let contents = required(contents)?;

    assert_eq!(
        required(required(contents.first())?.get("parts"))?,
        &json!([
            { "text": "see" },
            { "inlineData": { "mimeType": "image/png", "data": "AA" } },
            { "fileData": { "fileUri": "https://example.com/x.png" } },
        ])
    );
    assert_eq!(
        required(required(contents.get(1))?.get("parts"))?,
        &json!([{
            "functionCall": { "name": "f", "args": { "a": 1 }, "id": "c1" },
            "thoughtSignature": "sig",
        }])
    );

    Ok(())
}

/// Verifies the tool-result payload shapes: an object forwards verbatim, a non-object JSON value
/// wraps under `result`, and non-JSON text wraps as text; an unresolvable call id rejects.
#[test]
fn renders_function_response_payload_shapes() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![
                    IrPart::ToolCall {
                        call_id: "c1".to_string(),
                        name: "f1".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    },
                    IrPart::ToolCall {
                        call_id: "c2".to_string(),
                        name: "f2".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    },
                    IrPart::ToolCall {
                        call_id: "c3".to_string(),
                        name: "f3".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    },
                ],
            },
            IrMessage {
                role: IrRole::Tool,
                parts: vec![
                    IrPart::ToolOutput {
                        call_id: "c1".to_string(),
                        content: r#"{"a":1}"#.to_string(),
                        is_error: false,
                    },
                    IrPart::ToolOutput {
                        call_id: "c2".to_string(),
                        content: "\"plain\"".to_string(),
                        is_error: false,
                    },
                    IrPart::ToolOutput {
                        call_id: "c3".to_string(),
                        content: "plain text".to_string(),
                        is_error: false,
                    },
                ],
            },
        ],
        ..IrRequest::default()
    };

    let payload = rendered(&ir)?;

    assert_eq!(
        required(required(required(payload.get("contents"))?.get(1))?.get("parts"))?,
        &json!([
            { "functionResponse": { "name": "f1", "response": { "a": 1 }, "id": "c1" } },
            { "functionResponse": { "name": "f2", "response": { "result": "plain" }, "id": "c2" } },
            {
                "functionResponse": {
                    "name": "f3",
                    "response": { "result": "plain text" },
                    "id": "c3",
                },
            },
        ])
    );

    let orphan = IrRequest {
        messages: vec![tool_output("unknown", "{}")],
        ..IrRequest::default()
    };

    assert!(super::render_generate_content_request(&orphan).is_err());

    Ok(())
}

/// Verifies the same-family reasoning bridge: a Gemini thought step renders as the thinking part
/// with its signature, a signature-only block omits the text member, and foreign or text-only
/// reasoning renders nothing.
#[test]
fn bridges_thought_steps_and_drops_foreign_reasoning() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![reasoning(
            "thinking",
            Some("s1"),
            json!({
                "type": "thought",
                "summary": [{ "type": "text", "text": "thinking" }],
                "signature": "s1",
            }),
        )],
        ..IrRequest::default()
    };

    let payload = rendered(&ir)?;

    assert_eq!(
        required(required(required(payload.get("contents"))?.get(0))?.get("parts"))?,
        &json!([{
            "text": "thinking",
            "thought": true,
            "thoughtSignature": "s1",
        }])
    );

    let signature_only = IrRequest {
        messages: vec![reasoning(
            "",
            Some("s2"),
            json!({
                "type": "thought",
                "signature": "s2",
            }),
        )],
        ..IrRequest::default()
    };

    let payload = rendered(&signature_only)?;

    assert_eq!(
        required(required(required(payload.get("contents"))?.get(0))?.get("parts"))?,
        &json!([{ "thought": true, "thoughtSignature": "s2" }])
    );

    for opaque in [
        Value::Null,
        json!({ "type": "redacted_thinking", "data": "x" }),
    ] {
        let foreign = IrRequest {
            messages: vec![reasoning("text", None, opaque)],
            ..IrRequest::default()
        };

        let payload = rendered(&foreign)?;
        let contents = required(payload.get("contents"))?;

        assert_eq!(contents, &json!([]), "foreign reasoning drops");
    }

    Ok(())
}

/// Verifies a request carrying opaque compaction state rejects: the Gemini upstream has no state
/// channel, so foreign native state is never silently discarded.
#[test]
fn rejects_rendering_foreign_compaction_state() {
    let ir = IrRequest {
        state_items: vec![StateItem {
            offset: 0,
            item: json!({ "opaque": true }),
            is_gateway: false,
            is_compaction: true,
            family: StateFamily::Responses,
        }],
        ..IrRequest::default()
    };

    assert!(super::render_generate_content_request(&ir).is_err());
}

/// Verifies the upstream response parser: text, thought, and function-call parts map to their IR
/// events, with the signature channels carried and missing call ids minted.
#[test]
fn parses_streamed_parts_into_events() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    let events = parser.ingest_frame(&data_frame(json!({
        "candidates": [{
            "content": { "parts": [
                { "text": "ans" },
                { "text": "hmm", "thought": true },
                { "text": "sig carried", "thought": true, "thoughtSignature": "ts" },
            ] },
        }],
    })))?;

    assert_eq!(text_deltas(&events), vec!["ans"]);
    assert_eq!(reasoning_deltas(&events), vec!["hmm", "sig carried"]);
    assert_eq!(
        events.last(),
        Some(&IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("ts".to_string()),
            opaque: Value::Null,
        })
    );

    let events = parser.ingest_frame(&data_frame(json!({
        "candidates": [{
            "content": { "parts": [
                {
                    "functionCall": { "name": "f", "args": { "a": 1 }, "id": "c9" },
                    "thoughtSignature": "cs",
                },
                { "functionCall": { "name": "g" } },
            ] },
            "finishReason": "STOP",
        }],
    })))?;

    assert_eq!(
        events.first(),
        Some(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("c9".to_string()),
            name: Some("f".to_string()),
            arguments_delta: r#"{"a":1}"#.to_string(),
        })
    );

    let done = required(events.get(1))?;

    match done {
        IrEvent::ToolCallDone {
            index,
            call_id,
            name,
            arguments,
            thought_signature,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(call_id, "c9");
            assert_eq!(name, "f");
            assert_eq!(arguments, r#"{"a":1}"#);
            assert_eq!(thought_signature.as_deref(), Some("cs"));
        }

        other => return Err(format!("expected ToolCallDone, got {other:?}").into()),
    }

    let second = required(events.get(2))?;

    match second {
        IrEvent::ToolCallDelta {
            index,
            call_id,
            arguments_delta,
            ..
        } => {
            assert_eq!(*index, 1);
            assert!(call_id.as_deref().is_some_and(|id| id.starts_with("call_")));
            assert_eq!(arguments_delta, "{}");
        }

        other => return Err(format!("expected ToolCallDelta, got {other:?}").into()),
    }

    assert_eq!(finish_of(&events)?, IrFinishReason::ToolCall);
    assert!(parser.finish().is_empty(), "a real finish never degrades");

    Ok(())
}

/// Verifies the finish-reason mapping: the canonical reasons map directly, the safety vocabulary
/// maps to content filter, the invalid-generation reasons fail the payload, and a plain `STOP`
/// after delivered calls is a tool-call finish.
#[test]
fn maps_finish_reasons() -> Result<(), Box<dyn StdError>> {
    for (reason, expected) in [
        ("STOP", IrFinishReason::Stop),
        ("MAX_TOKENS", IrFinishReason::Length),
        ("SAFETY", IrFinishReason::ContentFilter),
        ("LANGUAGE", IrFinishReason::ContentFilter),
        ("BLOCKLIST", IrFinishReason::ContentFilter),
        ("PROHIBITED_CONTENT", IrFinishReason::ContentFilter),
        ("SPII", IrFinishReason::ContentFilter),
        ("RECITATION", IrFinishReason::ContentFilter),
        ("IMAGE_SAFETY", IrFinishReason::ContentFilter),
        ("IMAGE_PROHIBITED_CONTENT", IrFinishReason::ContentFilter),
        ("IMAGE_RECITATION", IrFinishReason::ContentFilter),
    ] {
        let mut parser = UpstreamParser::new();
        let body = serde_json::to_vec(&json!({ "candidates": [{ "finishReason": reason }] }))?;

        assert_eq!(
            finish_of(&parser.parse_json_response(&body)?)?,
            expected,
            "reason {reason}"
        );
    }

    for reason in [
        "MALFORMED_FUNCTION_CALL",
        "UNEXPECTED_TOOL_CALL",
        "TOO_MANY_TOOL_CALLS",
    ] {
        let mut parser = UpstreamParser::new();
        let body = serde_json::to_vec(&json!({ "candidates": [{ "finishReason": reason }] }))?;

        assert!(
            parser.parse_json_response(&body).is_err(),
            "reason {reason} must fail the payload"
        );
    }

    Ok(())
}

/// Verifies a prompt-level block: the verbatim block reason rides a content-filter finish when no
/// candidate finished, while a candidate finish wins over `promptFeedback`.
#[test]
fn parses_prompt_block_and_prefers_candidate_finish() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();
    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "candidates": [{ "finishReason": "STOP" }],
        "promptFeedback": { "blockReason": "OTHER" },
    }))?)?;

    assert_eq!(finish_of(&events)?, IrFinishReason::Stop);

    let mut parser = UpstreamParser::new();
    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "promptFeedback": { "blockReason": "PROHIBITED_CONTENT" },
    }))?)?;

    let Some(IrEvent::Finish {
        reason,
        prompt_block_reason,
    }) = events.last()
    else {
        return Err("expected the block finish".into());
    };

    assert_eq!(reason, &IrFinishReason::ContentFilter);
    assert_eq!(prompt_block_reason.as_deref(), Some("PROHIBITED_CONTENT"));
    assert!(parser.finish().is_empty());

    Ok(())
}

/// Verifies the usage fold: the output total includes reasoning, the breakdowns ride beside it,
/// and no cache-write accounting is invented.
#[test]
fn folds_usage_metadata() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();
    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 20,
            "thoughtsTokenCount": 5,
            "cachedContentTokenCount": 3,
        },
    }))?)?;

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

/// Verifies the Antigravity envelope: the wrapped response is ingested, an embedded error is
/// vetoed wherever it rides, and the opened stream carries the upstream identity.
#[test]
fn unwraps_envelope_and_vetoes_embedded_errors() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();
    let events = parser.parse_json_response(&serde_json::to_vec(&json!({
        "response": {
            "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }],
            "responseId": "r9",
            "modelVersion": "m9",
        },
    }))?)?;

    assert_eq!(start_of(&events)?, ("r9".to_string(), "m9".to_string()));
    assert_eq!(text_deltas(&events), vec!["hi"]);
    assert_eq!(finish_of(&parser.finish())?, IrFinishReason::Stop);

    for (body, detail) in [
        (json!({ "error": { "message": "outer" } }), "outer"),
        (
            json!({ "response": { "error": { "message": "inner" } } }),
            "inner",
        ),
    ] {
        let mut parser = UpstreamParser::new();
        let raw = serde_json::to_vec(&body)?;
        let failure = parser
            .parse_json_response(&raw)
            .err()
            .ok_or("an embedded error must fail the payload")?;

        assert!(failure.to_string().contains(detail), "detail {detail}");
    }

    Ok(())
}

/// Verifies the primary-candidate selection: the index-0 candidate wins, and an index-less array
/// falls back to the first entry.
#[test]
fn selects_primary_candidate() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();
    let events = parser.ingest_frame(&data_frame(json!({
        "candidates": [
            { "index": 1, "content": { "parts": [{ "text": "secondary" }] } },
            { "index": 0, "content": { "parts": [{ "text": "primary" }] } },
        ],
    })))?;

    assert_eq!(text_deltas(&events), vec!["primary"]);

    let mut parser = UpstreamParser::new();
    let events = parser.ingest_frame(&data_frame(json!({
        "candidates": [{ "content": { "parts": [{ "text": "first" }] } }],
    })))?;

    assert_eq!(text_deltas(&events), vec!["first"]);

    Ok(())
}

/// Verifies the opening identity: emitted once with the upstream's response id and model, never a
/// second time, and minted when the upstream omits one.
#[test]
fn emits_message_start_once_with_upstream_identity() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();
    let events = parser.ingest_frame(&data_frame(json!({
        "responseId": "r1", "modelVersion": "m1",
    })))?;

    assert_eq!(start_of(&events)?, ("r1".to_string(), "m1".to_string()));

    let events = parser.ingest_frame(&data_frame(json!({ "responseId": "r2" })))?;

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, IrEvent::MessageStart { .. }))
    );

    let mut parser = UpstreamParser::new();
    let (id, _) = start_of(&parser.ingest_frame(&data_frame(json!({})))?)?;

    assert!(id.starts_with("gen-"));

    Ok(())
}

/// Verifies a stream ending without a finish degrades to a clean stop exactly once, and a
/// malformed frame fails the payload.
#[test]
fn degrades_missing_finish_and_rejects_malformed_frames() -> Result<(), Box<dyn StdError>> {
    let mut parser = UpstreamParser::new();

    parser.ingest_frame(&data_frame(json!({})))?;

    assert_eq!(finish_of(&parser.finish())?, IrFinishReason::Stop);
    assert!(parser.finish().is_empty());

    let mut parser = UpstreamParser::new();

    assert!(
        parser
            .ingest_frame(&SseFrame {
                data: "{".to_string(),
                event: None,
            })
            .is_err()
    );

    Ok(())
}

/// Verifies the aggregated non-streaming body: reasoning renders as the thought-flagged part,
/// the usage split subtracts reasoning from the candidates member, the official total sums the
/// rendered members, and a tool-call turn still reports `STOP`.
#[test]
fn renders_aggregated_json_response() -> Result<(), Box<dyn StdError>> {
    let aggregation = IrAggregation {
        text: "answer".to_string(),
        reasoning: "thoughts".to_string(),
        reasoning_blocks: vec![IrReasoningBlock {
            text: "thoughts".to_string(),
            signature: None,
            opaque: Value::Null,
        }],
        tool_calls: vec![IrAggregatedToolCall {
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: Some("sig".to_string()),
        }],
        finish: Some(IrFinishReason::ToolCall),
        usage: Some(usage_fixture()),
        ..IrAggregation::default()
    };

    let payload = serde_json::to_value(client_render::render_json_response(
        "pub-model",
        &aggregation,
    ))?;

    assert_eq!(
        required(payload.get("candidates"))?,
        &json!([{
            "content": {
                "role": "model",
                "parts": [
                    { "text": "thoughts", "thought": true },
                    { "text": "answer" },
                    {
                        "functionCall": { "name": "f", "args": { "a": 1 }, "id": "c1" },
                        "thoughtSignature": "sig",
                    },
                ],
            },
            "finishReason": "STOP",
            "index": 0,
        }])
    );
    assert_eq!(
        required(payload.get("usageMetadata"))?,
        &json!({
            "promptTokenCount": 10,
            "candidatesTokenCount": 20,
            "totalTokenCount": 35,
            "thoughtsTokenCount": 5,
            "cachedContentTokenCount": 3,
        })
    );

    let response_id = required(payload.get("responseId"))?
        .as_str()
        .unwrap_or_default();

    assert!(response_id.starts_with("gen-"));
    assert_eq!(required(payload.get("modelVersion"))?, &json!("pub-model"));

    Ok(())
}

/// Verifies the finish-reason vocabulary of the aggregated body: tool-call turns still end with
/// `STOP`, the token limit is `MAX_TOKENS`, and a content filter is `SAFETY`.
#[test]
fn maps_finish_reasons_to_gemini_vocabulary() -> Result<(), Box<dyn StdError>> {
    for (finish, expected) in [
        (IrFinishReason::Stop, "STOP"),
        (IrFinishReason::ToolCall, "STOP"),
        (IrFinishReason::Length, "MAX_TOKENS"),
        (IrFinishReason::ContentFilter, "SAFETY"),
    ] {
        let aggregation = IrAggregation {
            finish: Some(finish),
            ..IrAggregation::default()
        };

        let payload = serde_json::to_value(client_render::render_json_response(
            "pub-model",
            &aggregation,
        ))?;

        assert_eq!(
            required(required(payload.get("candidates"))?.get(0))?.get("finishReason"),
            Some(&json!(expected)),
            "finish {finish:?}"
        );
    }

    Ok(())
}

/// Verifies the prompt-block shape: no candidates at all, the verbatim block reason under
/// `promptFeedback`, and zero-filled usage.
#[test]
fn renders_prompt_block_shape() -> Result<(), Box<dyn StdError>> {
    let aggregation = IrAggregation {
        finish: Some(IrFinishReason::ContentFilter),
        prompt_block_reason: Some("PROHIBITED_CONTENT".to_string()),
        ..IrAggregation::default()
    };

    let payload = serde_json::to_value(client_render::render_json_response(
        "pub-model",
        &aggregation,
    ))?;

    assert!(payload.get("candidates").is_none());
    assert_eq!(
        required(payload.get("promptFeedback"))?,
        &json!({ "blockReason": "PROHIBITED_CONTENT" })
    );
    assert_eq!(
        required(payload.get("usageMetadata"))?,
        &json!({
            "promptTokenCount": 0,
            "candidatesTokenCount": 0,
            "totalTokenCount": 0,
        })
    );

    Ok(())
}

/// Verifies the streaming chunks: text and thought deltas render per-event chunks, done-only
/// blocks render once, buffered tool calls render on their done, the finish chunk carries the
/// canonical reason, and usage rides the terminal chunk with a stable response id.
#[test]
fn streams_events_as_native_chunks() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("pub-model".to_string());
    let mut frames = Vec::new();

    let render =
        |renderer: &mut client_render::SseRenderer, frames: &mut Vec<String>, event: &IrEvent| {
            frames.extend(renderer.render(event));
        };

    render(
        &mut renderer,
        &mut frames,
        &IrEvent::MessageStart {
            id: "up".to_string(),
            model: "m".to_string(),
        },
    );
    assert!(frames.is_empty(), "MessageStart renders no chunk");

    render(&mut renderer, &mut frames, &IrEvent::TextDelta("a".into()));
    render(
        &mut renderer,
        &mut frames,
        &IrEvent::ReasoningDelta("hmm".into()),
    );
    render(
        &mut renderer,
        &mut frames,
        &IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("ts".to_string()),
            opaque: Value::Null,
        },
    );
    render(
        &mut renderer,
        &mut frames,
        &IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("c1".to_string()),
            name: Some("f".to_string()),
            arguments_delta: r#"{"a":"#.to_string(),
        },
    );

    let before_tool = frames.len();

    render(
        &mut renderer,
        &mut frames,
        &IrEvent::ToolCallDone {
            index: 0,
            call_id: "c1".to_string(),
            name: "f".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: Some("cs".to_string()),
        },
    );
    render(
        &mut renderer,
        &mut frames,
        &IrEvent::Finish {
            reason: IrFinishReason::ToolCall,
            prompt_block_reason: None,
        },
    );
    render(&mut renderer, &mut frames, &IrEvent::Usage(usage_fixture()));
    render(&mut renderer, &mut frames, &IrEvent::Done);

    let request = rendered_frames(&frames)?;
    let payloads: Vec<&Value> = request.iter().map(|(_, payload)| payload).collect();

    assert_eq!(
        required(payloads.first())?.get("candidates"),
        Some(&json!([{
            "content": { "role": "model", "parts": [{ "text": "a" }] },
            "index": 0,
        }]))
    );
    assert_eq!(
        required(payloads.get(1))?.get("candidates"),
        Some(&json!([{
            "content": { "role": "model", "parts": [{ "text": "hmm", "thought": true }] },
            "index": 0,
        }]))
    );

    // The done for the already-streamed reasoning block renders nothing, so the tool-call done
    // is the next chunk after the two deltas.
    assert_eq!(before_tool, 2);
    assert_eq!(
        required(payloads.get(2))?.get("candidates"),
        Some(&json!([{
            "content": { "role": "model", "parts": [{
                "functionCall": { "name": "f", "args": { "a": 1 }, "id": "c1" },
                "thoughtSignature": "cs",
            }] },
            "index": 0,
        }]))
    );
    assert_eq!(
        required(payloads.get(3))?.get("candidates"),
        Some(&json!([{
            "content": { "role": "model", "parts": [] },
            "finishReason": "STOP",
            "index": 0,
        }]))
    );

    let terminal = required(payloads.last())?;

    assert!(
        terminal.get("candidates").is_none(),
        "an empty candidate list skips the member"
    );
    assert_eq!(
        required(terminal.get("usageMetadata"))?,
        &json!({
            "promptTokenCount": 10,
            "candidatesTokenCount": 20,
            "totalTokenCount": 35,
            "thoughtsTokenCount": 5,
            "cachedContentTokenCount": 3,
        })
    );

    let response_ids: Vec<&str> = payloads
        .iter()
        .filter_map(|payload| payload.get("responseId").and_then(Value::as_str))
        .collect();

    assert!(response_ids.iter().all(|id| id.starts_with("gen-")));
    assert_eq!(response_ids.first(), response_ids.last());

    Ok(())
}

/// Verifies done-only reasoning blocks: a done carrying full text with no prior deltas renders
/// its thought part once, and an empty-text done without deltas renders nothing.
#[test]
fn streams_done_only_reasoning_blocks() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("pub-model".to_string());

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: "full".to_string(),
        signature: None,
        opaque: Value::Null,
    });

    assert_eq!(frames.len(), 1);

    let request = rendered_frames(&frames)?;
    let (_, payload) = required(request.first())?;

    assert_eq!(
        required(
            required(payload.get("candidates"))?
                .get(0)
                .and_then(|candidate| candidate.get("content"))
                .and_then(|content| content.get("parts"))
        )?,
        &json!([{ "text": "full", "thought": true }])
    );

    let frames = renderer.render(&IrEvent::ReasoningDone {
        text: String::new(),
        signature: None,
        opaque: Value::Null,
    });

    assert!(frames.is_empty());

    Ok(())
}

/// Verifies the streamed blocked shape: a prompt-level block renders no candidate finish chunk,
/// and the terminal chunk carries the `promptFeedback` instead — or nothing when the stream ends
/// without terminal facts.
#[test]
fn streams_prompt_block_terminal_chunk() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("pub-model".to_string());

    let frames = renderer.render(&IrEvent::Finish {
        reason: IrFinishReason::ContentFilter,
        prompt_block_reason: Some("SAFETY".to_string()),
    });

    assert!(
        frames.is_empty(),
        "the block buffers for the terminal chunk"
    );

    let frames = renderer.render(&IrEvent::Done);

    assert_eq!(frames.len(), 1);

    let request = rendered_frames(&frames)?;
    let (_, payload) = required(request.first())?;

    assert!(payload.get("candidates").is_none());
    assert!(payload.get("usageMetadata").is_none());
    assert_eq!(
        required(payload.get("promptFeedback"))?,
        &json!({ "blockReason": "SAFETY" })
    );

    let mut renderer = client_render::SseRenderer::new("pub-model".to_string());

    let frames = renderer.render(&IrEvent::Finish {
        reason: IrFinishReason::Stop,
        prompt_block_reason: None,
    });

    assert_eq!(frames.len(), 1, "a candidate finish renders its chunk");

    let frames = renderer.render(&IrEvent::Done);

    assert!(
        frames.is_empty(),
        "a stream without terminal facts renders no terminal chunk"
    );

    Ok(())
}

/// Verifies the mid-stream error frame: the shared Gemini error envelope, serialized as the one
/// bare `data:` frame that terminates the stream.
#[test]
fn renders_mid_stream_error_frame() -> Result<(), Box<dyn StdError>> {
    let mut renderer = client_render::SseRenderer::new("pub-model".to_string());

    let frame = renderer.error_frame(&Error::MalformedRequest {
        param: None,
        message: "boom".to_string(),
    });

    let request = rendered_frames(&[frame])?;
    let (event, payload) = required(request.first())?;

    assert_eq!(*event, None);
    assert_eq!(
        required(payload.get("error"))?,
        &json!({
            "code": 400,
            "message": "boom",
            "status": "INVALID_ARGUMENT",
        })
    );

    Ok(())
}

/// Verifies the affinity projection of the aggregated body fingerprints identically to the
/// streamed projection of the same completion: both transports of one response are one history.
#[test]
fn aggregated_and_streamed_projections_fingerprint_identically() -> Result<(), Box<dyn StdError>> {
    let events = vec![
        IrEvent::MessageStart {
            id: "up".to_string(),
            model: "m".to_string(),
        },
        IrEvent::ReasoningDelta("thinking".to_string()),
        IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("ts".to_string()),
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
            thought_signature: Some("cs".to_string()),
        },
        IrEvent::Finish {
            reason: IrFinishReason::ToolCall,
            prompt_block_reason: None,
        },
        IrEvent::Usage(usage_fixture()),
        IrEvent::Done,
    ];

    let mut streamed = client_render::SseRenderer::new("pub-model".to_string());

    streamed.begin_projection(Projector::new());

    for event in &events {
        streamed.render(event);
    }

    let streamed_projector = required(streamed.take_projection())?;

    let response = client_render::render_json_response("pub-model", &aggregated(&events));
    let mut aggregated_projector = Projector::new();

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

/// Verifies the projection is content-sensitive: two different answers fingerprint differently,
/// so the equality test above cannot pass vacuously.
#[test]
fn projection_digest_distinguishes_content() -> Result<(), Box<dyn StdError>> {
    let identity = |text: &str| {
        let aggregation = IrAggregation {
            text: text.to_string(),
            ..IrAggregation::default()
        };

        let response = client_render::render_json_response("pub-model", &aggregation);
        let mut projector = Projector::new();

        assert!(json_affinity::project_json_response(
            &response,
            &mut projector
        ));

        projector.evidence().retry.map(|retry| retry.digest)
    };

    assert!(
        identity("one") != identity("two"),
        "different answers fingerprint differently"
    );

    Ok(())
}

/// Verifies the aggregated projection anchors an empty answer's tool call: the seeded projector
/// completes a history whose retry identity includes the user seed text.
#[test]
fn projection_seeds_from_accepted_history() -> Result<(), Box<dyn StdError>> {
    let mut projector = Projector::new();

    projector.text(Role::User, b"prompt");

    let aggregation = IrAggregation {
        text: "answer".to_string(),
        ..IrAggregation::default()
    };

    let response = client_render::render_json_response("pub-model", &aggregation);

    assert!(json_affinity::project_json_response(
        &response,
        &mut projector
    ));

    let evidence = projector.evidence();

    assert!(evidence.retry.is_some(), "a seeded history fingerprints");

    Ok(())
}
