//! Upstream response parsing tests for the Anthropic Messages codec: complete-body mapping,
//! streamed block lifecycles and deltas, usage accumulation, compaction capture, and tolerance
//! and failure semantics (`docs/reference/inference.md`).
#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section on every \
              test would document nothing its name does not"
)]

use crate::{
    format::UpstreamEventParser as _,
    ir::{IrEvent, IrFinishReason, IrUsage, compaction::UsageIteration},
    sse::SseFrame,
};
use serde_json::Value;

/// Builds one upstream SSE frame carrying `event` and the JSON payload.
fn frame(event: &str, payload: Value) -> SseFrame {
    SseFrame {
        data: payload.to_string(),
        event: Some(event.to_string()),
    }
}

/// A `message_start` frame with a full input-side usage report.
fn head_frame() -> SseFrame {
    frame(
        "message_start",
        serde_json::json!({"message": {"id": "up-1", "model": "claude-up", "usage": {
            "input_tokens": 10, "cache_read_input_tokens": 3, "cache_creation_input_tokens": 4
        }}}),
    )
}

/// A complete non-streaming upstream message body.
fn complete_body(stop_reason: &str) -> Vec<u8> {
    serde_json::json!({
        "id": "msg_up-1",
        "model": "claude-up",
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": 10, "output_tokens": 20,
            "cache_read_input_tokens": 4, "cache_creation_input_tokens": 6,
            "output_tokens_details": {"thinking_tokens": 8}
        },
        "content": [
            {"type": "text", "text": "Hi"},
            {"type": "text"},
            {"type": "thinking", "thinking": "why", "signature": "S"},
            {"type": "redacted_thinking", "data": "blob"},
            {"type": "tool_use", "id": "t1", "name": "find", "input": {"q": 1}},
            {"type": "server_tool_use", "anything": true}
        ]
    })
    .to_string()
    .into_bytes()
}

/// A complete body maps in canonical order: opening, per-block events, finish, then usage —
/// with the thinking blocks' opaque payloads carrying their signatures and the tool call
/// delivered as the canonical delta/done pair.
#[test]
fn complete_body_maps_in_canonical_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.parse_json_response(&complete_body("tool_use"))?,
        vec![
            IrEvent::MessageStart {
                id: "msg_up-1".to_string(),
                model: "claude-up".to_string(),
            },
            IrEvent::TextDelta("Hi".to_string()),
            IrEvent::ReasoningDone {
                text: "why".to_string(),
                signature: Some("S".to_string()),
                opaque: serde_json::json!({"type": "thinking", "thinking": "why", "signature": "S"}),
            },
            IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: serde_json::json!({"type": "redacted_thinking", "data": "blob"}),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("t1".to_string()),
                name: Some("find".to_string()),
                // The body path announces the whole call, so the delta already carries the
                // complete arguments.
                arguments_delta: "{\"q\":1}".to_string(),
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "t1".to_string(),
                name: "find".to_string(),
                arguments: "{\"q\":1}".to_string(),
                thought_signature: None,
            },
            IrEvent::finish(IrFinishReason::ToolCall),
            IrEvent::Usage(IrUsage {
                input_tokens: 10,
                output_tokens: 20,
                cached_tokens: Some(4),
                cache_write_tokens: Some(6),
                reasoning_tokens: Some(8),
            }),
        ]
    );

    Ok(())
}

/// Every upstream stop reason maps onto the canonical finish vocabulary, with unknown values
/// degrading to a clean stop rather than failing a healthy stream.
#[test]
fn stop_reasons_map_onto_the_canonical_vocabulary() -> Result<(), Box<dyn std::error::Error>> {
    for (stop_reason, expected) in [
        ("end_turn", IrFinishReason::Stop),
        ("stop_sequence", IrFinishReason::Stop),
        ("pause_turn", IrFinishReason::Stop),
        ("max_tokens", IrFinishReason::Length),
        ("model_context_window_exceeded", IrFinishReason::Length),
        ("tool_use", IrFinishReason::ToolCall),
        ("refusal", IrFinishReason::ContentFilter),
        ("something_new", IrFinishReason::Stop),
    ] {
        let body = serde_json::json!({
            "content": [],
            "stop_reason": stop_reason
        })
        .to_string()
        .into_bytes();

        let mut parser = super::UpstreamParser::new();
        let events = parser.parse_json_response(&body)?;

        assert!(
            events.contains(&IrEvent::finish(expected)),
            "`{stop_reason}` must finish as {expected:?}: {events:?}"
        );
    }

    Ok(())
}

