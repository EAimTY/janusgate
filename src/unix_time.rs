//! Unix time helpers: the two clocks the gateway reads.
//!
//! The outward OpenAI protocol renders seconds (`created` fields), while every
//! stored timestamp uses UTC microseconds — so the two clocks are two functions
//! and a call site states its domain by which one it reads.

use std::{time, time::SystemTime};

/// Microseconds in one second: the factor between the outward protocol clock
/// (seconds) and the database clock (microseconds).
pub(crate) const MICROS_PER_SEC: i64 = 1_000_000;

/// Current unix time in seconds.
///
/// The outward protocol clock: OpenAI `created` fields render seconds.
pub(crate) fn unix_now() -> i64 {
    let duration = SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .unwrap_or_default();

    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

/// Current unix time in UTC microseconds.
///
/// The database clock: every stored timestamp (`expires_at`,
/// `refresh_started_at`, `created_at`) is written and compared in
/// microseconds.
pub(crate) fn unix_now_micros() -> i64 {
    let duration = SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .unwrap_or_default();

    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}
