//! Unix time helpers.

use std::{time, time::SystemTime};

/// Current unix time in seconds.
pub(crate) fn unix_now() -> i64 {
    let duration = SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .unwrap_or_default();

    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}
