//! Upstream HTTP clients and their timeout policy.
//!
//! Two clients are built at startup, one per traffic class: the data plane ([`crate::gateway`])
//! streams long-lived completions, while the control plane (OAuth token exchange and refresh, model
//! discovery, project onboarding) performs short JSON round-trips. Splitting them lets each client
//! carry the timeout profile its traffic class needs instead of one compromise profile
//! (`docs/reference/configuration.md`). Both clients refuse redirects outright
//! (`reqwest::redirect::Policy::none`): a 3xx surfaces as the upstream response it is and flows
//! through the caller's non-2xx path, so credential headers and request bodies are never
//! re-dispatched to the Location origin.

use reqwest::{Client, redirect::Policy};
use std::time::Duration;

/// Total wall-clock budget of one control-plane request.
///
/// Control-plane traffic is short JSON round-trips, so a stall is bounded outright instead of
/// per-read. The value must stay below the OAuth refresh-lease staleness window
/// (`REFRESH_LEASE_STALE_AFTER`, 90 s), so a stalled refresh attempt aborts — and its lease
/// clears — before another worker may consider the lease abandoned.
pub(crate) const CONTROL_PLANE_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for establishing an upstream connection.
///
/// Connection setup never carries data to reset a read deadline, so it is bounded separately from
/// the per-class read and total timeouts.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum idle time between two consecutive reads of a data-plane upstream response.
///
/// reqwest resets this deadline after every successful read, so it never bounds a slow-but-flowing
/// response; it only aborts a stalled upstream connection, which would otherwise park the request
/// task forever. The data plane sets no total request timeout, so a long generation is never
/// truncated.
const DATA_PLANE_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Builds the data-plane client that streams upstream completions.
///
/// # Panics
///
/// Panics if the TLS backend cannot be initialized, which is fatal at
/// startup.
pub(crate) fn data_plane() -> Client {
    #[expect(
        clippy::expect_used,
        reason = "building the data-plane HTTP client at startup is fatal on failure"
    )]
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(DATA_PLANE_READ_TIMEOUT)
        .redirect(Policy::none())
        .build()
        .expect("failed to build the data-plane HTTP client");

    client
}

/// Builds the control-plane client for short upstream round-trips.
///
/// # Panics
///
/// Panics if the TLS backend cannot be initialized, which is fatal at
/// startup.
pub(crate) fn control_plane() -> Client {
    #[expect(
        clippy::expect_used,
        reason = "building the control-plane HTTP client at startup is fatal on failure"
    )]
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(CONTROL_PLANE_TOTAL_TIMEOUT)
        .redirect(Policy::none())
        .build()
        .expect("failed to build the control-plane HTTP client");

    client
}