/// The stream head publishes its input columns immediately with a zero output observation, and
/// exactly once per response; a head without a usage report publishes no usage.
#[test]
fn stream_head_publishes_partial_usage() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.ingest_frame(&head_frame())?,
        vec![
            IrEvent::MessageStart {
                id: "up-1".to_string(),
                model: "claude-up".to_string(),
            },
            IrEvent::Usage(IrUsage {
                input_tokens: 10,
                output_tokens: 0,
                cached_tokens: Some(3),
                cache_write_tokens: Some(4),
                reasoning_tokens: None,
            }),
        ]
    );

    // The head announcement is once-only, but a repeated head report re-publishes its usage:
    // the last observation wins.
    assert_eq!(
        parser.ingest_frame(&head_frame())?,
        vec![IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 0,
            cached_tokens: Some(3),
            cache_write_tokens: Some(4),
            reasoning_tokens: None,
        })]
    );

    let mut bare = super::UpstreamParser::new();

    assert_eq!(
        bare.ingest_frame(&frame(
            "message_start",
            serde_json::json!({"message": {"id": "u"}})
        ))?,
        vec![IrEvent::MessageStart {
            id: "u".to_string(),
            model: String::new(),
        }]
    );

    Ok(())
}

/// A streamed turn rides the block lifecycles: text deltas on their block, a thinking block
/// closing through the streamed signature in its opaque payload, and the terminal delta
/// carrying the cumulative usage beside the finish. Deltas routed to the wrong block kind or
/// an untracked index are ignored.
#[test]
fn streamed_blocks_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_start",
            serde_json::json!({"index": 0, "content_block": {"type": "text"}})
        ))?,
        vec![IrEvent::MessageStart {
            id: String::new(),
            model: String::new()
        }]
    );

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_delta",
            serde_json::json!({"index": 0, "delta": {"type": "text_delta", "text": "He"}})
        ))?,
        vec![IrEvent::TextDelta("He".to_string())]
    );

    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_delta",
                serde_json::json!({"index": 0, "delta": {"type": "text_delta", "text": ""}})
            ))?
            .is_empty(),
        "empty fragments publish no deltas"
    );

    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_start",
                serde_json::json!({"index": 1, "content_block": {"type": "thinking"}})
            ))?
            .is_empty()
    );

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_delta",
            serde_json::json!({"index": 1, "delta": {"type": "thinking_delta", "thinking": "th"}})
        ))?,
        vec![IrEvent::ReasoningDelta("th".to_string())]
    );

    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_delta",
                serde_json::json!({"index": 1, "delta": {"type": "signature_delta", "signature": "SIG"}})
            ))?
            .is_empty(),
        "the signature rides the closing event, not its own IR delta"
    );

    // A text fragment routed onto the thinking block, and one naming no tracked block, are
    // both ignored.
    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_delta",
                serde_json::json!({"index": 1, "delta": {"type": "text_delta", "text": "x"}})
            ))?
            .is_empty()
    );

    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_delta",
                serde_json::json!({"index": 99, "delta": {"type": "text_delta", "text": "x"}})
            ))?
            .is_empty()
    );

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_stop",
            serde_json::json!({"index": 1})
        ))?,
        vec![IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("SIG".to_string()),
            opaque: serde_json::json!({"type": "thinking", "thinking": "th", "signature": "SIG"}),
        }]
    );

    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_stop",
                serde_json::json!({"index": 0})
            ))?
            .is_empty()
    );

    assert_eq!(
        parser.ingest_frame(&frame(
            "message_delta",
            serde_json::json!({"delta": {"stop_reason": "tool_use"}, "usage": {
                "output_tokens": 7, "output_tokens_details": {"thinking_tokens": 3}
            }})
        ))?,
        vec![
            // The stream carried no head report, so only the output columns ride the terminal
            // observation.
            IrEvent::Usage(IrUsage {
                input_tokens: 0,
                output_tokens: 7,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: Some(3),
            }),
            IrEvent::finish(IrFinishReason::ToolCall),
        ]
    );

    assert!(
        parser
            .ingest_frame(&frame("message_stop", serde_json::json!({})))?
            .is_empty()
    );

    assert!(
        parser
            .ingest_frame(&frame("ping", serde_json::json!({})))?
            .is_empty()
    );

    assert!(parser.finish().is_empty());

    Ok(())
}

