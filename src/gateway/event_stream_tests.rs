//! Lazy upstream parsing, clean flushing, and terminal failure boundaries.

use super::{Error, IrEvent, UpstreamFormat, json_event_stream, sse_event_stream, upstream_parser};
use axum::body::Bytes;
use futures::{StreamExt as _, stream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// One wrapped chat delta, optionally terminated by the SSE dispatch delimiter.
fn delta_frame(text: &str, has_delimiter: bool) -> String {
    let payload = serde_json::json!({"payload": {
        "id": "upstream", "model": "model", "choices": [{"index": 0, "delta": {"content": text}}]
    }});
    format!("data: {payload}{}", if has_delimiter { "\n\n" } else { "" })
}

/// Parsing starts on demand, drains each frame before reading another chunk, and flushes a
/// split trailing frame before the codec's degraded finish and the single terminal Done.
///
/// # Panics
///
/// Panics if lazy reading, frame ordering, envelope unwrapping, or clean flushing diverges.
#[tokio::test]
async fn reads_sse_on_demand_and_flushes_the_clean_tail() {
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&reads);
    let first = format!(
        "{}{}",
        delta_frame("first", true),
        delta_frame("second", true)
    );
    let tail = delta_frame("tail", false);
    let (head, tail) = tail.split_at(tail.len() / 2);
    let body = stream::iter(
        [first, head.to_string(), tail.to_string()].map(|chunk| Ok(Bytes::from(chunk))),
    )
    .inspect(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    });
    let mut events = sse_event_stream(
        body,
        upstream_parser(UpstreamFormat::OpenAiChatCompletion),
        "fixture".into(),
        false,
        Some("payload".into()),
    );
    tokio::task::yield_now().await;
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.next().await,
        Some(Ok(IrEvent::MessageStart { .. }))
    ));
    for text in ["first", "second"] {
        assert!(
            matches!(events.next().await, Some(Ok(IrEvent::TextDelta(delta))) if delta == text)
        );
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "pending frames never prefetch another chunk"
        );
    }
    assert!(matches!(events.next().await, Some(Ok(IrEvent::TextDelta(delta))) if delta == "tail"));
    assert!(matches!(
        events.next().await,
        Some(Ok(IrEvent::Finish { .. }))
    ));
    assert!(matches!(events.next().await, Some(Ok(IrEvent::Done))));
    assert!(events.next().await.is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 3);
}

/// Malformed frames, framing overflow, and transport failure end at error then Done, without
/// parsing later frames, reading later chunks, or flushing a degraded finish after failure.
///
/// # Panics
///
/// Panics if a failure emits subsequent payload or codec-flush events.
#[tokio::test]
async fn sse_failures_stop_parsing_and_skip_the_clean_flush() {
    let malformed = "data: {\n\n";
    for failure in [
        Ok(Bytes::from(format!(
            "{malformed}{}",
            delta_frame("discarded", true)
        ))),
        Ok(Bytes::from_static(b"data: {")),
        Ok(Bytes::from(vec![b'x'; 16 * 1024 * 1024 + 1])),
        Err(Error::Unavailable {
            detail: "fixture transport failure".into(),
        }),
    ] {
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reads);
        let has_trailing_failure = matches!(&failure, Ok(bytes) if bytes.as_ref() == b"data: {");
        let mut chunks = vec![Ok(Bytes::from(delta_frame("accepted", true))), failure];
        if !has_trailing_failure {
            chunks.push(Ok(Bytes::from(delta_frame("unread", true))));
        }
        let body = stream::iter(chunks).inspect(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        });
        let events = sse_event_stream(
            body,
            upstream_parser(UpstreamFormat::OpenAiChatCompletion),
            "fixture".into(),
            false,
            Some("payload".into()),
        )
        .collect::<Vec<_>>()
        .await;
        assert_eq!(events.len(), 4);
        let mut events = events.into_iter();
        assert!(matches!(
            events.next(),
            Some(Ok(IrEvent::MessageStart { .. }))
        ));
        assert!(
            matches!(events.next(), Some(Ok(IrEvent::TextDelta(delta))) if delta == "accepted")
        );
        assert!(matches!(
            events.next(),
            Some(Err(Error::Unavailable { .. }))
        ));
        assert!(matches!(events.next(), Some(Ok(IrEvent::Done))));
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }
}

/// A discarded lazy stream owns and drops its unread body instead of leaving a producer running.
///
/// # Panics
///
/// Panics if dropping the parsed stream retains or reads the upstream body.
#[tokio::test]
async fn dropping_sse_releases_the_unread_body() {
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&reads);
    let body_owner = Arc::new(());
    let retained = Arc::clone(&body_owner);
    let body = stream::once(async move {
        observed.fetch_add(1, Ordering::SeqCst);
        drop(retained);
        Ok(Bytes::from(delta_frame("unread", true)))
    });
    let events = sse_event_stream(
        body,
        upstream_parser(UpstreamFormat::OpenAiChatCompletion),
        "fixture".into(),
        false,
        Some("payload".into()),
    );
    drop(events);
    tokio::task::yield_now().await;
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(Arc::strong_count(&body_owner), 1);
}

/// JSON body reads are deferred with the parser, and every success or read/decoding/codec
/// failure ends in exactly one Done without inventing successful output for malformed bodies.
///
/// # Panics
///
/// Panics if lazy JSON ingestion, unwrapping, or terminal event ordering diverges.
#[tokio::test]
async fn reads_json_on_demand_and_terminates_success_and_failure() {
    let valid = serde_json::json!({"payload": {
        "id": "upstream", "model": "model", "choices": [{"message": {"content": "answer"}}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 7}
    }});
    for (body, is_valid) in [
        (Ok(Bytes::from(valid.to_string())), true),
        (Ok(Bytes::from_static(b"{")), false),
        (
            Ok(Bytes::from_static(
                b"{\"payload\":{\"choices\":[{\"message\":{\"content\":7}}]}}",
            )),
            false,
        ),
        (
            Err(Error::Unavailable {
                detail: "fixture read failure".into(),
            }),
            false,
        ),
    ] {
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reads);
        let body = async move {
            observed.fetch_add(1, Ordering::SeqCst);
            body
        };
        let events = json_event_stream(
            body,
            upstream_parser(UpstreamFormat::OpenAiChatCompletion),
            Some("payload".into()),
        );
        tokio::task::yield_now().await;
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        let mut events = events.collect::<Vec<_>>().await.into_iter();
        assert!(matches!(events.next_back(), Some(Ok(IrEvent::Done))));
        if is_valid {
            assert!(matches!(
                events.next(),
                Some(Ok(IrEvent::MessageStart { .. }))
            ));
            assert!(
                matches!(events.next(), Some(Ok(IrEvent::TextDelta(delta))) if delta == "answer")
            );
            assert!(matches!(events.next(), Some(Ok(IrEvent::Finish { .. }))));
            let usage = match events.next() {
                Some(Ok(IrEvent::Usage(usage))) => usage,
                #[expect(clippy::panic, reason = "the valid fixture reports usage")]
                _ => panic!("the JSON parser must preserve reported usage"),
            };
            assert_eq!((usage.input_tokens, usage.output_tokens), (5, 7));
        } else {
            assert!(matches!(
                events.next(),
                Some(Err(Error::Unavailable { .. }))
            ));
        }
        assert!(events.next().is_none());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }
}
