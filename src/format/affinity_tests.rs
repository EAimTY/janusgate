//! Accepted protocol history projection, source identities, and foreign result semantics.

use crate::{
    affinity::{Digest, Projection, fingerprint::Projector},
    format::{
        ClientFormat, ParsedRequest, RouteContext,
        anthropic::messages::Format as AnthropicMessages,
        gemini::{
            generate_content::Format as GeminiGenerateContent,
            interaction::Format as GeminiInteraction,
        },
        openai::{
            chat_completion::Format as OpenAiChatCompletion, response::Format as OpenAiResponse,
        },
    },
    ir::{InvalidRequest, IrPart},
};
use serde_json::{Value, json};

/// Parses the actual client wire, including URL routing facts for GenerateContent.
///
/// # Errors
///
/// Returns a rejection from the selected real client parser.
fn parse<F: ClientFormat>(body: Value) -> Result<ParsedRequest, InvalidRequest> {
    F::parse_request(
        body.to_string().as_bytes(),
        &RouteContext {
            public_id: Some("M".into()),
            is_streaming: Some(false),
            ..RouteContext::default()
        },
    )
}

/// Strongest full accepted history identity, with bytes hidden from failure reporting.
fn retry(projector: &Projector) -> Option<(Projection, Digest)> {
    projector
        .evidence()
        .retry
        .map(|retry| (retry.projection, retry.digest))
}

/// Five real client parsers project the same conversation independently of instructions,
/// model/control metadata, message splitting, and ignored envelope/resource identifiers.
///
/// # Errors
///
/// Returns an actual protocol parser rejection.
///
/// # Panics
///
/// Panics if presentation-only differences change canonical conversation identity.
#[test]
fn all_client_parsers_share_role_run_and_envelope_exclusion_rules() -> Result<(), InvalidRequest> {
    let chat = parse::<OpenAiChatCompletion>(json!({"model":"different-model","temperature":0.1,
        "messages":[{"role":"system","content":"instruction"},
            {"role":"user","content":"h"},{"role":"user","content":"i"},
            {"role":"assistant","content":"answer","reasoning_content":"discarded"},
            {"role":"user","content":"next"}]}))?;
    let response =
        parse::<OpenAiResponse>(json!({"model":"M","instructions":"another instruction",
        "input":[{"type":"message","role":"user","content":"hi"},
            {"type":"message","role":"assistant","id":"presentation-only", "status":"completed",
                "content":[{"type":"output_text","text":"answer"}]},
            {"type":"message","role":"user","content":"next"}]}))?;
    let generate = parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"user","parts":[{"text":"hi"}]},
        {"role":"model","parts":[{"text":"answer"}]},
        {"role":"user","parts":[{"text":"next"}]}]}))?;
    let interaction = parse::<GeminiInteraction>(json!({"model":"M","labels":{"ignored":"x"},
        "input":[{"type":"user_input","content":[{"type":"text","text":"hi"}]},
            {"type":"model_output","content":[{"type":"text","text":"answer"}]},
            {"type":"user_input","content":[{"type":"text","text":"next"}]}]}))?;
    let messages = parse::<AnthropicMessages>(json!({"model":"M","max_tokens":8,
        "messages":[{"role":"user","content":"hi"},
            {"role":"assistant","content":[{"type":"text","text":"answer"}]},
            {"role":"user","content":"next"}]}))?;
    let expected = retry(&chat.affinity.project(&chat.ir.messages));
    for request in [response, generate, interaction, messages] {
        let projected = request.affinity.project(&request.ir.messages);
        assert!(retry(&projected) == expected);
        assert_eq!(projected.evidence().history.len(), 3);
    }
    Ok(())
}

/// Generated IR IDs and the legacy `call_0` stand-in never masquerade as real client IDs.
/// Gemini's existing id-less result pairing remains the sole relationship resolver.
///
/// # Errors
///
/// Returns an actual protocol parser rejection.
///
/// # Panics
///
/// Panics if real/synthetic source information is lost or projection invents result pairing.
#[test]
fn idless_and_legacy_tool_sources_remain_distinct_from_real_wire_ids() -> Result<(), InvalidRequest>
{
    let body = json!({"contents":[{"role":"user","parts":[{"text":"question"}]},
        {"role":"model","parts":[{"functionCall":{"name":"tool","args":{}}}]},
        {"role":"user","parts":[{"functionResponse":{"name":"tool","response":"ok"}}]}]});
    let first = parse::<GeminiGenerateContent>(body.clone())?;
    let second = parse::<GeminiGenerateContent>(body)?;
    let call_id = |request: &ParsedRequest| {
        request
            .ir
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .find_map(|part| match part {
                IrPart::ToolCall { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
    };
    assert_ne!(call_id(&first), call_id(&second));
    let first_projection = first.affinity.project(&first.ir.messages);
    assert!(retry(&first_projection) == retry(&second.affinity.project(&second.ir.messages)));
    assert!(
        retry(&first_projection).is_some_and(|(strength, _)| strength == Projection::Relational)
    );
    let legacy = parse::<OpenAiChatCompletion>(json!({"model":"M","messages":[
        {"role":"assistant","function_call":{"name":"tool","arguments":"{}"}},
        {"role":"tool","tool_call_id":"call_0","content":"ok"}]}))?;
    let named = parse::<OpenAiChatCompletion>(json!({"model":"M","messages":[
        {"role":"assistant","tool_calls":[{"type":"function","id":"call_0",
            "function":{"name":"tool","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call_0","content":"ok"}]}))?;
    assert_eq!(legacy.ir.messages, named.ir.messages);
    assert!(
        retry(&legacy.affinity.project(&legacy.ir.messages))
            != retry(&named.affinity.project(&named.ir.messages))
    );
    let mixed = |is_text_first: bool| {
        let text = json!({"text":"text"});
        let result = json!({"functionResponse":{"id":"call","name":"tool","response":"ok"}});
        let parts = if is_text_first {
            vec![text, result]
        } else {
            vec![result, text]
        };
        parse::<GeminiGenerateContent>(json!({"contents":[
            {"role":"model","parts":[{"functionCall":{"id":"call","name":"tool","args":{}}}]},
            {"role":"user","parts":parts}]}))
    };
    let text_first = mixed(true)?;
    let result_first = mixed(false)?;
    assert_eq!(text_first.ir.messages, result_first.ir.messages);
    assert!(
        retry(&text_first.affinity.project(&text_first.ir.messages))
            != retry(&result_first.affinity.project(&result_first.ir.messages))
    );
    let unpaired = parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"user","parts":[{"text":"question"}]},
        {"role":"model","parts":[{"text":"answer"}]},
        {"role":"user","parts":[{"functionResponse":{"name":"unknown","response":"ok"}}]}]}))?;
    let unpaired = unpaired.affinity.project(&unpaired.ir.messages);
    assert!(retry(&unpaired).is_none());
    assert_eq!(unpaired.evidence().history.len(), 2);
    Ok(())
}

/// Structured foreign results keep their source type even when inference text is identical;
/// tool arguments canonicalize recursive JSON objects without normalizing plain result text.
///
/// # Errors
///
/// Returns an actual protocol parser rejection.
///
/// # Panics
///
/// Panics if plain JSON-looking text collapses into structured content or source null is lost.
#[test]
fn foreign_result_shapes_and_argument_canonicalization_survive_ir_flattening()
-> Result<(), InvalidRequest> {
    let generate = |result: Value| {
        parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"model","parts":[{"functionCall":{"id":"call","name":"tool","args":{}}}]},
        {"role":"user","parts":[{"functionResponse":{
            "id":"call","name":"tool","response":result}}]}]}))
    };
    for (structured, text) in [
        (json!({"a":1}), json!("{\"a\":1}")),
        (Value::Null, json!("")),
    ] {
        let structured = generate(structured)?;
        let text = generate(text)?;
        assert_eq!(structured.ir.messages, text.ir.messages);
        assert!(
            retry(&structured.affinity.project(&structured.ir.messages))
                != retry(&text.affinity.project(&text.ir.messages))
        );
    }
    let interaction = |result: Value| {
        parse::<GeminiInteraction>(json!({"model":"M","input":[
        {"type":"function_call","id":"call","name":"tool","arguments":{}},
        {"type":"function_result","call_id":"call","result":result}]}))
    };
    let object = interaction(json!({"a":1}))?;
    let text = interaction(json!("{\"a\":1}"))?;
    assert_eq!(object.ir.messages, text.ir.messages);
    assert!(
        retry(&object.affinity.project(&object.ir.messages))
            != retry(&text.affinity.project(&text.ir.messages))
    );
    let chat = |arguments: &str| {
        parse::<OpenAiChatCompletion>(json!({"model":"M","messages":[
        {"role":"assistant","tool_calls":[{"type":"function","id":"call",
            "function":{"name":"tool","arguments":arguments}}]}]}))
    };
    let first = chat("{\"b\": [2, 1], \"a\": {\"z\":1,\"x\":2}}")?;
    let second = chat("{\"a\": {\"x\":2,\"z\":1}, \"b\": [2,1]}")?;
    assert!(
        retry(&first.affinity.project(&first.ir.messages))
            == retry(&second.affinity.project(&second.ir.messages))
    );
    Ok(())
}

