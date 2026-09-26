//! HTTP server lifecycle and connection driving over plaintext TCP or TLS, speaking HTTP/1 and
//! HTTP/2.

use super::{api, context::Context, management_api};
use axum::{Router, body::Body, http::StatusCode};
use hserver::{
    ConnectionBuilder, Server as ConnectionDriver, Tls, UpgradableConnection,
    UpgradableTlsHandshake,
};
use hyper_util::rt::TokioTimer;
use log::{debug, error, info, warn};
use socket2::{Domain, Protocol, SockAddr, Socket as Socket2, Type};
use std::{
    net::{SocketAddr, TcpListener as StdTcpListener},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
};

/// Delay before the first accept retry after a failed accept attempt.
const ACCEPT_BACKOFF_INITIAL: Duration = Duration::from_millis(10);

/// Maximum delay between accept retries, so a persistently failing listener retries at most once
/// per second instead of spinning.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Maximum number of pending, not-yet-accepted connections the kernel queues for the listening
/// socket before it starts refusing new ones.
const LISTEN_BACKLOG: i32 = 32;

/// Deadline for an HTTP/1 connection to finish sending request headers.
///
/// Pins hyper's effective 30-second default explicitly: the default is declared unstable, and the
/// value is part of the documented resource bounds.
const HTTP1_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Idle time before an HTTP/2 connection is probed with a keep-alive PING.
const HTTP2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Time a peer has to answer a keep-alive PING before the connection is closed as dead.
const HTTP2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// Maximum number of concurrent request streams one HTTP/2 connection may hold.
///
/// Pins hyper's effective default of 200 explicitly: the default is declared unstable.
const HTTP2_MAX_CONCURRENT_STREAMS: u32 = 200;

/// Maximum HPACK-decompressed header bytes one HTTP/2 request may carry.
///
/// Hyper's default is ~16 MB and declared unstable; 16 KiB bounds HPACK-bomb memory while leaving
/// generous room for ordinary header sets.
const HTTP2_MAX_HEADER_LIST_SIZE: u32 = 16 * 1024;

/// The gateway's inbound HTTP server.
///
/// One instance serves for the whole process lifetime: the TCP listener is bound once from the
/// global configuration, and the connection driver — carrying the HTTP/1 and HTTP/2 connection
/// configurations over the root router — is likewise built once and shared by every accepted
/// connection, so handling a connection clones the driver's shared configuration and the router
/// instead of rebuilding either.
pub(crate) struct Server {
    /// TCP listener bound to the configured address.
    tcp_listener: TcpListener,

    /// Address the TCP listener is bound to.
    socket_address: SocketAddr,

    /// Connection driver shared by every accepted connection.
    connection_driver: ConnectionDriver<Router>,
}

impl Server {
    /// Creates the server: binds the TCP listener to the configured address and builds the shared
    /// connection driver over the root router carrying `context`.
    ///
    /// # Panics
    ///
    /// Panics if the TCP listener fails to initialize.
    pub(crate) fn new(context: Context) -> Self {
        let (tcp_listener, socket_address) = bind_tcp_listener();

        Self {
            tcp_listener,
            socket_address,
            connection_driver: ConnectionDriver::new(connection_builder(), router(context)),
        }
    }

    /// Serves connections until the process is terminated.
    ///
    /// Failed accepts are retried with exponential backoff. Each accepted connection is served —
    /// over HTTP/1 or HTTP/2, auto-detected per connection by hyper-util's protocol detection — in
    /// its own task, so the accept loop is never blocked.
    ///
    /// This method runs indefinitely and never returns.
    pub(crate) async fn run(mut self) -> ! {
        let socket_address = self.socket_address;

        let scheme = if crate::CONFIG.tls().is_some() {
            "https"
        } else {
            "http"
        };

        info!("listening on {scheme}://{socket_address}");

        let mut consecutive_accept_failures: u32 = 0;

        loop {
            let (stream, peer_address) = match self.tcp_listener.accept().await {
                Ok(connection) => {
                    consecutive_accept_failures = 0;

                    connection
                }

                Err(error) => {
                    consecutive_accept_failures = consecutive_accept_failures.saturating_add(1);

                    error!("TCP accept failed: {error}");

                    tokio::time::sleep(accept_backoff_delay(consecutive_accept_failures)).await;

                    continue;
                }
            };

            self.serve_connection(stream, peer_address);
        }
    }

