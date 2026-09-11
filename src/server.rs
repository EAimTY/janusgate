//! Plaintext HTTP server lifecycle and connection driving.

use super::{api, database::Database, management_api};
use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode},
};
use host_port_pair::Host;
use hserver::{ConnectionBuilder, Incoming, Server};
use hyper_util::rt::TokioTimer;
use log::{debug, error, info, warn};
use socket2::{Domain, Protocol, SockAddr, Socket as Socket2, Type};
use std::{
    error::Error,
    io::Error as IoError,
    net::{SocketAddr, TcpListener as StdTcpListener, ToSocketAddrs as _},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};
use tower::Service;

/// Delay before the first accept retry after a failed accept attempt.
const ACCEPT_BACKOFF_INITIAL: Duration = Duration::from_millis(10);

/// Maximum delay between accept retries, so a persistently failing listener
/// retries at most once per second instead of spinning.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Maximum number of pending, not-yet-accepted connections the kernel queues
/// for the listening socket before it starts refusing new ones.
const LISTEN_BACKLOG: i32 = 32;

/// Leading bytes of the HTTP/2 connection preface (RFC 9113, Section 3.4).
///
/// These three bytes discriminate an HTTP/2 preface client from every
/// supported HTTP/1 request-line: request-lines start with a method token,
/// and the closest near-matches ("PROPFIND"/"PROPPATCH" and a hypothetical
/// "PRI"-method request) diverge at the third byte, so a full match here can
/// only come from an HTTP/2 preface client or from a request that is outside
/// the documented plaintext HTTP/1.1 surface either way.
const HTTP2_PREFACE_PREFIX: [u8; 3] = *b"PRI";

/// Deadline for reading the leading request bytes of an accepted connection.
///
/// Matches the 30-second header-read deadline hyper enforces for HTTP/1
/// connections, so a client sending nothing is closed at the same point
/// instead of parking an accepted connection indefinitely.
const REQUEST_PEEK_DEADLINE: Duration = Duration::from_secs(30);

/// Computes the delay before the next accept attempt after
/// `consecutive_failures` consecutive accept failures. The delay doubles per
/// additional failure and saturates at [`ACCEPT_BACKOFF_MAX`].
fn accept_backoff_delay(consecutive_failures: u32) -> Duration {
    let doublings = consecutive_failures.saturating_sub(1).min(10);

    (ACCEPT_BACKOFF_INITIAL * (1 << doublings)).min(ACCEPT_BACKOFF_MAX)
}

/// Creates the HTTP connection builder used for every accepted connection.
///
/// Hyper's HTTP/1 timer is armed so its default 30-second header-read
/// deadline takes effect: without a timer, hyper silently disables its
/// timeouts and a slow client can park a connection task indefinitely.
fn connection_builder() -> ConnectionBuilder {
    let mut connection_builder = ConnectionBuilder::new();

    connection_builder.http1().timer(TokioTimer::new());

    connection_builder
}

/// Verdict of the leading-bytes read performed on every accepted connection.
enum RequestPrelude {
    /// The connection began with [`HTTP2_PREFACE_PREFIX`]; it must be closed
    /// without a response.
    Http2Preface,

    /// The connection produced no usable leading bytes: the peer sent nothing
    /// within [`REQUEST_PEEK_DEADLINE`], disconnected, or errored. It is
    /// closed without a response.
    Unreadable,

    /// The leading bytes are incompatible with the HTTP/2 preface; the
    /// connection proceeds as HTTP/1. Carries the leading bytes already
    /// consumed from the socket, which must be replayed to the connection
    /// before any further reads.
    Http1 {
        /// Leading request bytes consumed from the socket by the
        /// [`read_request_prelude`] read.
        replayed_bytes: Vec<u8>,
    },
}

/// A transport that serves the bytes consumed by [`read_request_prelude`]
/// ahead of the underlying stream.
///
/// The accept-edge read removes bytes from the socket, so accepted
/// connections are handed to `hserver` through this wrapper: its reads
/// replay the consumed bytes first and only then reach the stream, so no
/// consumed byte is lost.
struct PreludeReplayStream<TStream> {
    /// Accepted transport all reads eventually delegate to.
    stream: TStream,

    /// Bytes consumed by [`read_request_prelude`] and served again before
    /// any read reaches [`PreludeReplayStream::stream`].
    replayed_bytes: Vec<u8>,
}