/// Required real tool IDs agree across all five codecs, including the Responses presentation ID
/// carve-out; preserved tool-error flags still distinguish Messages and Interactions histories.
///
/// # Errors
///
/// Returns an actual protocol parser rejection.
///
/// # Panics
///
/// Panics if a codec loses tool source order/identity or a retained tool-error flag disappears.
#[test]
fn all_clients_preserve_named_tool_relationships_and_retained_error_flags()
-> Result<(), InvalidRequest> {
    let chat = parse::<OpenAiChatCompletion>(json!({"model":"M","messages":[
        {"role":"user","content":"question"},
        {"role":"assistant","tool_calls":[{"type":"function","id":"call",
            "function":{"name":"tool","arguments":"{\"city\":\"X\"}"}}]},
        {"role":"tool","tool_call_id":"call","content":"ok"},
        {"role":"user","content":"next"}]}))?;
    let response = parse::<OpenAiResponse>(json!({"model":"M","input":[
        {"type":"message","role":"user","content":"question"},
        {"type":"function_call","id":"ignored-resource-id","call_id":"call","name":"tool",
            "arguments":"{\"city\":\"X\"}"},
        {"type":"function_call_output","call_id":"call","output":"ok"},
        {"type":"message","role":"user","content":"next"}]}))?;
    let generate = parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"user","parts":[{"text":"question"}]},
        {"role":"model","parts":[{"functionCall":{
            "id":"call","name":"tool","args":{"city":"X"}}}]},
        {"role":"user","parts":[{"functionResponse":{
            "id":"call","name":"tool","response":"ok"}}]},
        {"role":"user","parts":[{"text":"next"}]}]}))?;
    let interaction = |is_error: bool| {
        parse::<GeminiInteraction>(json!({"model":"M","input":[
        {"type":"user_input","content":[{"type":"text","text":"question"}]},
        {"type":"function_call","id":"call","name":"tool","arguments":{"city":"X"}},
        {"type":"function_result","call_id":"call","result":"ok","is_error":is_error},
        {"type":"user_input","content":[{"type":"text","text":"next"}]}]}))
    };
    let messages = |is_error: bool| {
        parse::<AnthropicMessages>(json!({"model":"M","max_tokens":8,
        "messages":[{"role":"user","content":"question"},
            {"role":"assistant","content":[{"type":"tool_use","id":"call", "name":"tool",
                "input":{"city":"X"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call",
                "content":"ok","is_error":is_error}]},
            {"role":"user","content":"next"}]}))
    };
    let expected = retry(&chat.affinity.project(&chat.ir.messages));
    for request in [response, generate, interaction(false)?, messages(false)?] {
        assert!(retry(&request.affinity.project(&request.ir.messages)) == expected);
    }
    for request in [interaction(true)?, messages(true)?] {
        assert!(retry(&request.affinity.project(&request.ir.messages)) != expected);
    }
    Ok(())
}

