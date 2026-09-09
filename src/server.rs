//! Plaintext HTTP server lifecycle and connection driving.

use super::{api, database::Database, management_api};
use axum::{Router, http::StatusCode};
use host_port_pair::Host;
use hserver::{ConnectionBuilder, Server};
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
    let connection_builder = ConnectionBuilder::new();
    let mut server = Server::new(connection_builder, router(database));

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
    use super::{ACCEPT_BACKOFF_INITIAL, ACCEPT_BACKOFF_MAX, accept_backoff_delay};

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
}
