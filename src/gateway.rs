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
    provider::{
        Credentials, ModelRouting, Provider, ProviderType, endpoints, http_body, oauth, storage,
    },
    sse::{SseFrame, SseParseError, SseParser},
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

/// Incremental source of complete upstream SSE frames: the framing half of
/// the upstream pump, kept a trait so the end-of-stream flush contract is
/// testable independently of the real parser.
trait SseFrameSource {
    /// Ingests a raw byte chunk and returns every frame completed by it.
    fn ingest(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, SseParseError>;

    /// Flushes a trailing frame terminated by the end of the stream instead
    /// of a final empty line.
    fn finish(&mut self) -> Result<Vec<SseFrame>, SseParseError>;
}

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
        tokio::spawn(pump_sse(response, parser, SseParser::new(), sender));
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
    mut sse_parser: impl SseFrameSource + Send,
    sender: tokio::sync::mpsc::Sender<Result<IrEvent, ApiError>>,
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

                let _ = sender
                    .send(Err(ApiError::upstream_unavailable("oversized SSE frame")))
                    .await;
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

impl SseFrameSource for SseParser {
    fn ingest(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, SseParseError> {
        SseParser::ingest(self, chunk)
    }

    fn finish(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
        SseParser::finish(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SseFrameSource, build_upstream_request, compatible_base_url, execute, pump_sse,
        resolve_routing,
    };
    use crate::{
        error::{ApiError, ApiErrorKind},
        format::{
            UpstreamEventParser, UpstreamFormat,
            openai_chat_completion::UpstreamParser as ChatUpstreamParser,
        },
        ir::{IrEvent, IrMessage, IrPart, IrRequest, IrRole},
        provider::{Credentials, Model, ModelRouting, Provider, ProviderType, endpoints},
        sse::{SseFrame, SseParseError, SseParser},
        test_support,
    };
    use reqwest::header::HeaderMap;
    use serde_json::Value;
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

    /// SSE frame source whose end-of-stream flush fails, standing in for the
    /// flush-error branch of the framing layer. The real parser cannot raise
    /// it today — the flush appends the line terminator itself, and an
    /// oversized frame already aborts the main loop — so the pump contract is
    /// pinned with this stub instead.
    struct FailingFinishFrames;

    impl SseFrameSource for FailingFinishFrames {
        fn ingest(&mut self, _chunk: &[u8]) -> Result<Vec<SseFrame>, SseParseError> {
            Ok(Vec::new())
        }

        fn finish(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
            Err(SseParseError::FrameTooLarge)
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
        sse_parser: impl SseFrameSource + Send,
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

        pump_sse(response, parser, sse_parser, sender).await;

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
        let events = pump_fixture(
            b"",
            Box::new(FinishRecordingParser { is_finished: false }),
            SseParser::new(),
        )
        .await;

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
            SseParser::new(),
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
            SseParser::new(),
        )
        .await;

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "an aborted stream must end with the error and Done alone, got: {events:?}"
        );

        // An always-rejecting parser proves the buffered frame is not even
        // dispatched after the abort, so no second error can be emitted.
        let events = pump_fixture(body, Box::new(RejectingParser), SseParser::new()).await;

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

        let events =
            pump_fixture(body, Box::new(ChatUpstreamParser::new()), SseParser::new()).await;

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

        pump_sse(
            response,
            Box::new(RejectingParser),
            SseParser::new(),
            sender,
        )
        .await;

        let mut events = Vec::new();

        while let Some(event) = receiver.recv().await {
            events.push(event);
        }

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "the trailing-frame parse error must precede Done, got: {events:?}"
        );
    }

    /// Verifies that a framing error raised by the SSE parser's own
    /// end-of-stream flush — a trailing frame only rejected at flush time —
    /// reaches the event channel as an `Err` before `Done`, instead of
    /// terminating the stream cleanly as if the trailing frame had never
    /// existed.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn forwards_flush_time_framing_error() {
        let events = pump_fixture(b"", Box::new(RejectingParser), FailingFinishFrames).await;

        assert!(
            matches!(events.as_slice(), [Err(_), Ok(IrEvent::Done)]),
            "the flush-time framing error must precede Done, got: {events:?}"
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
            is_enabled: true,
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

        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(1, Duration::from_secs(5)).await?;

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

    /// Reads one request header as UTF-8, failing the test when absent.
    ///
    /// # Panics
    ///
    /// Panics if the header is absent or not UTF-8.
    fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
        #[expect(
            clippy::panic,
            reason = "a missing required upstream header fails the test"
        )]
        let header = headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_else(|| panic!("the upstream request should carry a UTF-8 `{name}` header"));

