//! Detached JSON aggregation and SSE delivery own final accounting and accepted affinity.

use super::json_response;
use crate::{
    accounting::ApiLogOutcome,
    api::{Error, log_error},
    format::{ClientFormat, ClientSseRenderer},
    gateway::{IrEventStream, UsageTracker},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
    storage::api_logs::{self, ApiLogDraft},
};
use axum::{
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::Response,
};
use serde::Serialize;
use std::io::Error as IoError;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Renders one fully-buffered compaction artifact as the request's
/// non-streaming body, finalizing the row as the successful hand-off it is.
///
/// # Panics
///
/// Panics if the prerendered SSE response cannot build — impossible for a
/// static status, header, and string body.
pub(super) fn finalize_buffered<T: Serialize>(
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    body: &T,
) -> Response {
    // The body is complete before this call, so the row's outcome is the
    // successful hand-off; a client departing after the hand-off is the
    // documented residual edge (README §3 *ApiLog*).

    let response = json_response(axum::http::StatusCode::OK, body);

    api_logs::spawn_api_log_finalization(
        api_log_store,
        log,
        ApiLogOutcome::Completed,
        Some(i64::from(axum::http::StatusCode::OK.as_u16())),
        None,
    );

    response
}

/// Renders the pre-rendered SSE frames as the response body, finalizing the
/// row for the completed artifact.
///
/// # Panics
///
/// Panics if the static content-type header value fails to parse —
/// impossible for a fixed literal.
pub(super) fn prerendered_sse_response(
    frames: Vec<String>,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
) -> Response {
    let body = frames.concat();

    api_logs::spawn_api_log_finalization(
        api_log_store,
        log,
        ApiLogOutcome::Completed,
        Some(i64::from(axum::http::StatusCode::OK.as_u16())),
        None,
    );

    let mut response = axum::response::Response::new(axum::body::Body::from(body));

    *response.status_mut() = axum::http::StatusCode::OK;

    #[expect(
        clippy::expect_used,
        reason = "the static content-type value is a valid header value"
    )]
    let content_type: axum::http::HeaderValue = "text/event-stream"
        .parse()
        .expect("the static content-type parses");

    response
        .headers_mut()
        .insert(axum::http::header::CONTENT_TYPE, content_type);

    response
}

/// The checkpoint prefix a checkpoint-continue lifecycle renders ahead of its
/// live continuation: the executor's surface decides the variant, and each
/// variant carries exactly the pairing that surface's lifecycle produces — the
/// executor outcomes can never merge or confuse the two.
#[derive(Clone)]
pub(super) enum CheckpointPrefix {
    /// The Responses surface's checkpoint items; no iteration entries ride it.
    Responses(Vec<crate::format::compaction_wire::Checkpoint>),

    /// The Messages surface's checkpoint blocks beside the lifecycle's
    /// vendor-shaped iteration entries.
    Messages(
        Vec<crate::format::compaction_wire::Checkpoint>,
        Vec<crate::ir::compaction::UsageIteration>,
    ),
}

/// The execution coordinator's selected JSON rendering for one delivery.
#[derive(Clone)]
pub(super) enum AggregationRender {
    /// Preserves the format's ordinary rendering semantics.
    Ordinary,

    /// A prepared checkpoint prefix and the operation's reported iteration entries.
    WithPrefix {
        /// The single surface's checkpoint prefix selected by execution.
        prefix: CheckpointPrefix,
    },
}

/// Starts detached non-streaming delivery using the coordinator's selected ordinary or
/// checkpoint-prefixed JSON render. Its single task owns draining, affinity publication,
/// handoff, and final accounting independently of the handler's lifetime.
///
/// # Panics
///
/// Panics if the aggregation task never hands its outcome to the handler —
/// an invariant violation, since the task owns exactly one send.
pub(super) async fn detached_aggregation<TFormat>(
    events: IrEventStream,
    public_id: String,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    log_context: String,
    render: AggregationRender,
    affinity: Option<crate::affinity::Accepted>,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let (outcome_sender, outcome_receiver) = tokio::sync::oneshot::channel();

    let aggregation = tokio::spawn(aggregated_response_task::<TFormat>(
        events,
        public_id,
        api_log_store,
        log,
        log_context,
        outcome_sender,
        affinity,
        render,
    ));

    drop(aggregation);

    #[expect(
        clippy::expect_used,
        reason = "the aggregation task always hands its outcome to the handler exactly once; \
                  a missing hand-off is an invariant violation that fails fast"
    )]
    outcome_receiver
        .await
        .expect("the aggregation task always hands its outcome to the handler")
}

