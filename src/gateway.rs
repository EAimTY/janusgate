//! Gateway execution pipeline: routing resolution, token acquisition, upstream dispatch, and
//! translation into IR event streams.

use super::{
    context::Context,
    format::{PayloadError, UpstreamEventParser, UpstreamFormat, anthropic, gemini, openai},
    http_body, ids,
    ir::{Aggregator, InvalidRequest, IrAggregation, IrEvent, IrRequest},
    provider::{
        Error as ProviderError, ModelRouting, Provider,
        definition::{
            eval::ValueEnv,
            inference::{Inference, UpstreamStreaming},
            template::{ParamRow, StringExpr, TemplateNode},
        },
        oauth,
    },
    sse::{SseFrame, SseParser},
    storage::{self, Error as StorageError, api_logs::ApiLogDraft},
};
use crate::compaction::accounting::{CompactionLedger, IterationKind};
#[cfg(test)]
use crate::database::Database;
use futures::{Stream, StreamExt as _};
use log::debug;
use reqwest::{Client, StatusCode};
use std::pin::Pin;

mod event_stream;
mod response_body;
use event_stream::{json_event_stream, sse_event_stream};

mod attempt;
pub(crate) use attempt::{AttemptOutcome, execute_attempt};
mod selection;
pub(crate) use selection::{CompactionSelection, execute_selection};

#[cfg(test)]
mod attempt_tests;
#[cfg(test)]
mod event_stream_tests;
#[cfg(test)]
mod selection_tests;

/// Outward completion pipeline failures, from routing resolution through upstream dispatch. The
/// client request can fail validation here too — the upstream renderers reject requests the chosen
/// upstream cannot carry — so client faults travel in the same type and the surface classifies
/// them.
///
/// The Displays carry the complete human message the outward surface renders; the surface only maps
/// each variant onto its status, `type`, `code`, and `param`.
///
/// Failure variants do not imply dispatch timing: rejection evidence persistence can fail after
/// headers, while payload and renewal failures occur before dispatch. The account draft's typed
/// progress and dispatch stamp determine commitment and recording. Quota classifications remain
/// typed until the routing or compaction coordinator has decided whether a fallback is permitted;
/// conversion into [`crate::api::Error`] belongs to the final outward boundary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The client's request was rejected while preparing the upstream call.
    #[error(transparent)]
    InvalidRequest(#[from] InvalidRequest),

    /// The requested model is unknown or disabled.
    #[error("The model `{model}` does not exist or you do not have access to it.")]
    ModelNotFound {
        /// Public model identifier the client requested.
        model: String,
    },

    /// An enabled pool route has no account with an enabled exact physical model.
    #[error("The model `{model}` has no available account in its pool.")]
    PoolUnavailable {
        /// Requested public model name.
        model: String,
    },

    /// Every structural candidate was blocked or positively rejected for quota.
    #[error("The model `{model}` has exhausted quota on every account in its pool.")]
    PoolQuotaExhausted {
        /// Requested public alias, never the foreign error body.
        model: String,
        /// Positive whole seconds to the earliest completely known candidate reset.
        retry_after: Option<std::num::NonZeroU64>,
    },

    /// The established account cannot serve the requested public model in this pool.
    #[error("The model `{model}` cannot use its required account.")]
    BoundAccountUnavailable {
        /// Requested public model alias.
        model: String,
    },

    /// The established account has an applicable quota restriction; switching is forbidden.
    #[error("The model `{model}` has exhausted quota on its required account.")]
    BoundAccountQuotaExhausted {
        /// Requested public model alias.
        model: String,
        /// Positive whole seconds until every applicable restriction has a known future reset.
        retry_after: Option<std::num::NonZeroU64>,
    },

    /// Active authoritative/artifact routing evidence conflicts with a single live binding.
    #[error("The model `{model}` has conflicting routing requirements.")]
    RoutingStateConflict {
        /// Requested public model alias.
        model: String,
    },

    /// The upstream answered a non-2xx status, which the outward surface mirrors verbatim. The
    /// status is carried as a [`StatusCode`] taken verbatim from the upstream response, so an
    /// invalid status is unrepresentable and the mirror needs no conversion.
    #[error("Upstream request failed: {detail}")]
    Upstream {
        /// Upstream status code, mirrored by the surface.
        status: StatusCode,

        /// Truncated upstream body excerpt.
        detail: String,
    },

    /// A rejected data-plane response positively matched its authored quota policy.
    #[error("Upstream request failed: {detail}")]
    QuotaRejected {
        /// Actual rejected upstream header status, mirrored by direct routes.
        status: StatusCode,
        /// Normal outward excerpt; never used as classification evidence.
        detail: String,
        /// Exact normal identity and applicability, resolved from the dispatch's physical model.
        restriction: Box<crate::provider::quota_rejection::ClassifiedQuotaRejection>,
    },

    /// The upstream failed without a usable HTTP response: a transport failure, an oversized frame,
    /// or a malformed payload.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// The provider control engine failed: token refresh, model discovery, a flow stage, or a
    /// lease failure — [`ProviderError`] is that engine's one shared failure vocabulary, so all
    /// of them travel through this variant.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// Storage failed under the pipeline: routing lookup, lease driving, or credentials
    /// persistence.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// The gateway violated an invariant — stored configuration drift, never a fault of the
    /// requesting client.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message.
        message: String,
    },
}

