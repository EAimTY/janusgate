//! Gateway execution pipeline: routing resolution, token acquisition, upstream dispatch, and
//! translation into IR event streams.

use super::{
    api::Error as ApiError,
    context::Context,
    database::Database,
    format::{
        PayloadError, UpstreamEventParser, UpstreamFormat, anthropic_messages,
        gemini_generate_content, gemini_interaction, openai_chat_completion, openai_response,
    },
    http_body, ids,
    ir::{InvalidRequest, IrEvent, IrRequest},
    provider::{
        ModelRouting, Provider,
        definition::{
            eval::ValueEnv,
            inference::{Inference, UpstreamStreaming},
            template::TemplateNode,
        },
        oauth,
        oauth::Error as OAuthError,
    },
    sse::{SseFrame, SseParser},
    storage::{
        self, Error as StorageError,
        api_logs::{ApiLogDraft, finalize_api_log},
    },
};
use log::{debug, error};
use reqwest::{Client, StatusCode};
use std::pin::Pin;
use tokio_stream::{Stream, StreamExt as _, wrappers::ReceiverStream};

/// Number of IR events buffered between the upstream pump task and the response stream.
const EVENT_CHANNEL_BUFFER: usize = 64;

/// Outward completion pipeline failures, from routing resolution through upstream dispatch. The
/// client request can fail validation here too — the upstream renderers reject requests the chosen
/// upstream cannot carry — so client faults travel in the same type and the surface classifies
/// them.
///
/// The Displays carry the complete human message the outward surface renders; the surface only maps
/// each variant onto its status, `type`, `code`, and `param`.
///
/// **Dispatch timing invariant**: only [`Error::Upstream`] (a mirrored non-2xx status) and
/// [`Error::Unavailable`] (a transport or payload failure) can be produced *after* dispatch —
/// after the upstream `send()` inside [`execute`] — because every other variant arises strictly
/// before it (routing, validation, token refresh, payload rendering). The set of post-dispatch
/// variants therefore contains every failure [`execute`] can report after it stamped the
/// accounting draft, which is what lets [`execute`] finalize the request's accounting row itself,
/// exactly once, and lets every later finalization point classify a stream failure after its
/// conversion into [`ApiError`] (README §3 *ApiLog*).
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

    /// The upstream failed without a usable HTTP response: a transport failure, an oversized frame,
    /// or a malformed payload.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// Token acquisition failed (the lease-guarded refresh).
    #[error(transparent)]
    OAuth(#[from] OAuthError),

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

/// Finalizes the accounting row of a post-dispatch failure inside [`execute`]: classifies the
/// failure through the shared [`ApiError`] accounting — the one classification every finalization
/// point shares — and hands the draft to [`finalize_api_log`], which never reports back —
/// accounting failures stay out of the HTTP error domain (README §8).
async fn finalize_dispatch_failure(context: &Context, log: &ApiLogDraft, error: &ApiError) {
    let (outcome, http_status) = error.accounting();

    finalize_api_log(
        context.api_log_store.as_ref(),
        log,
        outcome,
        Some(http_status),
        None,
    )
    .await;
}

/// A stream of translated IR events; always terminates with [`IrEvent::Done`].
pub(crate) type IrEventStream = Pin<Box<dyn Stream<Item = Result<IrEvent, Error>> + Send>>;

/// Resolves a public routing ID (README §3) to its enabled routing row.
///
/// Zero special-casing: a `vanished` row resolves like any other, and its false three-input
/// effective status lands it in the same 404 `model_not_found` outcome as an unknown or disabled
/// model — same error classification, same message (README §6.5).
///
/// # Errors
///
/// Returns [`Error::ModelNotFound`] for unknown, disabled, or vanished models.
pub(crate) async fn resolve_routing(
    database: &Database,
    public_id: &str,
) -> Result<ModelRouting, Error> {
    match storage::models::find_routing(database, public_id).await? {
        Some(routing) if routing.is_effectively_enabled => Ok(routing),
        _ => Err(Error::ModelNotFound {
            model: public_id.to_string(),
        }),
    }
}

/// Executes a completion request end to end, returning the translated IR event stream.
///
/// The stream is pumped by a spawned task reading the upstream response; the client response is
/// rendered on the handler side from the returned events.
///
/// `log` is the request's accounting draft: this function stamps its dispatch instant the moment
/// before `send()` leaves and — should it then fail after dispatch — finalizes the row itself,
/// leaving an unstamped draft untouched for every pre-dispatch failure (README §3 *ApiLog*).
///
/// # Errors
///
/// Returns [`ApiError`] — converted from the pipeline's own [`Error`] at the boundary, in one
/// place per failure — for routing, token, and upstream failures detected before the response
/// stream starts, and — per the dispatch timing invariant on [`Error`] — for the two
/// post-dispatch failures it finalizes itself.
pub(crate) async fn execute(
    context: &Context,
    routing: ModelRouting,
    mut ir: IrRequest,
    is_client_streaming: bool,
    log: &mut ApiLogDraft,
) -> Result<IrEventStream, ApiError> {
    // The client's reasoning effort string needs no interpretation here: it is opaque end to end
    // (README §6.4) — no stored override is consulted, nothing is filtered or clamped — and the
    // renderers write it verbatim into the upstream's own parameter.
    ir.model = routing.model.upstream_id.clone();

    // Names the upstream in every error-level record the pump tasks emit. Which single site logs
    // such a record is partitioned by the client's streaming mode (README §8): for a streaming
    // client the failure becomes a terminal mid-stream error frame that never passes a response
    // boundary, so the detecting pump logs it; for a non-streaming client it becomes the aggregated
    // HTTP error response, so the response boundary rendering it logs it and the pumps stay silent.
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
        routing.model.public_id,
        upstream_format.as_str(),
        is_client_streaming
    );

    let request_builder = build_upstream_request(
        &context.data_plane,
        &provider,
        &routing,
        &ir,
        is_client_streaming,
    )?;

    // The dispatch mark: everything above ran before dispatch (no accounting row exists yet for
    // any failure above), and everything below ran after it. With accounting disabled the stamp
    // is skipped, and the request stays unrecordable end to end (README §3 *ApiLog*).
    log.stamp();

    let response = match request_builder.send().await {
        Ok(response) => response,

        Err(error) => {
            let error = ApiError::from(Error::Unavailable {
                detail: format!("upstream unreachable: {error}"),
            });

            finalize_dispatch_failure(context, log, &error).await;

            return Err(error);
        }
    };

    let status = response.status();

    if !status.is_success() {
        let error = ApiError::from(upstream_error_of(response).await);

        finalize_dispatch_failure(context, log, &error).await;

        return Err(error);
    }

    debug!("{upstream_label}: upstream responded {status}");

    // The parser is chosen by the upstream streaming mode the gateway requested, never by
    // response-header sniffing: the Codex upstream answers its 200 SSE streams without a
    // `Content-Type` header, and a 200 response to a streaming request is contractually a stream
    // while a 200 response to a non-streaming request is contractually a JSON document (README
    // §6.2).
    let is_upstream_streaming =
        is_upstream_streaming(&provider.definition.payload.inference, is_client_streaming);

    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<IrEvent, Error>>(EVENT_CHANNEL_BUFFER);

    let parser = upstream_parser(upstream_format);

    let response_unwrap = provider
        .definition
        .payload
        .inference
        .response_unwrap
        .clone();

    if is_upstream_streaming {
        tokio::spawn(pump_sse(
            response,
            parser,
            SseParser::new(),
            sender,
            upstream_label,
            is_client_streaming,
            response_unwrap,
        ));
    } else {
        tokio::spawn(pump_json(
            response,
            parser,
            sender,
            upstream_label,
            is_client_streaming,
            response_unwrap,
        ));
    }

    Ok(Box::pin(ReceiverStream::new(receiver)))
}