/// The detached non-streaming aggregation task (finalization point 2, README §3 *ApiLog*):
/// consumes the upstream event stream to completion, renders the aggregated body, and hands
/// `Ok(response)` or `Err(error)` to the handler through `outcome_sender` — the handler renders
/// it as the request's HTTP response when it is still there. The task owns the request's
/// accounting draft, so the row finalizes even when hyper dropped the handler with the departing
/// connection, and the hand-off attempt is the client-liveness signal:
///
/// - a successful stream finalizes `completed` + 200 + usage when the hand-off reaches the handler,
///   and `client_aborted` + `NULL` status when it does not — no response was ever produced for a
///   departed client;
/// - a failed stream finalizes the outcome and status the shared [`Error`] classification yields,
///   regardless of the client — the upstream failure, not the departure, is what the row records —
///   carrying whatever usage the stream delivered before it failed (`NULL` columns when it
///   delivered none, README §6.2's last-observation rule).
#[expect(
    clippy::too_many_arguments,
    reason = "one detached task owns stream, accounting, render selection, affinity, and handoff"
)]
pub(super) async fn aggregated_response_task<TFormat>(
    events: IrEventStream,
    public_id: String,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    log_context: String,
    outcome_sender: tokio::sync::oneshot::Sender<Result<Response, Error>>,
    mut affinity: Option<crate::affinity::Accepted>,
    render: AggregationRender,
) where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    let aggregation = aggregated_response(events, |aggregation| {
        let response = match &render {
            AggregationRender::Ordinary => TFormat::render_json_response(&public_id, aggregation),
            AggregationRender::WithPrefix { prefix } => {
                let (checkpoints, iterations): (
                    &[crate::format::compaction_wire::Checkpoint],
                    &[crate::ir::compaction::UsageIteration],
                ) = match prefix {
                    CheckpointPrefix::Responses(items) => (items, &[]),
                    CheckpointPrefix::Messages(blocks, iterations) => (blocks, iterations),
                };
                TFormat::render_json_response_with_prefix(
                    &public_id,
                    aggregation,
                    checkpoints,
                    iterations,
                )
            }
        };
        // Known handler departure creates no completed output. A later failed terminal handoff
        // leaves the already-created account hint harmless, without claiming client receipt.
        if !outcome_sender.is_closed()
            && let Some(affinity) = affinity.as_mut()
        {
            complete_accepted_json::<TFormat>(affinity, &response);
        }
        response
    })
    .await;

    match aggregation {
        Ok((response, usage)) => {
            if outcome_sender.send(Ok(response)).is_ok() {
                // The handler received the response: the client was still there.
                api_logs::finalize_api_log(
                    api_log_store.as_ref(),
                    &log,
                    ApiLogOutcome::Completed,
                    Some(i64::from(StatusCode::OK.as_u16())),
                    usage,
                )
                .await;
            } else {
                // The handler is gone — the client departed mid-generation — and no response was
                // ever produced for it.
                api_logs::finalize_api_log(
                    api_log_store.as_ref(),
                    &log,
                    ApiLogOutcome::ClientAborted,
                    None,
                    usage,
                )
                .await;
            }
        }

        Err((error, usage)) => {
            // An upstream failure records its classification regardless of the client, carrying
            // the usage the stream delivered before it failed — `None` when it delivered none.
            let (outcome, http_status) = error.accounting();

            api_logs::finalize_api_log(
                api_log_store.as_ref(),
                &log,
                outcome,
                Some(http_status),
                usage,
            )
            .await;

            // A failed hand-off returns the outcome: the handler is gone, so no response
            // boundary will render this failure and this task is its single logger (README §8).
            if let Err(Err(error)) = outcome_sender.send(Err(error)) {
                log_error(&log_context, &error);
            }
        }
    }
}

/// Starts the accepted request's outward codec projection: a checkpoint-continue
/// lifecycle omits its already rendered checkpoint prefix, an ordinary lifecycle
/// projects the complete accepted client-visible history.
fn begin_accepted_projection(
    accepted: &crate::affinity::Accepted,
    renderer: &mut dyn ClientSseRenderer,
) {
    if accepted.is_checkpoint() {
        renderer.begin_checkpoint_projection(accepted.continuation());
    } else {
        renderer.begin_projection(accepted.continuation());
    }
}

