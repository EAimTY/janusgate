//! Plaintext HTTP server lifecycle and connection driving.

use super::{api, database::Database, management_api};
use axum::{Router, http::StatusCode};
use host_port_pair::Host;
use hserver::{ConnectionBuilder, Server};
use hyper_util::rt::TokioTimer;
use socket2::{Domain, Protocol, SockAddr, Socket as Socket2, Type};
use std::{
    net::{SocketAddr, TcpListener as StdTcpListener, ToSocketAddrs as _},
    time::Duration,
};
use tokio::net::TcpListener;

/// Delay before the first accept retry after a failed accept attempt.
const ACCEPT_BACKOFF_INITIAL: Duration = Duration::from_millis(10);

/// Maximum delay between accept retries, so a persistently failing listener
/// retries at most once per second instead of spinning.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

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

/// Runs the HTTP server, accepting incoming TCP connections and serving HTTP
/// requests using an Axum router over `hserver`.
///
/// This function runs indefinitely and never returns.
///
/// # Panics
///
/// Panics if the TCP listener fails to initialize.
pub(crate) async fn run(database: Database) -> ! {
    let tcp_listener = init_tcp_listener();
    let mut server = Server::new(connection_builder(), router(database));

    let mut consecutive_failures: u32 = 0;

    loop {
        let (stream, peer_address) = match tcp_listener.accept().await {
            Ok(connection) => {
                consecutive_failures = 0;

                connection
            }

            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);

                eprintln!("TCP accept failed: {error}");

                tokio::time::sleep(accept_backoff_delay(consecutive_failures)).await;

                continue;
            }
        };

        let connection = server.handle_upgradable(stream, peer_address);

        tokio::spawn(async move {
            let _ = connection.await;
        });
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
/// configuration.
///
/// # Panics
///
/// Panics if `CONFIG` is uninitialized, if socket creation, configuration, or
/// binding fails, or if a DNS name cannot be resolved to a socket address.
fn init_tcp_listener() -> TcpListener {
    let socket_address = match crate::CONFIG.listen.host() {
        Host::IpAddr(ip_address) => SocketAddr::from((*ip_address, crate::CONFIG.listen.port())),
        Host::DnsName(dns_name) => {
            #[expect(
                clippy::unwrap_used,
                reason = "binding to unresolvable listen host is fatal"
            )]
            let mut socket_addresses = (dns_name.as_str(), crate::CONFIG.listen.port())
                .to_socket_addrs()
                .unwrap();

            #[expect(
                clippy::unwrap_used,
                reason = "binding to empty address resolution is fatal"
            )]
            socket_addresses.next().unwrap()
        }
    };

    let domain = match socket_address {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    #[expect(clippy::unwrap_used, reason = "TCP socket creation failure is fatal")]
    let socket = Socket2::new(domain, Type::STREAM, Some(Protocol::TCP)).unwrap();

    if socket_address.is_ipv6() {
        #[expect(
            clippy::unwrap_used,
            reason = "TCP socket dual-stack configuration failure is fatal"
        )]
        socket.set_only_v6(false).unwrap();
    }

    #[expect(
        clippy::unwrap_used,
        reason = "TCP socket nonblocking configuration failure is fatal"
    )]
    socket.set_nonblocking(true).unwrap();

    // Make socket reusable right after restart in debug builds.
    #[cfg(debug_assertions)]
    {
        #[expect(
            clippy::unwrap_used,
            reason = "TCP socket address reuse configuration failure is fatal"
        )]
        socket.set_reuse_address(true).unwrap();
    }

    #[expect(clippy::unwrap_used, reason = "binding TCP listener is fatal")]
    socket.bind(&SockAddr::from(socket_address)).unwrap();

    // Initialize listening with a backlog of 32 connections.
    #[expect(clippy::unwrap_used, reason = "listening on TCP socket is fatal")]
    socket.listen(32).unwrap();

    #[expect(
        clippy::unwrap_used,
        reason = "converting TCP listener to tokio is fatal"
    )]
    TcpListener::from_std(StdTcpListener::from(socket)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::{
        ACCEPT_BACKOFF_INITIAL, ACCEPT_BACKOFF_MAX, accept_backoff_delay, connection_builder,
    };
    use axum::{
        body::Body,
        http::{Request, Response},
    };
    use hserver::{Incoming, Server};
    use std::{
        convert::Infallible,
        future::{self, Ready},
        task::{Context, Poll},
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
    };
    use tower::Service;

    /// Verifies that the accept backoff starts at the initial delay and
    /// doubles per additional consecutive failure.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn doubles_from_initial_delay() {
        assert_eq!(accept_backoff_delay(1), ACCEPT_BACKOFF_INITIAL);
        assert_eq!(accept_backoff_delay(2), ACCEPT_BACKOFF_INITIAL * 2);
        assert_eq!(accept_backoff_delay(3), ACCEPT_BACKOFF_INITIAL * 4);
    }

    /// Verifies that the accept backoff saturates at the maximum delay,
    /// including for absurd failure counts.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn caps_at_maximum_delay() {
        assert_eq!(accept_backoff_delay(1_000), ACCEPT_BACKOFF_MAX);
        assert_eq!(accept_backoff_delay(u32::MAX), ACCEPT_BACKOFF_MAX);
    }

    /// A no-op request service that immediately answers with an empty
    /// response.
    #[derive(Clone)]
    struct ImmediateEmptyService;

    impl Service<Request<Incoming>> for ImmediateEmptyService {
        type Error = Infallible;
        type Future = Ready<Result<Response<Body>, Infallible>>;
        type Response = Response<Body>;

        fn call(&mut self, _request: Request<Incoming>) -> Self::Future {
            future::ready(Ok(Response::new(Body::empty())))
        }

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Verifies that a client stalling mid-request-headers is disconnected
    /// quickly when the header-read deadline is configured below a second,
    /// proving the connection builder's HTTP/1 timer is armed: hyper
    /// silently disables its timeouts without a timer and panics when a
    /// timeout is configured without one.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails or a deadline is exceeded.
    #[tokio::test]
    async fn drops_connections_stalling_in_header_read() {
        #[expect(clippy::expect_used, reason = "a bind failure fails the test")]
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");

        #[expect(clippy::expect_used, reason = "a local address failure fails the test")]
        let address = listener
            .local_addr()
            .expect("listener should have a local address");

        let mut connection_builder = connection_builder();

        connection_builder
            .http1()
            .header_read_timeout(Duration::from_millis(50));

        let mut server = Server::new(connection_builder, ImmediateEmptyService);

        let connection_task = tokio::spawn(async move {
            #[expect(clippy::expect_used, reason = "an accept failure fails the test")]
            let (stream, peer_address) = listener.accept().await.expect("accept should succeed");

            server.handle_upgradable(stream, peer_address).await
        });

        #[expect(clippy::expect_used, reason = "a connect failure fails the test")]
        let mut client = TcpStream::connect(address)
            .await
            .expect("client should connect");

        #[expect(clippy::expect_used, reason = "a write failure fails the test")]
        client
            .write_all(b"GET / ")
            .await
            .expect("partial headers should be writable");

        let mut dropped = [0_u8; 16];

        #[expect(clippy::expect_used, reason = "a stalled drop fails the test")]
        let bytes_read = tokio::time::timeout(Duration::from_secs(2), client.read(&mut dropped))
            .await
            .expect("connection should drop within two seconds")
            .expect("read should succeed");

        assert_eq!(bytes_read, 0);

        #[expect(
            clippy::expect_used,
            reason = "a panicking connection future means the timer is not armed"
        )]
        let connection_result = tokio::time::timeout(Duration::from_secs(2), connection_task)
            .await
            .expect("connection future should finish within two seconds")
            .expect("connection future should not panic");

        assert!(
            connection_result.is_err(),
            "the header-read timeout should surface as a connection error"
        );
    }
}