/// The upstream streaming mode one dispatch requests: the definition's mode decides — always
/// streaming upstream (the forced-streaming upstreams, such as Codex), or matching the client's
/// requested mode. The same decision selects the SSE or JSON response parser.
fn is_upstream_streaming(inference: &Inference, is_client_streaming: bool) -> bool {
    match inference.upstream_streaming {
        UpstreamStreaming::Always => true,
        UpstreamStreaming::MatchClient => is_client_streaming,
    }
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

    let url_expr = match is_upstream_streaming && inference.stream_url.is_some() {
        true => inference.stream_url.as_ref().unwrap_or(&inference.url),
        false => &inference.url,
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

    for row in &inference.headers {
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
) -> Result<reqwest::RequestBuilder, Error> {
    let inference = &provider.definition.payload.inference;

    let envelope = inference.envelope.as_ref();

    match provider.definition.payload.format {
        UpstreamFormat::OpenAiResponse => attach_body(
            builder,
            env,
            envelope,
            &openai_response::render_request_body(
                ir,
                is_upstream_streaming,
                &inference.render_options,
            ),
        ),

        UpstreamFormat::OpenAiChatCompletion => attach_body(
            builder,
            env,
            envelope,
            &openai_chat_completion::render_request_body(ir, is_upstream_streaming),
        ),

        UpstreamFormat::GeminiGenerateContent => attach_body(
            builder,
            env,
            envelope,
            &gemini_generate_content::render_generate_content_request(ir).map_err(Error::from)?,
        ),

        UpstreamFormat::GeminiInteraction => attach_body(
            builder,
            env,
            envelope,
            &gemini_interaction::render_request_body(ir, &ir.model, is_upstream_streaming)
                .map_err(Error::from)?,
        ),

        UpstreamFormat::AnthropicMessages => attach_body(
            builder,
            env,
            envelope,
            &anthropic_messages::render_request_body(ir, is_upstream_streaming)
                .map_err(Error::from)?,
        ),
    }
}

/// Attaches one codec-rendered body to the request builder: without an envelope the typed body
/// rides to reqwest and serializes exactly once onto the wire; with one, the body converts into
/// the JSON value the envelope's template DOM splices — the one fragment whose shape is not the
/// gateway's to define — and the rendered envelope replaces the body wholesale.
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
        .render_json(envelope, serialize_body(body), "inference envelope")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    Ok(builder.json(&body))
}

/// Serializes one codec-rendered body into the JSON value the envelope's template DOM splices: a
/// first-party serde type always renders, so the failure arm is the documented unreachable
/// invariant.
///
/// # Panics
///
/// Panics if serialization fails, which cannot occur for a first-party serde type.
fn serialize_body<T: serde::Serialize>(body: &T) -> serde_json::Value {
    #[expect(
        clippy::unwrap_used,
        reason = "a codec-rendered body is a first-party serde type, which always renders"
    )]
    serde_json::to_value(body).unwrap()
}

