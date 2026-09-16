//! HTTP server lifecycle and connection driving over plaintext TCP or TLS,
//! speaking HTTP/1 and HTTP/2.

use super::{api, context, management_api};
use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode},
};
use host_port_pair::Host;
use hserver::{ConnectionBuilder, Incoming, Server, Tls};
use hyper_util::rt::TokioTimer;
use log::{debug, error, info, warn};
use rustls::{
    ServerConfig,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use socket2::{Domain, Protocol, SockAddr, Socket as Socket2, Type};
use std::{
    error::Error,
    net::{SocketAddr, TcpListener as StdTcpListener, ToSocketAddrs as _},
    sync::Arc,
    time::Duration,
};
use tokio::net::{TcpListener, TcpStream};
use tower::Service;

/// Delay before the first accept retry after a failed accept attempt.
const ACCEPT_BACKOFF_INITIAL: Duration = Duration::from_millis(10);

/// Maximum delay between accept retries, so a persistently failing listener
/// retries at most once per second instead of spinning.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Maximum number of pending, not-yet-accepted connections the kernel queues
/// for the listening socket before it starts refusing new ones.
const LISTEN_BACKLOG: i32 = 32;

/// ALPN protocols offered on TLS listeners, in preference order.
///
/// Rustls negotiates one of them with the TLS client, and hyper-util's
/// protocol auto-detection independently identifies the spoken protocol
/// from the decrypted stream's leading bytes, so both ALPN-negotiated and
/// plaintext prior-knowledge HTTP/2 clients are served.
const ALPN_PROTOCOLS: [&[u8]; 2] = [b"h2", b"http/1.1"];

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
///
/// No deadline is imposed on the phase before the first request headers —
/// the TLS handshake, hyper-util's protocol auto-detection, or a client
/// simply sending nothing. Neither hyper-util nor rustls exposes a hook for
/// one, and adding one is rejected on the merits: a first-request deadline
/// burdens legitimately slow clients while failing to bound resource
/// exhaustion, because an attacker holding sockets with slow request bodies
/// or slow keep-alive traffic makes the same investment as one holding
/// sockets silently. The socket-exhaustion threat is answered, if ever, by
/// connection-count limiting at the accept edge, not by opening-phase
/// timeouts.
fn connection_builder() -> ConnectionBuilder {
    let mut connection_builder = ConnectionBuilder::new();

    connection_builder.http1().timer(TokioTimer::new());

    connection_builder
}

/// Builds the inbound TLS server configuration from the `tls` section of the
/// global application configuration, returning `None` when that section is
/// absent and the listener serves plaintext HTTP.
///
/// The configuration offers the protocols of [`ALPN_PROTOCOLS`] through ALPN
/// and performs no client-certificate authentication; inbound access is
/// governed by the gateway's own token authentication either way.
///
/// # Panics
///
/// Panics if `CONFIG` is uninitialized or if the certificate or private key
/// files cannot be read or combined into a TLS server configuration.
fn tls_server_config() -> Option<Arc<ServerConfig>> {
    let tls = crate::CONFIG.tls.as_ref()?;

    #[expect(
        clippy::expect_used,
        reason = "an unreadable or unparsable TLS certificate chain file is fatal"
    )]
    let certificates = CertificateDer::pem_file_iter(&tls.certificate)
        .expect("cannot read the TLS certificate chain")
        .collect::<Result<Vec<CertificateDer>, _>>()
        .expect("cannot parse the TLS certificate chain");

    #[expect(
        clippy::expect_used,
        reason = "an unreadable or unparsable TLS private key file is fatal"
    )]
    let private_key = PrivateKeyDer::from_pem_file(&tls.private_key)
        .expect("cannot read or parse the TLS private key");

    #[expect(
        clippy::expect_used,
        reason = "an unusable TLS certificate chain or private key is fatal"
    )]
    let mut server_config =
        ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("cannot select TLS protocol versions")
            .with_no_client_auth()
            .with_single_cert(certificates, private_key)
            .expect("cannot build the TLS server configuration");

    server_config.alpn_protocols = ALPN_PROTOCOLS.into_iter().map(Vec::from).collect();

    Some(Arc::new(server_config))
}