/// Registers a completed aggregated JSON rendering on the accepted affinity: the
/// format's completed artifacts publish at their own boundary, and the response's
/// projection publishes only when it is replayable and changed the accepted
/// request's retry identity — the JSON twin of the streaming path's
/// [`crate::affinity::Accepted::complete_projection`].
pub(super) fn complete_accepted_json<F: ClientFormat>(
    accepted: &mut crate::affinity::Accepted,
    response: &F::JsonResponse,
) {
    accepted.complete_artifacts();
    accepted.register_artifacts(&F::completed_json_artifacts(response));
    let before = accepted
        .projector()
        .evidence()
        .retry
        .map(|retry| retry.digest);
    let is_replayable = if accepted.is_checkpoint() {
        F::project_json_checkpoint(response, accepted.projector_mut())
    } else {
        F::project_json_response(response, accepted.projector_mut())
    };
    if is_replayable
        && before
            != accepted
                .projector()
                .evidence()
                .retry
                .map(|retry| retry.digest)
    {
        accepted.register_artifacts(&accepted.projector().completed_anchors());
    }
}

/// Capacity of the SSE body channel; upstream parsing is driven directly by its render task.
const SSE_BODY_CHANNEL_BUFFER: usize = 64;

/// Wraps an IR event stream into a live `text/event-stream` response, carrying the request's
/// accounting draft and api-log store handle into the render task: that task is the streaming
/// finalization point (README §3 *ApiLog*) and records exactly one row when the stream closes —
/// `completed` + 200 for a clean end, the classified outcome under the already-declared 200 for
/// an error frame (the first enqueued frame declares the status, so an error frame never changes
/// it), and `client_aborted` for a broken body channel — with 200 only once a frame had been
/// enqueued and `NULL` before the first. The recorded usage is whatever the stream delivered
/// before closure.
///
/// # Panics
///
/// Panics if the static response parts fail to build, which cannot occur.
pub(super) fn sse_response(
    events: IrEventStream,
    mut renderer: Box<dyn ClientSseRenderer + Send>,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    mut affinity: Option<crate::affinity::Accepted>,
) -> Response {
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Bytes, IoError>>(SSE_BODY_CHANNEL_BUFFER);

    tokio::spawn(async move {
        let mut events = events;
        // The codec receives only accepted client history; the task retains account ownership.
        if let Some(affinity) = affinity.as_ref() {
            begin_accepted_projection(affinity, renderer.as_mut());
        }

        // Whether any frame reached the client's channel; the first one declares HTTP 200.
        let mut is_status_declared = false;

        // The usage the stream reported, recorded best-effort at finalization: the iteration
        // totals override the ordinary usage ([`UsageTracker`]).
        let mut usage_tracker = UsageTracker::default();
        // A codec can render no additional terminal frame when usage is absent; EOF completes it.
        let mut has_completed_output = false;

        while let Some(item) = events.next().await {
            match item {
                Ok(event) => {
                    usage_tracker.observe(&event);

                    has_completed_output |= matches!(event, IrEvent::Done);
                    let frames = renderer.render(&event);
                    let completed_artifact = renderer.take_completed_artifact();
                    let terminal_index = if matches!(event, IrEvent::Done) {
                        frames.len().checked_sub(1)
                    } else {
                        None
                    };
                    let prefix_boundary = renderer.take_prefix_boundary();
                    let artifact_terminal_index = affinity
                        .as_ref()
                        .and_then(|accepted| accepted.prepared_prefix_boundary(prefix_boundary));
                    for (index, frame) in frames.into_iter().enumerate() {
                        if completed_artifact
                            .as_ref()
                            .is_some_and(|artifact| artifact.terminal_index == index)
                            && !sender.is_closed()
                            && let Some(affinity) = affinity.as_ref()
                            && let Some(artifact) = &completed_artifact
                        {
                            affinity.register_artifacts(&artifact.anchors);
                        }
                        // The validated native item or prepared single-block prefix is complete.
                        // Publish its digest immediately before that item's
                        // final emitted frame, after its earlier frames
                        // passed the channel. A later answer failure does
                        // not erase a completed released checkpoint.
                        if Some(index) == artifact_terminal_index
                            && !sender.is_closed()
                            && let Some(affinity) = affinity.as_mut()
                        {
                            affinity.complete_artifacts();
                        }
                        // Earlier completion-tail frames still flow normally. Only the actual
                        // terminal handoff publishes a fully replayable output; a later failed
                        // handoff leaves a harmless hint, never proof that the client received it.
                        if Some(index) == terminal_index
                            && !sender.is_closed()
                            && let Some(affinity) = affinity.as_mut()
                            && let Some(projector) = renderer.take_projection()
                        {
                            affinity.complete_projection(projector);
                        }
                        if sender.send(Ok(frame.into())).await.is_err() {
                            // The client's body channel is gone: a client abort, keyed only on
                            // what was actually enqueued before it broke.
                            api_logs::finalize_api_log(
                                api_log_store.as_ref(),
                                &log,
                                ApiLogOutcome::ClientAborted,
                                declared_status(is_status_declared),
                                usage_tracker.resolved(),
                            )
                            .await;

                            return;
                        }

                        is_status_declared = true;
                    }
                }

                Err(gateway_error) => {
                    let error = Error::from(gateway_error);
                    let (outcome, _) = error.accounting();

                    if sender
                        .send(Ok(renderer.error_frame(&error).into()))
                        .await
                        .is_err()
                    {
                        // The channel broke before the error frame could be enqueued: the client
                        // left, so the disconnect rules decide alone.
                        api_logs::finalize_api_log(
                            api_log_store.as_ref(),
                            &log,
                            ApiLogOutcome::ClientAborted,
                            declared_status(is_status_declared),
                            usage_tracker.resolved(),
                        )
                        .await;

                        return;
                    }

                    // The error frame never changes the status already declared; only the
                    // outcome moves to the classified failure (README §3 *ApiLog*).
                    api_logs::finalize_api_log(
                        api_log_store.as_ref(),
                        &log,
                        outcome,
                        Some(i64::from(StatusCode::OK.as_u16())),
                        usage_tracker.resolved(),
                    )
                    .await;

                    return;
                }
            }
        }

        if sender.is_closed() {
            api_logs::finalize_api_log(
                api_log_store.as_ref(),
                &log,
                ApiLogOutcome::ClientAborted,
                declared_status(is_status_declared),
                usage_tracker.resolved(),
            )
            .await;
            return;
        }
        if has_completed_output
            && let Some(affinity) = affinity.as_mut()
            && let Some(projector) = renderer.take_projection()
        {
            affinity.complete_projection(projector);
        }

        // A clean end of stream: every event rendered and the body closed normally.
        api_logs::finalize_api_log(
            api_log_store.as_ref(),
            &log,
            ApiLogOutcome::Completed,
            Some(i64::from(StatusCode::OK.as_u16())),
            usage_tracker.resolved(),
        )
        .await;
        drop(affinity);
    });

    #[expect(clippy::expect_used, reason = "SSE responses have known-valid parts")]
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .expect("static response parts are always valid")
}

