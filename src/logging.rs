//! Process-wide log emission infrastructure built on the `log` facade with
//! `env_logger` as the single installed logging implementation.

use env_logger::Builder;
use log::Record;
use std::{fmt, io};

/// Initializes the process-global logger exactly once during startup.
///
/// The minimum severity is taken exclusively from the `log_level` value of
/// the global configuration; environment variables such as `RUST_LOG` are
/// never consulted. Records are written to standard error as plain text
/// without ANSI styling.
///
/// # Panics
///
/// Panics if the global logger cannot be installed, which only a duplicate
/// startup initialization can cause.
pub(crate) fn init() {
    #[expect(
        clippy::expect_used,
        reason = "installing the global logger at startup is fatal on failure"
    )]
    Builder::new()
        .filter_level(crate::CONFIG.log_level)
        .format(|buffer, record| {
            let timestamp = buffer.timestamp_millis();

            write_record(buffer, timestamp, record)
        })
        .try_init()
        .expect("failed to install the global logger");
}

/// Renders one log record as a single terminated line: an RFC 3339 UTC
/// timestamp, the level padded to five characters, the record target, and
/// the message body.
fn write_record(
    out: &mut impl io::Write,
    timestamp: impl fmt::Display,
    record: &Record<'_>,
) -> Result<(), io::Error> {
    write!(
        out,
        "{timestamp} {:<5} {}: ",
        record.level(),
        record.target()
    )?;

    writeln!(out, "{}", record.args())
}
