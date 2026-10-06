//! Gateway execution pipeline: routing resolution, token acquisition, upstream dispatch, and
//! translation into IR event streams.

use super::{
    context::Context,
    format::{PayloadError, UpstreamEventParser, UpstreamFormat, anthropic, gemini, openai},
    ir::{Aggregator, InvalidRequest, IrAggregation, IrEvent, IrRequest, IrUsage},
    provider::{
        Error as ProviderError, ModelRouting,
        definition::inference::{Inference, UpstreamStreaming},
        oauth,
    },
    sse::{SseFrame, SseParser},
    storage::{Error as StorageError, api_logs::ApiLogDraft},
};
use crate::compaction_model::{accounting::CompactionLedger, boundary::BoundaryError};
#[cfg(test)]
use crate::database::Database;
use futures::{Stream, StreamExt as _};
use log::debug;
use reqwest::StatusCode;
use std::pin::Pin;

mod dispatch;
pub(crate) use dispatch::{DispatchOutcome, QuotaPolicy, dispatch};
mod request;
use request::build_upstream_request;
pub(crate) use request::{build_internal_request, overlay_header_rows};

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
    preparation: &crate::compaction_model::boundary::BoundaryPreparation,
    api_key_id: i64,
) -> Result<CompactionSelection, Error> {
    selection::select_compaction(context, parsed, preparation, api_key_id).await
}

/// Native compaction body controls and header overlays carried by an ordinary dispatch.
#[derive(Debug, Default)]
pub(crate) struct CompactionDispatch {
    /// The vendor-shaped Responses `context_management` entries riding the
    /// upstream body verbatim.
    pub(crate) context_management: Option<Vec<crate::ir::compaction::AutomaticEntry>>,