        header
    }

    /// Builds a Codex OAuth provider with fixed credential values.
    fn codex_provider() -> Provider {
        Provider {
            id: 1,
            name: "codex".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiOauth,
            protocol: None,
            base_url: None,
            credentials: Credentials::OpenAiOauth {
                access_token: "test-access".to_string(),
                refresh_token: "test-refresh".to_string(),
                account_id: "test-account".to_string(),
            },
            expires_at: None,
            is_enabled: true,
        }
    }

    /// Builds an Antigravity OAuth provider with fixed credential values.
    fn antigravity_provider() -> Provider {
        Provider {
            id: 1,
            name: "agy".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::AntigravityOauth,
            protocol: None,
            base_url: None,
            credentials: Credentials::AntigravityOauth {
                access_token: "test-access".to_string(),
                refresh_token: "test-refresh".to_string(),
                project_id: "test-project".to_string(),
            },
            expires_at: None,
            is_enabled: true,
        }
    }

    /// Builds an `openai_compatible` provider speaking the given protocol.
    fn compatible_provider_with_protocol(protocol: UpstreamFormat) -> Provider {
        Provider {
            id: 1,
            name: "compat".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiCompatible,
            protocol: Some(protocol),
            base_url: Some("https://upstream.example".to_string()),
            credentials: Credentials::ApiKey("test-key".to_string()),
            expires_at: None,
            is_enabled: true,
        }
    }

    /// Builds a routing of `provider` to one upstream model named `u1`.
    fn routing_for(provider: &Provider) -> ModelRouting {
        ModelRouting {
            provider: provider.clone(),

            model: Model {
                provider_id: provider.id,
                upstream_name: "u1".to_string(),
                name: "u1".to_string(),
                name_override: None,
                enabled_override: None,
                thinking_levels_override: None,
            },

            is_enabled: true,
        }
    }

    /// Builds the upstream request for the given provider and wire format
    /// with a one-turn fixture request, returning the constructed request
    /// with its headers and serialized body.
    ///
    /// # Panics
    ///
    /// Panics if request construction fails, which fails the test.
    fn built_request(
        upstream_format: UpstreamFormat,
        provider: &Provider,
        client_streaming: bool,
    ) -> reqwest::Request {
        init_http_client();

        let routing = routing_for(provider);

        let ir = IrRequest {
            model: routing.model.upstream_name.clone(),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the provider fixture always renders an upstream request"
        )]
        let builder = build_upstream_request(
            crate::http(),
            upstream_format,
            provider,
            &routing,
            &ir,
            client_streaming,
        )
        .expect("the provider fixture should render an upstream request");

        #[expect(clippy::expect_used, reason = "the rendered builder always builds")]
        builder.build().expect("the request builder should build")
    }

    /// Parses the JSON body of a constructed upstream request.
    ///
    /// # Panics
    ///
    /// Panics if the body is absent or not JSON.
    fn request_json(request: &reqwest::Request) -> Value {
        #[expect(
            clippy::expect_used,
            reason = "every built upstream request carries a serialized JSON body"
        )]
        let bytes = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("the upstream request should carry a JSON body");

        #[expect(clippy::expect_used, reason = "the rendered body is JSON")]
        serde_json::from_slice(bytes).expect("the upstream request body should be JSON")
    }

    /// Verifies the Codex upstream request carries the mandatory six-header
    /// set of README §5.1 and, per the same contract, forces `stream: true`
    /// upstream alongside `store: false` even when the client requested
    /// non-streaming mode.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn codex_upstream_request_carries_mandatory_header_set() {
        let request = built_request(UpstreamFormat::OpenAiResponse, &codex_provider(), false);

        let headers = request.headers();

        assert_eq!(header_value(headers, "authorization"), "Bearer test-access");

        assert_eq!(header_value(headers, "chatgpt-account-id"), "test-account");

        assert_eq!(
            header_value(headers, "originator"),
            endpoints::CODEX_ORIGINATOR
        );

        assert_eq!(
            header_value(headers, "user-agent"),
            endpoints::CODEX_USER_AGENT
        );

        assert!(
            !header_value(headers, "session-id").is_empty(),
            "the Session-Id header must carry a request identifier"
        );

        assert_eq!(header_value(headers, "accept"), "text/event-stream");

        let body = request_json(&request);

        assert_eq!(body.get("stream"), Some(&Value::Bool(true)));

        assert_eq!(body.get("store"), Some(&Value::Bool(false)));
    }

    /// Verifies the Antigravity upstream request presents
    /// `User-Agent: antigravity` with the bearer access token and a JSON
    /// content type, dials the Code Assist endpoint (README §5.2).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn antigravity_upstream_request_carries_user_agent_and_bearer() {
        let request = built_request(
            UpstreamFormat::GeminiGenerateContent,
            &antigravity_provider(),
            false,
        );

        let headers = request.headers();

        assert_eq!(header_value(headers, "authorization"), "Bearer test-access");

        assert_eq!(
            header_value(headers, "user-agent"),
            endpoints::ANTIGRAVITY_USER_AGENT
        );

        assert_eq!(header_value(headers, "content-type"), "application/json");

        assert!(
            request
                .url()
                .as_str()
                .starts_with(endpoints::ANTIGRAVITY_BASE_URL),
            "the antigravity request must dial the Code Assist endpoint"
        );
    }

    /// Verifies the compatible Gemini upstream request authenticates through
    /// `x-goog-api-key` instead of a bearer token (README §5.2, §5.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn compatible_gemini_upstream_request_uses_api_key_header() {
        let request = built_request(
            UpstreamFormat::GeminiGenerateContent,
            &compatible_provider_with_protocol(UpstreamFormat::GeminiGenerateContent),
            false,
        );

        assert_eq!(
            header_value(request.headers(), "x-goog-api-key"),
            "test-key"
        );

        assert!(
            request.headers().get("authorization").is_none(),
            "the compatible Gemini request must not carry a bearer token"
        );
    }

    /// Verifies a compatible OpenAI-protocol upstream renders the client's
    /// requested streaming mode verbatim — unlike the Codex endpoint, no
    /// upstream invariant forces `stream: true` here (README §6.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn compatible_openai_upstream_request_passes_client_stream_mode_through() {
        let provider = compatible_provider_with_protocol(UpstreamFormat::OpenAiResponse);

        let non_streaming = built_request(UpstreamFormat::OpenAiResponse, &provider, false);

        assert_eq!(
            request_json(&non_streaming).get("stream"),
            Some(&Value::Bool(false))
        );

        let streaming = built_request(UpstreamFormat::OpenAiResponse, &provider, true);

        assert_eq!(
            request_json(&streaming).get("stream"),
            Some(&Value::Bool(true))
        );
    }

    /// Verifies routing resolution requires an effectively enabled model row:
    /// unknown, override-disabled, and provider-disabled public names all
    /// yield 404 with `param: "model"` and `code: "model_not_found"`
    /// (README §6.5, §6.7), while a live row resolves to its upstream name.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn resolve_routing_rejects_unknown_and_disabled_models() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        test_support::seed_model(&database, "alpha", "u1", "a-m1").await?;

        #[expect(
            clippy::expect_used,
            reason = "the unknown-model rejection is the subject of the assertion"
        )]
        let unknown = resolve_routing(&database, "missing")
            .await
            .expect_err("an unknown model must be rejected");

        assert_eq!(unknown.status, 404);
        assert_eq!(unknown.param.as_deref(), Some("model"));
        assert_eq!(unknown.code.as_deref(), Some("model_not_found"));

        let routing = resolve_routing(&database, "a-m1").await?;

        assert_eq!(routing.model.upstream_name, "u1");

        database
            .execute(
                "UPDATE models SET enabled_override = 0 WHERE upstream_name = 'u1'",
                (),
            )
            .await?;

        #[expect(
            clippy::expect_used,
            reason = "the disabled-model rejection is the subject of the assertion"
        )]
        let disabled = resolve_routing(&database, "a-m1")
            .await
            .expect_err("an override-disabled model must be rejected");

        assert_eq!(disabled.status, 404);
        assert_eq!(disabled.param.as_deref(), Some("model"));
        assert_eq!(disabled.code.as_deref(), Some("model_not_found"));

        database
            .execute(
                "UPDATE models SET enabled_override = NULL WHERE upstream_name = 'u1'",
                (),
            )
            .await?;

        database
            .execute("UPDATE providers SET enabled = 0 WHERE name = 'alpha'", ())
            .await?;

        #[expect(
            clippy::expect_used,
            reason = "the provider-disabled rejection is the subject of the assertion"
        )]
        let provider_disabled = resolve_routing(&database, "a-m1")
            .await
            .expect_err("a model of a disabled provider must be rejected");

        assert_eq!(provider_disabled.status, 404);
        assert_eq!(provider_disabled.param.as_deref(), Some("model"));
        assert_eq!(provider_disabled.code.as_deref(), Some("model_not_found"));

        Ok(())
    }
}