/// Actual rendered conversation fields replay to the same canonical continuation on all codecs,
/// including discarded/cross-family reasoning, emitted tool IDs, and object normalization.
///
/// # Errors
///
/// Returns serialization or actual client parser failures.
///
/// # Panics
///
/// Panics if outward projection hashes information absent from the codec's replay.
#[test]
#[expect(
    clippy::unwrap_used,
    reason = "replay fixtures contain the native conversation arrays"
)]
fn rendered_json_history_matches_actual_client_replay() -> Result<(), Box<dyn std::error::Error>> {
    for opaque in [
        Value::Null,
        json!({"type":"thinking","thinking":"native","signature":"signed"}),
        json!({"type":"thought","summary":[{"type":"text","text":"native"}],
            "signature":"signed"}),
        json!({"type":"thought","summary":[
            {"type":"text","text":"na","annotations":{"z":[2,1],"a":null}},
            {"type":"image","data":"opaque","mime_type":"image/png","resolution":null},
            {"type":"text","text":"tive","annotations":null}],"signature":null}),
        json!({"type":"thought","summary":[],"signature":"signed"}),
        json!({"type":"thought","summary":null,"signature":null}),
    ] {
        for arguments in ["{\"z\":2,\"a\":1}", "", " \t\n"] {
            let aggregation = crate::ir::IrAggregation {
                text: "answer λ".into(),
                reasoning: "native".into(),
                reasoning_blocks: vec![crate::ir::IrReasoningBlock {
                    text: "native".into(),
                    signature: Some("signed".into()),
                    opaque: opaque.clone(),
                }],
                tool_calls: vec![crate::ir::IrAggregatedToolCall {
                    call_id: "actual-call".into(),
                    name: "lookup".into(),
                    arguments: arguments.into(),
                    thought_signature: Some("call-signature".into()),
                }],
                ..crate::ir::IrAggregation::default()
            };
            check_json_replay::<OpenAiChatCompletion>(
                &aggregation,
                json!({"model":"M","messages":[{"role":"user","content":"input"}]}),
                |input, response| {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "fixtures contain the native conversation fields"
                    )]
                    input["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(response["choices"][0]["message"].clone())
                },
            )?;
            check_json_replay::<OpenAiResponse>(
                &aggregation,
                json!({"model":"M","input":[{"type":"message","role":"user","content":"input"}]}),
                |input, response| {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "fixtures contain the native conversation fields"
                    )]
                    input["input"]
                        .as_array_mut()
                        .unwrap()
                        .extend(response["output"].as_array().unwrap().iter().cloned())
                },
            )?;
            check_json_replay::<GeminiGenerateContent>(
                &aggregation,
                json!({"contents":[{"role":"user","parts":[{"text":"input"}]}]}),
                |input, response| {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "fixtures contain the native conversation fields"
                    )]
                    input["contents"]
                        .as_array_mut()
                        .unwrap()
                        .push(response["candidates"][0]["content"].clone())
                },
            )?;
            check_json_replay::<GeminiInteraction>(
                &aggregation,
                json!({"model":"M","input":[{"type":"user_input",
                "content":[{"type":"text","text":"input"}]}]}),
                |input, response| {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "fixtures contain the native conversation fields"
                    )]
                    input["input"]
                        .as_array_mut()
                        .unwrap()
                        .extend(response["steps"].as_array().unwrap().iter().cloned())
                },
            )?;
            check_json_replay::<AnthropicMessages>(
                &aggregation,
                json!({"model":"M","max_tokens":8,"messages":[{"role":"user","content":"input"}]}),
                |input, response| {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "fixtures contain the native conversation fields"
                    )]
                    input["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"assistant","content":response["content"]}))
                },
            )?;
        }
    }
    Ok(())
}

/// Compares codec-owned projection with a client replay assembled from actual rendered fields.
///
/// # Errors
///
/// Returns serialization or actual client parser failures.
///
/// # Panics
///
/// Panics if the emitted content does not produce both canonical aliases on replay.
fn check_json_replay<F: ClientFormat>(
    aggregation: &crate::ir::IrAggregation,
    mut input: Value,
    replay: impl FnOnce(&mut Value, Value),
) -> Result<(), Box<dyn std::error::Error>>
where
    F::JsonResponse: serde::Serialize,
{
    let original = parse::<F>(input.clone())?;
    let mut continuation = original.affinity.project(&original.ir.messages);
    let response = F::render_json_response("presentation-only", aggregation);
    assert!(F::project_json_response(&response, &mut continuation));
    replay(&mut input, serde_json::to_value(response)?);
    let actual = parse::<F>(input)?;
    let expected = actual.affinity.project(&actual.ir.messages);
    assert!(
        retry(&continuation) == retry(&expected),
        "{}",
        std::any::type_name::<F>()
    );
    let anchors = continuation.completed_anchors();
    let expected_anchors = expected.completed_anchors();
    assert_eq!(anchors.len(), expected_anchors.len());
    for (left, right) in anchors.iter().zip(expected_anchors.iter()) {
        #[expect(
            clippy::panic,
            reason = "an unexpected anchor is the test failure report"
        )]
        let (crate::affinity::Anchor::History(left), crate::affinity::Anchor::History(right)) =
            (left, right)
        else {
            panic!("ordinary conversation anchors");
        };
        assert!(
            left.digest == right.digest
                && left.projection == right.projection
                && left.units == right.units
        );
    }
    Ok(())
}

/// Malformed upstream argument strings remain outward content, but cannot register a partial
/// assistant history as a completed response or destroy the accepted request's retry binding.
///
/// # Panics
///
/// Panics if malformed output creates a completed history anchor.
#[test]
fn unreplayable_calls_never_anchor_a_partial_assistant() {
    let _configuration = crate::test_support::configuration_lock();
    crate::test_support::install_global_configuration();
    for arguments in ["{", "[]", "null"] {
        check_unreplayable_call::<OpenAiChatCompletion>(arguments);
        check_unreplayable_call::<OpenAiResponse>(arguments);
        for mut renderer in [
            OpenAiChatCompletion::sse_renderer(false, "M".into()),
            OpenAiResponse::sse_renderer(false, "M".into()),
            AnthropicMessages::sse_renderer(false, "M".into()),
            GeminiInteraction::sse_renderer(false, "M".into()),
        ] {
            let mut projector = Projector::new();
            projector.text(crate::affinity::fingerprint::Role::User, b"input");
            renderer.begin_projection(projector);
            for event in [
                crate::ir::IrEvent::TextDelta("answer".into()),
                crate::ir::IrEvent::ToolCallDone {
                    index: 7,
                    call_id: "call".into(),
                    name: "lookup".into(),
                    arguments: arguments.into(),
                    thought_signature: None,
                },
                crate::ir::IrEvent::Done,
            ] {
                renderer.render(&event);
            }
            assert!(renderer.take_projection().is_none());
        }
    }
}

/// Tests actual emitted tool argument replayability rather than upstream JSON parser primitives.
///
/// # Panics
///
/// Panics if output registration or independent request retry binding changes.
fn check_unreplayable_call<F: ClientFormat>(arguments: &str) {
    use crate::affinity::{Accepted, Cache, Decision, Namespace, fingerprint::Role};
    let cache = std::sync::Arc::new(Cache::default());
    let namespace = Namespace {
        api_key_id: 3,
        pool_id: 11,
    };
    let mut original = Projector::new();
    original.text(Role::User, b"input");
    let evidence = original.evidence();
    #[expect(
        clippy::panic,
        reason = "an unexpected decision is the test failure report"
    )]
    let Decision::Fresh(lease) = cache.acquire(namespace, &evidence) else {
        panic!("new request establishment");
    };
    let mut accepted = Accepted::new(lease.commit(5), original.clone());
    let response = F::render_json_response(
        "M",
        &crate::ir::IrAggregation {
            text: "answer".into(),
            tool_calls: vec![crate::ir::IrAggregatedToolCall {
                call_id: "call".into(),
                name: "lookup".into(),
                arguments: arguments.into(),
                thought_signature: None,
            }],
            ..crate::ir::IrAggregation::default()
        },
    );
    assert!(!F::project_json_response(&response, &mut original.clone()));
    // The delivery side's JSON completion glue, composed from the primitives it drives: the
    // completed artifacts register, and the unreplayable response's projection folds into the
    // accepted state without ever registering.
    accepted.complete_artifacts();
    accepted.register_artifacts(&F::completed_json_artifacts(&response));
    assert!(!F::project_json_response(
        &response,
        accepted.projector_mut()
    ));
    let mut partial_replay = original;
    partial_replay.text(Role::Assistant, b"answer");
    partial_replay.text(Role::User, b"next");
    assert!(matches!(
        cache.acquire(namespace, &partial_replay.evidence()),
        Decision::Fresh(_)
    ));
    #[expect(
        clippy::panic,
        reason = "an unexpected decision is the test failure report"
    )]
    let Decision::Bound(binding) = cache.acquire(namespace, &evidence) else {
        panic!("accepted headers preserve the independent retry binding");
    };
    assert_eq!(binding.provider_id(), 5);
}

