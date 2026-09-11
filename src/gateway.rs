//! Gateway execution pipeline: routing resolution, token acquisition, upstream
//! dispatch, and translation into IR event streams.

use crate::{
    context::Context,
    database::Database,
    error::ApiError,
    format::{
        UpstreamEventParser, UpstreamFormat, gemini, openai_chat_completion, openai_response,
    },
    ids::new_uuid,
    ir::{IrEvent, IrRequest, apply_thinking_levels, parse_thinking_levels_override},
    provider::{
        Credentials, ModelRouting, Provider, ProviderType, endpoints, http_body, oauth, storage,
    },
    sse::SseParser,
};
use futures::Stream;
use log::{debug, error};
use reqwest::Client;
use std::pin::Pin;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Number of IR events buffered between the upstream pump task and the
/// response stream.
const EVENT_CHANNEL_BUFFER: usize = 64;

/// A stream of translated IR events; always terminates with
/// [`IrEvent::Done`].
pub(crate) type IrEventStream = Pin<Box<dyn Stream<Item = Result<IrEvent, ApiError>> + Send>>;

/// Resolves a public model name to its enabled routing row.
///
/// # Errors
///
/// Returns [`ApiError`] with `model_not_found` for unknown or disabled
/// models.
pub(crate) async fn resolve_routing(
    database: &Database,
    public_name: &str,
) -> Result<ModelRouting, ApiError> {
    match storage::find_routing(database, public_name).await? {
        Some(routing) if routing.is_enabled => Ok(routing),
        _ => Err(ApiError::model_not_found(public_name)),
    }
}

/// Executes a completion request end to end, returning the translated IR
/// event stream.
///
/// The stream is pumped by a spawned task reading the upstream response; the
/// client response is rendered on the handler side from the returned events.
///
/// # Errors
///
/// Returns [`ApiError`] for routing, token, and upstream failures detected
/// before the response stream starts.
pub(crate) async fn execute(
    context: &Context,
    routing: ModelRouting,
    mut ir: IrRequest,
    is_client_streaming: bool,
) -> Result<IrEventStream, ApiError> {
    // Creation validates `thinking_levels_override` at the management API, so
    // a malformed stored value is drift inside the database — an invariant
    // violation — and never a fault of the requesting client.
    let thinking_levels = routing
        .model
        .thinking_levels_override
        .as_deref()
        .map(|json| {
            parse_thinking_levels_override(json).map_err(|error| {
                ApiError::internal(format!(
                    "stored thinking_levels_override is invalid: {error}"
                ))
            })
        })
        .transpose()?;

    ir.reasoning_effort = apply_thinking_levels(ir.reasoning_effort, thinking_levels.as_deref());
    ir.model = routing.model.upstream_id.clone();

    // Names every mid-stream log record the pump tasks emit; the pump errors
    // surface to streaming clients as terminal error frames and never pass a
    // response boundary, so the pump is the only place they are logged.
    let upstream_label = format!(
        "provider `{}` upstream model `{}`",
        routing.provider.name, ir.model
    );

    // The token refresh is a control-plane round-trip, so it rides the
    // bounded client even though the completion itself is data-plane traffic.
    let provider =
        oauth::ensure_fresh_token(&context.database, &context.control_plane, &routing.provider)
            .await?;

    // The wire format is resolved once per request and shared by request
    // building and event parsing.
    let upstream_format = provider.upstream_format()?;

    debug!(
        "dispatching public model `{}` via {upstream_label} ({}, streaming: {})",
        routing.model.id,
        upstream_format.as_str(),
        is_client_streaming
    );

    let request_builder = build_upstream_request(
        &context.data_plane,
        upstream_format,
        &provider,
        &routing,
        &ir,
        is_client_streaming,
    )?;

    let response = request_builder.send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
    })?;

    let status = response.status();

    if !status.is_success() {
        return Err(upstream_error_of(response).await);
    }

    debug!("{upstream_label}: upstream responded {status}");

    // The parser is chosen by the upstream streaming mode the gateway
    // requested, never by response-header sniffing: the Codex upstream
    // answers its 200 SSE streams without a `Content-Type` header, and a
    // 200 response to a streaming request is contractually a stream while a
    // 200 response to a non-streaming request is contractually a JSON
    // document (README §6.2).
    let is_upstream_streaming =
        is_upstream_streaming(upstream_format, provider.provider_type, is_client_streaming);

    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<IrEvent, ApiError>>(EVENT_CHANNEL_BUFFER);

    let parser = upstream_parser(upstream_format);

    if is_upstream_streaming {
        tokio::spawn(pump_sse(
            response,
            parser,
            SseParser::new(),
            sender,
            upstream_label,
        ));
    } else {
        tokio::spawn(pump_json(response, parser, sender, upstream_label));
    }

    Ok(Box::pin(ReceiverStream::new(receiver)))
}