/// A stream of translated IR events; always terminates with [`IrEvent::Done`].
pub(crate) type IrEventStream = Pin<Box<dyn Stream<Item = Result<IrEvent, Error>> + Send>>;

/// Resolves an enabled explicit public route to one real account and physical model.
///
/// Direct targets bypass pooled selection. Pool membership and physical state come from one
/// coherent snapshot released before I/O, and eligible members follow authored priority.
///
/// # Errors
///
/// Returns model-not-found for unknown or disabled routes, pools, or direct targets;
/// pool-unavailable for enabled pools with no structural candidate; or storage failures.
///
/// # Panics
///
/// Panics if a selected structural candidate has no physical model, an enforced local invariant.
#[cfg(test)]
pub(crate) async fn resolve_routing(
    database: &Database,
    public_id: &str,
) -> Result<ModelRouting, Error> {
    selection::Selection::load(
        database,
        public_id,
        crate::test_support::structural_api_key(database).await?,
    )
    .await?
    .first_structural()
}

/// Selects a compaction target using the already authenticated active producer requirement.
///
/// # Errors
///
/// Returns route/storage failures, incompatible native state, or the required pool account's
/// availability/quota failure. Unbound pools use quota-aware priority selection; direct routes
/// bypass affinity.
///
/// # Panics
///
/// Panics if the storage snapshot disagrees with its authored route target.
pub(crate) async fn resolve_compaction_routing(
    context: &Context,
    parsed: &crate::format::ParsedRequest,
    preparation: &crate::compaction::history::BoundaryPreparation,
    api_key_id: i64,
) -> Result<CompactionSelection, Error> {
    selection::select_compaction(context, parsed, preparation, api_key_id).await
}

/// Native compaction body controls and header overlays carried by an ordinary dispatch.
#[derive(Debug, Default)]
pub(crate) struct CompactionDispatch {
    /// The vendor-shaped Responses `context_management` entries riding the
    /// upstream body verbatim.
    pub(crate) context_management: Option<Vec<crate::format::compaction_wire::AutomaticEntry>>,

    /// The vendor-shaped Messages threshold edits riding the upstream body
    /// verbatim.
    pub(crate) threshold_edits: Option<Vec<crate::format::compaction_wire::ThresholdEdit>>,

    /// Header rows overlaying the inference descriptor's own, replacing
    /// same-named rows — the operation betas a native passthrough requires.
    pub(crate) overlay_headers: Vec<crate::provider::definition::template::ParamRow>,
}