impl<TStream> PreludeReplayStream<TStream>
where
    TStream: AsyncRead + AsyncWrite + Unpin,
{
    /// Wraps `stream` so that `replayed_bytes` are served before stream reads.
    fn new(stream: TStream, replayed_bytes: Vec<u8>) -> Self {
        Self {
            stream,
            replayed_bytes,
        }
    }
}

impl<TStream> AsyncRead for PreludeReplayStream<TStream>
where
    TStream: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), IoError>> {
        let this = self.get_mut();

        while buf.remaining() > 0 {
            let Some(&next_byte) = this.replayed_bytes.first() else {
                break;
            };

            buf.put_slice(&[next_byte]);

            this.replayed_bytes.remove(0);
        }

        if this.replayed_bytes.is_empty() {
            return Pin::new(&mut this.stream).poll_read(cx, buf);
        }

        Poll::Ready(Ok(()))
    }
}

impl<TStream> AsyncWrite for PreludeReplayStream<TStream>
where
    TStream: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, IoError>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// Reads the leading request bytes from `stream`, up to the length of
/// [`HTTP2_PREFACE_PREFIX`], within [`REQUEST_PEEK_DEADLINE`], and returns
/// the [`RequestPrelude`] verdict.
///
/// The bytes carried by a `RequestPrelude::Http1` verdict are consumed from
/// the socket, so they must be served through [`PreludeReplayStream`] to
/// avoid losing them.
async fn read_request_prelude(stream: &mut TcpStream) -> RequestPrelude {
    let read = async {
        let mut replayed_bytes = Vec::with_capacity(HTTP2_PREFACE_PREFIX.len());

        for expected_byte in HTTP2_PREFACE_PREFIX {
            let mut byte = [0_u8; 1];

            match stream.read(&mut byte).await {
                Ok(1) => {}
                Ok(_) | Err(_) => return RequestPrelude::Unreadable,
            }

            replayed_bytes.push(byte[0]);

            if byte[0] != expected_byte {
                return RequestPrelude::Http1 { replayed_bytes };
            }
        }

        RequestPrelude::Http2Preface
    };

    tokio::time::timeout(REQUEST_PEEK_DEADLINE, read)
        .await
        .unwrap_or(RequestPrelude::Unreadable)
}

/// Runs the HTTP server, accepting incoming TCP connections and serving HTTP
/// requests using an Axum router over `hserver`.
///
/// This function runs indefinitely and never returns.
///
/// # Panics
///
/// Panics if the TCP listener fails to initialize.
pub(crate) async fn run(database: Database) -> ! {
    let (tcp_listener, socket_address) = init_tcp_listener();

    info!("listening on {socket_address}");

    let router = router(database);

    let mut consecutive_failures: u32 = 0;

    loop {
        let (stream, peer_address) = match tcp_listener.accept().await {
            Ok(connection) => {
                consecutive_failures = 0;

                connection
            }

            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);

                error!("TCP accept failed: {error}");

                tokio::time::sleep(accept_backoff_delay(consecutive_failures)).await;

                continue;
            }
        };

        // Disabling Nagle's algorithm lets small streaming frames, such as
        // SSE events, leave immediately instead of stalling against the
        // peer's delayed ACK. Best-effort, in the same log-and-continue
        // style as the accept error above: a socket that keeps the kernel
        // default merely degrades latency, so the connection is served
        // anyway.
        if let Err(error) = stream.set_nodelay(true) {
            warn!("failed to set TCP_NODELAY for {peer_address}: {error}");
        }

        tokio::spawn(serve_accepted_connection(
            router.clone(),
            stream,
            peer_address,
        ));
    }
}