/// Decides the upstream streaming mode the gateway requests: Codex always
/// streams upstream — the ChatGPT Codex endpoint requires `stream: true`
/// (README §5.1 invariant 2) — and every other built-in upstream matches the
/// client's requested mode (README §6.3). The same decision selects the SSE
/// or JSON response parser.
fn is_upstream_streaming(
    upstream_format: UpstreamFormat,
    provider_type: ProviderType,
    is_client_streaming: bool,
) -> bool {
    matches!(
        (upstream_format, provider_type),
        (UpstreamFormat::OpenAiResponse, ProviderType::OpenAiOauth)
    ) || is_client_streaming
}

/// Builds the upstream HTTP request for the provider's wire format.
///
/// # Errors
///
/// Returns [`ApiError`] when the provider configuration cannot render a
/// request.
fn build_upstream_request(
    http: &Client,
    upstream_format: UpstreamFormat,
    provider: &Provider,
    routing: &ModelRouting,
    ir: &IrRequest,
    is_client_streaming: bool,
) -> Result<reqwest::RequestBuilder, ApiError> {
    match (upstream_format, provider.provider_type) {
        (UpstreamFormat::OpenAiResponse, ProviderType::OpenAiOauth) => {
            let Credentials::OpenAiOauth {
                access_token,
                account_id,
                ..
            } = &provider.credentials
            else {
                return Err(ApiError::internal(
                    "codex provider lacks OpenAI OAuth credentials",
                ));
            };

            let body = openai_response::render_codex_request(ir);

            Ok(http
                .post(endpoints::CODEX_INFERENCE_URL)
                .bearer_auth(access_token)
                .header("chatgpt-account-id", account_id)
                .header("Originator", endpoints::CODEX_ORIGINATOR)
                .header("User-Agent", endpoints::codex_user_agent())
                .header("Session-Id", new_uuid())
                .header("Accept", "text/event-stream")
                .json(&body))
        }

        (UpstreamFormat::OpenAiResponse, _) => {
            let base_url = compatible_base_url(provider)?;

            Ok(http
                .post(format!("{}/responses", base_url.trim_end_matches('/')))
                .bearer_auth(provider.credentials.secret())
                .json(&openai_response::render_request(ir, is_client_streaming)))
        }

        (UpstreamFormat::OpenAiChatCompletion, _) => {
            let base_url = compatible_base_url(provider)?;

            Ok(http
                .post(format!(
                    "{}/chat/completions",
                    base_url.trim_end_matches('/')
                ))
                .bearer_auth(provider.credentials.secret())
                .json(&openai_chat_completion::render_request(
                    ir,
                    is_client_streaming,
                )))
        }

        (UpstreamFormat::GeminiGenerateContent, ProviderType::AntigravityOauth) => {
            let Credentials::AntigravityOauth {
                access_token,
                project_id,
                ..
            } = &provider.credentials
            else {
                return Err(ApiError::internal(
                    "antigravity provider lacks Google OAuth credentials",
                ));
            };

            let (url, envelope) = gemini::render_antigravity_request(
                ir,
                endpoints::ANTIGRAVITY_BASE_URL,
                &routing.model.upstream_id,
                project_id,
                is_client_streaming,
            )?;

            Ok(http
                .post(url)
                .bearer_auth(access_token)
                .header("User-Agent", endpoints::ANTIGRAVITY_USER_AGENT)
                .json(&envelope))
        }

        (UpstreamFormat::GeminiGenerateContent, _) => {
            let (url, body) = gemini::render_compatible_request(
                ir,
                compatible_base_url(provider)?,
                &routing.model.upstream_id,
                is_client_streaming,
            )?;

            Ok(http
                .post(url)
                .header("x-goog-api-key", provider.credentials.secret())
                .json(&body))
        }
    }
}

/// Resolves the stored base URL of a compatible provider.
///
/// OAuth providers carry fixed endpoints and never consult this. Creation
/// validates `base_url` at the management API, so a missing value is stored
/// drift — an invariant violation — and never a fault of the requesting
/// client.
fn compatible_base_url(provider: &Provider) -> Result<&str, ApiError> {
    provider
        .base_url
        .as_deref()
        .ok_or_else(|| ApiError::internal("compatible provider lacks a stored base_url"))
}

/// Boxes the upstream event parser for the given wire format.
fn upstream_parser(upstream_format: UpstreamFormat) -> Box<dyn UpstreamEventParser + Send> {
    match upstream_format {
        UpstreamFormat::OpenAiResponse => Box::new(openai_response::UpstreamParser::new()),
        UpstreamFormat::OpenAiChatCompletion => {
            Box::new(openai_chat_completion::UpstreamParser::new())
        }
        UpstreamFormat::GeminiGenerateContent => Box::new(gemini::UpstreamParser::new()),
    }
}