/// A streamed tool call reveals its identity at the block start and assembles its arguments
/// from the argument fragments, completing on the block stop.
#[test]
fn streamed_tool_calls_assemble() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_start",
            serde_json::json!({"index": 0, "content_block": {"type": "tool_use", "id": "t9", "name": "find"}})
        ))?,
        vec![
            // The stream opened at its first content block: the recovery opens the event
            // stream with the empty identity.
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("t9".to_string()),
                name: Some("find".to_string()),
                arguments_delta: String::new(),
            },
        ]
    );

    for fragment in ["{\"a\":", "1}"] {
        assert_eq!(
            parser.ingest_frame(&frame(
                "content_block_delta",
                serde_json::json!({"index": 0, "delta": {"type": "input_json_delta", "partial_json": fragment}})
            ))?,
            vec![IrEvent::ToolCallDelta {
                index: 0,
                call_id: None,
                name: None,
                arguments_delta: fragment.to_string(),
            }]
        );
    }

    assert!(parser
        .ingest_frame(&frame(
            "content_block_delta",
            serde_json::json!({"index": 0, "delta": {"type": "input_json_delta", "partial_json": ""}})
        ))?
        .is_empty());

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_stop",
            serde_json::json!({"index": 0})
        ))?,
        vec![IrEvent::ToolCallDone {
            index: 0,
            call_id: "t9".to_string(),
            name: "find".to_string(),
            arguments: "{\"a\":1}".to_string(),
            thought_signature: None,
        }]
    );

    Ok(())
}

/// A clean end-of-stream flushes every still-open block in content order, publishes stored
/// usage columns that never rode a report, and degrades the missing finish to a clean stop —
/// while a pending compaction block without its stop stays withheld.
#[test]
fn clean_end_flushes_open_blocks() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    // A head report carrying only the output breakdown opens the stream but publishes no
    // immediate usage event.
    assert_eq!(
        parser.ingest_frame(&frame(
            "message_start",
            serde_json::json!({"message": {"usage": {"output_tokens_details": {"thinking_tokens": 5}}}})
        ))?,
        vec![IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        }]
    );

    // Open three blocks and close none; the tracked order scrambles on purpose.
    for (index, block) in [
        (0, serde_json::json!({"type": "text"})),
        (2, serde_json::json!({"type": "thinking"})),
        (1, serde_json::json!({"type": "thinking"})),
    ] {
        assert!(
            parser
                .ingest_frame(&frame(
                    "content_block_start",
                    serde_json::json!({"index": index, "content_block": block})
                ))?
                .is_empty()
        );
    }

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_delta",
            serde_json::json!({"index": 2, "delta": {"type": "thinking_delta", "thinking": "T"}})
        ))?,
        vec![IrEvent::ReasoningDelta("T".to_string())]
    );

    // A compaction block whose stop never arrives stays withheld.
    assert!(parser
        .ingest_frame(&frame(
            "content_block_start",
            serde_json::json!({"index": 3, "content_block": {"type": "compaction", "signature": "s"}})
        ))?
        .is_empty());

    let events = parser.finish();

    assert_eq!(
        events,
        vec![
            IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: serde_json::json!({"type": "thinking", "thinking": ""}),
            },
            IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: serde_json::json!({"type": "thinking", "thinking": "T"}),
            },
            IrEvent::Usage(IrUsage {
                input_tokens: 0,
                output_tokens: 0,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: Some(5),
            }),
            IrEvent::finish(IrFinishReason::Stop),
        ]
    );

    assert!(parser.compaction_blocks.is_empty());

    Ok(())
}

/// A mid-stream `error` event aborts the stream carrying its reported detail, with the fixed
/// detail when the event carries none.
#[test]
fn error_events_abort_the_stream() {
    let mut parser = super::UpstreamParser::new();

    #[expect(
        clippy::expect_used,
        reason = "the aborting frame is the behavior under test"
    )]
    let detailed = parser
        .ingest_frame(&frame(
            "error",
            serde_json::json!({"error": {"type": "overloaded_error", "message": "overloaded"}}),
        ))
        .expect_err("an error event aborts the stream");

    assert!(detailed.to_string().contains("overloaded"));

    #[expect(
        clippy::expect_used,
        reason = "the aborting frame is the behavior under test"
    )]
    let bare = parser
        .ingest_frame(&frame("error", serde_json::json!({})))
        .expect_err("a bare error event aborts too");

    assert!(bare.to_string().contains("upstream error event"));
}