/// Runs the HTTP server, accepting incoming TCP connections and serving
/// HTTP/1 or HTTP/2 requests using an Axum router over `hserver`, wrapped in
/// TLS when the `tls` configuration section is present.
///
/// This function runs indefinitely and never returns.
///
/// # Panics
///
/// Panics if the TCP listener or the TLS server configuration fails to
/// initialize.
pub(crate) async fn run(context: context::Context) -> ! {
    let tls_server_config = tls_server_config();

    let (tcp_listener, socket_address) = init_tcp_listener();

    let scheme = if tls_server_config.is_some() {
        "https"
    } else {
        "http"
    };

    info!("listening on {scheme}://{socket_address}");

    let router = router(context);

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
            tls_server_config.clone(),
            stream,
            peer_address,
        ));
    }
}

/// Serves one accepted connection — directly, or behind a TLS handshake
/// first when TLS is configured — with HTTP/1 or HTTP/2 auto-detected per
/// connection by hyper-util's protocol detection.
///
/// This future runs the connection to completion, so callers must execute it
/// in its own task to keep the accept loop unblocked.
async fn serve_accepted_connection<S>(
    service: S,
    tls_server_config: Option<Arc<ServerConfig>>,
    stream: TcpStream,
    peer_address: SocketAddr,
) where
    S: Service<Request<Incoming>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send,
    S::Error: Into<Box<dyn Error + Send + Sync>>,
{
    let mut server = Server::new(connection_builder(), service);

    match tls_server_config {
        Some(config) => {
            // A handshake failure is routinely a plaintext or otherwise
            // TLS-incompatible client, so it is recorded at debug level like
            // a connection error.
            match server
                .handle_upgradable(Tls(config, stream), peer_address)
                .await
            {
                Ok(connection) => {
                    // A connection error is routinely the client vanishing
                    // mid-request or mid-stream, so it is recorded at debug
                    // level only.
                    if let Err(error) = connection.await {
                        debug!("{peer_address}: connection ended with error: {error}");
                    }
                }

                Err(error) => debug!("{peer_address}: TLS handshake failed: {error}"),
            }
        }

        None => {
            let connection = server.handle_upgradable(stream, peer_address);

            // A connection error is routinely the client vanishing mid-request
            // or mid-stream, so it is recorded at debug level only.
            if let Err(error) = connection.await {
                debug!("{peer_address}: connection ended with error: {error}");
            }
        }
    }
}

/// Constructs the Axum root router configured with the shared request
/// context.
fn router(context: context::Context) -> Router {
    Router::new()
        .nest("/api", api::router())
        .nest("/management-api", management_api::router())
        .fallback(fallback)
        .with_state(context)
}

/// Answers a request that matches no route anywhere with an empty 404
/// response.
///
/// The root router owns this response, and the `/api` and `/management-api`
/// parent routers declare this very handler as their own fallback, so a path
/// that belongs to no domain is answered identically whichever router catches
/// it; see the TODO in [`api::router`](crate::api::router) for why the parent
/// routers declare it rather than inherit it.
pub(crate) async fn fallback() -> StatusCode {
    StatusCode::NOT_FOUND
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
            #[expect(
                clippy::expect_used,
                reason = "binding to an unresolvable listen host is fatal"
            )]
            let mut socket_addresses = (dns_name.as_str(), crate::CONFIG.listen.port())
                .to_socket_addrs()
                .expect("cannot resolve the listen host");

            #[expect(
                clippy::expect_used,
                reason = "binding to an empty address resolution is fatal"
            )]
            socket_addresses
                .next()
                .expect("the listen host resolved to no addresses")
        }
    };

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

    // Make socket reusable right after restart in debug builds.
    #[cfg(debug_assertions)]
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