    /// Serves one accepted connection in its own task, through the TLS handshake first when the
    /// live configuration carries TLS material.
    ///
    /// The TLS configuration is read per connection at accept time, so a configuration or TLS
    /// material reload reaches new connections while established ones keep serving.
    fn serve_connection(&mut self, stream: TcpStream, peer_address: SocketAddr) {
        // Disabling Nagle's algorithm lets small streaming frames, such as SSE events, leave
        // immediately instead of stalling against the peer's delayed ACK. Best-effort, in the same
        // log-and-continue style as the accept error above: a socket that keeps the kernel default
        // merely degrades latency, so the connection is served anyway.
        if let Err(error) = stream.set_nodelay(true) {
            warn!("failed to set TCP_NODELAY for {peer_address}: {error}");
        }

        match crate::CONFIG.tls() {
            Some(tls_server_config) => {
                tokio::spawn(drive_tls_connection(
                    self.connection_driver
                        .handle_upgradable(Tls(tls_server_config, stream), peer_address),
                    peer_address,
                ));
            }

            None => {
                tokio::spawn(drive_connection(
                    self.connection_driver
                        .handle_upgradable(stream, peer_address),
                    peer_address,
                ));
            }
        }
    }
}

/// Answers a request that matches no route anywhere with an empty 404 response.
///
/// The root router owns this response, and the `/api` and `/management-api` parent routers declare
/// this very handler as their own fallback, so a path that belongs to no domain is answered
/// identically whichever router catches it; see the TODO in [`api::router`] for
/// why the parent routers declare it rather than inherit it.
pub(crate) async fn fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// Computes the delay before the next accept attempt after `consecutive_failures` consecutive
/// accept failures. The delay doubles per additional failure and saturates at
/// [`ACCEPT_BACKOFF_MAX`].
fn accept_backoff_delay(consecutive_failures: u32) -> Duration {
    let doublings = consecutive_failures.saturating_sub(1).min(10);

    (ACCEPT_BACKOFF_INITIAL * (1 << doublings)).min(ACCEPT_BACKOFF_MAX)
}

/// Creates the HTTP connection builder used for every accepted connection.
///
/// HTTP/1 arms hyper's timer and pins its header-read deadline explicitly: without a timer hyper
/// silently disables its timeouts and a slow client can park a connection task indefinitely, and
/// the 30-second default is declared unstable. HTTP/2 pins its stream and header-list bounds (both
/// defaults are declared unstable as well) and enables PING-based keep-alive: an idle connection is
/// probed every 30 seconds and closed when a probe goes unacknowledged for 20 seconds. A PING must
/// be answered by the peer's HTTP/2 stack — the kernel cannot answer on its behalf, unlike TCP
/// keep-alive — so live-but-idle clients survive while dead or silent HTTP/2 peers are reaped.
/// Hyper's maintained rapid-reset mitigation defaults (pending-accept and local-error reset-stream
/// limits) are left untouched.
///
/// No deadline is imposed on the phase before the first request headers — the TLS handshake,
/// hyper-util's protocol auto-detection, or a client simply sending nothing. Neither hyper-util nor
/// rustls exposes a hook for one, and adding one is rejected on the merits: a first-request
/// deadline burdens legitimately slow clients while failing to bound resource exhaustion, because
/// an attacker holding sockets with slow request bodies or slow keep-alive traffic makes the same
/// investment as one holding sockets silently. The socket-exhaustion threat is answered, if ever,
/// by connection-count limiting at the accept edge, not by opening-phase timeouts.
fn connection_builder() -> ConnectionBuilder {
    let mut connection_builder = ConnectionBuilder::new();

    connection_builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HTTP1_HEADER_READ_TIMEOUT);

    connection_builder
        .http2()
        .timer(TokioTimer::new())
        .keep_alive_interval(HTTP2_KEEP_ALIVE_INTERVAL)
        .keep_alive_timeout(HTTP2_KEEP_ALIVE_TIMEOUT)
        .max_concurrent_streams(HTTP2_MAX_CONCURRENT_STREAMS)
        .max_header_list_size(HTTP2_MAX_HEADER_LIST_SIZE);

    connection_builder
}

/// Drives one served connection to completion.
///
/// A connection error is routinely the client vanishing mid-request or mid-stream, so it is
/// recorded at debug level only.
async fn drive_connection<T>(
    connection: UpgradableConnection<T, Router, Body>,
    peer_address: SocketAddr,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if let Err(error) = connection.await {
        debug!("{peer_address}: connection ended with error: {error}");
    }
}