/// Stream projection follows emitted deltas, not upstream done identities or skipped arguments;
/// simultaneous calls retain outward ordinal order even when their done events arrive reversed.
///
/// # Errors
///
/// Returns fixture parser, SSE, JSON, or actual client replay failures.
///
/// # Panics
///
/// Panics if streamed projection differs from the client-visible replay.
#[test]
fn chat_stream_projection_matches_emitted_call_order_and_residuals()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::ir::IrEvent;
    let original = parse::<OpenAiChatCompletion>(json!({"model":"M","messages":[
        {"role":"user","content":"input"}]}))?;
    let mut renderer = OpenAiChatCompletion::sse_renderer(true, "M".into());
    renderer.begin_projection(original.affinity.project(&original.ir.messages));
    let events = [
        IrEvent::MessageStart {
            id: "resource-only".into(),
            model: "upstream-only".into(),
        },
        IrEvent::TextDelta("answer ".into()),
        IrEvent::ReasoningDelta("discarded".into()),
        IrEvent::ToolCallDelta {
            index: 17,
            call_id: Some("wire-id".into()),
            name: Some("look".into()),
            arguments_delta: "{\"wire\":1}".into(),
        },
        IrEvent::ToolCallDelta {
            index: 17,
            call_id: None,
            name: Some("up".into()),
            arguments_delta: String::new(),
        },
        IrEvent::TextDelta("λ".into()),
        IrEvent::ToolCallDelta {
            index: 3,
            call_id: Some("second-id".into()),
            name: Some("second".into()),
            arguments_delta: "{".into(),
        },
        IrEvent::ToolCallDone {
            index: 3,
            call_id: "ignored-second-id".into(),
            name: "ignored-second-name".into(),
            arguments: "{\"value\":2}".into(),
            thought_signature: None,
        },
        IrEvent::ToolCallDone {
            index: 17,
            call_id: "ignored-first-id".into(),
            name: "ignored-first-name".into(),
            arguments: "{\"upstream\":9}".into(),
            thought_signature: Some("actual-signature".into()),
        },
        IrEvent::finish(crate::ir::IrFinishReason::ToolCall),
        IrEvent::Usage(crate::ir::IrUsage::default()),
        IrEvent::Done,
    ];
    let mut parser = crate::sse::SseParser::new();
    let mut text = String::new();
    let mut calls = std::collections::BTreeMap::new();
    for event in events {
        for frame in renderer.render(&event) {
            for frame in parser.ingest(frame.as_bytes())? {
                if frame.data == "[DONE]" {
                    continue;
                }
                let frame: Value = serde_json::from_str(&frame.data)?;
                let Some(delta) = frame.pointer("/choices/0/delta") else {
                    continue;
                };
                if let Some(part) = delta.get("content").and_then(Value::as_str) {
                    text.push_str(part);
                }
                for call in delta
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    #[expect(
                        clippy::unwrap_used,
                        reason = "rendered calls have concrete outward indexes"
                    )]
                    let index = call.get("index").and_then(Value::as_u64).unwrap();
                    let entry = calls.entry(index).or_insert_with(|| {
                        (String::new(), String::new(), String::new(), None::<Value>)
                    });
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        entry.0.push_str(id);
                    }
                    if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                        entry.1.push_str(name);
                    }
                    if let Some(arguments) =
                        call.pointer("/function/arguments").and_then(Value::as_str)
                    {
                        entry.2.push_str(arguments);
                    }
                    if let Some(signature) = call.get("thought_signature") {
                        entry.3 = Some(signature.clone());
                    }
                }
            }
        }
    }
    #[expect(
        clippy::expect_used,
        reason = "the emitted call suffixes are fully replayable"
    )]
    let projection = renderer
        .take_projection()
        .expect("complete emitted projection");
    let calls: Vec<_> = calls
        .into_values()
        .map(|(id, name, arguments, signature)| {
            json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments},
            "thought_signature":signature})
        })
        .collect();
    let replay = parse::<OpenAiChatCompletion>(json!({"model":"renamed", "messages":[
        {"role":"user","content":"input"},
        {"role":"assistant","content":text,"tool_calls":calls}]}))?;
    assert!(retry(&projection) == retry(&replay.affinity.project(&replay.ir.messages)));
    assert_eq!(projection.completed_anchors().len(), 2);
    // Equal actual client histories must agree across JSON and SSE chunking. The first fixture
    // intentionally diverges from upstream done objects; this conformant fixture does not.
    let mut renderer = OpenAiChatCompletion::sse_renderer(false, "presentation-only".into());
    renderer.begin_projection(original.affinity.project(&original.ir.messages));
    let mut aggregation = crate::ir::Aggregator::default();
    for event in [
        IrEvent::TextDelta("answer ".into()),
        IrEvent::TextDelta("λ".into()),
        IrEvent::ToolCallDone {
            index: 9,
            call_id: "actual-id".into(),
            name: "lookup".into(),
            arguments: "{\"z\":2,\"a\":1}".into(),
            thought_signature: Some("signature".into()),
        },
        IrEvent::finish(crate::ir::IrFinishReason::ToolCall),
        IrEvent::Done,
    ] {
        aggregation.ingest(&event);
        renderer.render(&event);
    }
    let response =
        OpenAiChatCompletion::render_json_response("different-public-name", &aggregation.finish());
    let mut json_projection = original.affinity.project(&original.ir.messages);
    assert!(OpenAiChatCompletion::project_json_response(
        &response,
        &mut json_projection
    ));
    #[expect(
        clippy::expect_used,
        reason = "the conformant fixture has a complete emitted projection"
    )]
    let stream_projection = renderer
        .take_projection()
        .expect("complete emitted projection");
    assert!(retry(&json_projection) == retry(&stream_projection));
    Ok(())
}