/// The status recorded for a stream close: 200 once any frame — an error frame included — was
/// enqueued, because the first enqueued frame is what declares the response to the client; `NULL`
/// before the first, when no response ever reached it (README §3 *ApiLog*).
fn declared_status(is_status_declared: bool) -> Option<i64> {
    if is_status_declared {
        Some(i64::from(StatusCode::OK.as_u16()))
    } else {
        None
    }
}

/// Folds an IR event stream into one aggregated JSON response, yielding the response beside the
/// usage the stream delivered — the terminal usage of a clean end, and for a failed stream the
/// last usage observation it reported before failing (`None` when it delivered none), so a failed
/// non-streaming dispatch records the usage the upstream already delivered exactly as its
/// streaming twin does (README §3 *ApiLog*, §6.2's last-observation rule).
///
/// # Errors
///
/// Returns the stream's failure beside the usage observed before it.
async fn aggregated_response<TBody>(
    events: IrEventStream,
    render_json_fn: impl FnOnce(&IrAggregation) -> TBody,
) -> Result<(Response, Option<IrUsage>), (Error, Option<IrUsage>)>
where
    TBody: Serialize,
{
    let mut aggregator = Aggregator::default();

    // The usage the stream reported, kept beside either way the stream settles: a mid-stream
    // failure does not undo the usage the upstream already delivered, and the iteration totals
    // override the ordinary usage ([`UsageTracker`]).
    let mut usage_tracker = UsageTracker::default();

    let mut events = events;

    while let Some(item) = events.next().await {
        match item {
            Ok(event) => {
                usage_tracker.observe(&event);
                aggregator.ingest(&event);
            }

            Err(error) => return Err((Error::from(error), usage_tracker.resolved())),
        }
    }

    let aggregation = aggregator.finish();

    // The aggregator retains the same last-wins observations the tracker holds, so the resolved
    // stream usage is the aggregation's own resolved usage.
    let usage = usage_tracker.resolved();

    Ok((
        json_response(StatusCode::OK, &render_json_fn(&aggregation)),
        usage,
    ))
}