/// Boxes the upstream event parser for the given wire format.
fn upstream_parser(upstream_format: UpstreamFormat) -> Box<dyn UpstreamEventParser + Send> {
    match upstream_format {
        UpstreamFormat::OpenAiResponse => Box::new(openai_response::UpstreamParser::new()),
        UpstreamFormat::OpenAiChatCompletion => {
            Box::new(openai_chat_completion::UpstreamParser::new())
        }
        UpstreamFormat::GeminiGenerateContent => {
            Box::new(gemini_generate_content::UpstreamParser::new())
        }
        UpstreamFormat::GeminiInteraction => Box::new(gemini_interaction::UpstreamParser::new()),
        UpstreamFormat::AnthropicMessages => Box::new(anthropic_messages::UpstreamParser::new()),
    }
}

/// Delivers an upstream stream failure as the stream's terminal error frame, marking the stream
/// aborted so no events follow the error frame, and logs it only when the client is streaming: then
/// the failure renders as a mid-stream error frame that never passes a response boundary, so the
/// detecting pump is its single logger (README §8); a non-streaming client instead renders it as
/// the aggregated HTTP error response, which the response boundary logs. The send is best-effort: a
/// consumer that already went away must not fail or block the abort.
async fn abort_stream(
    is_aborted: &mut bool,
    sender: &tokio::sync::mpsc::Sender<Result<IrEvent, Error>>,
    upstream_label: &str,
    is_client_streaming: bool,
    error: Error,
) {
    *is_aborted = true;

    if is_client_streaming {
        error!("{upstream_label}: {error}");
    }

    let _ = sender.send(Err(error)).await;
}

/// Aborts an SSE pump for a framing-layer rejection — an oversized frame — stating the policy
/// once: a framing-layer rejection aborts identically wherever it surfaces, in the main chunk
/// loop or at the trailing flush (README §6.3).
async fn abort_oversized_frame(
    is_aborted: &mut bool,
    sender: &tokio::sync::mpsc::Sender<Result<IrEvent, Error>>,
    upstream_label: &str,
    is_client_streaming: bool,
) {
    abort_stream(
        is_aborted,
        sender,
        upstream_label,
        is_client_streaming,
        Error::Unavailable {
            detail: "oversized SSE frame".to_string(),
        },
    )
    .await;
}

