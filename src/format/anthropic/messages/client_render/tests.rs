//! Client response rendering tests for the Anthropic Messages codec: the aggregated body
//! shapes, the streamed SSE frame lifecycles, and the compaction prefix and usage framings
//! (`docs/reference/inference.md`, `docs/design/compaction.md`).
#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section on every \
              test would document nothing its name does not"
)]

use crate::{
    affinity::fingerprint::Projector,
    api::Error,
    format::{
        ClientSseRenderer as _,
        compaction_wire::{Checkpoint, MessagesCheckpoint},
    },
    ir::{
        IrAggregatedToolCall, IrAggregation, IrEvent, IrFinishReason, IrReasoningBlock, IrUsage,
        compaction::UsageIteration,
    },
};
use serde_json::Value;
use std::error::Error as StdError;

/// A complete native compaction block as an upstream would report it.
fn native_block() -> Value {
    serde_json::json!({
        "type": "compaction",
        "content": "Native summary",
        "signature": "native-sig"
    })
}

/// A gateway compaction checkpoint with readable content.
fn gateway_block(content: &str) -> Checkpoint {
    Checkpoint::Messages(MessagesCheckpoint {
        content: content.to_string(),
        signature: "janusgate.compaction.v1.primary.ABCD".to_string(),
    })
}

/// Splits one rendered SSE frame into its event name and JSON payload.
///
/// # Panics
///
/// Panics when a rendered frame carries no event name or no data line, or a data line that
/// does not parse as JSON: the renderer under test always writes both.
fn frame_parts(frame: &str) -> (&str, Value) {
    let mut event = None;
    let mut data = None;

    for line in frame.lines() {
        if let Some(name) = line.strip_prefix("event: ") {
            event = Some(name);
        }

        if let Some(payload) = line.strip_prefix("data: ") {
            data = Some(payload);
        }
    }

    #[expect(clippy::expect_used, reason = "rendered frames carry event names")]
    let (event_name, payload) = (
        event.expect("rendered frames carry event names"),
        data.expect("rendered frames carry data"),
    );

    #[expect(clippy::expect_used, reason = "rendered payloads are first-party JSON")]
    let parsed = serde_json::from_str(payload).expect("rendered payloads parse");

    (event_name, parsed)
}

/// Renders IR events through a fresh streaming renderer into `(event, payload)` pairs.
fn rendered_frames(events: &[IrEvent]) -> Vec<(String, Value)> {
    let mut renderer = super::SseRenderer::new("pub-model".to_string());

    events
        .iter()
        .flat_map(|event| renderer.render(event))
        .map(|frame| {
            let (name, payload) = frame_parts(&frame);

            (name.to_string(), payload)
        })
        .collect()
}

/// Renders IR events through a fresh streaming renderer, returning only the event names.
fn rendered_names(events: &[IrEvent]) -> Vec<String> {
    rendered_frames(events)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Splits a rendered frame batch carrying several frames and parses each one.
fn parse_batch(batch: &str) -> Vec<(String, Value)> {
    batch
        .split("\n\n")
        .filter(|frame| !frame.is_empty())
        .map(|frame| {
            let (name, payload) = frame_parts(frame);

            (name.to_string(), payload)
        })
        .collect()
}

/// Parses one renderer batch of raw frames into `(event, payload)` pairs.
fn rendered_batch(frames: Vec<String>) -> Vec<(String, Value)> {
    parse_batch(&frames.concat())
}

/// The JSON payload of the frame at `index` in a parsed batch.
fn payload(frames: &[(String, Value)], index: usize) -> &Value {
    #[expect(
        clippy::indexing_slicing,
        reason = "a batch shorter than the asserted frame positions is the failure under test"
    )]
    let (_, payload) = &frames[index];

    payload
}

