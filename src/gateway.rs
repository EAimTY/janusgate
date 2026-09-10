//! Gateway execution pipeline: routing resolution, token acquisition, upstream
//! dispatch, and translation into IR event streams.

use crate::{
    database::Database,
    error::ApiError,
    format::{
        UpstreamEventParser, UpstreamFormat, gemini, openai_chat_completion, openai_response,
    },
    ids::new_uuid,
    ir::{IrEvent, IrRequest, apply_thinking_levels, parse_thinking_levels_override},
    provider::{Credentials, ModelRouting, Provider, ProviderType, endpoints, oauth, storage},
    sse::SseParser,
};
use futures::Stream;
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
    database: &Database,
    http: &Client,
    routing: ModelRouting,
    mut ir: IrRequest,
    client_streaming: bool,
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
    ir.model = routing.model.upstream_name.clone();

    let provider = oauth::ensure_fresh_token(database, http, &routing.provider).await?;

    // The wire format is resolved once per request and shared by request
    // building and event parsing.
    let upstream_format = provider.upstream_format()?;

    let request_builder = build_upstream_request(
        http,
        upstream_format,
        &provider,
        &routing,
        &ir,
        client_streaming,
    )?;

    let response = request_builder.send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
    })?;

    let status = response.status();

    if !status.is_success() {
        return Err(upstream_error_of(response).await);
    }

    let is_event_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));

    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<IrEvent, ApiError>>(EVENT_CHANNEL_BUFFER);

    let parser = upstream_parser(upstream_format);

    if is_event_stream {
        tokio::spawn(pump_sse(response, parser, sender));
    } else {
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
    upstream_format: UpstreamFormat,
    provider: &Provider,
    routing: &ModelRouting,
    ir: &IrRequest,
    client_streaming: bool,
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

            let body = openai_response::render_request(ir, true);

            Ok(http
                .post(endpoints::CODEX_INFERENCE_URL)
                .bearer_auth(access_token)
                .header("chatgpt-account-id", account_id)
                .header("Originator", endpoints::CODEX_ORIGINATOR)
                .header("User-Agent", endpoints::CODEX_USER_AGENT)
                .header("Session-Id", new_uuid())
                .header("Accept", "text/event-stream")
                .json(&body))
        }

        (UpstreamFormat::OpenAiResponse, _) => {
            let base_url = compatible_base_url(provider)?;

            Ok(http
                .post(format!("{}/responses", base_url.trim_end_matches('/')))
                .bearer_auth(provider.credentials.secret())
                .json(&openai_response::render_request(ir, client_streaming)))
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
                    client_streaming,
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
                &routing.model.upstream_name,
                project_id,
                client_streaming,
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
                &routing.model.upstream_name,
                client_streaming,
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
async fn pump_sse(
    response: reqwest::Response,
    mut parser: Box<dyn UpstreamEventParser + Send>,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, ApiError>>,
) {
    let mut sse_parser = SseParser::new();

    let mut body = response.bytes_stream();

    // Whether an error aborted the stream; only a clean end of stream lets
    // the parser flush buffered state before the terminal `Done`.
    let mut is_aborted = false;

    'stream: while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                is_aborted = true;

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
                is_aborted = true;

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
                    is_aborted = true;

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
    if !is_aborted && let Ok(frames) = sse_parser.finish() {
        for frame in frames {
            match parser.ingest_frame(&frame) {
                Ok(events) => {
                    for event in events {
                        if sender.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                }

                // A malformed trailing frame aborts the stream with an error
                // exactly like a malformed frame in the main loop; the final
                // `Done` below still terminates the event stream.
                Err(error) => {
                    is_aborted = true;

                    let _ = sender.send(Err(error)).await;

                    break;
                }
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

/// Maximum number of bytes read from a non-2xx upstream body before it is
/// truncated into an error message. The excerpt is capped at 400 characters
/// anyway, so a few kilobytes leave ample margin for multi-byte characters
/// while bounding what a hostile upstream can make the gateway buffer.
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

/// Builds the upstream error for a non-2xx response, reading at most
/// [`MAX_ERROR_BODY_BYTES`] of its body. The full body is never buffered:
/// a hostile upstream must not be able to stream unbounded bytes into
/// gateway memory just to shape an error message.
async fn upstream_error_of(response: reqwest::Response) -> ApiError {
    let status = response.status();

    let mut stream = response.bytes_stream();

    let mut bytes: Vec<u8> = Vec::new();

    while bytes.len() < MAX_ERROR_BODY_BYTES {
        match stream.next().await {
            Some(Ok(chunk)) => bytes.extend_from_slice(&chunk),
            Some(Err(_)) | None => break,
        }
    }

    // A chunk boundary can split a multi-byte character; the excerpt is
    // human-facing, so a replacement character is acceptable.
    let body = String::from_utf8_lossy(&bytes);

    ApiError::upstream(status.as_u16(), truncate_excerpt(&body))
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

#[cfg(test)]
mod tests {
    use super::{compatible_base_url, execute, pump_sse};
    use crate::{
        error::{ApiError, ApiErrorKind},
        format::{
            UpstreamEventParser, UpstreamFormat,
            openai_chat_completion::UpstreamParser as ChatUpstreamParser,
        },
        ir::{IrEvent, IrRequest},
        provider::{Credentials, Model, ModelRouting, Provider, ProviderType},
        sse::SseFrame,
        test_support,
    };
    use std::{
        error::Error as StdError,
        io::{Read as _, Write as _},
        time::Duration,
    };

    /// Serves one raw HTTP response on a loopback socket after reading the
    /// client request, returning the bound address.
    ///
    /// # Panics
    ///
    /// Panics if the socket fails, which fails the test.
    fn serve_once(head: String, body: &[u8]) -> std::net::SocketAddr {
        #[expect(clippy::expect_used, reason = "a bind failure fails the test")]
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");

        #[expect(
            clippy::expect_used,
            reason = "an address lookup failure fails the test"
        )]
        let address = listener
            .local_addr()
            .expect("bound listener should report an address");

        let body = body.to_vec();

        std::thread::spawn(move || {
            #[expect(clippy::expect_used, reason = "a socket failure fails the test")]
            let mut socket = listener.accept().expect("accept should succeed").0;

            // Read the client request before responding: a response that
            // arrives while the request is still being processed makes the
            // HTTP client reject the connection with an unexpected-message
            // error instead of returning the response.
            let mut request = Vec::new();
            let mut scratch = [0u8; 1024];

            loop {
                #[expect(clippy::expect_used, reason = "a read failure fails the test")]
                let read = socket
                    .read(&mut scratch)
                    .expect("request should be readable");

                #[expect(
                    clippy::indexing_slicing,
                    reason = "Read::read returns a count within the buffer length"
                )]
                request.extend_from_slice(&scratch[..read]);

                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }

            #[expect(clippy::expect_used, reason = "a write failure fails the test")]
            socket.write_all(head.as_bytes()).expect("head is writable");

            #[expect(clippy::expect_used, reason = "a write failure fails the test")]
            socket.write_all(&body).expect("body is writable");
        });

        address
    }

    /// Points the process-global HTTP client at a plain client, exactly once
    /// per test process.
    ///
    /// # Panics
    ///
    /// Panics if the test HTTP client fails to build, which fails the test.
    fn init_http_client() {
        // Tests share the process-global client; the first initialization
        // wins and later ones are ignored.
        #[expect(clippy::expect_used, reason = "a client build failure fails the test")]
        let client = reqwest::Client::builder()
            .build()
            .expect("test HTTP client should build");

        let _ = crate::HTTP.set(client);
    }

    /// Upstream parser that rejects every frame, standing in for a parser
    /// failing on a malformed payload.
    struct RejectingParser;

    impl UpstreamEventParser for RejectingParser {
        fn ingest_frame(&mut self, _frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
            Err(ApiError::upstream_unavailable("malformed frame"))
        }

        fn parse_json_response(
            &mut self,
            _value: serde_json::Value,
        ) -> Result<Vec<IrEvent>, ApiError> {
            Ok(Vec::new())
        }
    }

    /// Upstream parser that records whether the end-of-stream hook ran,
    /// standing in for a parser holding flushable state.
    struct FinishRecordingParser {
        is_finished: bool,
    }

    impl UpstreamEventParser for FinishRecordingParser {
        fn ingest_frame(&mut self, _frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
            Err(ApiError::upstream_unavailable("malformed frame"))
        }

        fn parse_json_response(
            &mut self,
            _value: serde_json::Value,
        ) -> Result<Vec<IrEvent>, ApiError> {
            Ok(Vec::new())
        }

        fn finish(&mut self) -> Vec<IrEvent> {
            self.is_finished = true;

            vec![IrEvent::TextDelta("flushed".to_string())]
        }
    }

    /// Drives `pump_sse` against a loopback upstream serving one SSE response
    /// and collects the emitted events.
    ///
    /// # Panics
    ///
    /// Panics if a request failure fails the test.
    async fn pump_fixture(
        body: &[u8],
        parser: Box<dyn UpstreamEventParser + Send>,
    ) -> Vec<Result<IrEvent, ApiError>> {
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );

        let address = serve_once(head, body);

        init_http_client();

        #[expect(clippy::expect_used, reason = "a request failure fails the test")]
        let response = crate::http()
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("local upstream should respond");

        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);

        pump_sse(response, parser, sender).await;

        let mut events = Vec::new();

        while let Some(event) = receiver.recv().await {
            events.push(event);
        }

        events
    }

    /// Verifies the end-of-stream hook flushes a parser's buffered state on a
    /// clean EOF and is skipped once an error aborts the stream.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn finishes_parsers_only_on_clean_eof() {
        // Clean EOF: an empty SSE body ends the stream without error.
        let events =
            pump_fixture(b"", Box::new(FinishRecordingParser { is_finished: false })).await;

        let is_flush_then_done = matches!(
            events.first(),
            Some(Ok(IrEvent::TextDelta(delta))) if delta == "flushed"
        ) && matches!(events.get(1), Some(Ok(IrEvent::Done)));

        assert!(
            is_flush_then_done,
            "clean EOF must flush parser state before Done, got: {events:?}"
        );

        // Aborted stream: the trailing malformed frame suppresses the flush,
        // so no flushed state appears between the error and `Done`.
        let events = pump_fixture(
            b"data: {",
            Box::new(FinishRecordingParser { is_finished: false }),
        )
        .await;

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "an aborted stream must surface the error before Done, got: {events:?}"
        );
    }

    /// Upstream parser that errors on the first frame it ingests and accepts
    /// every later frame by emitting a text delta, standing in for an
    /// upstream whose trailing frame is still buffered in the SSE parser when
    /// the stream aborts.
    struct AbortsFirstFrameParser {
        is_first_frame: bool,
    }

    impl UpstreamEventParser for AbortsFirstFrameParser {
        fn ingest_frame(&mut self, _frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
            if self.is_first_frame {
                self.is_first_frame = false;

                return Err(ApiError::upstream_unavailable("malformed frame"));
            }

            Ok(vec![IrEvent::TextDelta("replayed".to_string())])
        }

        fn parse_json_response(
            &mut self,
            _value: serde_json::Value,
        ) -> Result<Vec<IrEvent>, ApiError> {
            Ok(Vec::new())
        }
    }

    /// Verifies that a stream aborted by a frame-parse error never replays a
    /// trailing frame still buffered in the SSE parser: after the abort
    /// error, the event stream must carry exactly one terminal `Done`, with
    /// neither a replayed delta nor a second error from the flushed frame.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn skips_buffered_trailing_frames_on_aborted_streams() {
        // The first frame aborts the main pump loop; `data: tail` stays
        // buffered in the SSE parser because it lacks a terminating blank
        // line and would only surface through the end-of-stream flush.
        let body = b"data: ok\n\ndata: tail";

        let events = pump_fixture(
            body,
            Box::new(AbortsFirstFrameParser {
                is_first_frame: true,
            }),
        )
        .await;

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "an aborted stream must end with the error and Done alone, got: {events:?}"
        );

        // An always-rejecting parser proves the buffered frame is not even
        // dispatched after the abort, so no second error can be emitted.
        let events = pump_fixture(body, Box::new(RejectingParser)).await;

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "an aborted stream must surface exactly one error before Done, got: {events:?}"
        );
    }

    /// Verifies that a chat upstream closing cleanly after tool-call deltas —
    /// with neither a finish reason nor a `[DONE]` sentinel — still flushes
    /// the pending calls before the terminal `Done`.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn flushes_pending_chat_tool_calls_at_clean_eof() {
        let body = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[\
                     {\"index\":0,\"id\":\"call_1\",\"function\":\
                     {\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n";

        let events = pump_fixture(body, Box::new(ChatUpstreamParser::new())).await;

        let is_flushed_call = |event: &Result<IrEvent, ApiError>| {
            matches!(
                event,
                Ok(IrEvent::ToolCallDone { call_id, .. }) if call_id == "call_1"
            )
        };

        #[expect(
            clippy::expect_used,
            reason = "a missing flush of the pending call fails the test"
        )]
        let call_position = events
            .iter()
            .position(is_flushed_call)
            .expect("pending tool call should flush at clean EOF");

        #[expect(clippy::expect_used, reason = "a missing terminal Done fails the test")]
        let done_position = events
            .iter()
            .position(|event| matches!(event, Ok(IrEvent::Done)))
            .expect("stream should terminate with Done");

        assert!(call_position < done_position);
    }

    /// Verifies that a parse error raised for the trailing frame flushed at
    /// end-of-stream reaches the event channel as an `Err` before `Done`,
    /// instead of being swallowed.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn forwards_trailing_frame_parse_error() {
        // A hand-rolled HTTP/1.1 server serving an SSE body whose final
        // frame is terminated by end-of-stream instead of a blank line, so
        // it is only dispatched by the flush path of `pump_sse`.
        let body = b"data: tail";

        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );

        let address = serve_once(head, body);

        init_http_client();

        #[expect(clippy::expect_used, reason = "a request failure fails the test")]
        let response = crate::http()
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("local upstream should respond");

        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);

        pump_sse(response, Box::new(RejectingParser), sender).await;

        let mut events = Vec::new();

        while let Some(event) = receiver.recv().await {
            events.push(event);
        }

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "the trailing-frame parse error must precede Done, got: {events:?}"
        );
    }

    /// Verifies that a non-2xx upstream body whose declared length exceeds
    /// what the server actually delivers still yields its leading bytes as
    /// the bounded error excerpt, instead of an empty message from a failed
    /// full-body read.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn bounds_upstream_error_body_reads() {
        // Declares a megabyte-sized body but sends and delivers only a few
        // kilobytes before closing, so reading the full body fails.
        let head = "HTTP/1.1 500 Internal Server Error\r\n\
                    content-type: text/plain\r\n\
                    content-length: 1000000\r\n\
                    connection: close\r\n\r\n";

        let body = vec![b'x'; 6000];

        let address = serve_once(head.to_string(), &body);

        init_http_client();

        #[expect(clippy::expect_used, reason = "a request failure fails the test")]
        let response = crate::http()
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("local upstream should respond");

        let error: ApiError = super::upstream_error_of(response).await;

        assert_eq!(error.status, 500);

        assert_eq!(
            error.message.chars().filter(|char| *char == 'x').count(),
            400
        );

        assert!(error.message.ends_with('…'));
    }

    /// Builds a compatible provider carrying the given stored `base_url`.
    fn compatible_provider_with_base_url(base_url: Option<String>) -> Provider {
        Provider {
            id: 1,
            name: "compat".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiCompatible,
            protocol: Some(UpstreamFormat::OpenAiChatCompletion),
            base_url,
            credentials: Credentials::ApiKey("test-key".to_string()),
            expires_at: None,
            enabled: true,
        }
    }

    /// Builds a routing whose stored model row carries the given raw
    /// `thinking_levels_override` JSON.
    fn routing_with_thinking_levels_override(json: &str) -> ModelRouting {
        let provider = compatible_provider_with_base_url(Some("http://127.0.0.1:9".to_string()));

        let model = Model {
            provider_id: provider.id,
            upstream_name: "gpt-test".to_string(),
            name: "gpt-test".to_string(),
            name_override: None,
            enabled_override: None,
            thinking_levels_override: Some(json.to_string()),
        };

        ModelRouting {
            provider,
            model,
            is_enabled: true,
        }
    }

    /// Verifies that a malformed stored `thinking_levels_override` reaching
    /// `execute` surfaces as an internal error naming the drift, never as a
    /// 400 blaming the requesting client (the management API validates the
    /// override at write time, so a bad stored value is database drift).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while opening the test database.
    #[tokio::test]
    async fn malformed_stored_thinking_levels_fail_internally() -> Result<(), Box<dyn StdError>> {
        init_http_client();

        let (database, _) = test_support::open_test_database(1, Duration::from_secs(5)).await?;

        // A bare JSON string is not the array the override requires, so
        // parsing the stored value must fail.
        let routing = routing_with_thinking_levels_override(r#""high""#);

        // The success type is a boxed stream without `Debug`, so the
        // unexpected case is rejected with a panic instead of `expect_err`.
        #[expect(clippy::panic, reason = "an unexpected success fails the test")]
        let Err(error) = execute(
            &database,
            crate::http(),
            routing,
            IrRequest::default(),
            false,
        )
        .await
        else {
            panic!("drifted routing must fail before any upstream call");
        };

        assert_eq!(error.status, 500);

        assert_eq!(error.kind, ApiErrorKind::Internal);

        assert!(
            error
                .message
                .contains("stored thinking_levels_override is invalid"),
            "the error must name the stored drift, got: {error:?}"
        );

        Ok(())
    }

    /// Verifies that a compatible provider missing its stored `base_url`
    /// yields an internal error naming the drift, instead of a misleading 502
    /// for a relative request URL.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn missing_compatible_base_url_yields_internal_error() {
        let provider = compatible_provider_with_base_url(None);

        #[expect(
            clippy::expect_used,
            reason = "a missing internal error fails the test"
        )]
        let error = compatible_base_url(&provider)
            .expect_err("missing stored base_url must be rejected as internal drift");

        assert_eq!(error.status, 500);

        assert_eq!(error.kind, ApiErrorKind::Internal);

        assert!(
            error.message.contains("lacks a stored base_url"),
            "the error must name the stored drift, got: {error:?}"
        );
    }
}