    /// The vendor-shaped Messages threshold edits riding the upstream body
    /// verbatim.
    pub(crate) threshold_edits: Option<Vec<crate::ir::compaction::ThresholdEdit>>,

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
    ir: IrRequest,
    is_client_streaming: bool,
    log: &mut ApiLogDraft,
    compaction: Option<&CompactionDispatch>,
    affinity: Option<&mut crate::affinity::Ownership>,
) -> Result<IrEventStream, Error> {
    let AcceptedDispatch {
        response,
        parser,
        response_unwrap,
        is_upstream_streaming,
        upstream_label,
    } = prepare_and_dispatch(
        context,
        &routing,
        ir,
        is_client_streaming,
        compaction,
        log,
        affinity,
    )
    .await?;

    let status = response.status();

    debug!("{upstream_label}: upstream responded {status}");

    // The parser is chosen by the upstream streaming mode the gateway requested, never by
    // response-header sniffing: the Codex upstream answers its 200 SSE streams without a
    // `Content-Type` header, and a 200 response to a streaming request is contractually a stream
    // while a 200 response to a non-streaming request is contractually a JSON document (README
    // §6.2).
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

/// One accepted upstream dispatch beside the parsing facts its response pumping needs: the
/// setup the provider definition pins, resolved once per request and shared by both entry
/// points.
struct AcceptedDispatch {
    /// The accepted 2xx upstream response.
    response: reqwest::Response,

    /// The upstream event parser for the definition's wire format.
    parser: Box<dyn UpstreamEventParser + Send>,

    /// The provider-configured response envelope member to unwrap.
    response_unwrap: Option<String>,

    /// The streaming mode the gateway requested: the definition's mode decides — always
    /// streaming upstream, or matching the client's requested mode ([`is_upstream_streaming`]).
    is_upstream_streaming: bool,

    /// Human-readable provider and upstream model label naming the upstream in error-level
    /// records the lazy parser emits and in the accepted-headers debug line.
    upstream_label: String,
}

/// The dispatch prelude shared by [`execute`] and [`execute_internal`]: stamp the routed
/// upstream model onto the request, fail fast on an unprepared gateway carrier, renew the
/// provider's credential, render the upstream request, and send once, committing headers.
///
/// The client's reasoning effort string needs no interpretation here: it is opaque end to end
/// (README §6.4) — no stored override is consulted, nothing is filtered or clamped — and the
/// renderers write it verbatim into the upstream's own parameter.
///
/// # Errors
///
/// Returns typed [`Error`] values for preparation, transport, and rejected headers. Classified
/// quota evidence remains distinguishable until the caller decides its operation's fallback.
async fn prepare_and_dispatch(
    context: &Context,
    routing: &ModelRouting,
    mut ir: IrRequest,
    is_client_streaming: bool,
    compaction: Option<&CompactionDispatch>,
    log: &mut ApiLogDraft,
    affinity: Option<&mut crate::affinity::Ownership>,
) -> Result<AcceptedDispatch, Error> {
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
        routing,
        &ir,
        is_client_streaming,
        compaction,
    )?;

    let response = dispatch(
        context,
        request_builder,
        &provider,
        &routing.model.upstream_id,
        log,
        affinity,
        QuotaPolicy::Classify,
    )
    .await?
    .into_accepted()?;

    Ok(AcceptedDispatch {
        is_upstream_streaming: is_upstream_streaming(
            &provider.definition.payload.inference,
            is_client_streaming,
        ),
        parser: upstream_parser(upstream_format),
        response_unwrap: provider
            .definition
            .payload
            .inference
            .response_unwrap
            .clone(),
        response,
        upstream_label,
    })
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
/// upstream reported into the ledger.
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
    ir: IrRequest,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<IrAggregation, Error> {
    let AcceptedDispatch {
        response,
        mut parser,
        response_unwrap,
        is_upstream_streaming,
        ..
    } = prepare_and_dispatch(context, routing, ir, false, None, log, ownership).await?;

    let (aggregation, is_terminal_real) = pump_internal_response(
        response,
        parser.as_mut(),
        is_upstream_streaming,
        response_unwrap,
        ledger,
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

/// Tracks one event stream's usage observations under the accounting rule that a compaction
/// lifecycle's reported iteration totals override the ordinary terminal usage: the resolved
/// usage is the latest iteration total when one arrived, otherwise the last ordinary usage
/// observation (README §6.2's last-observation rule).
///
/// Every consumer of a stream's usage observation — the internal compaction pump, the outward
/// SSE render task, and the detached non-streaming aggregation — resolves its recorded usage
/// through this tracker, so the override rule is implemented exactly once.
#[derive(Debug, Default)]
pub(crate) struct UsageTracker {
    /// The last ordinary usage the stream reported.
    last_usage: Option<IrUsage>,

    /// The iteration totals the last compaction metadata reported, overriding the ordinary
    /// usage once present.
    iteration_usage: Option<IrUsage>,
}

impl UsageTracker {
    /// Observes one event's usage report, replacing the matching observation. Returns whether
    /// the event carried a usage observation, so stamping call sites record only on new
    /// evidence.
    pub(crate) fn observe(&mut self, event: &IrEvent) -> bool {
        match event {
            IrEvent::Usage(usage) => self.last_usage = Some(*usage),
            IrEvent::CompactionMetadata { iterations, .. } => {
                self.iteration_usage =
                    crate::compaction_model::accounting::reported_iteration_totals(iterations);
            }
            _ => return false,
        }
        true
    }

    /// The usage the stream's accounting records: the iteration totals override the ordinary
    /// usage.
    pub(crate) fn resolved(&self) -> Option<IrUsage> {
        self.iteration_usage.or(self.last_usage)
    }
}

/// Folds one internal upstream response into an aggregation through the given
/// parser, reporting whether a *real* terminal finish arrived: a finish the
/// parser emitted from a terminal event or body. The clean-end-of-stream flush's
/// degraded finish (`crate::format::degrade_finish_once`) does not count — the
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
    log: &mut ApiLogDraft,
) -> Result<(IrAggregation, bool), Error>
where
    P: UpstreamEventParser + ?Sized + Send,
{
    let mut aggregator = Aggregator::default();
    let billed_before = ledger.billed_totals();
    let mut usage_tracker = UsageTracker::default();

    // Whether a real terminal finish arrived: only finishes the parser emitted
    // from its frame or body ingestion count; the clean-end flush's degraded
    // finish is emitted after this flag's last real observation.
    let mut is_terminal_real = false;

    // One event-ingest step over the pump's shared state: every event updates
    // the usage tracker and stamps the resulting billed totals immediately, so
    // failure and cancellation keep billed work, and folds into the
    // aggregation. A terminal finish counts as real only when the event source
    // can carry one — the clean end-of-stream flush's degraded finish
    // (`crate::format::degrade_finish_once`) and the parser's own finish hook
    // never do.
    let mut ingest = |events: Vec<IrEvent>, is_terminal_source: bool| {
        for event in events {
            if is_terminal_source && matches!(event, IrEvent::Finish { .. }) {
                is_terminal_real = true;
            }

            if usage_tracker.observe(&event)
                && let Some(usage) = usage_tracker.resolved()
            {
                log.stamp_billed_usage(Some(billed_before.saturating_sum(usage)));
            }

            aggregator.ingest(&event);
        }
    };

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

                ingest(parser.ingest_frame(&frame)?, true);
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

            ingest(parser.ingest_frame(&frame)?, false);
        }

        ingest(parser.finish(), false);
    } else {
        let bytes = response.bytes().await.map_err(|error| Error::Unavailable {
            detail: format!("upstream read failed: {error}"),
        })?;

        let body = unwrap_response(&bytes, &response_unwrap)?;

        ingest(parser.parse_json_response(body)?, true);
    }

    ledger.record(usage_tracker.resolved());
    Ok((aggregator.finish(), is_terminal_real))
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

impl From<PayloadError> for Error {
    fn from(error: PayloadError) -> Self {
        Self::Unavailable {
            detail: error.to_string(),
        }
    }
}

impl From<BoundaryError> for Error {
    fn from(error: BoundaryError) -> Self {
        match error {
            // Configuration is the server's own gap; its detail reaches the log
            // record only, beside the stable code the error carries.
            BoundaryError::ConfigurationMissing { message } => Self::Internal { message },

            // Client-state failures render as invalid requests, their stable
            // code riding the rendered message.
            error @ (BoundaryError::InvalidGatewayState { .. }
            | BoundaryError::InaccessibleNativeState { .. }) => {
                Self::InvalidRequest(crate::ir::InvalidRequest::request(error.to_string()))
            }
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
            DiscoveryState, Model, Provider, StoredCredentials,
            definition::{
                DefinitionPayload, ProviderDefinition,
                credential::CredentialAcquisition,
                template::{StringExpr, TemplateNode, ValueExpr},
            },
        },
    };
    use reqwest::{Client, StatusCode};
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