/// The aggregated body renders the native Messages envelope: compaction state verbatim at the
/// head, replayable reasoning verbatim, foreign reasoning degraded to the text channel, the
/// answer text, tool calls with their input objects, and the full usage shape.
#[test]
fn aggregated_body_renders_native_shapes() -> Result<(), Box<dyn StdError>> {
    let aggregation = IrAggregation {
        text: "Answer".to_string(),
        reasoning_blocks: vec![
            IrReasoningBlock {
                text: "think".to_string(),
                signature: Some("sig".to_string()),
                opaque: serde_json::json!({
                    "type": "thinking",
                    "thinking": "think",
                    "signature": "sig"
                }),
            },
            IrReasoningBlock {
                text: "foreign reasoning".to_string(),
                signature: None,
                opaque: Value::Null,
            },
            IrReasoningBlock {
                text: String::new(),
                signature: None,
                opaque: Value::Null,
            },
        ],
        tool_calls: vec![
            IrAggregatedToolCall {
                call_id: "t1".to_string(),
                name: "find".to_string(),
                arguments: "{\"a\":2}".to_string(),
                thought_signature: None,
            },
            IrAggregatedToolCall {
                call_id: "t2".to_string(),
                name: "greet".to_string(),
                arguments: "oops".to_string(),
                thought_signature: None,
            },
        ],
        finish: Some(IrFinishReason::Stop),
        usage: Some(IrUsage {
            input_tokens: 10,
            output_tokens: 20,
            cached_tokens: Some(4),
            cache_write_tokens: Some(6),
            reasoning_tokens: Some(8),
        }),
        compaction_states: vec![native_block()],
        ..IrAggregation::default()
    };

    let body = serde_json::to_value(super::render_json_response("pub-model", &aggregation))?;

    assert_eq!(crate::test_support::at(&body, "/type"), "message");

    assert_eq!(crate::test_support::at(&body, "/role"), "assistant");

    assert_eq!(crate::test_support::at(&body, "/model"), "pub-model");

    assert_eq!(crate::test_support::at(&body, "/stop_reason"), "end_turn");

    assert_eq!(
        crate::test_support::at(&body, "/stop_sequence"),
        &Value::Null
    );

    assert!(
        crate::test_support::at(&body, "/id")
            .as_str()
            .is_some_and(|id| id.starts_with("msg_"))
    );

    assert_eq!(
        crate::test_support::at(&body, "/content"),
        &serde_json::json!([
            {"type": "compaction", "content": "Native summary", "signature": "native-sig"},
            {"type": "thinking", "thinking": "think", "signature": "sig"},
            {"type": "text", "text": "foreign reasoning"},
            {"type": "text", "text": "Answer"},
            {"type": "tool_use", "id": "t1", "name": "find", "input": {"a": 2}},
            {"type": "tool_use", "id": "t2", "name": "greet", "input": {}}
        ])
    );

    assert_eq!(
        crate::test_support::at(&body, "/usage"),
        &serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 20,
            "cache_creation_input_tokens": 6,
            "cache_read_input_tokens": 4,
            "output_tokens_details": {"thinking_tokens": 8}
        })
    );

    Ok(())
}

/// Finish reasons map onto the canonical stop reasons, a missing finish degrades to
/// `end_turn`, and a compaction pause forces the `compaction` stop.
#[test]
fn stop_reasons_map_onto_the_canonical_table() -> Result<(), Box<dyn StdError>> {
    for (finish, expected) in [
        (Some(IrFinishReason::Stop), "end_turn"),
        (Some(IrFinishReason::Length), "max_tokens"),
        (Some(IrFinishReason::ToolCall), "tool_use"),
        (Some(IrFinishReason::ContentFilter), "refusal"),
        (None, "end_turn"),
    ] {
        let aggregation = IrAggregation {
            finish,
            ..IrAggregation::default()
        };

        let body = serde_json::to_value(super::render_json_response("m", &aggregation))?;

        assert_eq!(crate::test_support::at(&body, "/stop_reason"), expected);
    }

    let paused = IrAggregation {
        finish: Some(IrFinishReason::Stop),
        is_compaction_paused: true,
        ..IrAggregation::default()
    };

    let body = serde_json::to_value(super::render_json_response("m", &paused))?;

    assert_eq!(crate::test_support::at(&body, "/stop_reason"), "compaction");

    Ok(())
}