/// Forwards parsed IR events to the consumer, reporting whether the consumer is still listening. A
/// closed channel ends the forwarding without an error frame: the consumer left, so nobody is left
/// to receive one.
async fn forward_events(
    sender: &tokio::sync::mpsc::Sender<Result<IrEvent, Error>>,
    events: Vec<IrEvent>,
) -> bool {
    for event in events {
        if sender.send(Ok(event)).await.is_err() {
            return false;
        }
    }

    true
}

/// Feeds a batch of SSE frames through the upstream parser and forwards the produced events,
/// reporting whether the stream is still live. A parser failure aborts the stream through
/// [`abort_stream`]; a departed consumer stops the forwarding without touching the abort flag, so
/// the caller's trailing flush still runs exactly as it does for a consumer that leaves anywhere
/// else mid-stream.
async fn forward_frames(
    is_aborted: &mut bool,
    parser: &mut Box<dyn UpstreamEventParser + Send>,
    frames: Vec<SseFrame>,
    sender: &tokio::sync::mpsc::Sender<Result<IrEvent, Error>>,
    upstream_label: &str,
    is_client_streaming: bool,
) -> bool {
    for frame in frames {
        match parser.ingest_frame(&frame) {
            Ok(events) => {
                if !forward_events(sender, events).await {
                    return false;
                }
            }

            // A malformed frame aborts the stream with an error exactly like any other upstream
            // failure; the caller's final `Done` still terminates the event stream.
            Err(parser_error) => {
                abort_stream(
                    is_aborted,
                    sender,
                    upstream_label,
                    is_client_streaming,
                    Error::from(parser_error),
                )
                .await;

                return false;
            }
        }
    }

    true
}

/// Relays an upstream SSE stream into the IR event channel, always terminating with
/// [`IrEvent::Done`].
///
/// `upstream_label` names the upstream in the pump's error records. The pump logs a failure only
/// for a streaming client, whose terminal error frame never reaches a response boundary, making the
/// pump the failure's single logger (README §8); a non-streaming client receives the failure as the
/// aggregated HTTP error response, which the response boundary logs.
async fn pump_sse(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    mut sse_parser: SseParser,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, Error>>,
    upstream_label: String,
    is_client_streaming: bool,
    response_unwrap: Option<String>,
) {
    let mut body = response.bytes_stream();

    // Whether an error aborted the stream; only a clean end of stream lets the parser flush
    // buffered state before the terminal `Done`.
    let mut is_aborted = false;

    'stream: while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                abort_stream(
                    &mut is_aborted,
                    &sender,
                    &upstream_label,
                    is_client_streaming,
                    Error::Unavailable {
                        detail: format!("upstream stream failed: {error}"),
                    },
                )
                .await;

                break 'stream;
            }
        };

        let frames = match sse_parser.ingest(&chunk) {
            Ok(frames) => frames
                .into_iter()
                .map(|mut frame| {
                    unwrap_frame(&mut frame, &response_unwrap);

                    frame
                })
                .collect(),
            Err(_) => {
                abort_oversized_frame(
                    &mut is_aborted,
                    &sender,
                    &upstream_label,
                    is_client_streaming,
                )
                .await;

                break 'stream;
            }
        };

        let is_stream_live = forward_frames(
            &mut is_aborted,
            &mut parser,
            frames,
            &sender,
            &upstream_label,
            is_client_streaming,
        )
        .await;

        if !is_stream_live {
            break 'stream;
        }
    }

    // Trailing frames buffered in the SSE parser are flushed through the parser only on a clean end
    // of stream: replaying them after an abort would emit events after the stream's error and break
    // the error-then-`Done` contract of the event stream.
    if !is_aborted {
        match sse_parser.finish() {
            Ok(frames) => {
                forward_frames(
                    &mut is_aborted,
                    &mut parser,
                    frames,
                    &sender,
                    &upstream_label,
                    is_client_streaming,
                )
                .await;
            }

            // Silently completing the stream here would drop the trailing frame on the floor. The
            // final `Done` below still terminates the event stream.
            Err(_) => {
                abort_oversized_frame(
                    &mut is_aborted,
                    &sender,
                    &upstream_label,
                    is_client_streaming,
                )
                .await;
            }
        }
    }

    if !is_aborted {
        forward_events(&sender, parser.finish()).await;
    }

    let _ = sender.send(Ok(IrEvent::Done)).await;
}

