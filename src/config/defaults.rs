//! Default values for the optional fields of the configuration schema, referenced by the schema's
//! serde attributes.

use log::LevelFilter;
use std::{num::NonZero, time::Duration};

/// Default maximum number of concurrent database pool connections.
pub(super) fn max_connections() -> NonZero<usize> {
    positive_usize::<4>()
}

/// Default minimum log severity.
pub(super) fn log_level() -> LevelFilter {
    LevelFilter::Info
}

/// Default connection pool wait in milliseconds.
pub(super) fn pool_timeout() -> Duration {
    Duration::from_millis(5000)
}

/// Default request-accounting retention, in whole days.
pub(super) fn api_logs_retention_days() -> NonZero<u32> {
    positive::<90>()
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