/// A compaction-only reply renders exactly its checkpoint block, the `compaction` stop reason,
/// and the zero top-level usage carrying the vendor iteration entries.
#[test]
fn compaction_only_response_shape() -> Result<(), Box<dyn StdError>> {
    let usage = super::CompactionOnlyUsage {
        iterations: vec![UsageIteration::from(serde_json::json!({
            "type": "compaction",
            "input_tokens": 100,
            "output_tokens": 50
        }))],
        input_tokens: 0,
        output_tokens: 0,
        cache_write_tokens: Some(2),
        cached_tokens: None,
    };

    let response =
        super::render_compaction_only_response("pub-model", gateway_block("Sum."), &usage);

    let body = serde_json::to_value(&response)?;

    assert_eq!(
        crate::test_support::at(&body, "/content"),
        &serde_json::json!([{
            "type": "compaction",
            "content": "Sum.",
            "signature": "janusgate.compaction.v1.primary.ABCD"
        }])
    );

    assert_eq!(crate::test_support::at(&body, "/stop_reason"), "compaction");

    assert_eq!(
        crate::test_support::at(&body, "/usage"),
        &serde_json::json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 2,
            "iterations": [{"type": "compaction", "input_tokens": 100, "output_tokens": 50}]
        })
    );

    Ok(())
}

/// A prefixed body renders the checkpoint blocks ahead of the derived content and closes the
/// rendered iterations with the message stage's entry: the aggregation's own compaction
/// iterations when it carried any, else the entry synthesized from its observed usage.
#[test]
fn prefixed_body_orders_checkpoints_and_iterations() -> Result<(), Box<dyn StdError>> {
    let message_usage = IrUsage {
        input_tokens: 5,
        output_tokens: 9,
        cached_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
    };

    let with_own = IrAggregation {
        text: "Answer".to_string(),
        finish: Some(IrFinishReason::Stop),
        usage: Some(message_usage),
        compaction_iterations: vec![UsageIteration::from(serde_json::json!({
            "type": "compaction",
            "output_tokens": 1
        }))],
        ..IrAggregation::default()
    };

    let response = super::render_json_response_with_prefix_blocks(
        "pub-model",
        &with_own,
        &[gateway_block("Sum.")],
        &[UsageIteration::from(serde_json::json!({"stage": "prefix"}))],
    );

    let body = serde_json::to_value(&response)?;

    assert_eq!(
        crate::test_support::at(&body, "/content"),
        &serde_json::json!([
            {"type": "compaction", "content": "Sum.",
             "signature": "janusgate.compaction.v1.primary.ABCD"},
            {"type": "text", "text": "Answer"}
        ])
    );

    assert_eq!(
        crate::test_support::at(&body, "/usage")["iterations"],
        serde_json::json!([
            {"stage": "prefix"},
            {"type": "compaction", "output_tokens": 1}
        ])
    );

    // Without the aggregation's own iterations, the observed usage becomes the message entry.
    let derived = IrAggregation {
        text: "Answer".to_string(),
        finish: Some(IrFinishReason::Stop),
        usage: Some(message_usage),
        ..IrAggregation::default()
    };

    let response = super::render_json_response_with_prefix_blocks(
        "pub-model",
        &derived,
        &[],
        &[UsageIteration::from(serde_json::json!({"stage": "prefix"}))],
    );

    let body = serde_json::to_value(&response)?;

    assert_eq!(
        crate::test_support::at(&body, "/usage")["iterations"],
        serde_json::json!([
            {"stage": "prefix"},
            {
                "type": "message",
                "input_tokens": 5,
                "output_tokens": 9
            }
        ])
    );

    // No prefix and no iterations is the ordinary response.
    let plain = super::render_json_response_with_prefix_blocks("pub-model", &derived, &[], &[]);

    let body = serde_json::to_value(&plain)?;

    assert!(
        crate::test_support::at(&body, "/usage")
            .get("iterations")
            .is_none()
    );

    assert_eq!(
        crate::test_support::at(&body, "/content"),
        &serde_json::json!([{"type": "text", "text": "Answer"}])
    );

    Ok(())
}

/// The buffered stream head folds the stream-report's input columns into the opening usage and
/// is emitted exactly once; the terminal delta later carries the final report.
#[test]
fn head_folds_the_stream_report() -> Result<(), Box<dyn StdError>> {
    let frames = rendered_frames(&[
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        },
        IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 0,
            cached_tokens: Some(3),
            cache_write_tokens: Some(4),
            reasoning_tokens: None,
        }),
        IrEvent::TextDelta("hi".to_string()),
        IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 7,
            cached_tokens: Some(3),
            cache_write_tokens: Some(4),
            reasoning_tokens: None,
        }),
        IrEvent::Done,
    ]);

    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 0), "/message/usage"),
        &serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 0,
            "cache_creation_input_tokens": 4,
            "cache_read_input_tokens": 3
        })
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 4), "/usage"),
        &serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 7,
            "cache_creation_input_tokens": 4,
            "cache_read_input_tokens": 3
        })
    );

    // A head resolved by anything other than the report opens with zero usage.
    let frames = rendered_frames(&[
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        },
        IrEvent::TextDelta("hi".to_string()),
    ]);

    assert_eq!(
        crate::test_support::at(payload(&frames, 0), "/message/usage"),
        &serde_json::json!({"input_tokens": 0, "output_tokens": 0})
    );

    Ok(())
}