/// Executes one completion dispatch against the routed provider and returns the parsed IR event
/// stream.
///
/// # Errors
///
/// Returns typed [`Error`] values for preparation, transport, and rejected headers. Classified
/// quota evidence remains distinguishable until the caller decides its operation's fallback.
/// Its caller owns the draft and every finalization decision, including cancellation.
pub(crate) async fn execute(
    context: &Context,
    routing: ModelRouting,
    mut ir: IrRequest,
    is_client_streaming: bool,
    log: &mut ApiLogDraft,
    compaction: Option<&CompactionDispatch>,
    affinity: Option<&mut crate::affinity::Ownership>,
) -> Result<IrEventStream, Error> {
    // The client's reasoning effort string needs no interpretation here: it is opaque end to end
    // (README §6.4) — no stored override is consulted, nothing is filtered or clamped — and the
    // renderers write it verbatim into the upstream's own parameter.
    ir.model = routing.model.upstream_id.clone();
    // A gateway carrier never reaches dispatch: compaction preparation must decode, verify, and
    // remove it first, and a damaged marker must never be forwarded upstream. Its presence here is
    // an internal invariant violation, not client state, so it fails fast (README §6.8).
    if ir.state_items.iter().any(|item| item.is_gateway) {
        return Err(Error::Internal {
            message: "a gateway compaction carrier reached dispatch without preparation"
                .to_string(),
        });
    }

    // Names the upstream in every error-level record the lazy parser emits. Which single site logs
    // such a record is partitioned by the client's streaming mode (README §8): for a streaming
    // client the failure becomes a terminal mid-stream error frame that never passes a response
    // boundary, so the detecting parser logs it; for a non-streaming client it becomes the
    // aggregated HTTP error response, so the response boundary rendering it logs it.
    // The client's mode is distinct from the upstream's: an `always`-streaming definition streams
    // upstream while its client may not (README §5.2).
    let upstream_label = format!(
        "provider `{}` upstream model `{}`",
        routing.provider.name, ir.model
    );

    // The token refresh is a control-plane round-trip, so it rides the bounded client even though
    // the completion itself is data-plane traffic.
    let provider =
        oauth::ensure_fresh_token(&context.database, &context.control_plane, &routing.provider)
            .await?;

    // The wire format is resolved once per request and shared by request building and event
    // parsing: the definition pins it outright.
    let upstream_format = provider.definition.payload.format;

    debug!(
        "dispatching public model `{}` via {upstream_label} ({}, streaming: {})",
        routing.public_id,
        upstream_format.as_str(),
        is_client_streaming
    );

    let request_builder = build_upstream_request(
        &context.data_plane,
        &provider,
        &routing,
        &ir,
        is_client_streaming,
        compaction,
    )?;

    // The dispatch mark fixes this attempt's recording switch and advances commitment even
    // when recording is disabled. Every earlier failure remains unrecordable (README §3).
    log.stamp();

    let response = match request_builder.send().await {
        Ok(response) => response,

        Err(error) => {
            let error = Error::Unavailable {
                detail: format!("upstream unreachable: {error}"),
            };

            return Err(error);
        }
    };

    let status = response.status();
    log.observe_response(status);

    if !status.is_success() {
        let body = http_body::read_bounded_error_body(response).await;
        let failure = quota_error_of(
            context,
            &provider,
            &routing.model.upstream_id,
            status,
            &body,
        )
        .await
        .unwrap_or_else(Some)
        .unwrap_or_else(|| Error::Upstream {
            status,
            detail: http_body::truncate_excerpt(&body.lossy_text()),
        });
        return Err(failure);
    }

    if let Some(affinity) = affinity {
        affinity.accept(routing.provider.id);
    }

    debug!("{upstream_label}: upstream responded {status}");

    // The parser is chosen by the upstream streaming mode the gateway requested, never by
    // response-header sniffing: the Codex upstream answers its 200 SSE streams without a
    // `Content-Type` header, and a 200 response to a streaming request is contractually a stream
    // while a 200 response to a non-streaming request is contractually a JSON document (README
    // §6.2).
    let is_upstream_streaming =
        is_upstream_streaming(&provider.definition.payload.inference, is_client_streaming);

    let parser = upstream_parser(upstream_format);

    let response_unwrap = provider
        .definition
        .payload
        .inference
        .response_unwrap
        .clone();

    if is_upstream_streaming {
        let body = response.bytes_stream().map(|chunk| {
            chunk.map_err(|error| Error::Unavailable {
                detail: format!("upstream stream failed: {error}"),
            })
        });
        Ok(sse_event_stream(
            body,
            parser,
            upstream_label,
            is_client_streaming,
            response_unwrap,
        ))
    } else {
        let body = async move {
            response.bytes().await.map_err(|error| Error::Unavailable {
                detail: format!("upstream read failed: {error}"),
            })
        };
        Ok(json_event_stream(body, parser, response_unwrap))
    }
}

/// The upstream streaming mode one dispatch requests: the definition's mode decides — always
/// streaming upstream (the forced-streaming upstreams, such as Codex), or matching the client's
/// requested mode. The same decision selects the SSE or JSON response parser.
pub(crate) fn is_upstream_streaming(inference: &Inference, is_client_streaming: bool) -> bool {
    match inference.upstream_streaming {
        UpstreamStreaming::Always => true,
        UpstreamStreaming::MatchClient => is_client_streaming,
    }
}

