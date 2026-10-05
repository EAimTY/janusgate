//! Upstream body parsing driven directly by the owning response task.

use super::{Error, IrEventStream, unwrap_frame, unwrap_response};
use crate::{
    format::UpstreamEventParser,
    ir::IrEvent,
    sse::{SseFrame, SseParser},
};
use axum::body::Bytes;
use futures::{Stream, StreamExt as _, stream};
use std::{future::Future, pin::Pin, vec};

/// SSE framing and codec state, retained only while its consumer polls the event stream.
struct SseEventState<TBody> {
    /// Present until clean EOF; dropping the state releases an unread upstream body.
    body: Option<Pin<Box<TBody>>>,
    /// The selected upstream wire codec.
    parser: Box<dyn UpstreamEventParser + Send>,
    /// Incremental framing across body chunks.
    framing: SseParser,
    /// Frames from the latest chunk, drained before another chunk is read.
    frames: vec::IntoIter<SseFrame>,
    /// Whether clean flushing or an error already emitted the terminal batch.
    has_finished: bool,
    /// Optional provider-authored response envelope member.
    response_unwrap: Option<String>,
}

impl<TBody> SseEventState<TBody>
where
    TBody: Stream<Item = Result<Bytes, Error>>,
{
    /// Reads only until one frame produces events, or the stream terminates.
    ///
    /// # Errors
    ///
    /// Returns transport, framing, or codec failures without flushing codec state after them.
    async fn next_events(&mut self) -> Result<Option<Vec<IrEvent>>, Error> {
        if self.has_finished {
            return Ok(None);
        }
        loop {
            if let Some(mut frame) = self.frames.next() {
                unwrap_frame(&mut frame, &self.response_unwrap);
                let events = self.parser.ingest_frame(&frame)?;
                if !events.is_empty() {
                    return Ok(Some(events));
                }
                continue;
            }

            let Some(body) = self.body.as_mut() else {
                // EOF's trailing frames were drained first. Only this clean path flushes the
                // codec, including a degraded finish when no upstream terminal was reported.
                self.has_finished = true;
                let mut events = self.parser.finish();
                events.push(IrEvent::Done);
                return Ok(Some(events));
            };
            let frames = match body.next().await {
                Some(chunk) => self.framing.ingest(&chunk?),
                None => {
                    self.body = None;
                    self.framing.finish()
                }
            }
            .map_err(|_| Error::Unavailable {
                detail: "oversized SSE frame".into(),
            })?;
            self.frames = frames.into_iter();
        }
    }
}

/// Parses SSE on demand, draining each frame's events before parsing the next frame.
/// A failure yields error then Done and never polls or flushes the upstream again. Failures
/// log here only for streaming clients; non-streaming failures log at the response boundary.
pub(super) fn sse_event_stream(
    body: impl Stream<Item = Result<Bytes, Error>> + Send + 'static,
    parser: Box<dyn UpstreamEventParser + Send>,
    upstream_label: String,
    is_client_streaming: bool,
    response_unwrap: Option<String>,
) -> IrEventStream {
    let state = SseEventState {
        body: Some(Box::pin(body)),
        parser,
        framing: SseParser::new(),
        frames: Vec::new().into_iter(),
        has_finished: false,
        response_unwrap,
    };
    let events = stream::unfold(state, |mut state| async move {
        let events = match state.next_events().await {
            Ok(Some(events)) => events.into_iter().map(Ok).collect(),
            Ok(None) => return None,
            Err(error) => {
                state.has_finished = true;
                state.body = None;
                vec![Err(error), Ok(IrEvent::Done)]
            }
        };
        Some((events, state))
    })
    .flat_map(stream::iter)
    .inspect(move |event| {
        if is_client_streaming && let Err(error) = event {
            log::error!("{upstream_label}: {error}");
        }
    });
    Box::pin(events)
}

/// Reads and parses one JSON body only when its response task requests events.
/// Every body or codec failure ends at error then Done; the non-streaming response boundary
/// owns error logging. Success parses the whole document before yielding its ordered events.
pub(super) fn json_event_stream(
    body: impl Future<Output = Result<Bytes, Error>> + Send + 'static,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    response_unwrap: Option<String>,
) -> IrEventStream {
    let events = stream::once(async move {
        let outcome = async {
            let bytes = body.await?;
            let body = unwrap_response(&bytes, &response_unwrap)?;
            parser.parse_json_response(body).map_err(Error::from)
        }
        .await;
        let mut events: Vec<_> = match outcome {
            Ok(events) => events.into_iter().map(Ok).collect(),
            Err(error) => vec![Err(error)],
        };
        events.push(Ok(IrEvent::Done));
        events
    })
    .flat_map(stream::iter);
    Box::pin(events)
}