/// Ordinary blocks ride their native lifecycles: a text block opens once and accumulates,
/// a reasoning block closes the open one at the next index, the signature streams as its own
/// delta, and the block reopens at a fresh index after closing.
#[test]
fn block_lifecycles_and_indices() -> Result<(), Box<dyn StdError>> {
    let frames = rendered_frames(&[
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        },
        IrEvent::TextDelta("a".to_string()),
        IrEvent::TextDelta("b".to_string()),
        IrEvent::ReasoningDelta("r".to_string()),
        IrEvent::ReasoningDelta("s".to_string()),
        IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("SIG".to_string()),
            opaque: Value::Null,
        },
        IrEvent::TextDelta("c".to_string()),
    ]);

    assert_eq!(
        frames
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
        ]
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 1), "/content_block"),
        &serde_json::json!({"type": "text", "text": ""})
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 2), "/delta/text"),
        "a"
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 3), "/delta/text"),
        "b"
    );

    assert_eq!(crate::test_support::at(payload(&frames, 4), "/index"), 0);

    assert_eq!(
        crate::test_support::at(payload(&frames, 5), "/content_block"),
        &serde_json::json!({"type": "thinking", "thinking": "", "signature": ""})
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 6), "/delta/thinking"),
        "r"
    );

    assert_eq!(crate::test_support::at(payload(&frames, 7), "/index"), 1);

    assert_eq!(
        crate::test_support::at(payload(&frames, 8), "/delta/signature"),
        "SIG"
    );

    assert_eq!(crate::test_support::at(payload(&frames, 9), "/index"), 1);

    assert_eq!(crate::test_support::at(payload(&frames, 10), "/index"), 2);

    assert_eq!(
        crate::test_support::at(payload(&frames, 11), "/delta/text"),
        "c"
    );

    Ok(())
}

/// A reasoning done carrying full text opens, delivers, signs, and closes its block in one
/// batch when no deltas preceded it; the stream had no head announcement, so no
/// `message_start` precedes the block.
#[test]
fn reasoning_done_with_full_text_renders_whole() {
    let frames = rendered_names(&[IrEvent::ReasoningDone {
        text: "T".to_string(),
        signature: Some("S".to_string()),
        opaque: Value::Null,
    }]);

    assert_eq!(
        frames,
        vec![
            "content_block_start".to_string(),
            "content_block_delta".to_string(),
            "content_block_delta".to_string(),
            "content_block_stop".to_string(),
        ]
    );
}

/// Tool deltas buffer client-side; the done event renders the whole call — identity in the
/// start frame, complete arguments in one delta — falling back to the streamed bytes when the
/// done carries none, and closing whatever block was open.
#[test]
fn tool_calls_render_whole_on_done() {
    let frames = rendered_frames(&[
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        },
        IrEvent::TextDelta("thinking out loud".to_string()),
        IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("t9".to_string()),
            name: Some("find".to_string()),
            arguments_delta: String::new(),
        },
        IrEvent::ToolCallDelta {
            index: 0,
            call_id: None,
            name: None,
            arguments_delta: "{\"q\":".to_string(),
        },
        IrEvent::ToolCallDelta {
            index: 0,
            call_id: None,
            name: None,
            arguments_delta: "1}".to_string(),
        },
        IrEvent::ToolCallDone {
            index: 0,
            call_id: "t9".to_string(),
            name: "find".to_string(),
            arguments: String::new(),
            thought_signature: None,
        },
        IrEvent::ToolCallDone {
            index: 1,
            call_id: "t2".to_string(),
            name: "greet".to_string(),
            arguments: "{\"name\":\"x\"}".to_string(),
            thought_signature: None,
        },
    ]);

    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
        ]
    );

    // The buffered fragments assemble when the done carries no arguments of its own.
    assert_eq!(
        crate::test_support::at(payload(&frames, 4), "/content_block"),
        &serde_json::json!({
            "type": "tool_use",
            "id": "t9",
            "name": "find",
            "input": {}
        })
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 5), "/delta/partial_json"),
        "{\"q\":1}"
    );

    // A done carrying complete arguments renders them instead of any buffered bytes.
    assert_eq!(
        crate::test_support::at(payload(&frames, 7), "/content_block"),
        &serde_json::json!({
            "type": "tool_use",
            "id": "t2",
            "name": "greet",
            "input": {}
        })
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 8), "/delta/partial_json"),
        "{\"name\":\"x\"}"
    );
}