/// Unknown event names, block kinds, and delta kinds are tolerated and ignored, so newer
/// upstream surface members never break parsing.
#[test]
fn unknown_shapes_are_tolerated() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert!(
        parser
            .ingest_frame(&frame("blob_update", serde_json::json!({"anything": true})))?
            .is_empty()
    );

    // The first content-bearing frame opens the event stream with the empty identity even when
    // the block kind itself is unknown.
    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_start",
            serde_json::json!({"index": 0, "content_block": {"type": "server_tool_use", "id": "x"}})
        ))?,
        vec![IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        }]
    );

    assert!(parser
        .ingest_frame(&frame(
            "content_block_delta",
            serde_json::json!({"index": 0, "delta": {"type": "citations_delta", "citation": {}}})
        ))?
        .is_empty());

    assert!(
        parser
            .ingest_frame(&frame(
                "content_block_delta",
                serde_json::json!({"index": 0})
            ))?
            .is_empty()
    );

    Ok(())
}

/// Consumed fields of the wrong JSON kind are malformed payloads, failing the frame or body.
#[test]
fn wrong_shapes_fail_as_malformed() {
    for (event, payload) in [
        (
            "content_block_start",
            serde_json::json!({"index": 0, "content_block": {"id": "x"}}),
        ),
        (
            "content_block_start",
            serde_json::json!({"index": 0, "content_block": {"type": "redacted_thinking", "data": 5}}),
        ),
        ("content_block_start", serde_json::json!({"index": 0})),
    ] {
        let mut parser = super::UpstreamParser::new();

        assert!(
            parser.ingest_frame(&frame(event, payload)).is_err(),
            "a malformed {event} frame must fail"
        );
    }

    for body in [
        serde_json::json!({"content": [{"type": "tool_use", "id": "t", "name": "n", "input": 5}]}),
        serde_json::json!({"content": [{"type": "compaction", "content": 5}]}),
        serde_json::json!({"content": [{"type": "redacted_thinking", "data": {"blob": true}}]}),
        serde_json::json!({"content": "not-an-array"}),
    ] {
        let mut parser = super::UpstreamParser::new();

        assert!(
            parser
                .parse_json_response(&body.to_string().into_bytes())
                .is_err(),
            "a malformed body must fail: {body}"
        );
    }
}

/// A streamed threshold compaction block completes through its summary delta and stop, and a
/// block opening complete is state immediately; the compaction stop reason and iteration
/// reports ride beside the canonical finish as metadata.
#[test]
fn streamed_compaction_blocks_capture_state() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_start",
            serde_json::json!({"index": 2, "content_block": {"type": "compaction", "content": null,
            "signature": "sig-x"}})
        ))?,
        vec![IrEvent::MessageStart {
            id: String::new(),
            model: String::new()
        }]
    );

    assert!(parser.compaction_blocks.is_empty());

    assert!(parser
        .ingest_frame(&frame(
            "content_block_delta",
            serde_json::json!({"index": 2, "delta": {"type": "compaction_delta", "content": "SUM"}})
        ))?
        .is_empty());

    // The summary delta replaces the opening null content in the captured block.
    let completed = serde_json::json!({"type": "compaction", "content": "SUM",
        "signature": "sig-x"});

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_stop",
            serde_json::json!({"index": 2})
        ))?,
        vec![IrEvent::CompactionState { payload: completed }]
    );

    let complete_block = serde_json::json!({"type": "compaction", "content": "FULL", "signature": "sig-y",
        "encrypted_content": "enc"});

    assert_eq!(
        parser.ingest_frame(&frame(
            "content_block_start",
            serde_json::json!({"index": 3, "content_block": complete_block})
        ))?,
        vec![IrEvent::CompactionState {
            payload: complete_block,
        }]
    );

    assert_eq!(
        parser.ingest_frame(&frame(
            "message_delta",
            serde_json::json!({"delta": {"stop_reason": "compaction"}, "usage": {
                "output_tokens": 3, "iterations": [{"type": "compaction", "output_tokens": 2}]
            }})
        ))?,
        vec![
            IrEvent::Usage(IrUsage {
                input_tokens: 0,
                output_tokens: 3,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
            IrEvent::finish(IrFinishReason::Stop),
            IrEvent::CompactionMetadata {
                is_paused: true,
                iterations: vec![UsageIteration::from(serde_json::json!({
                    "type": "compaction",
                    "output_tokens": 2
                }))],
            },
        ]
    );

    assert_eq!(parser.compaction_blocks.len(), 2);

    assert!(parser.finish().is_empty());

    Ok(())
}

/// The head's `usage.iterations` array publishes the compaction metadata before any content,
/// and the last observed array wins.
#[test]
fn head_iterations_publish_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.ingest_frame(&frame(
            "message_start",
            serde_json::json!({"message": {"usage": {"iterations": [{"stage": 1}]}}})
        ))?,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::CompactionMetadata {
                is_paused: false,
                iterations: vec![UsageIteration::from(serde_json::json!({"stage": 1}))],
            },
        ]
    );

    assert_eq!(
        parser.ingest_frame(&frame(
            "message_delta",
            serde_json::json!({"usage": {"iterations": [{"stage": 2}]}})
        ))?,
        vec![
            IrEvent::Usage(IrUsage::default()),
            IrEvent::CompactionMetadata {
                is_paused: false,
                iterations: vec![UsageIteration::from(serde_json::json!({"stage": 2}))],
            },
        ]
    );

    assert_eq!(
        parser.compaction_iterations,
        vec![UsageIteration::from(serde_json::json!({"stage": 2}))]
    );

    Ok(())
}