/// GenerateContent projects actual part order, accepted thought text, emitted signatures, and
/// id-less replay semantics without hashing the parser's newly generated internal UUIDs.
///
/// # Errors
///
/// Returns fixture SSE, JSON, or actual client replay failures.
///
/// # Panics
///
/// Panics if streamed projection differs from actual emitted-part replay.
#[test]
fn generate_stream_projection_matches_actual_part_order_and_idless_replay()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::ir::IrEvent;
    let original = parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"user","parts":[{"text":"input"}]}]}))?;
    let mut renderer = GeminiGenerateContent::sse_renderer(false, "M".into());
    renderer.begin_projection(original.affinity.project(&original.ir.messages));
    let mut parser = crate::sse::SseParser::new();
    let mut parts = Vec::new();
    for event in [
        IrEvent::ReasoningDelta("thought ".into()),
        IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("unemitted".into()),
            opaque: Value::Null,
        },
        IrEvent::TextDelta("answer".into()),
        IrEvent::ToolCallDone {
            index: 19,
            call_id: String::new(),
            name: "idless".into(),
            arguments: "{\"z\":2,\"a\":1}".into(),
            thought_signature: Some("call-signature".into()),
        },
        IrEvent::TextDelta(" tail λ".into()),
        IrEvent::ToolCallDone {
            index: 3,
            call_id: "actual-id".into(),
            name: "normalize".into(),
            arguments: "[]".into(),
            thought_signature: None,
        },
        IrEvent::finish(crate::ir::IrFinishReason::ToolCall),
        IrEvent::Done,
    ] {
        for frame in renderer.render(&event) {
            for frame in parser.ingest(frame.as_bytes())? {
                let frame: Value = serde_json::from_str(&frame.data)?;
                if let Some(actual) = frame
                    .pointer("/candidates/0/content/parts")
                    .and_then(Value::as_array)
                {
                    parts.extend(actual.iter().cloned());
                }
            }
        }
    }
    #[expect(
        clippy::expect_used,
        reason = "the actual emitted parts are fully replayable"
    )]
    let projection = renderer
        .take_projection()
        .expect("complete emitted projection");
    let replay = parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"user","parts":[{"text":"input"}]}, {"role":"model","parts":parts}]}))?;
    let expected = replay.affinity.project(&replay.ir.messages);
    assert!(retry(&projection) == retry(&expected));
    assert_eq!(
        projection.completed_anchors().len(),
        1,
        "idless output has only its relational anchor"
    );
    // JSON output shares the same empty-ID source semantics, rather than retaining that string
    // as an exact identity that the actual client parser treats as absent.
    let response = GeminiGenerateContent::render_json_response(
        "M",
        &crate::ir::IrAggregation {
            text: "answer".into(),
            tool_calls: vec![crate::ir::IrAggregatedToolCall {
                call_id: String::new(),
                name: "idless".into(),
                arguments: "{}".into(),
                thought_signature: None,
            }],
            ..crate::ir::IrAggregation::default()
        },
    );
    let mut json_projection = original.affinity.project(&original.ir.messages);
    assert!(GeminiGenerateContent::project_json_response(
        &response,
        &mut json_projection
    ));
    let response = serde_json::to_value(response)?;
    let content = response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"));
    let replay = parse::<GeminiGenerateContent>(json!({"contents":[
        {"role":"user","parts":[{"text":"input"}]}, content]}))?;
    assert!(retry(&json_projection) == retry(&replay.affinity.project(&replay.ir.messages)));
    Ok(())
}

/// Responses retain first-seen output positions despite interleaved deltas and reverse call
/// completion, and text extends an existing assistant tail without an extra canonical unit.
///
/// # Errors
///
/// Returns real client parsing or SSE decoding failures.
///
/// # Panics
///
/// Panics if streamed output differs from replay of its actual terminal output array.
#[test]
fn response_stream_projection_preserves_positions_and_open_assistant_tail()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::ir::IrEvent;
    for has_leading_call in [false, true] {
        for arguments in ["{}", "", " \t\n"] {
            let original = parse::<OpenAiResponse>(json!({"model":"M","input":[
            {"type":"message","role":"user","content":"input"},
            {"type":"message","role":"assistant","content":"existing "}]}))?;
            let mut renderer = OpenAiResponse::sse_renderer(false, "M".into());
            renderer.begin_projection(original.affinity.project(&original.ir.messages));
            let first_call = IrEvent::ToolCallDelta {
                index: 19,
                call_id: Some("provisional".into()),
                name: Some("provisional".into()),
                arguments_delta: "discarded".into(),
            };
            let mut events = vec![IrEvent::MessageStart {
                id: "ignored".into(),
                model: "M".into(),
            }];
            if has_leading_call {
                events.push(first_call.clone());
            }
            events.extend([
                IrEvent::TextDelta("answer ".into()),
                IrEvent::ReasoningDelta("discarded".into()),
            ]);
            if !has_leading_call {
                events.push(first_call);
            }
            events.extend([
                IrEvent::ToolCallDelta {
                    index: 3,
                    call_id: None,
                    name: None,
                    arguments_delta: "{".into(),
                },
                IrEvent::TextDelta("λ".into()),
                IrEvent::ToolCallDone {
                    index: 3,
                    call_id: "second".into(),
                    name: "second".into(),
                    arguments: "{\"z\":2,\"a\":1}".into(),
                    thought_signature: None,
                },
                IrEvent::ToolCallDone {
                    index: 19,
                    call_id: "first".into(),
                    name: "first".into(),
                    arguments: arguments.into(),
                    thought_signature: Some("signature".into()),
                },
                IrEvent::finish(crate::ir::IrFinishReason::ToolCall),
                IrEvent::Done,
            ]);
            let mut parser = crate::sse::SseParser::new();
            let mut actual_output = None;
            for event in events {
                for frame in renderer.render(&event) {
                    for frame in parser.ingest(frame.as_bytes())? {
                        let body: Value = serde_json::from_str(&frame.data)?;
                        if body.get("type").and_then(Value::as_str) == Some("response.completed") {
                            actual_output = body.pointer("/response/output").cloned();
                        }
                    }
                }
            }
            #[expect(clippy::expect_used, reason = "the fixture emits a completed response")]
            let projection = renderer
                .take_projection()
                .expect("complete output projection");
            let mut input = vec![
                json!({"type":"message","role":"user","content":"input"}),
                json!({"type":"message","role":"assistant","content":"existing "}),
            ];
            #[expect(
                clippy::expect_used,
                reason = "the terminal fixture carries an output array"
            )]
            input.extend(
                actual_output
                    .expect("terminal output")
                    .as_array()
                    .expect("output array")
                    .iter()
                    .cloned(),
            );
            let replay = parse::<OpenAiResponse>(json!({"model":"M","input":input}))?;
            assert!(retry(&projection) == retry(&replay.affinity.project(&replay.ir.messages)));
        }
    }
    Ok(())
}