/// The terminal frames ride Done, not Finish: the finish only closes open blocks, usage
/// arriving after the finish still lands in the terminal delta, a clean end without a finish
/// degrades to `end_turn`, and a stream without any report renders zero usage.
#[test]
fn terminal_frames_carry_stop_and_usage() {
    let frames = rendered_frames(&[
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        },
        IrEvent::TextDelta("hi".to_string()),
        IrEvent::Finish {
            reason: IrFinishReason::Length,
            prompt_block_reason: None,
        },
        IrEvent::Usage(IrUsage {
            input_tokens: 1,
            output_tokens: 2,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        }),
        IrEvent::Done,
    ]);

    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 4), "/delta"),
        &serde_json::json!({"stop_reason": "max_tokens", "stop_sequence": null})
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 4), "/usage"),
        &serde_json::json!({"input_tokens": 1, "output_tokens": 2})
    );

    // A missing finish degrades to end_turn; a report-less stream renders zero usage.
    let frames = rendered_frames(&[
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        },
        IrEvent::Done,
    ]);

    assert_eq!(
        crate::test_support::at(payload(&frames, 1), "/delta/stop_reason"),
        "end_turn"
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 1), "/usage"),
        &serde_json::json!({"input_tokens": 0, "output_tokens": 0})
    );
}

/// A compaction lifecycle's vendor iteration entries ride the terminal delta, closed by the
/// message stage's own entry synthesized from the observed usage.
#[test]
fn compaction_iterations_close_with_the_message_entry() -> Result<(), Box<dyn StdError>> {
    let mut renderer =
        super::SseRenderer::new("pub-model".to_string()).with_usage_iterations(vec![
            UsageIteration::from(serde_json::json!({
                "type": "compaction",
                "output_tokens": 1
            })),
        ]);

    let mut frames = rendered_batch(renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    }));

    assert!(frames.is_empty());

    frames.extend(rendered_batch(renderer.render(&IrEvent::Usage(IrUsage {
        input_tokens: 5,
        output_tokens: 9,
        cached_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
    }))));

    frames.extend(rendered_batch(renderer.render(&IrEvent::Done)));

    let delta = frames
        .iter()
        .find(|(name, _)| name == "message_delta")
        .map(|(_, payload)| payload);

    assert_eq!(
        crate::test_support::at(
            delta.ok_or("the terminal delta renders")?,
            "/usage/iterations"
        ),
        &serde_json::json!([
            {"type": "compaction", "output_tokens": 1},
            {"type": "message", "input_tokens": 5, "output_tokens": 9}
        ])
    );

    Ok(())
}

/// The compaction prefix drains right after the head: each block opens verbatim, a block with
/// readable content adds one full-content delta under the threshold framing, and the ordinary
/// blocks continue after the prefix indices with the forced stop reason at the terminal.
#[test]
fn compaction_prefix_frames() {
    let mut renderer = super::SseRenderer::with_compaction_prefix(
        "pub-model".to_string(),
        vec![
            gateway_block("Summary."),
            Checkpoint::Native(serde_json::json!({
                "type": "compaction",
                "signature": "native"
            })),
        ],
        Some("compaction".to_string()),
    );

    let mut frames = rendered_batch(renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    }));

    assert!(frames.is_empty(), "the head stays buffered one event");

    frames.extend(rendered_batch(
        renderer.render(&IrEvent::TextDelta("hi".to_string())),
    ));

    frames.extend(rendered_batch(renderer.render(&IrEvent::Done)));

    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );

    // The threshold start moves the summary into the delta, keeping the carrier signature.
    assert_eq!(
        crate::test_support::at(payload(&frames, 1), "/content_block"),
        &serde_json::json!({
            "type": "compaction",
            "content": null,
            "signature": "janusgate.compaction.v1.primary.ABCD"
        })
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 2), "/delta"),
        &serde_json::json!({"type": "compaction_delta", "content": "Summary."})
    );

    // The null-content native block delivers whole in its start frame, verbatim.
    assert_eq!(
        crate::test_support::at(payload(&frames, 4), "/content_block"),
        &serde_json::json!({"type": "compaction", "signature": "native"})
    );

    assert_eq!(crate::test_support::at(payload(&frames, 6), "/index"), 2);

    assert_eq!(
        crate::test_support::at(payload(&frames, 9), "/delta/stop_reason"),
        "compaction",
        "the forced stop reason replaces the finish-derived one"
    );
}

