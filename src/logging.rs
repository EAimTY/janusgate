//! Process-wide log emission infrastructure built on the `log` facade with `env_logger` as the
//! single installed logging implementation.

use env_logger::Builder;
use log::{LevelFilter, Record};
use std::{
    fmt::Display,
    io::{Error as IoError, Write},
};

/// Initializes the process-global logger exactly once during startup.
///
/// The installed filter stays open at trace: the effective minimum severity is the global maximum
/// level, taken exclusively from the `log_level` value of the global configuration and set right
/// after installation, so a configuration reload moves the severity without reinstalling the
/// logger. Environment variables such as `RUST_LOG` are never consulted. Records are written to
/// standard error as plain text without ANSI styling.
///
/// # Panics
///
/// Panics if the global logger cannot be installed, which only a duplicate
/// startup initialization can cause.
pub(crate) fn init() {
    let filter = crate::CONFIG.log_level;

    #[expect(
        clippy::expect_used,
        reason = "installing the global logger at startup is fatal on failure"
    )]
    Builder::new()
        .filter_level(LevelFilter::Trace)
        .format(|buffer, record| {
            let timestamp = buffer.timestamp_millis();

            write_record(buffer, timestamp, record)
        })
        .try_init()
        .expect("failed to install the global logger");

    log::set_max_level(filter);
}

/// Renders one log record as a single terminated line: an RFC 3339 UTC timestamp, the level padded
/// to five characters, the record target, and the message body.
fn write_record(
    out: &mut impl Write,
    timestamp: impl Display,
    record: &Record<'_>,
) -> Result<(), IoError> {
    write!(
        out,
        "{timestamp} {:<5} {}: ",
        record.level(),
        record.target()
    )?;

    writeln!(out, "{}", record.args())
}
