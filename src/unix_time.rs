//! Unix time helpers: the two clocks the gateway reads, plus the one composition the
//! database clock's durations share.
//!
//! The outward OpenAI protocol renders seconds (`created` fields), while every
//! stored timestamp uses UTC microseconds — so the two clocks are two functions
//! and a call site states its domain by which one it reads. An upstream-reported
//! duration scales onto the database clock through [`seconds_from_now`], whose
//! overflow each call site maps to its own typed failure.

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

/// The instant exactly `seconds` from the current database clock, in UTC microseconds, or
/// `None` when the duration scaled to microseconds or the addition to now overflows the
/// database clock.
///
/// The input is usually an upstream-reported magnitude, so the caller — the only party that
/// knows its failure vocabulary — maps an overflow to its own typed error.
pub(crate) fn seconds_from_now(seconds: i64) -> Option<i64> {
    let micros = seconds.checked_mul(MICROS_PER_SEC)?;

    unix_now_micros().checked_add(micros)
}

#[cfg(test)]
mod tests {
    //! Tests for the database-clock composition: an upstream-reported magnitude scales onto the
    //! microsecond clock and overflows map to `None` for the caller's typed failure.

    #![expect(
        clippy::missing_panics_doc,
        clippy::expect_used,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{MICROS_PER_SEC, seconds_from_now, unix_now_micros};

    /// A duration scales exactly by the seconds-to-microseconds factor onto the current
    /// database clock: the scheduled instant sits at the scaling of one of the clock reads
    /// bracketing the call, whatever the reads' drift.
    #[test]
    fn durations_scale_onto_the_microsecond_clock() {
        let before = unix_now_micros();
        let scheduled = seconds_from_now(90).expect("a small duration never overflows");
        let after = unix_now_micros();

        assert!(scheduled >= before + 90 * MICROS_PER_SEC);
        assert!(scheduled <= after + 90 * MICROS_PER_SEC);
        assert!(seconds_from_now(0).is_some_and(|instant| instant >= before));
    }

    /// A magnitude composes while both its scaling and its addition to the current clock fit the
    /// database clock, and yields `None` past whichever boundary the current clock puts first:
    /// with a second of slack the composition succeeds, and a magnitude provably beyond the
    /// current addition boundary fails however much the next clock read drifts.
    #[test]
    fn overflow_yields_none_at_the_first_boundary() {
        let fitting = (i64::MAX - unix_now_micros()) / MICROS_PER_SEC - 1;

        assert!(seconds_from_now(fitting).is_some());

        let overflowing = (i64::MAX - unix_now_micros()) / MICROS_PER_SEC + 2;

        assert_eq!(seconds_from_now(overflowing), None);
        assert_eq!(seconds_from_now(i64::MAX), None);
    }
}