/// On-demand framing delivers the whole block in its start frame without a delta.
#[test]
fn on_demand_prefix_delivers_whole_blocks() {
    let mut renderer = super::SseRenderer::with_compaction_prefix(
        "pub-model".to_string(),
        vec![gateway_block("Summary.")],
        None,
    )
    .with_threshold_compaction(false);

    let mut frames = rendered_batch(renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    }));

    frames.extend(rendered_batch(renderer.render(&IrEvent::Done)));

    assert_eq!(
        crate::test_support::at(payload(&frames, 1), "/content_block"),
        &serde_json::json!({
            "type": "compaction",
            "content": "Summary.",
            "signature": "janusgate.compaction.v1.primary.ABCD"
        })
    );

    assert!(
        !frames.iter().any(|(name, _)| name == "content_block_delta"),
        "on-demand blocks carry no compaction delta"
    );

    assert_eq!(
        crate::test_support::at(payload(&frames, 3), "/delta/stop_reason"),
        "end_turn",
        "without a forced reason the finish-derived one stands"
    );
}

/// An upstream-observed compaction state block renders the prefix shape at the next index,
/// closing whatever block was open and delivering the whole vendor block in its start frame,
/// and registers its completed artifact beside the frames.
#[test]
fn compaction_state_event_renders_prefix_shape() {
    let mut renderer = super::SseRenderer::new("pub-model".to_string());

    let mut frames = rendered_batch(renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    }));

    assert!(frames.is_empty(), "the head stays buffered one event");

    frames.extend(rendered_batch(
        renderer.render(&IrEvent::TextDelta("before".to_string())),
    ));

    assert_eq!(
        frames
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta"
        ]
    );

    renderer.begin_projection(Projector::new());

    frames.extend(rendered_batch(renderer.render(&IrEvent::CompactionState {
        payload: native_block(),
    })));

    // The state block rides its start frame whole.
    assert_eq!(
        crate::test_support::at(payload(&frames, 4), "/content_block"),
        &native_block()
    );

    // A complete native compaction registers its artifact at its last rendered frame, valid
    // until the next render batch clears it.
    let artifact = renderer.take_completed_artifact();

    #[expect(
        clippy::expect_used,
        reason = "a complete native block registering its artifact is the behavior under test"
    )]
    let artifact = artifact.expect("a complete native block registers its artifact");

    assert_eq!(artifact.terminal_index, 2);

    assert!(!artifact.anchors.is_empty());

    frames.extend(rendered_batch(renderer.render(&IrEvent::Done)));

    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );

    // A no-op block is state but registers nothing.
    let mut frames = rendered_batch(renderer.render(&IrEvent::CompactionState {
        payload: serde_json::json!({"type": "compaction", "signature": "native"}),
    }));

    frames.extend(rendered_batch(renderer.render(&IrEvent::Done)));

    assert!(renderer.take_completed_artifact().is_none());
}

/// A failure while the `message_start` head is still buffered flushes the head ahead of the
/// terminal error frame — exactly once and first, in one body write.
#[test]
fn error_frame_flushes_the_buffered_head() {
    let mut renderer = super::SseRenderer::new("pub-model".to_string());

    let frames = renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    assert!(frames.is_empty());

    let batch = renderer.error_frame(&Error::MalformedRequest {
        param: None,
        message: "bad request".to_string(),
    });

    let parsed = parse_batch(&batch);

    let names: Vec<&str> = parsed.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(names, vec!["message_start", "error"]);

    assert_eq!(
        crate::test_support::at(payload(&parsed, 1), "/error"),
        &serde_json::json!({
            "type": "invalid_request_error",
            "message": "bad request"
        })
    );
}