/// Relays an upstream JSON response into the IR event channel, always terminating with
/// [`IrEvent::Done`].
///
/// `upstream_label` names the upstream in the pump's error records. The pump logs a failure only
/// for a streaming client, whose terminal error frame never reaches a response boundary, making the
/// pump the failure's single logger (README §8); a non-streaming client receives the failure as the
/// aggregated HTTP error response, which the response boundary logs.
async fn pump_json(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, Error>>,
    upstream_label: String,
    is_client_streaming: bool,
    response_unwrap: Option<String>,
) {
    let outcome = async {
        let bytes = response.bytes().await.map_err(|error| Error::Unavailable {
            detail: format!("upstream read failed: {error}"),
        })?;

        let mut value = serde_json::from_slice(&bytes).map_err(|error| Error::Unavailable {
            detail: format!("malformed upstream response: {error}"),
        })?;

        unwrap_response(&mut value, &response_unwrap);

        parser.parse_json_response(value).map_err(Error::from)
    }
    .await;

    match outcome {
        Ok(events) => {
            forward_events(&sender, events).await;
        }

        Err(error) => {
            if is_client_streaming {
                error!("{upstream_label}: {error}");
            }

            let _ = sender.send(Err(error)).await;
        }
    }

    let _ = sender.send(Ok(IrEvent::Done)).await;
}

/// Unwraps one JSON response through the definition's configured envelope member: when
/// configured and present, the member's value becomes the parser's document. Absent or not
/// configured, the document is ingested as-is (README §5.2 `response_unwrap`).
fn unwrap_response(value: &mut serde_json::Value, response_unwrap: &Option<String>) {
    let Some(member) = response_unwrap else {
        return;
    };

    // The member is moved out of the parent object instead of cloned, so an unwrapped body is
    // never copied a second time on this path.
    if let serde_json::Value::Object(members) = value
        && let Some(ingest) = members.remove(member)
    {
        *value = ingest;
    }
}

/// Unwraps one SSE frame's payload the same way the JSON responses unwrap: the frame's data is
/// rewritten to the configured member's value when the frame carries a JSON object with it.
/// Frames whose data is not such a JSON object — a `[DONE]` sentinel, for instance — pass
/// through unchanged.
fn unwrap_frame(frame: &mut SseFrame, response_unwrap: &Option<String>) {
    let Some(member) = response_unwrap else {
        return;
    };

    let Ok(value) = serde_json::from_str::<serde_json::Value>(&frame.data) else {
        return;
    };

    if let Some(wrapped) = value.get(member) {
        frame.data = serialize_frame(wrapped);
    }
}

/// Re-serializes one unwrapped frame payload.
///
/// # Panics
///
/// Panics if serialization fails, which cannot occur for a parsed JSON value.
fn serialize_frame(value: &serde_json::Value) -> String {
    #[expect(
        clippy::unwrap_used,
        reason = "a re-serialized JSON value always renders"
    )]
    serde_json::to_string(value).unwrap()
}

/// Builds the upstream error for a non-2xx response through the shared bounded error-body read
/// ([`http_body::read_error_body`]) and the one excerpt policy ([`http_body::truncate_excerpt`]):
/// the full body is never buffered, because a hostile upstream must not be able to stream unbounded
/// bytes into gateway memory just to shape an error message.
async fn upstream_error_of(response: reqwest::Response) -> Error {
    let status = response.status();

    let body = http_body::read_error_body(response).await;

    Error::Upstream {
        status,
        detail: http_body::truncate_excerpt(&body),
    }
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
    fn fixture_ir() -> IrRequest {
        IrRequest {
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
    fn fixture_provider(envelope: Option<TemplateNode>) -> Provider {
        Provider {
            id: 1,
            name: "test-provider".to_string(),
            model_prefix: String::new(),
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
                    },
                    credential: CredentialAcquisition::ApiKey,
                    provisioning: None,
                    discovery: None,
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
            provider: provider.clone(),
            model: Model {
                id: 1,
                upstream_id: ir.model.clone(),
                public_id: "public-model".to_string(),
                id_override: None,
                is_enabled: true,
                discovery_state: DiscoveryState::Manual,
            },
            is_effectively_enabled: true,
        };

        let request = build_upstream_request(http, provider, &routing, ir, false)
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
        serde_json::to_value(openai_chat_completion::render_request_body(ir, false))
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