/// Relays an upstream SSE stream into the IR event channel, always
/// terminating with [`IrEvent::Done`].
///
/// `upstream_label` names the upstream in every error the pump logs; these
/// failures become terminal error frames for streaming clients and never
/// reach a response boundary, so this is the only place they are logged.
async fn pump_sse(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    mut sse_parser: SseParser,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, ApiError>>,
    upstream_label: String,
) {
    let mut body = response.bytes_stream();

    // Whether an error aborted the stream; only a clean end of stream lets
    // the parser flush buffered state before the terminal `Done`.
    let mut is_aborted = false;

    'stream: while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                is_aborted = true;

                let error =
                    ApiError::upstream_unavailable(format!("upstream stream failed: {error}"));

                error!("{upstream_label}: {error}");

                let _ = sender.send(Err(error)).await;

                break 'stream;
            }
        };

        let frames = match sse_parser.ingest(&chunk) {
            Ok(frames) => frames,
            Err(_) => {
                is_aborted = true;

                let error = ApiError::upstream_unavailable("oversized SSE frame");

                error!("{upstream_label}: {error}");

                let _ = sender.send(Err(error)).await;

                break 'stream;
            }
        };

        for frame in frames {
            match parser.ingest_frame(&frame) {
                Ok(events) => {
                    for event in events {
                        if sender.send(Ok(event)).await.is_err() {
                            break 'stream;
                        }
                    }
                }

                Err(error) => {
                    is_aborted = true;

                    error!("{upstream_label}: {error}");

                    let _ = sender.send(Err(error)).await;

                    break 'stream;
                }
            }
        }
    }

    // Trailing frames buffered in the SSE parser are flushed through the
    // parser only on a clean end of stream: replaying them after an abort
    // would emit events after the stream's error and break the
    // error-then-`Done` contract of the event stream.
    if !is_aborted {
        match sse_parser.finish() {
            Ok(frames) => {
                for frame in frames {
                    match parser.ingest_frame(&frame) {
                        Ok(events) => {
                            for event in events {
                                if sender.send(Ok(event)).await.is_err() {
                                    break;
                                }
                            }
                        }

                        // A malformed trailing frame aborts the stream with an
                        // error exactly like a malformed frame in the main
                        // loop; the final `Done` below still terminates the
                        // event stream.
                        Err(error) => {
                            is_aborted = true;

                            error!("{upstream_label}: {error}");

                            let _ = sender.send(Err(error)).await;

                            break;
                        }
                    }
                }
            }

            // A trailing frame the framing layer rejects at flush time aborts
            // the stream exactly like an oversized frame in the main loop;
            // silently completing the stream here would drop the trailing
            // frame on the floor. The final `Done` below still terminates the
            // event stream.
            Err(_) => {
                is_aborted = true;

                let error = ApiError::upstream_unavailable("oversized SSE frame");

                error!("{upstream_label}: {error}");

                let _ = sender.send(Err(error)).await;
            }
        }
    }

    if !is_aborted {
        for event in parser.finish() {
            if sender.send(Ok(event)).await.is_err() {
                break;
            }
        }
    }

    let _ = sender.send(Ok(IrEvent::Done)).await;
}

/// Relays an upstream JSON response into the IR event channel, always
/// terminating with [`IrEvent::Done`].
///
/// `upstream_label` names the upstream in every error the pump logs; for a
/// streaming client these failures become terminal error frames and never
/// reach a response boundary, so the pump is the only place they are logged.
async fn pump_json(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, ApiError>>,
    upstream_label: String,
) {
    let outcome = async {
        let bytes = response.bytes().await.map_err(|error| {
            ApiError::upstream_unavailable(format!("upstream read failed: {error}"))
        })?;

        let value = serde_json::from_slice(&bytes).map_err(|error| {
            ApiError::upstream_unavailable(format!("malformed upstream response: {error}"))
        })?;

        parser.parse_json_response(value)
    }
    .await;

    match outcome {
        Ok(events) => {
            for event in events {
                if sender.send(Ok(event)).await.is_err() {
                    break;
                }
            }
        }

        Err(error) => {
            error!("{upstream_label}: {error}");

            let _ = sender.send(Err(error)).await;
        }
    }

    let _ = sender.send(Ok(IrEvent::Done)).await;
}

/// Builds the upstream error for a non-2xx response through the shared
/// bounded error-body read ([`http_body::read_error_body`]) and the one
/// excerpt policy ([`http_body::truncate_excerpt`]): the full body is never
/// buffered, because a hostile upstream must not be able to stream unbounded
/// bytes into gateway memory just to shape an error message.
async fn upstream_error_of(response: reqwest::Response) -> ApiError {
    let status = response.status();

    let body = http_body::read_error_body(response).await;

    ApiError::upstream(status.as_u16(), http_body::truncate_excerpt(&body))
}