/// Drives one served TLS connection to completion: the Rustls handshake first, then the connection
/// over the established TLS stream.
///
/// A handshake failure is routinely a plaintext or otherwise TLS-incompatible client, so it is
/// recorded at debug level like a connection error.
async fn drive_tls_connection(
    handshake: UpgradableTlsHandshake<TcpStream, Router, Body>,
    peer_address: SocketAddr,
) {
    match handshake.await {
        Ok(connection) => drive_connection(connection, peer_address).await,

        Err(error) => debug!("{peer_address}: TLS handshake failed: {error}"),
    }
}

/// Constructs the Axum root router configured with the shared request context.
fn router(context: Context) -> Router {
    Router::new()
        .nest("/api", api::router(context.database.clone()))
        .nest("/management-api", management_api::router())
        .fallback(fallback)
        .with_state(context)
}

/// Binds the Tokio TCP listener to the configured address, returning the listener with the address
/// it is bound to.
///
/// # Panics
///
/// Panics if `CONFIG` is uninitialized, if socket creation, configuration, or binding fails.
fn bind_tcp_listener() -> (TcpListener, SocketAddr) {
    let socket_address = crate::CONFIG.listen;

    let domain = match socket_address {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    #[expect(clippy::expect_used, reason = "TCP socket creation failure is fatal")]
    let socket = Socket2::new(domain, Type::STREAM, Some(Protocol::TCP))
        .expect("cannot create the TCP socket");

    if socket_address.is_ipv6() {
        #[expect(
            clippy::expect_used,
            reason = "TCP socket dual-stack configuration failure is fatal"
        )]
        socket
            .set_only_v6(false)
            .expect("cannot configure dual-stack mode on the TCP socket");
    }

    #[expect(
        clippy::expect_used,
        reason = "TCP socket nonblocking configuration failure is fatal"
    )]
    socket
        .set_nonblocking(true)
        .expect("cannot configure nonblocking mode on the TCP socket");

    // Pin `SO_REUSEADDR` on Unix platforms so a prompt restart never fails to bind while the
    // previous run's connections still drain in TIME_WAIT. Windows is left at the socket default:
    // its `SO_REUSEADDR` semantics let a second socket bind over a port another socket actively
    // holds, a hijack hazard socket2's documentation warns about.
    #[cfg(not(windows))]
    {
        #[expect(
            clippy::expect_used,
            reason = "TCP socket address reuse configuration failure is fatal"
        )]
        socket
            .set_reuse_address(true)
            .expect("cannot configure address reuse on the TCP socket");
    }

    #[expect(clippy::expect_used, reason = "binding the TCP listener is fatal")]
    socket
        .bind(&SockAddr::from(socket_address))
        .expect("cannot bind the TCP listener");

    #[expect(clippy::expect_used, reason = "listening on the TCP socket is fatal")]
    socket
        .listen(LISTEN_BACKLOG)
        .expect("cannot listen on the TCP socket");

    #[expect(
        clippy::expect_used,
        reason = "converting the TCP listener to tokio is fatal"
    )]
    let tcp_listener = TcpListener::from_std(StdTcpListener::from(socket))
        .expect("cannot register the TCP listener with tokio");

    (tcp_listener, socket_address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ConfigLoader,
        database::Database,
        storage::{
            self,
            api_keys::{self, ApiKeyFields},
            models::ModelCreate,
        },
        test_support::{MockAction, MockUpstream, TempDirGuard},
    };
    use std::{
        error::Error as StdError,
        num::NonZero,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicU8, Ordering},
        },
        time::Duration,
    };
    use tokio_stream::StreamExt as _;

    /// The mock answers one chat completion JSON carrying usage (prompt 11, completion 7, no
    /// detail breakdown).
    const MOCK_JSON_SUCCESS: u8 = 0;

    /// The mock answers `429` — an upstream failure the gateway mirrors verbatim.
    const MOCK_UPSTREAM_429: u8 = 1;

    /// The mock accepts and closes without answering — a transport failure the gateway maps to
    /// 502 (README §6.7).
    const MOCK_CLOSE_ON_ARRIVAL: u8 = 2;

    /// The mock streams one complete SSE completion and closes cleanly.
    const MOCK_SSE_COMPLETE: u8 = 3;

    /// The mock streams forever, so the client can abort mid-stream.
    const MOCK_SSE_FOREVER: u8 = 4;

    /// The mock holds its first frame back, so a client can leave before any frame was enqueued:
    /// the frame arrives long after the gateway handed its response to the client, and the
    /// frame's arrival is what makes the render task notice the broken channel.
    const MOCK_SSE_DELAYED: u8 = 5;

    /// One recorded row as the scenario assertions read it.
    #[derive(Debug)]
    struct RecordedRow {
        /// Dispatch-start instant, ordering the rows merged from the shard files.
        started_at: i64,

        /// Recorded outcome.
        outcome: String,

        /// Status delivered to the client, `NULL` when none was.
        http_status: Option<i64>,

        /// Whether the client requested streaming.
        is_stream: bool,

        /// Recorded outward endpoint.
        endpoint_type: String,

        /// Attributed API key row id.
        api_key_id: i64,

        /// Attributed provider row id.
        provider_id: i64,

        /// Attributed model row id.
        model_id: i64,

        /// Recorded input tokens.
        input_tokens: Option<i64>,

        /// Recorded output tokens.
        output_tokens: Option<i64>,

        /// Recorded cached input tokens.
        cached_tokens: Option<i64>,

        /// Recorded cache-write input tokens.
        cache_write_tokens: Option<i64>,

        /// Recorded reasoning output tokens.
        reasoning_tokens: Option<i64>,
    }

    /// The success completion the mock answers with, carrying totals without a detail breakdown —
    /// so the recorded row pins partial-usage semantics (README §3 *ApiLog*).
    const MOCK_COMPLETION_BODY: &str = concat!(
        r#"{"id":"chatcmpl-mock","object":"chat.completion","created":1700000000,"#,
        r#""model":"mock","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"#,
        r#""finish_reason":"stop"}],"#,
        r#""usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#,
    );

    /// The upstream error body the mock's 429 carries.
    const MOCK_ERROR_BODY: &str = r#"{"error":{"message":"rate limited","type":"server_error"}}"#;

    /// One complete mock SSE completion: a delta, a terminal chunk with usage, and `[DONE]`.
    const MOCK_SSE_COMPLETION: &str = concat!(
        "data: {\"id\":\"chatcmpl-mock\",\"object\":\"chat.completion.chunk\",",
        "\"created\":1700000000,\"model\":\"mock\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"role\":\"assistant\",\"content\":\"hi\"},",
        "\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-mock\",\"object\":\"chat.completion.chunk\",",
        "\"created\":1700000000,\"model\":\"mock\",\"choices\":[{\"index\":0,",
        "\"delta\":{},\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}\n\n",
        "data: [DONE]\n\n",
    );

    /// The single frame the forever-streaming mock repeats until the client leaves.
    const MOCK_SSE_HEARTBEAT: &str = concat!(
        "data: {\"id\":\"chatcmpl-mock\",\"object\":\"chat.completion.chunk\",",
        "\"created\":1700000000,\"model\":\"mock\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"tick\"},\"finish_reason\":null}]}\n\n",
    );

    /// Renders one fixed-length HTTP/1.1 response.
    fn http_response(status_line: &str, content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Renders the head of an EOF-framed SSE response; the frames follow and the connection close
    /// delimits the body.
    fn sse_head() -> &'static str {
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
    }

    /// Starts the shared mock upstream on an ephemeral port, returning the mock and the shared
    /// mode selector the test flips between scenarios; every request is answered per the
    /// currently selected mode.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind — a broken mock is the failure report.
    async fn spawn_mock_upstream() -> (MockUpstream, Arc<AtomicU8>) {
        let mode = Arc::new(AtomicU8::new(MOCK_JSON_SUCCESS));
        let selected = Arc::clone(&mode);

        let mock = MockUpstream::start_responding(move || {
            match selected.load(Ordering::SeqCst) {
                MOCK_JSON_SUCCESS => MockAction::Respond(http_response(
                    "200 OK",
                    "application/json",
                    MOCK_COMPLETION_BODY,
                )),

                MOCK_UPSTREAM_429 => MockAction::Respond(http_response(
                    "429 Too Many Requests",
                    "application/json",
                    MOCK_ERROR_BODY,
                )),

                // Dropping the accepted socket without answering fails the gateway's send():
                // the transport-failure scenario.
                MOCK_CLOSE_ON_ARRIVAL => MockAction::Close,

                MOCK_SSE_COMPLETE => {
                    MockAction::Respond(format!("{}{MOCK_SSE_COMPLETION}", sse_head()))
                }

                MOCK_SSE_FOREVER => MockAction::StreamForever {
                    head: sse_head().to_string(),
                    frame: MOCK_SSE_HEARTBEAT.to_string(),
                    interval: Duration::from_millis(20),
                },

                MOCK_SSE_DELAYED => MockAction::StreamAfterDelay {
                    head: sse_head().to_string(),
                    frame: MOCK_SSE_HEARTBEAT.to_string(),
                    delay: Duration::from_millis(1_200),
                },

                other => {
                    #[expect(
                        clippy::panic,
                        reason = "the test only ever selects the six declared mock modes"
                    )]
                    {
                        panic!("unknown mock mode {other}");
                    }
                }
            }
        })
        .await;

        (mock, mode)
    }

    /// Renders the end-to-end configuration, pointed at the temporary database and an ephemeral
    /// listen port. The `api_logs` section rides exactly when `shard_directory` is given: the
    /// section's presence is the recording switch.
    fn configuration_body(database_path: &Path, shard_directory: Option<&Path>) -> String {
        let database_path = database_path.to_string_lossy().replace('\\', "/");

        let api_logs = shard_directory.map(|directory| {
            format!(
                ",\"api_logs\": {{\"path\": \"{}\"}}",
                directory.to_string_lossy().replace('\\', "/")
            )
        });

        let api_logs = api_logs.as_deref().unwrap_or("");

        format!(
            r#"{{
                "listen": "127.0.0.1:0",
                "database": {{"path": "{database_path}"}},
                "management_api": {{"token": "e2e-token"}},
                "api": {{}}{api_logs}
            }}"#
        )
    }

    /// Sends one completion request to the gateway under the given key.
    ///
    /// # Errors
    ///
    /// Returns the transport failure when the gateway cannot be reached.
    async fn send_completion(
        client: &reqwest::Client,
        base: &str,
        api_key: &str,
        model: &str,
        is_streaming: bool,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let stream_clause = if is_streaming {
            r#", "stream":true"#
        } else {
            ""
        };

        let body = format!(
            r#"{{"model":"{model}","messages":[{{"role":"user","content":"hi"}}]{stream_clause}}}"#
        );

        client
            .post(format!("{base}/api/openai-v1/chat/completions"))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
    }

    /// Reads every recorded row of the shard store's directory, ordered by the dispatch stamp.
    ///
    /// # Errors
    ///
    /// Returns an error when a shard cannot be opened or a query fails.
    ///
    /// # Panics
    ///
    /// Never: the fixed pool-size literal is nonzero, and the `expect` states that invariant.
    async fn recorded_rows(shard_directory: &Path) -> Result<Vec<RecordedRow>, Box<dyn StdError>> {
        let mut recorded = Vec::new();

        // The directory listing is the only truth: every shard file answers its own rows, and
        // an absent directory is the empty store.
        let entries = match std::fs::read_dir(shard_directory) {
            Ok(entries) => entries,

            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                return Ok(recorded);
            }

            Err(failure) => return Err(failure.into()),
        };

        for entry in entries {
            let path = entry?.path();

            if path.extension().is_none_or(|extension| extension != "db") {
                continue;
            }

            #[expect(clippy::unwrap_used, reason = "non-zero literal")]
            let max_connections = NonZero::new(1).unwrap();
            let database = Database::open(
                &path,
                crate::database::schema::Registry::ApiLogShard,
                max_connections,
                Duration::from_secs(5),
            )
            .await?;

            let rows = database
                .query(
                    "SELECT started_at, outcome, http_status, is_stream, endpoint_type, \
                     api_key_id, provider_id, model_id, input_tokens, output_tokens, \
                     cached_tokens, cache_write_tokens, reasoning_tokens FROM api_logs",
                    (),
                )
                .await?;

            for row in &rows {
                recorded.push(RecordedRow {
                    started_at: row.get(0)?,
                    outcome: row.get(1)?,
                    http_status: row.get(2)?,
                    is_stream: row.get::<i64>(3)? != 0,
                    endpoint_type: row.get(4)?,
                    api_key_id: row.get(5)?,
                    provider_id: row.get(6)?,
                    model_id: row.get(7)?,
                    input_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    cached_tokens: row.get(10)?,
                    cache_write_tokens: row.get(11)?,
                    reasoning_tokens: row.get(12)?,
                });
            }
        }

        recorded.sort_by_key(|row| row.started_at);

        Ok(recorded)
    }

    /// Waits until at least `count` rows are recorded — the only asynchronous finalization is the
    /// streaming task's — polling for five seconds before failing.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows never appear or the query fails.
    async fn wait_for_rows(
        shard_directory: &Path,
        count: usize,
    ) -> Result<Vec<RecordedRow>, Box<dyn StdError>> {
        for _ in 0..100 {
            let rows = recorded_rows(shard_directory).await?;

            if rows.len() >= count {
                return Ok(rows);
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(std::io::Error::other(format!(
            "expected at least {count} recorded rows within five seconds"
        ))
        .into())
    }

    /// Asserts exactly one new row beyond `baseline`, with the expected outcome and status.
    ///
    /// # Panics
    ///
    /// Panics when the row count or the newest row's outcome or status diverges.
    fn assert_one_new_row(
        rows: &[RecordedRow],
        baseline: usize,
        outcome: &str,
        http_status: Option<i64>,
    ) {
        assert_eq!(
            rows.len(),
            baseline + 1,
            "exactly one row per dispatched request"
        );

        #[expect(clippy::expect_used, reason = "the row was just asserted present")]
        let newest = rows.last().expect("the newest row exists");

        assert_eq!(newest.outcome, outcome);
        assert_eq!(newest.http_status, http_status);
    }

    /// End-to-end accounting smoke over a real server start and a mock upstream (README §3
    /// *ApiLog*): one — and only one — row per dispatched request across the documented outcomes
    /// (a success, a mirrored upstream non-2xx, a transport failure mapped to 502, a clean
    /// streaming completion, and a mid-stream client abort); zero rows for every pre-dispatch
    /// failure (authentication, routing, token refresh, upstream payload rendering); and, after a
    /// live configuration reload switches recording off, a successful dispatch writes nothing
    /// while the management reads keep serving the rows already stored.
    ///
    /// # Errors
    ///
    /// Returns an error if a setup, database, or HTTP operation fails.
    ///
    /// # Panics
    ///
    /// Panics when a scenario records the wrong number or shape of rows, or the reload never
    /// applies.
    #[tokio::test]
    async fn records_exactly_one_row_per_dispatch_end_to_end() -> Result<(), Box<dyn StdError>> {
        let directory = std::env::temp_dir().join(format!("janusgate-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory)?;
        let _guard = TempDirGuard(directory.clone());

        let database_path = directory.join("e2e.db");
        let config_path = directory.join("config.jsonc");
        let shard_directory = directory.join("api-logs");

        let (mock, mock_mode) = spawn_mock_upstream().await;

        std::fs::write(
            &config_path,
            configuration_body(&database_path, Some(&shard_directory)),
        )?;

        let (loader, watcher) = ConfigLoader::new(config_path.clone())?;

        #[expect(
            clippy::expect_used,
            reason = "the global configuration is initialized exactly once, in this single \
                      end-to-end test"
        )]
        crate::CONFIG
            .set(loader)
            .expect("the global configuration initializes exactly once");

        tokio::spawn(watcher);

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let max_connections = NonZero::new(4).unwrap();

        let database = Database::open(
            &database_path,
            crate::database::schema::Registry::Main,
            max_connections,
            Duration::from_secs(5),
        )
        .await?;

        // Seed: the authenticated key, a mock-backed provider and model, a drift provider whose
        // missing base URL fails rendering before dispatch, and a stale OAuth provider whose
        // refresh fails before dispatch.
        let api_key = api_keys::insert_api_key(
            &database,
            &ApiKeyFields {
                name: "e2e",
                key: "sk-e2e",
                is_enabled: true,
            },
        )
        .await?;

        let mock_base = mock.base();

        let definition = crate::test_support::insert_static_key_definition(
            &database,
            "static-key",
            crate::format::UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        let provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "mock",
            "e2e-",
            Some(&mock_base),
            crate::provider::StoredCredentials::ApiKey {
                api_key: "upstream-secret".to_string(),
            },
        )
        .await;

        let model = storage::models::insert_model_checked(
            &database,
            ModelCreate {
                provider_id: provider.id,
                upstream_id: "model",
                id_override: None,
                is_enabled: true,
            },
        )
        .await?;

        let drift_provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "drift",
            "drift-",
            None,
            crate::provider::StoredCredentials::ApiKey {
                api_key: "drift-secret".to_string(),
            },
        )
        .await;

        storage::models::insert_model_checked(
            &database,
            ModelCreate {
                provider_id: drift_provider.id,
                upstream_id: "model",
                id_override: None,
                is_enabled: true,
            },
        )
        .await?;

        let refresh_definition =
            crate::test_support::insert_failing_refresh_definition(&database, "failing-refresh")
                .await;

        let refresh_provider = crate::test_support::insert_provider(
            &database,
            &refresh_definition,
            "refresh",
            "rf-",
            None,
            crate::provider::StoredCredentials::OAuth {
                access_token: "stale".to_string(),
                refresh_token: Some("garbage".to_string()),
                slots: {
                    let mut slots = std::collections::BTreeMap::new();
                    slots.insert("project".to_string(), "project".to_string());
                    slots
                },
                expires_at: Some(1),
                refresh_started_at: None,
            },
        )
        .await;

        storage::models::insert_model_checked(
            &database,
            ModelCreate {
                provider_id: refresh_provider.id,
                upstream_id: "model",
                id_override: None,
                is_enabled: true,
            },
        )
        .await?;

        // Real startup: the server binds its own ephemeral port from the loaded configuration.
        #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
        let retention_days = NonZero::new(90).unwrap();
        let server = Server::new(Context {
            database: database.clone(),
            api_log_store: Some(crate::storage::api_logs::ApiLogStore::new(
                shard_directory.clone(),
                retention_days,
            )),
            data_plane: crate::http_client::data_plane(),
            control_plane: crate::http_client::control_plane(),
        });

        #[expect(clippy::expect_used, reason = "the listener is bound by Server::new")]
        let address = server
            .tcp_listener
            .local_addr()
            .expect("the listener is bound by Server::new");

        tokio::spawn(async move {
            server.run().await;
        });

        let base = format!("http://{address}");
        let client = reqwest::Client::new();

        // Pre-dispatch failures: authentication, routing, rendering, refresh — zero rows each.
        let response = send_completion(&client, &base, "sk-wrong", "e2e-model", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(recorded_rows(&shard_directory).await?.len(), 0);

        let response = send_completion(&client, &base, "sk-e2e", "ghost", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        assert_eq!(recorded_rows(&shard_directory).await?.len(), 0);

        let response = send_completion(&client, &base, "sk-e2e", "drift-model", false).await?;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(recorded_rows(&shard_directory).await?.len(), 0);

        // The failed refresh happens before dispatch; whichever transport shape the token
        // endpoint rejection takes, no row may exist once the response is rendered.
        let response = send_completion(&client, &base, "sk-e2e", "rf-model", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
        assert_eq!(recorded_rows(&shard_directory).await?.len(), 0);

        // Dispatched scenarios, one row each.
        let mut baseline = recorded_rows(&shard_directory).await?.len();

        // Success (non-streaming): completed + 200, attributed ids, totals with a NULL breakdown.
        mock_mode.store(MOCK_JSON_SUCCESS, Ordering::SeqCst);
        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let _body = response.text().await?;

        let rows = recorded_rows(&shard_directory).await?;
        assert_one_new_row(&rows, baseline, "completed", Some(200));

        #[expect(clippy::expect_used, reason = "the row was just asserted present")]
        let success = rows.last().expect("the success row exists");
        assert!(!success.is_stream);
        assert_eq!(success.endpoint_type, "openai_chat_completions");
        assert_eq!(success.api_key_id, api_key.id);
        assert_eq!(success.provider_id, provider.id);
        assert_eq!(success.model_id, model.model.id);
        assert_eq!(success.input_tokens, Some(11));
        assert_eq!(success.output_tokens, Some(7));
        assert_eq!(success.cached_tokens, None);
        assert_eq!(success.cache_write_tokens, None);
        assert_eq!(success.reasoning_tokens, None);

        baseline = rows.len();

        // Upstream non-2xx mirrored: upstream_error with the very status, no usage reported.
        mock_mode.store(MOCK_UPSTREAM_429, Ordering::SeqCst);
        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

        let rows = recorded_rows(&shard_directory).await?;
        assert_one_new_row(&rows, baseline, "upstream_error", Some(429));

        #[expect(clippy::expect_used, reason = "the row was just asserted present")]
        let mirrored = rows.last().expect("the mirrored row exists");
        assert_eq!(mirrored.input_tokens, None);
        assert_eq!(mirrored.output_tokens, None);

        baseline = rows.len();

        // Transport failure: the client receives 502 and the row mirrors that 502 (README §6.7).
        mock_mode.store(MOCK_CLOSE_ON_ARRIVAL, Ordering::SeqCst);
        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);

        let rows = recorded_rows(&shard_directory).await?;
        assert_one_new_row(&rows, baseline, "upstream_error", Some(502));

        baseline = rows.len();

        // A clean streaming completion: completed + 200, recorded once the body ends — and the
        // body end happens strictly after the row landed.
        mock_mode.store(MOCK_SSE_COMPLETE, Ordering::SeqCst);
        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", true).await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let stream_body = response.text().await?;
        assert!(stream_body.contains("data: [DONE]"));

        let rows = recorded_rows(&shard_directory).await?;
        assert_one_new_row(&rows, baseline, "completed", Some(200));

        #[expect(clippy::expect_used, reason = "the row was just asserted present")]
        let streamed = rows.last().expect(" the streaming row exists");
        assert!(streamed.is_stream);
        assert_eq!(streamed.input_tokens, Some(11));
        assert_eq!(streamed.output_tokens, Some(7));

        baseline = rows.len();

        // A client that aborts after the first delivered frame: client_aborted under the
        // already-declared 200, with no usage ever reported — five NULL token columns.
        mock_mode.store(MOCK_SSE_FOREVER, Ordering::SeqCst);
        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", true).await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let mut stream = response.bytes_stream();

        #[expect(
            clippy::expect_used,
            reason = "the forever-streaming mock delivers its first frame"
        )]
        stream
            .next()
            .await
            .expect("the forever-streaming mock delivers its first frame")
            .expect("the first frame arrives over a healthy transport");

        drop(stream);

        let rows = wait_for_rows(&shard_directory, baseline + 1).await?;
        assert_one_new_row(&rows, baseline, "client_aborted", Some(200));

        #[expect(clippy::expect_used, reason = "the row was just asserted present")]
        let aborted = rows.last().expect(" the aborted row exists");
        assert!(aborted.is_stream);
        assert_eq!(aborted.input_tokens, None);
        assert_eq!(aborted.output_tokens, None);
        assert_eq!(aborted.cached_tokens, None);
        assert_eq!(aborted.cache_write_tokens, None);
        assert_eq!(aborted.reasoning_tokens, None);

        baseline = rows.len();

        // Leaving before the first frame was enqueued: client_aborted with a NULL status — no
        // response ever reached the client (README §3 *ApiLog* streaming rules).
        mock_mode.store(MOCK_SSE_DELAYED, Ordering::SeqCst);

        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", true).await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        drop(response);

        let rows = wait_for_rows(&shard_directory, baseline + 1).await?;
        assert_one_new_row(&rows, baseline, "client_aborted", None);

        #[expect(clippy::expect_used, reason = "the row was just asserted present")]
        let silent = rows.last().expect("the silent abort row exists");
        assert!(silent.is_stream);

        // The switch: a live reload removes the `api_logs` section, so the next successful
        // dispatch writes nothing — while both management reads keep serving everything already
        // stored.
        std::fs::write(&config_path, configuration_body(&database_path, None))?;

        for _ in 0..300 {
            if crate::CONFIG.api_logs.is_none() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            crate::CONFIG.api_logs.is_none(),
            "the reload must apply within fifteen seconds"
        );

        let baseline = rows.len();

        mock_mode.store(MOCK_JSON_SUCCESS, Ordering::SeqCst);
        let response = send_completion(&client, &base, "sk-e2e", "e2e-model", false).await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let rows = recorded_rows(&shard_directory).await?;
        assert_eq!(
            rows.len(),
            baseline,
            "a dispatch under a disabled switch writes no row"
        );

        let listed = client
            .post(format!("{base}/management-api/v0/usage:list"))
            .header("Authorization", "e2e-token")
            .header("Content-Type", "application/json")
            .body(r#"{"limit":1000}"#)
            .send()
            .await?;

        assert_eq!(listed.status(), reqwest::StatusCode::OK);

        let listed: serde_json::Value = listed.json().await?;

        assert_eq!(
            listed
                .get("api_logs")
                .and_then(|logs| logs.as_array())
                .map(|logs| logs.len()),
            Some(baseline),
            "usage:list keeps serving stored rows while recording is off"
        );

        let queried = client
            .post(format!("{base}/management-api/v0/usage:query"))
            .header("Authorization", "e2e-token")
            .header("Content-Type", "application/json")
            .body(r#"{"group_by":["provider"]}"#)
            .send()
            .await?;

        assert_eq!(queried.status(), reqwest::StatusCode::OK);

        let queried: serde_json::Value = queried.json().await?;

        assert_eq!(
            queried
                .get("buckets")
                .and_then(|buckets| buckets.as_array())
                .and_then(|buckets| buckets.first())
                .and_then(|bucket| bucket.get("requests"))
                .and_then(|requests| requests.as_i64()),
            Some(baseline as i64),
            "usage:query counts every stored row of the sole provider"
        );

        Ok(())
    }
}