/// Executes one internal compaction-path dispatch end to end: the ordinary
/// inference body rendered from the given IR — whose compaction controls the
/// executor has already stripped — with non-streaming client framing, pumped to
/// completion and folded into an aggregation.
///
/// One dispatch, one send: pre-dispatch failures (routing drift, token
/// acquisition, payload rendering, transport, a mirrored non-2xx) leave the
/// ledger untouched, because pre-dispatch and usage-less failures acquire no
/// fabricated token counts. A successful completion records the usage the
/// upstream reported into the ledger under the given iteration kind.
///
/// A clean upstream end that carried no real terminal event fails the dispatch:
/// the degraded-finish rule of the ordinary pipeline never papers over a
/// missing terminal on the compaction paths, where the result must be complete
/// before any artifact stands (README §6.8).
///
/// Cancellation and deadlines: the dispatch runs inside the handler future, so
/// a departing client drops it; the data-plane read-idle timeout bounds a
/// stalled upstream. No fresh deadline is granted per internal call.
///
/// The request's accounting draft is stamped the moment the send leaves, so the
/// outer request's row exists exactly when a dispatch occurred (README §3
/// *ApiLog*); finalizing the row stays the outer boundary's job.
///
/// # Errors
///
/// Returns [`Error`] for token, transport, mirrored-upstream, and payload
/// failures, and for a completed upstream response that never reached a real
/// terminal finish.
pub(crate) async fn execute_internal(
    context: &Context,
    routing: &ModelRouting,
    mut ir: IrRequest,
    ledger: &mut CompactionLedger,
    kind: IterationKind,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<IrAggregation, Error> {
    ir.model = routing.model.upstream_id.clone();

    // The same fail-fast guard the ordinary dispatch applies: a gateway carrier
    // must have been decoded and removed during compaction preparation.
    if ir.state_items.iter().any(|item| item.is_gateway) {
        return Err(Error::Internal {
            message: "a gateway compaction carrier reached dispatch without preparation"
                .to_string(),
        });
    }

    let provider =
        oauth::ensure_fresh_token(&context.database, &context.control_plane, &routing.provider)
            .await?;

    let request =
        build_upstream_request(&context.data_plane, &provider, routing, &ir, false, None)?;

    log.stamp();

    let response = match request.send().await {
        Ok(response) => response,

        Err(send_error) => {
            return Err(Error::Unavailable {
                detail: format!("upstream unreachable: {send_error}"),
            });
        }
    };

    log.observe_response(response.status());
    if !response.status().is_success() {
        let status = response.status();
        let body = http_body::read_bounded_error_body(response).await;
        return Err(quota_error_of(
            context,
            &provider,
            &routing.model.upstream_id,
            status,
            &body,
        )
        .await?
        .unwrap_or_else(|| Error::Upstream {
            status,
            detail: http_body::truncate_excerpt(&body.lossy_text()),
        }));
    }

    if let Some(ownership) = ownership {
        ownership.accept(routing.provider.id);
    }

    let is_streaming = is_upstream_streaming(&provider.definition.payload.inference, false);

    let mut parser = upstream_parser(provider.definition.payload.format);

    let response_unwrap = provider
        .definition
        .payload
        .inference
        .response_unwrap
        .clone();

    let (aggregation, is_terminal_real) = pump_internal_response(
        response,
        parser.as_mut(),
        is_streaming,
        response_unwrap,
        ledger,
        kind,
        log,
    )
    .await?;
    if !is_terminal_real {
        return Err(Error::Unavailable {
            detail: "the upstream response ended without a terminal completion".to_string(),
        });
    }

    Ok(aggregation)
}

/// Saves each internal usage observation immediately so failure and cancellation keep billed work.
fn observe_internal_usage(
    event: &IrEvent,
    billed_before: crate::ir::IrUsage,
    last_usage: &mut Option<crate::ir::IrUsage>,
    iteration_usage: &mut Option<crate::ir::IrUsage>,
    log: &mut ApiLogDraft,
) {
    match event {
        IrEvent::Usage(usage) => *last_usage = Some(*usage),
        IrEvent::CompactionMetadata { iterations, .. } => {
            *iteration_usage = crate::compaction::accounting::reported_iteration_totals(iterations)
        }
        _ => return,
    }
    if let Some(usage) = iteration_usage.or(*last_usage) {
        log.stamp_billed_usage(Some(billed_before.saturating_sum(usage)));
    }
}

/// Folds one internal upstream response into an aggregation through the given
/// parser, reporting whether a *real* terminal finish arrived: a finish the
/// parser emitted from a terminal event or body. The clean-end-of-stream flush's
/// degraded finish ([`crate::format::degrade_finish_once`]) does not count — the
/// caller decides what a missing terminal means for its own contract.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] for a transport failure, an oversized frame, a
/// malformed payload, or a 2xx body that is not valid JSON.
pub(crate) async fn pump_internal_response<P>(
    response: reqwest::Response,
    parser: &mut P,
    is_upstream_streaming: bool,
    response_unwrap: Option<String>,
    ledger: &mut CompactionLedger,
    kind: IterationKind,
    log: &mut ApiLogDraft,
) -> Result<(IrAggregation, bool), Error>
where
    P: UpstreamEventParser + ?Sized + Send,
{
    let mut aggregator = Aggregator::default();
    let billed_before = ledger.billed_totals();
    let mut last_usage = None;
    let mut iteration_usage = None;

    // Whether a real terminal finish arrived: only finishes the parser emitted
    // from its frame or body ingestion count; the clean-end flush's degraded
    // finish is emitted after this flag's last real observation.
    let mut is_terminal_real = false;

    if is_upstream_streaming {
        let mut body = response.bytes_stream();
        let mut sse_parser = SseParser::new();

        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|error| Error::Unavailable {
                detail: format!("upstream stream failed: {error}"),
            })?;

            let frames = sse_parser.ingest(&chunk).map_err(|_| Error::Unavailable {
                detail: "oversized SSE frame".to_string(),
            })?;

            for mut frame in frames {
                unwrap_frame(&mut frame, &response_unwrap);

                for event in parser.ingest_frame(&frame)? {
                    if matches!(event, IrEvent::Finish { .. }) {
                        is_terminal_real = true;
                    }

                    observe_internal_usage(
                        &event,
                        billed_before,
                        &mut last_usage,
                        &mut iteration_usage,
                        log,
                    );
                    aggregator.ingest(&event);
                }
            }
        }

        // Clean end of stream: the trailing frames flush through the parser,
        // then the parser's own flush hook runs. Neither may report a real
        // terminal — a finish here is the degraded one.
        for frame in sse_parser.finish().map_err(|_| Error::Unavailable {
            detail: "oversized SSE frame".to_string(),
        })? {
            let mut frame = frame;

            unwrap_frame(&mut frame, &response_unwrap);

            for event in parser.ingest_frame(&frame)? {
                observe_internal_usage(
                    &event,
                    billed_before,
                    &mut last_usage,
                    &mut iteration_usage,
                    log,
                );
                aggregator.ingest(&event);
            }
        }

        for event in parser.finish() {
            observe_internal_usage(
                &event,
                billed_before,
                &mut last_usage,
                &mut iteration_usage,
                log,
            );
            aggregator.ingest(&event);
        }
    } else {
        let bytes = response.bytes().await.map_err(|error| Error::Unavailable {
            detail: format!("upstream read failed: {error}"),
        })?;

        let body = unwrap_response(&bytes, &response_unwrap)?;

        for event in parser.parse_json_response(body)? {
            if matches!(event, IrEvent::Finish { .. }) {
                is_terminal_real = true;
            }

            observe_internal_usage(
                &event,
                billed_before,
                &mut last_usage,
                &mut iteration_usage,
                log,
            );
            aggregator.ingest(&event);
        }
    }

    ledger.record(kind, iteration_usage.or(last_usage));
    Ok((aggregator.finish(), is_terminal_real))
}