/// Messages projects emitted text and whole tool blocks, including buffered-argument fallback,
/// authoritative done identity, the actual order of completed invocations, and discarded opaque
/// reasoning that never appears on the streaming wire.
///
/// # Errors
///
/// Returns real parser, SSE, or JSON failures.
///
/// # Panics
///
/// Panics if a complete stream differs from its actual replayed blocks.
#[test]
fn messages_stream_projection_matches_actual_blocks_and_argument_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::ir::IrEvent;
    let original = parse::<AnthropicMessages>(json!({"model":"M","max_tokens":8,
        "messages":[{"role":"user","content":"input"},
            {"role":"assistant","content":"existing "}]}))?;
    let mut renderer = AnthropicMessages::sse_renderer(false, "M".into());
    renderer.begin_projection(original.affinity.project(&original.ir.messages));
    let mut parser = crate::sse::SseParser::new();
    let mut blocks = std::collections::BTreeMap::<u64, Value>::new();
    let mut arguments = std::collections::BTreeMap::<u64, String>::new();
    for event in [
        IrEvent::TextDelta("answer ".into()),
        IrEvent::TextDelta("λ".into()),
        IrEvent::ReasoningDone {
            text: String::new(),
            signature: None,
            opaque: json!({"type":"redacted_thinking","data":"not emitted"}),
        },
        IrEvent::ReasoningDelta("plan ".into()),
        IrEvent::ReasoningDelta("λ".into()),
        IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("wire-signature".into()),
            opaque: json!({"type":"thinking","thinking":"plan λ",
                "signature":"not-emitted-signature"}),
        },
        IrEvent::ToolCallDelta {
            index: 19,
            call_id: Some("provisional".into()),
            name: Some("provisional".into()),
            arguments_delta: "{\"z\":2,\"a\":1}".into(),
        },
        IrEvent::ToolCallDone {
            index: 3,
            call_id: "first".into(),
            name: "lookup".into(),
            arguments: "{}".into(),
            thought_signature: Some("not-emitted".into()),
        },
        IrEvent::TextDelta(" after call".into()),
        IrEvent::ReasoningDone {
            text: "whole\n\"thought".into(),
            signature: Some("whole-signature".into()),
            opaque: json!({"type":"thinking","thinking":"not emitted"}),
        },
        IrEvent::ToolCallDone {
            index: 19,
            call_id: "actual".into(),
            name: "fallback".into(),
            arguments: String::new(),
            thought_signature: None,
        },
        IrEvent::finish(crate::ir::IrFinishReason::ToolCall),
        IrEvent::Done,
    ] {
        for frame in renderer.render(&event) {
            for frame in parser.ingest(frame.as_bytes())? {
                let body: Value = serde_json::from_str(&frame.data)?;
                let Some(index) = body.get("index").and_then(Value::as_u64) else {
                    continue;
                };
                match body.get("type").and_then(Value::as_str) {
                    Some("content_block_start") => {
                        #[expect(clippy::expect_used, reason = "block start has its block")]
                        blocks.insert(index, body.get("content_block").expect("block").clone());
                    }
                    Some("content_block_delta") => {
                        #[expect(
                            clippy::expect_used,
                            reason = "the emitted delta follows its opened fixture block"
                        )]
                        if let Some(text) = body.pointer("/delta/text").and_then(Value::as_str) {
                            let block = blocks.get_mut(&index).expect("opened block");
                            let actual = block["text"].as_str().expect("text block");
                            block["text"] = Value::String(format!("{actual}{text}"));
                        } else if let Some(text) =
                            body.pointer("/delta/thinking").and_then(Value::as_str)
                        {
                            let block = blocks.get_mut(&index).expect("opened thinking block");
                            let actual = block["thinking"].as_str().expect("thinking text");
                            block["thinking"] = Value::String(format!("{actual}{text}"));
                        } else if let Some(signature) =
                            body.pointer("/delta/signature").and_then(Value::as_str)
                        {
                            let block = blocks.get_mut(&index).expect("opened thinking block");
                            let actual = block["signature"].as_str().unwrap_or_default();
                            block["signature"] = Value::String(format!("{actual}{signature}"));
                        } else if let Some(part) =
                            body.pointer("/delta/partial_json").and_then(Value::as_str)
                        {
                            arguments.entry(index).or_default().push_str(part);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for (index, arguments) in arguments {
        #[expect(
            clippy::expect_used,
            reason = "arguments belong to an opened tool block"
        )]
        {
            blocks.get_mut(&index).expect("tool block")["input"] =
                serde_json::from_str(&arguments)?;
        }
    }
    #[expect(
        clippy::expect_used,
        reason = "all fixture blocks are fully replayable"
    )]
    let projection = renderer
        .take_projection()
        .expect("complete stream projection");
    let replay = parse::<AnthropicMessages>(json!({"model":"M","max_tokens":8,"messages":[
        {"role":"user","content":"input"}, {"role":"assistant","content":"existing "},
        {"role":"assistant","content":blocks.into_values().collect::<Vec<_>>()}]}))?;
    assert!(retry(&projection) == retry(&replay.affinity.project(&replay.ir.messages)));
    Ok(())
}

/// Missing or contradictory completion carriers never replace actual emitted reasoning fields.
///
/// # Panics
///
/// Panics if upstream-only reasoning replaces actual outward text in a completion anchor.
#[test]
fn messages_stream_reasoning_uses_emitted_fields() {
    use crate::ir::IrEvent;
    let mut expected = None;
    for (opaque, should_project) in [
        (json!({"type":"thinking","thinking":"plan λ"}), true),
        (json!({"type":"thinking","thinking":"different"}), true),
        (Value::Null, true),
    ] {
        let mut renderer = AnthropicMessages::sse_renderer(false, "M".into());
        renderer.begin_projection(Projector::new());
        for event in [
            IrEvent::ReasoningDelta("plan ".into()),
            IrEvent::ReasoningDelta("λ".into()),
            IrEvent::ReasoningDone {
                text: String::new(),
                signature: Some("wire-signature".into()),
                opaque,
            },
            IrEvent::TextDelta("answer".into()),
            IrEvent::finish(crate::ir::IrFinishReason::Stop),
            IrEvent::Done,
        ] {
            renderer.render(&event);
        }
        let actual = renderer
            .take_projection()
            .map(|projection| retry(&projection));
        assert_eq!(actual.is_some(), should_project);
        if let Some(expected) = expected {
            assert!(actual == expected);
        }
        expected = Some(actual);
    }
}

