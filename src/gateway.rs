//! Gateway execution pipeline: routing resolution, token acquisition, upstream
//! dispatch, and translation into IR event streams.

use crate::{
    database::Database,
    error::ApiError,
    format::{UpstreamFormat, gemini, openai_chat_completion, openai_response},
    ids::new_uuid,
    ir::{IrEvent, IrRequest, apply_thinking_levels, parse_thinking_levels_override},
    provider::{Credentials, ModelRouting, ProviderType, oauth, storage},
    sse::{SseFrame, SseParser},
};
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Upstream URL of the Codex Responses endpoint.
const CODEX_UPSTREAM_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// `Originator` header value required by the Codex endpoint.
const CODEX_ORIGINATOR: &str = "codex_cli_rs";

/// `User-Agent` header value required by the Codex endpoint.
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.147.0";

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
    database: &Database,
    http: &Client,
    routing: ModelRouting,
    mut ir: IrRequest,
    client_streaming: bool,
) -> Result<IrEventStream, ApiError> {
    let thinking_levels = routing
        .model
        .thinking_levels_override
        .as_deref()
        .map(parse_thinking_levels_override)
        .transpose()?;

    ir.reasoning_effort = apply_thinking_levels(ir.reasoning_effort, thinking_levels.as_deref());
    ir.model = routing.model.upstream_name.clone();

    let provider = oauth::ensure_fresh_token(database, http, &routing.provider).await?;

    let upstream_format = provider.upstream_format()?;

    let request_builder = build_upstream_request(http, &provider, &routing, &ir, client_streaming)?;

    let response = request_builder.send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
    })?;

    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();

        return Err(ApiError::upstream(status.as_u16(), truncate_excerpt(&body)));
    }

    let is_event_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));

    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<IrEvent, ApiError>>(EVENT_CHANNEL_BUFFER);

    if is_event_stream {
        let parser = upstream_parser(upstream_format);

        tokio::spawn(pump_sse(response, parser, sender));
    } else {
        let parser = upstream_parser(upstream_format);

        tokio::spawn(pump_json(response, parser, sender));
    }

    Ok(Box::pin(ReceiverStream::new(receiver)))
}

/// Builds the upstream HTTP request for the provider's wire format.
///
/// # Errors
///
/// Returns [`ApiError`] when the provider configuration cannot render a
/// request.
fn build_upstream_request(
    http: &Client,
    provider: &crate::provider::Provider,
    routing: &ModelRouting,
    ir: &IrRequest,
    client_streaming: bool,
) -> Result<reqwest::RequestBuilder, ApiError> {
    let upstream_format = provider.upstream_format()?;

    let base_url = provider.base_url.as_deref().unwrap_or_default();

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

            let body = openai_response::render_request(ir, true);

            Ok(http
                .post(CODEX_UPSTREAM_URL)
                .bearer_auth(access_token)
                .header("chatgpt-account-id", account_id)
                .header("Originator", CODEX_ORIGINATOR)
                .header("User-Agent", CODEX_USER_AGENT)
                .header("Session-Id", new_uuid())
                .header("Accept", "text/event-stream")
                .json(&body))
        }

        (UpstreamFormat::OpenAiResponse, _) => Ok(http
            .post(format!("{}/responses", base_url.trim_end_matches('/')))
            .bearer_auth(access_token_of(&provider.credentials)?)
            .json(&openai_response::render_request(ir, client_streaming))),

        (UpstreamFormat::OpenAiChatCompletion, _) => Ok(http
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .bearer_auth(access_token_of(&provider.credentials)?)
            .json(&openai_chat_completion::render_request(
                ir,
                client_streaming,
            ))),

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
                &routing.model.upstream_name,
                project_id,
                client_streaming,
            )?;

            Ok(http
                .post(url)
                .bearer_auth(access_token)
                .header("User-Agent", "antigravity")
                .json(&envelope))
        }

        (UpstreamFormat::GeminiGenerateContent, _) => {
            let (url, body) = gemini::render_compatible_request(
                ir,
                base_url,
                &routing.model.upstream_name,
                client_streaming,
            )?;

            Ok(http
                .post(url)
                .header("x-goog-api-key", access_token_of(&provider.credentials)?)
                .json(&body))
        }
    }
}

/// The bearer secret used against an upstream: the API key itself for
/// key-based providers.
///
/// # Errors
///
/// Returns [`ApiError`] when the credential kind cannot authorize a request.
fn access_token_of(credentials: &Credentials) -> Result<&str, ApiError> {
    match credentials {
        Credentials::ApiKey(api_key) => Ok(api_key),
        Credentials::OpenAiOauth { access_token, .. }
        | Credentials::AntigravityOauth { access_token, .. } => Ok(access_token),
    }
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

/// Format-independent upstream parsing contract.
trait UpstreamEventParser {
    /// Ingests one upstream SSE frame.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed upstream payloads.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError>;

    /// Parses one complete upstream JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed upstream payloads.
    fn parse_json_response(&mut self, value: serde_json::Value) -> Result<Vec<IrEvent>, ApiError>;
}

impl UpstreamEventParser for openai_chat_completion::UpstreamParser {
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        Self::ingest_frame(self, frame)
    }

    fn parse_json_response(&mut self, value: serde_json::Value) -> Result<Vec<IrEvent>, ApiError> {
        Self::parse_json_response(self, value)
    }
}

impl UpstreamEventParser for openai_response::UpstreamParser {
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        Self::ingest_frame(self, frame)
    }

    fn parse_json_response(&mut self, value: serde_json::Value) -> Result<Vec<IrEvent>, ApiError> {
        Self::parse_json_response(self, value)
    }
}

impl UpstreamEventParser for gemini::UpstreamParser {
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        Self::ingest_frame(self, frame)
    }

    fn parse_json_response(&mut self, value: serde_json::Value) -> Result<Vec<IrEvent>, ApiError> {
        Self::parse_json_response(self, value)
    }
}

/// Relays an upstream SSE stream into the IR event channel, always
/// terminating with [`IrEvent::Done`].
async fn pump_sse(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, ApiError>>,
) {
    let mut sse_parser = SseParser::new();

    let mut body = response.bytes_stream();

    'stream: while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = sender
                    .send(Err(ApiError::upstream_unavailable(format!(
                        "upstream stream failed: {error}"
                    ))))
                    .await;

                break 'stream;
            }
        };

        let frames = match sse_parser.ingest(&chunk) {
            Ok(frames) => frames,
            Err(_) => {
                let _ = sender
                    .send(Err(ApiError::upstream_unavailable("oversized SSE frame")))
                    .await;

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
                    let _ = sender.send(Err(error)).await;

                    break 'stream;
                }
            }
        }
    }

    if let Ok(frames) = sse_parser.finish() {
        for frame in frames {
            match parser.ingest_frame(&frame) {
                Ok(events) => {
                    for event in events {
                        if sender.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                }

                Err(_) => break,
            }
        }
    }

    let _ = sender.send(Ok(IrEvent::Done)).await;
}

/// Relays an upstream JSON response into the IR event channel, always
/// terminating with [`IrEvent::Done`].
async fn pump_json(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, ApiError>>,
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
            let _ = sender.send(Err(error)).await;
        }
    }

    let _ = sender.send(Ok(IrEvent::Done)).await;
}

/// Truncates an upstream body to a bounded excerpt for error messages.
fn truncate_excerpt(body: &str) -> String {
    const MAX_CHARS: usize = 400;

    if body.chars().count() <= MAX_CHARS {
        body.to_string()
    } else {
        let excerpt: String = body.chars().take(MAX_CHARS).collect();

        format!("{excerpt}…")
    }
}
