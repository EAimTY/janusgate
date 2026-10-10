//! Default values for the optional fields of the configuration schema, referenced by the schema's
//! serde attributes.

use log::LevelFilter;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::NonZero,
    path::PathBuf,
};

/// Default minimum log severity.
pub(super) fn log_level() -> LevelFilter {
    LevelFilter::Info
}

/// Default listen address: loopback only, so a defaulted gateway is never exposed beyond its
/// host. The port is four-digit, unassigned in the IANA registry, and echoes the development
/// configuration's port.
pub(super) fn listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4718)
}

/// Default request-accounting recording switch.
pub(super) fn logs_is_enabled() -> bool {
    true
}

/// Default request-accounting directory: a relative path resolved beside the configuration file.
pub(super) fn logs_path() -> PathBuf {
    PathBuf::from("api_logs")
}

/// Default request-accounting retention, in whole days.
pub(super) fn logs_retention_days() -> NonZero<u32> {
    positive::<90>()
}

/// Default main database file path: a relative path resolved beside the configuration file.
pub(super) fn database_path() -> PathBuf {
    PathBuf::from("janusgate.db")
}

/// Constructs one positive first-party default constant.
///
/// # Panics
///
/// Panics if a first-party default literal is zero.
fn positive<const VALUE: u32>() -> NonZero<u32> {
    #[expect(clippy::expect_used, reason = "all default constants are positive")]
    let value = NonZero::new(VALUE).expect("default is positive");
    value
}

/// Constructs one positive first-party default `usize` constant.
///
/// # Panics
///
/// Panics if a first-party default literal is zero.
fn positive_usize<const VALUE: usize>() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "all default constants are positive")]
    let value = NonZero::new(VALUE).expect("default is positive");
    value
}

/// Default automatic quota check interval, in whole seconds.
pub(super) fn quota_refresh_interval_seconds() -> NonZero<u32> {
    positive::<60>()
}

/// Default historical pool affinity idle lifetime, in whole seconds.
pub(super) fn sticky_idle_timeout_seconds() -> NonZero<u32> {
    positive::<86400>()
}

/// Default exact-request retry hint lifetime, in whole seconds.
pub(super) fn sticky_retry_timeout_seconds() -> NonZero<u32> {
    positive::<60>()
}

/// Default maximum number of individually counted affinity entries.
pub(super) fn sticky_maximum_entries() -> NonZero<usize> {
    positive_usize::<100000>()
}