/// Interactions follows actual step indexes and emitted identities, preserving divergent-done
/// reconciliation and residual argument bytes without treating upstream-only fields as history.
///
/// # Errors
///
/// Returns real native request, SSE, or JSON parsing failures.
///
/// # Panics
///
/// Panics if completed projection differs from the actual replayed outward steps.
#[test]
fn interaction_stream_projection_preserves_steps_and_emitted_call_identity()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::ir::IrEvent;
    for has_leading_call in [false, true] {
        let original = parse::<GeminiInteraction>(json!({"model":"M","input":[
            {"type":"user_input","content":[{"type":"text","text":"input"}]},
            {"type":"model_output","content":[{"type":"text","text":"existing "}]}]}))?;
        let mut renderer = GeminiInteraction::sse_renderer(false, "M".into());
        renderer.begin_projection(original.affinity.project(&original.ir.messages));
        let call = IrEvent::ToolCallDelta {
            index: 19,
            call_id: Some("wire-id".into()),
            name: Some("wire-name".into()),
            arguments_delta: "{\"wire\":1}".into(),
        };
        let mut events = Vec::new();
        if has_leading_call {
            events.push(call.clone());
        }
        events.extend([
            IrEvent::TextDelta("answer ".into()),
            IrEvent::TextDelta("λ".into()),
        ]);
        if !has_leading_call {
            events.push(call);
        }
        events.extend([
            IrEvent::ReasoningDelta("plan ".into()),
            IrEvent::ReasoningDelta("λ".into()),
            IrEvent::ReasoningDone {
                text: String::new(),
                signature: Some("reasoning-signature".into()),
                opaque: json!({"type":"thinking","thinking":"plan λ","signature":"not emitted"}),
            },
            IrEvent::ReasoningDone {
                text: "whole\n\"thought".into(),
                signature: None,
                opaque: json!({"type":"thought","signature":"not emitted"}),
            },
            IrEvent::ToolCallDelta {
                index: 3,
                call_id: Some("second".into()),
                name: Some("second".into()),
                arguments_delta: "{".into(),
            },
            IrEvent::TextDelta(" tail".into()),
            IrEvent::ToolCallDone {
                index: 3,
                call_id: "not-emitted".into(),
                name: "not-emitted".into(),
                arguments: "{\"z\":2,\"a\":1}".into(),
                thought_signature: Some("second-signature".into()),
            },
            IrEvent::ToolCallDone {
                index: 19,
                call_id: "not-emitted".into(),
                name: "not-emitted".into(),
                arguments: "{\"upstream\":9}".into(),
                thought_signature: Some("wire-signature".into()),
            },
            IrEvent::finish(crate::ir::IrFinishReason::ToolCall),
            IrEvent::Done,
        ]);
        let mut parser = crate::sse::SseParser::new();
        let mut steps = std::collections::BTreeMap::<u64, Value>::new();
        let mut arguments = std::collections::BTreeMap::<u64, String>::new();
        for event in events {
            for frame in renderer.render(&event) {
                for frame in parser.ingest(frame.as_bytes())? {
                    let body: Value = serde_json::from_str(&frame.data)?;
                    let Some(index) = body.get("index").and_then(Value::as_u64) else {
                        continue;
                    };
                    match body.get("event_type").and_then(Value::as_str) {
                        Some("step.start") => {
                            #[expect(
                                clippy::expect_used,
                                reason = "the opening fixture carries its step"
                            )]
                            let mut step = body.get("step").expect("step").clone();
                            if step.get("type").and_then(Value::as_str) == Some("model_output") {
                                #[expect(
                                    clippy::indexing_slicing,
                                    reason = "the own opening step is an object"
                                )]
                                {
                                    step["content"] = json!([{"type":"text","text":""}]);
                                }
                            }
                            steps.insert(index, step);
                        }
                        Some("step.delta") => {
                            if let Some(text) = body.pointer("/delta/text").and_then(Value::as_str)
                            {
                                #[expect(
                                    clippy::expect_used,
                                    clippy::indexing_slicing,
                                    reason = "text follows the opened one-part output fixture"
                                )]
                                {
                                    let step = steps.get_mut(&index).expect("output step");
                                    let actual = step["content"][0]["text"].as_str().expect("text");
                                    step["content"][0]["text"] =
                                        Value::String(format!("{actual}{text}"));
                                }
                            } else if let Some(text) =
                                body.pointer("/delta/content/text").and_then(Value::as_str)
                            {
                                #[expect(
                                    clippy::expect_used,
                                    reason = "summary deltas follow the opened thought fixture"
                                )]
                                let step = steps.get_mut(&index).expect("thought step");
                                let previous = step
                                    .pointer("/summary/0/text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                step["summary"] =
                                    json!([{"type":"text","text":format!("{previous}{text}")}]);
                            } else if let Some(signature) =
                                body.pointer("/delta/signature").and_then(Value::as_str)
                            {
                                #[expect(
                                    clippy::expect_used,
                                    reason = "signature deltas follow the opened thought fixture"
                                )]
                                let step = steps.get_mut(&index).expect("thought step");
                                step["signature"] = Value::String(signature.into());
                            } else if let Some(part) =
                                body.pointer("/delta/arguments").and_then(Value::as_str)
                            {
                                arguments.entry(index).or_default().push_str(part);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        for (index, arguments) in arguments {
            #[expect(
                clippy::expect_used,
                reason = "arguments follow their opened invocation fixture"
            )]
            {
                steps.get_mut(&index).expect("call step")["arguments"] =
                    serde_json::from_str(&arguments)?;
            }
        }
        #[expect(
            clippy::expect_used,
            reason = "every emitted step is complete and replayable"
        )]
        let projection = renderer
            .take_projection()
            .expect("completed stream projection");
        let mut input = vec![
            json!({"type":"user_input","content":[{"type":"text","text":"input"}]}),
            json!({"type":"model_output","content":[{"type":"text","text":"existing "}]}),
        ];
        input.extend(steps.into_values());
        let replay = parse::<GeminiInteraction>(json!({"model":"M","input":input}))?;
        assert!(retry(&projection) == retry(&replay.affinity.project(&replay.ir.messages)));
    }
    Ok(())
}