/// A complete compaction body captures the block verbatim, and the compaction stop reason
/// keeps the canonical stop finish beside its metadata.
#[test]
fn complete_body_compaction_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let block = serde_json::json!({"type": "compaction", "content": "S", "signature": "sig"});
    let body = serde_json::json!({
        "stop_reason": "compaction",
        "usage": {"input_tokens": 1, "output_tokens": 2},
        "content": [block]
    })
    .to_string()
    .into_bytes();

    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.parse_json_response(&body)?,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::CompactionState { payload: block },
            IrEvent::finish(IrFinishReason::Stop),
            IrEvent::Usage(IrUsage {
                input_tokens: 1,
                output_tokens: 2,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
            IrEvent::CompactionMetadata {
                is_paused: true,
                iterations: Vec::new(),
            },
        ]
    );

    assert_eq!(parser.compaction_blocks.len(), 1);

    Ok(())
}

/// A null-content compaction block in a complete body is the native no-op in its final form:
/// every body block is complete, so it captures as state without a pending lifecycle.
#[test]
fn complete_body_captures_null_content_compaction_as_no_op()
-> Result<(), Box<dyn std::error::Error>> {
    let block = serde_json::json!({"type": "compaction", "content": null, "signature": "sig"});
    let body = serde_json::json!({"content": [block]})
        .to_string()
        .into_bytes();

    let mut parser = super::UpstreamParser::new();

    assert_eq!(
        parser.parse_json_response(&body)?,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::CompactionState { payload: block },
            IrEvent::finish(IrFinishReason::Stop),
        ]
    );

    assert_eq!(parser.compaction_blocks.len(), 1);

    Ok(())
}

/// `tool_use` input absent or null is the accepted empty-arguments form.
#[test]
fn absent_tool_input_is_empty_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let body = serde_json::json!({
        "content": [
            {"type": "tool_use", "id": "t1", "name": "find"},
            {"type": "tool_use", "id": "t2", "name": "greet", "input": null}
        ]
    })
    .to_string()
    .into_bytes();

    let mut parser = super::UpstreamParser::new();
    let events = parser.parse_json_response(&body)?;

    assert_eq!(
        events,
        vec![
            IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("t1".to_string()),
                name: Some("find".to_string()),
                arguments_delta: String::new(),
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "t1".to_string(),
                name: "find".to_string(),
                arguments: String::new(),
                thought_signature: None,
            },
            IrEvent::ToolCallDelta {
                index: 1,
                call_id: Some("t2".to_string()),
                name: Some("greet".to_string()),
                arguments_delta: String::new(),
            },
            IrEvent::ToolCallDone {
                index: 1,
                call_id: "t2".to_string(),
                name: "greet".to_string(),
                arguments: String::new(),
                thought_signature: None,
            },
            IrEvent::finish(IrFinishReason::Stop),
        ]
    );

    Ok(())
}