/// Builds one internal compaction-path HTTP request from explicit connection
/// facts: the evaluated URL expression, the evaluated header rows, the JSON
/// body spliced through the optional envelope, and the fixed `Accept:
/// application/json` the non-streaming compaction framing carries (the header
/// rows override it by name, exactly as the inference dispatch's do).
///
/// # Errors
///
/// Returns [`Error::Internal`] when the connection's expressions reference a
/// value this dispatch does not carry — stored drift — or when a header row
/// carries an unrenderable name or value.
pub(crate) fn build_internal_request<T: serde::Serialize>(
    http: &Client,
    provider: &Provider,
    routing: &ModelRouting,
    url: &StringExpr,
    header_rows: &[ParamRow],
    envelope: Option<&TemplateNode>,
    body: &T,
) -> Result<reqwest::RequestBuilder, Error> {
    let request_uuid = ids::new_uuid();

    let env = ValueEnv::for_compaction_operation(
        provider.credentials.slots(),
        provider.base_url.as_deref(),
        provider.credentials.secret(),
        &request_uuid,
        &routing.model.upstream_id,
    );

    let url = env
        .evaluate(url, "compaction operation URL")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    let mut headers = reqwest::header::HeaderMap::new();

    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("application/json"),
    );

    for row in header_rows {
        let value = env
            .evaluate(&row.value, "compaction operation header")
            .map_err(|error| Error::Internal {
                message: error.to_string(),
            })?;

        let name = reqwest::header::HeaderName::from_bytes(row.name.as_bytes()).map_err(|_| {
            Error::Internal {
                message: format!(
                    "the definition carries an invalid header name `{}`",
                    row.name
                ),
            }
        })?;

        let value =
            reqwest::header::HeaderValue::from_str(&value).map_err(|_| Error::Internal {
                message: "the definition carries an unrenderable header value".to_string(),
            })?;

        headers.insert(name, value);
    }

    let builder = http.post(url).headers(headers);
    let Some(envelope) = envelope else {
        return Ok(builder.json(body));
    };
    let envelope = env
        .render_json(envelope, body, "compaction operation envelope")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;
    Ok(builder.json(&envelope))
}