/// Delta-only reasoning preserves canonical escaping, empty text, and codec-owned signature shape.
///
/// # Errors
///
/// Returns actual accepted replay parser failures.
///
/// # Panics
///
/// Panics if incremental reasoning or a late signature loses its actual replay identity.
#[test]
fn delta_only_reasoning_matches_actual_replay() -> Result<(), Box<dyn std::error::Error>> {
    use crate::ir::IrEvent;
    for chunks in [
        vec![""],
        vec!["plan ", "λ"],
        vec!["\\", "\n\"", "\0", "\u{001f}"],
    ] {
        for signature in [None, Some("late-signature λ")] {
            for has_done in [false, true] {
                if !has_done && signature.is_some() {
                    continue;
                }
                for is_messages in [false, true] {
                    let mut renderer = if is_messages {
                        AnthropicMessages::sse_renderer(false, "M".into())
                    } else {
                        GeminiInteraction::sse_renderer(false, "M".into())
                    };
                    renderer.begin_projection(Projector::new());
                    for chunk in &chunks {
                        renderer.render(&IrEvent::ReasoningDelta((*chunk).into()));
                    }
                    if has_done {
                        renderer.render(&IrEvent::ReasoningDone {
                            text: if is_messages {
                                String::new()
                            } else {
                                "not emitted".into()
                            },
                            signature: signature.map(str::to_owned),
                            opaque: Value::Null,
                        });
                    }
                    renderer.render(&IrEvent::TextDelta("answer".into()));
                    renderer.render(&IrEvent::Done);
                    #[expect(
                        clippy::expect_used,
                        reason = "the completed emitted summary and signature are fully replayable"
                    )]
                    let actual = renderer
                        .take_projection()
                        .expect("completed thought projection");
                    let replay = if is_messages {
                        parse::<AnthropicMessages>(json!({
                            "model":"M", "max_tokens":8,
                            "messages":[{"role":"assistant","content":[
                                {"type":"thinking","thinking":chunks.concat(),
                                 "signature":signature.unwrap_or("")},
                                {"type":"text","text":"answer"}
                            ]}]
                        }))?
                    } else {
                        parse::<GeminiInteraction>(json!({"model":"M","input":[
                            {"type":"thought","signature":signature,
                             "summary":[{"type":"text","text":chunks.concat()}]},
                            {"type":"model_output","content":[{"type":"text","text":"answer"}]}
                        ]}))?
                    };
                    assert!(retry(&actual) == retry(&replay.affinity.project(&replay.ir.messages)));
                }
            }
        }
    }
    Ok(())
}

/// Actual emitted summary fields remain complete even when upstream done text differs or is absent.
///
/// # Panics
///
/// Panics if upstream completion metadata prevents an actual emitted summary anchor.
#[test]
fn interaction_stream_reasoning_uses_emitted_fields() {
    use crate::ir::IrEvent;
    let mut expected = None;
    for (text, opaque, should_project) in [
        ("plan λ", Value::Null, true),
        ("different", Value::Null, true),
        ("", Value::Null, true),
        (
            "",
            json!({"thinking":"plan λ","signature":"not emitted"}),
            true,
        ),
        ("", json!({"thinking":"different"}), true),
    ] {
        let mut renderer = GeminiInteraction::sse_renderer(false, "M".into());
        renderer.begin_projection(Projector::new());
        for event in [
            IrEvent::ReasoningDelta("plan ".into()),
            IrEvent::ReasoningDelta("λ".into()),
            IrEvent::ReasoningDone {
                text: text.into(),
                signature: Some("wire-signature".into()),
                opaque,
            },
            IrEvent::TextDelta("answer".into()),
            IrEvent::finish(crate::ir::IrFinishReason::Stop),
            IrEvent::Done,
        ] {
            renderer.render(&event);
        }
        let actual = renderer
            .take_projection()
            .map(|projection| retry(&projection));
        assert_eq!(actual.is_some(), should_project);
        if let Some(expected) = expected {
            assert!(actual == expected);
        }
        expected = Some(actual);
    }
}

/// A validated covered prefix must not consume the uncovered calls' source records or move
/// opaque state across messages; projection uses the client window rather than restored text.
///
/// # Errors
///
/// Returns a fixture rejection from the real Responses parser.
///
/// # Panics
///
/// Panics if prefix coverage, tool provenance, or state placement changes the replay identity.
#[test]
fn active_window_projection_preserves_tool_sources_and_state_positions()
-> Result<(), InvalidRequest> {
    let covered = vec![
        json!({"type":"message","role":"user","content":"covered"}),
        json!({"type":"function_call","call_id":"old","name":"old_tool","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"old","output":"old result"}),
    ];
    let suffix = vec![
        json!({"type":"reasoning","id":"rs_live","encrypted_content":"signed state","summary":[]}),
        json!({"type":"message","role":"user","content":"uncovered"}),
        json!({"type":"function_call","call_id":"new","name":"new_tool","arguments":"{\"x\":1}"}),
        json!({"type":"function_call_output","call_id":"new","output":"new result"}),
        json!({"type":"reasoning","id":"rs_tail","encrypted_content":"tail state","summary":[]}),
    ];
    let mut input = covered.clone();
    input.extend(suffix.clone());
    let full = parse::<OpenAiResponse>(json!({"model":"M","input":input}))?;
    let omitted = parse::<OpenAiResponse>(json!({"model":"M","input":suffix}))?;
    let first_message = parse::<OpenAiResponse>(json!({"model":"M","input":covered}))?
        .ir
        .messages
        .len();
    let mut retained = Projector::new();
    full.affinity.append_window(
        &full.ir.messages,
        &full.ir.state_items,
        first_message,
        &mut retained,
    );
    let mut discarded = Projector::new();
    omitted.affinity.append_window(
        &omitted.ir.messages,
        &omitted.ir.state_items,
        0,
        &mut discarded,
    );
    assert!(retry(&retained).is_some());
    assert!(retry(&retained) == retry(&discarded));
    let mut moved = omitted.ir.state_items.clone();
    if let Some(first) = moved.first_mut() {
        first.offset = 1;
    }
    let mut reordered = Projector::new();
    omitted
        .affinity
        .append_window(&omitted.ir.messages, &moved, 0, &mut reordered);
    assert!(retry(&reordered) != retry(&retained));
    assert!(retry(&omitted.affinity.project(&omitted.ir.messages)) != retry(&retained));
    // Covered id-less/null sources must not turn a later real-ID/object exchange into a
    // relational-only or text result projection. GenerateContent also regroups the mixed tail.
    let covered = vec![
        json!({"role":"model","parts":[{"functionCall":{"name":"old_tool","args":{}}}]}),
        json!({"role":"user","parts":[{"functionResponse":{"name":"old_tool","response":null}}]}),
    ];
    let suffix = vec![
        json!({"role":"model","parts":[
            {"functionCall":{"id":"new","name":"new_tool","args":{"x":1}}}
        ]}),
        json!({"role":"user","parts":[{"text":"uncovered"},
            {"functionResponse":{"id":"new","name":"new_tool","response":{"ok":true}}}]}),
    ];
    let first_message = parse::<GeminiGenerateContent>(json!({"contents":covered}))?
        .ir
        .messages
        .len();
    let mut contents = covered;
    contents.extend(suffix.clone());
    let full = parse::<GeminiGenerateContent>(json!({"contents":contents}))?;
    let omitted = parse::<GeminiGenerateContent>(json!({"contents":suffix}))?;
    let mut retained = Projector::new();
    full.affinity
        .append_window(&full.ir.messages, &[], first_message, &mut retained);
    assert!(retry(&retained).is_some_and(|(projection, _)| projection == Projection::Exact));
    assert!(retry(&retained) == retry(&omitted.affinity.project(&omitted.ir.messages)));
    Ok(())
}