/// Serves one accepted connection: a client opening with the HTTP/2
/// connection preface is closed without a response, and every other
/// connection is handed to `hserver` as HTTP/1 with the leading bytes
/// replayed.
///
/// The leading-bytes read applies [`REQUEST_PEEK_DEADLINE`], after which the
/// connection is closed like one whose client sends nothing. This future
/// runs the connection to completion, so callers must execute it in its own
/// task to keep the accept loop unblocked during that deadline.
async fn serve_accepted_connection<S>(service: S, mut stream: TcpStream, peer_address: SocketAddr)
where
    S: Service<Request<Incoming>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send,
    S::Error: Into<Box<dyn Error + Send + Sync>>,
{
    match read_request_prelude(&mut stream).await {
        RequestPrelude::Http1 { replayed_bytes } => {
            let transport = PreludeReplayStream::new(stream, replayed_bytes);

            let mut server = Server::new(connection_builder(), service);

            let connection = server.handle_upgradable(transport, peer_address);

            // A connection error is routinely the client vanishing mid-request
            // or mid-stream, so it is recorded at debug level only.
            if let Err(error) = connection.await {
                debug!("{peer_address}: connection ended with error: {error}");
            }
        }

        // Dropping the socket closes the connection without a response;
        // hserver is never invoked, so an HTTP/2 preface client can never
        // reach hyper-util's deadline-less auto protocol detection.
        RequestPrelude::Http2Preface => {
            debug!("{peer_address}: closed an HTTP/2 preface connection");
        }

        RequestPrelude::Unreadable => {
            debug!("{peer_address}: closed an unresponsive connection");
        }
    }
}

/// Constructs the Axum root router configured with database state.
fn router(database: Database) -> Router {
    Router::new()
        .nest("/api", api::router())
        .nest("/management-api", management_api::router())
        .fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(database)
}

/// Initializes the Tokio TCP listener using the global application
/// configuration, returning the listener with the socket address it is bound
/// to.
///
/// # Panics
///
/// Panics if `CONFIG` is uninitialized, if socket creation, configuration, or
/// binding fails, or if a DNS name cannot be resolved to a socket address.
fn init_tcp_listener() -> (TcpListener, SocketAddr) {
    let socket_address = match crate::CONFIG.listen.host() {
        Host::IpAddr(ip_address) => SocketAddr::from((*ip_address, crate::CONFIG.listen.port())),
        Host::DnsName(dns_name) => {
            #[expect(clippy::panic, reason = "binding to unresolvable listen host is fatal")]
            let mut socket_addresses = (dns_name.as_str(), crate::CONFIG.listen.port())
                .to_socket_addrs()
                .unwrap_or_else(|error| panic!("cannot resolve listen host {dns_name}: {error}"));

            #[expect(clippy::panic, reason = "binding to empty address resolution is fatal")]
            socket_addresses
                .next()
                .unwrap_or_else(|| panic!("listen host {dns_name} resolved to no addresses"))
        }
    };

    let domain = match socket_address {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    #[expect(clippy::panic, reason = "TCP socket creation failure is fatal")]
    let socket = Socket2::new(domain, Type::STREAM, Some(Protocol::TCP))
        .unwrap_or_else(|error| panic!("cannot create TCP socket for {socket_address}: {error}"));

    if socket_address.is_ipv6() {
        #[expect(
            clippy::panic,
            reason = "TCP socket dual-stack configuration failure is fatal"
        )]
        socket.set_only_v6(false).unwrap_or_else(|error| {
            panic!("cannot configure dual-stack mode on {socket_address}: {error}")
        });
    }

    #[expect(
        clippy::panic,
        reason = "TCP socket nonblocking configuration failure is fatal"
    )]
    socket.set_nonblocking(true).unwrap_or_else(|error| {
        panic!("cannot configure nonblocking mode on {socket_address}: {error}")
    });

    // Make socket reusable right after restart in debug builds.
    #[cfg(debug_assertions)]
    {
        #[expect(
            clippy::panic,
            reason = "TCP socket address reuse configuration failure is fatal"
        )]
        socket.set_reuse_address(true).unwrap_or_else(|error| {
            panic!("cannot configure address reuse on {socket_address}: {error}")
        });
    }

    #[expect(clippy::panic, reason = "binding TCP listener is fatal")]
    socket
        .bind(&SockAddr::from(socket_address))
        .unwrap_or_else(|error| panic!("cannot bind TCP listener on {socket_address}: {error}"));

    #[expect(clippy::panic, reason = "listening on TCP socket is fatal")]
    socket.listen(LISTEN_BACKLOG).unwrap_or_else(|error| {
        panic!("cannot listen on TCP socket for {socket_address}: {error}")
    });

    #[expect(clippy::panic, reason = "converting TCP listener to tokio is fatal")]
    let tcp_listener =
        TcpListener::from_std(StdTcpListener::from(socket)).unwrap_or_else(|error| {
            panic!("cannot register TCP listener on {socket_address} with tokio: {error}")
        });

    (tcp_listener, socket_address)
}