/// Builds the upstream HTTP request for one dispatch, entirely from the provider definition
/// (README §5.2): the evaluated URL (the streaming URL when this dispatch streams and
/// the definition carries one), the codec-rendered body attached through the definition's
/// envelope, the engine-owned `Accept` with the definition's header rows overriding it, and one
/// request-UUID cached across every expression of the dispatch.
///
/// # Errors
///
/// Returns [`Error`] when the definition's expressions reference a value this dispatch does not
/// carry — stored drift — or when the client request cannot be rendered into the body.
fn build_upstream_request(
    http: &Client,
    provider: &Provider,
    routing: &ModelRouting,
    ir: &IrRequest,
    is_client_streaming: bool,
    compaction: Option<&CompactionDispatch>,
) -> Result<reqwest::RequestBuilder, Error> {
    let inference = &provider.definition.payload.inference;

    let is_upstream_streaming = is_upstream_streaming(inference, is_client_streaming);

    // One request UUID per dispatch, reused by every expression reference in it.
    let request_uuid = ids::new_uuid();

    let slots = provider.credentials.slots();

    let env = ValueEnv::for_inference(
        slots,
        provider.base_url.as_deref(),
        provider.credentials.secret(),
        &request_uuid,
        &routing.model.upstream_id,
    );

    // The streaming URL only when this dispatch streams and one is configured (README §5.2).
    let url_expr = if is_upstream_streaming {
        inference.stream_url.as_ref().unwrap_or(&inference.url)
    } else {
        &inference.url
    };

    let url = env
        .evaluate(url_expr, "inference URL")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    let mut headers = reqwest::header::HeaderMap::new();

    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static(if is_upstream_streaming {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );

    // The compaction dispatch's overlay rows replace same-named inference rows
    // (the operation betas ride the operation definition), then the surviving
    // inference rows follow in order.
    let overlay = compaction
        .map(|dispatch| dispatch.overlay_headers.as_slice())
        .unwrap_or(&[]);

    let merged_rows =
        crate::format::anthropic::messages::overlay_header_rows(&inference.headers, overlay);

    for row in &merged_rows {
        let value = env
            .evaluate(&row.value, "inference header")
            .map_err(|error| Error::Internal {
                message: error.to_string(),
            })?;

        let name = reqwest::header::HeaderName::from_bytes(row.name.as_bytes()).map_err(|_| {
            Error::Internal {
                message: format!(
                    "the definition carries an invalid header name `{}`",
                    row.name
                ),
            }
        })?;

        let value =
            reqwest::header::HeaderValue::from_str(&value).map_err(|_| Error::Internal {
                message: "the definition carries an unrenderable header value".to_string(),
            })?;

        headers.insert(name, value);
    }

    attach_upstream_body(
        http.post(url).headers(headers),
        &env,
        provider,
        ir,
        is_upstream_streaming,
        compaction,
    )
}

/// Renders the upstream body through the definition format's codec and attaches it to the
/// dispatch's request builder (README §5.2).
///
/// # Errors
///
/// Returns [`Error::InvalidRequest`] when the client request cannot be rendered into the
/// definition format's body, and [`Error::Internal`] when the envelope's expressions reference a
/// value this dispatch does not carry — stored drift.
fn attach_upstream_body(
    builder: reqwest::RequestBuilder,
    env: &ValueEnv<'_>,
    provider: &Provider,
    ir: &IrRequest,
    is_upstream_streaming: bool,
    compaction: Option<&CompactionDispatch>,
) -> Result<reqwest::RequestBuilder, Error> {
    let inference = &provider.definition.payload.inference;

    let envelope = inference.envelope.as_ref();

    match provider.definition.payload.format {
        UpstreamFormat::OpenAiResponse => attach_body(
            builder,
            env,
            envelope,
            &openai::response::render_request_body_with_extensions(
                ir,
                is_upstream_streaming,
                &inference.render_options,
                compaction.and_then(|dispatch| dispatch.context_management.as_deref()),
            )
            .map_err(Error::from)?,
        ),

        UpstreamFormat::OpenAiChatCompletion => attach_body(
            builder,
            env,
            envelope,
            &openai::chat_completion::render_request_body(ir, is_upstream_streaming)
                .map_err(Error::from)?,
        ),

        UpstreamFormat::GeminiGenerateContent => attach_body(
            builder,
            env,
            envelope,
            &gemini::generate_content::render_generate_content_request(ir).map_err(Error::from)?,
        ),

        UpstreamFormat::GeminiInteraction => attach_body(
            builder,
            env,
            envelope,
            &gemini::interaction::render_request_body(ir, &ir.model, is_upstream_streaming)
                .map_err(Error::from)?,
        ),

        UpstreamFormat::AnthropicMessages => {
            // A native threshold passthrough renders the vendor-shaped edits
            // through the threshold body render; every other Messages dispatch
            // renders the stripped form (the base render carries no compaction
            // members).
            let body = match compaction.and_then(|dispatch| dispatch.threshold_edits.as_deref()) {
                Some(edits) => {
                    anthropic::messages::render_threshold_body(ir, is_upstream_streaming, edits)
                }

                None => anthropic::messages::render_request_body(ir, is_upstream_streaming),
            }
            .map_err(Error::from)?;

            attach_body(builder, env, envelope, &body)
        }
    }
}

/// Attaches one codec-rendered body to the request builder: without an envelope the typed body
/// rides to reqwest and serializes exactly once onto the wire; with an envelope, the evaluated
/// template borrows that same concrete body at its placeholder without constructing a JSON DOM.
///
/// # Errors
///
/// Returns [`Error::Internal`] when the envelope's expressions reference a value this dispatch
/// does not carry — stored drift.
fn attach_body<T: serde::Serialize>(
    builder: reqwest::RequestBuilder,
    env: &ValueEnv<'_>,
    envelope: Option<&TemplateNode>,
    body: &T,
) -> Result<reqwest::RequestBuilder, Error> {
    let Some(envelope) = envelope else {
        return Ok(builder.json(body));
    };

    let body = env
        .render_json(envelope, body, "inference envelope")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    Ok(builder.json(&body))
}

/// Boxes the upstream event parser for the given wire format.
fn upstream_parser(upstream_format: UpstreamFormat) -> Box<dyn UpstreamEventParser + Send> {
    match upstream_format {
        UpstreamFormat::OpenAiResponse => Box::new(openai::response::UpstreamParser::new()),
        UpstreamFormat::OpenAiChatCompletion => {
            Box::new(openai::chat_completion::UpstreamParser::new())
        }
        UpstreamFormat::GeminiGenerateContent => {
            Box::new(gemini::generate_content::UpstreamParser::new())
        }
        UpstreamFormat::GeminiInteraction => Box::new(gemini::interaction::UpstreamParser::new()),
        UpstreamFormat::AnthropicMessages => Box::new(anthropic::messages::UpstreamParser::new()),
    }
}

/// Selects the provider-configured response member without materializing protocol JSON.
///
/// # Errors
///
/// Returns an upstream failure for a malformed configured envelope.
pub(crate) fn unwrap_response<'a>(
    body: &'a [u8],
    response_unwrap: &Option<String>,
) -> Result<&'a [u8], Error> {
    response_body::unwrap(body, response_unwrap.as_deref()).map_err(|error| Error::Unavailable {
        detail: format!("malformed upstream response: {error}"),
    })
}

