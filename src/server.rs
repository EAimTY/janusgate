//! HTTP server lifecycle and connection driving over plaintext TCP or TLS, speaking HTTP/1 and
//! HTTP/2.

use super::{api, context::Context, management_api, router_fallback};
use axum::{Router, body::Body};
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
        .fallback(router_fallback::fallback)
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