/// Selects a configured foreign SSE envelope; non-JSON sentinels pass through unchanged.
///
/// # Panics
///
/// Panics if bytes borrowed from a UTF-8 SSE string cease to be UTF-8, an internal invariant.
fn unwrap_frame(frame: &mut SseFrame, response_unwrap: &Option<String>) {
    if response_unwrap.is_none() {
        return;
    }
    if let Ok(body) = unwrap_response(frame.data.as_bytes(), response_unwrap) {
        // Selected bytes are a substring of valid UTF-8 frame data.
        #[expect(
            clippy::expect_used,
            reason = "selected JSON bytes borrow a UTF-8 SSE string"
        )]
        let selected = std::str::from_utf8(body).expect("JSON frame is UTF-8");
        if selected.len() != frame.data.len() {
            frame.data = selected.to_owned();
        }
    }
}

/// Applies exact quota rules to a rejected generation/native body and orders durable evidence.
/// Token counters and every control-plane operation deliberately never call this function.
///
/// # Errors
///
/// Returns storage failures; a stale generation or deleted account never recreates evidence.
pub(crate) async fn quota_error_of(
    context: &Context,
    provider: &Provider,
    upstream_id: &str,
    status: StatusCode,
    body: &http_body::ErrorBody,
) -> Result<Option<Error>, Error> {
    let Some(restriction) = crate::provider::quota_rejection::classify_rejection(
        status.as_u16(),
        body,
        provider
            .definition
            .payload
            .inference
            .quota_exhaustion
            .as_ref(),
        upstream_id,
    ) else {
        return Ok(None);
    };
    storage::quota_blocks::record_rejection(
        &context.database,
        provider.id,
        provider.quota_generation,
        &restriction.key,
        &restriction.applies_to,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    Ok(Some(Error::QuotaRejected {
        status,
        detail: http_body::truncate_excerpt(&body.lossy_text()),
        restriction: Box::new(restriction),
    }))
}

impl From<PayloadError> for Error {
    fn from(error: PayloadError) -> Self {
        Self::Unavailable {
            detail: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        accounting::ApiLogOutcome,
        api::Error as ApiError,
        ir::{IrMessage, IrPart, IrRole},
        provider::{
            DiscoveryState, Model, StoredCredentials,
            definition::{
                DefinitionPayload, ProviderDefinition,
                credential::CredentialAcquisition,
                template::{StringExpr, ValueExpr},
            },
        },
    };
    use reqwest::StatusCode;
    use std::{collections::BTreeMap, num::NonZeroI64};

    /// Verifies the dispatch accounting derives from the shared API classification: a mirrored
    /// upstream non-2xx records `upstream_error` with that very status, a transport failure
    /// records `upstream_error` with 502, and a gateway-side failure records `internal` with 500.
    ///
    /// # Panics
    ///
    /// Panics if a branch classifies differently from the documented mapping.
    #[test]
    fn classifies_dispatch_failures_through_the_shared_error() {
        let (outcome, http_status) = ApiError::from(Error::Upstream {
            status: StatusCode::TOO_MANY_REQUESTS,
            detail: "rate limited".to_string(),
        })
        .accounting();

        assert_eq!((outcome, http_status), (ApiLogOutcome::UpstreamError, 429));

        let (outcome, http_status) = ApiError::from(Error::Unavailable {
            detail: "upstream unreachable".to_string(),
        })
        .accounting();

        assert_eq!((outcome, http_status), (ApiLogOutcome::UpstreamError, 502));

        let (outcome, http_status) = ApiError::from(Error::Internal {
            message: "stored configuration drift".to_string(),
        })
        .accounting();

        assert_eq!((outcome, http_status), (ApiLogOutcome::Internal, 500));
    }

    /// Builds a minimal one-turn chat request the dispatch bodies render from.
    pub(super) fn fixture_ir() -> IrRequest {
        IrRequest {
            has_compaction_trigger: false,
            state_items: Vec::new(),
            model: "upstream-model".to_string(),
            instructions: None,
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "hello".to_string(),
                }],
            }],
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            response_format: None,
            reasoning_effort: None,
        }
    }

    /// Builds an API-key provider of the chat-completions format carrying the given envelope.
    ///
    /// # Panics
    ///
    /// Panics if the fixture version fails to construct, which the positive literal prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture version literal is positive"
    )]
    pub(super) fn fixture_provider(envelope: Option<TemplateNode>) -> Provider {
        Provider {
            model_prefix: String::new(),
            quota_generation: 0,
            quota_revision: 0,
            id: 1,
            name: "test-provider".to_string(),
            base_url: None,
            credentials: StoredCredentials::ApiKey {
                api_key: "key".to_string(),
            },
            is_enabled: true,
            definition: ProviderDefinition {
                name: "test-definition".to_string(),
                version: NonZeroI64::new(1).expect("one is positive"),
                payload: DefinitionPayload {
                    format: UpstreamFormat::OpenAiChatCompletion,
                    inference: Inference {
                        url: StringExpr::literal("https://upstream.test/v1/chat/completions"),
                        stream_url: None,
                        headers: vec![],
                        upstream_streaming: UpstreamStreaming::MatchClient,
                        render_options: vec![],
                        envelope,
                        response_unwrap: None,
                        quota_exhaustion: None,
                    },
                    credential: CredentialAcquisition::ApiKey,
                    provisioning: None,
                    discovery: None,
                    quota: None,
                    compaction: None,
                },
                created_at: 0,
            },
        }
    }

    /// Builds the dispatch request for the fixture provider and parses the body it would send.
    ///
    /// # Panics
    ///
    /// Panics if the fixture dispatch fails to build or its body does not parse, which the
    /// fixture prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture dispatch is constructed to build and parse"
    )]
    fn fixture_wire_body(http: &Client, provider: &Provider, ir: &IrRequest) -> serde_json::Value {
        let routing = ModelRouting {
            model_route_id: 1,
            public_id: ir.model.clone(),
            provider: provider.clone(),
            model: Model {
                id: 1,
                upstream_id: ir.model.clone(),
                is_enabled: true,
                discovery_state: DiscoveryState::Manual,
                thinking_levels: None,
                max_input_tokens: None,
                max_output_tokens: None,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support: None,
                compaction_support_override: None,
            },
        };

        let request = build_upstream_request(http, provider, &routing, ir, false, None)
            .expect("the fixture dispatch builds")
            .build()
            .expect("the request builder completes");

        let body = request
            .body()
            .expect("a JSON dispatch carries a body")
            .as_bytes()
            .expect("a JSON dispatch body is buffered")
            .to_vec();

        serde_json::from_slice(&body).expect("the fixture body parses")
    }

    /// Serializes the codec-rendered body the dispatch should carry on the wire.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize, which cannot occur for a first-party serde
    /// type.
    #[expect(
        clippy::expect_used,
        reason = "a first-party rendered body always serializes"
    )]
    fn expected_rendered_body(ir: &IrRequest) -> serde_json::Value {
        serde_json::to_value(
            openai::chat_completion::render_request_body(ir, false).expect("the fixture renders"),
        )
        .expect("a first-party rendered body serializes")
    }

    /// Verifies the no-envelope dispatch: the codec-rendered body rides to reqwest in its typed
    /// form, so the wire body is exactly that rendering with no envelope wrapped around it.
    ///
    /// # Panics
    ///
    /// Panics if the fixture fails to build or the wire body differs from the rendering.
    #[tokio::test]
    async fn dispatches_the_rendered_body_alone_without_an_envelope() {
        let ir = fixture_ir();
        let provider = fixture_provider(None);

        assert_eq!(
            fixture_wire_body(&Client::new(), &provider, &ir),
            expected_rendered_body(&ir)
        );
    }

    /// Verifies the envelope dispatch: the codec-rendered body is spliced into the envelope's
    /// template DOM at its body placeholder, and the envelope's string leaves evaluate against
    /// the dispatch's values — here the routed upstream model.
    ///
    /// # Panics
    ///
    /// Panics if the fixture fails to build or the wire body differs from the rendered envelope.
    #[tokio::test]
    async fn splices_the_rendered_body_into_the_envelope_dom() {
        let ir = fixture_ir();

        let envelope = TemplateNode::Object {
            fields: BTreeMap::from([
                (
                    "model".to_string(),
                    TemplateNode::String {
                        expr: StringExpr::single(ValueExpr::Model),
                    },
                ),
                ("request".to_string(), TemplateNode::Body),
            ]),
        };

        let provider = fixture_provider(Some(envelope));

        assert_eq!(
            fixture_wire_body(&Client::new(), &provider, &ir),
            serde_json::json!({
                "model": ir.model,
                "request": expected_rendered_body(&ir),
            })
        );
    }
}
