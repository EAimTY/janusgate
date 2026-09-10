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

#[cfg(test)]
mod tests {
    use super::write_record;
    use log::{Level, Record};

    /// Renders a log record with the given level, target, and message in one
    /// statement so the `format_args!` temporaries live long enough.
    ///
    /// # Panics
    ///
    /// Panics if writing the rendered line fails.
    #[expect(clippy::expect_used, reason = "a rendering failure fails the test")]
    fn render(level: Level, target: &str, timestamp: &str, message: &str) -> String {
        let mut line = Vec::new();

        write_record(
            &mut line,
            timestamp,
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{message}"))
                .build(),
        )
        .expect("rendering into an in-memory buffer should not fail");

        String::from_utf8(line).expect("rendered line should be valid UTF-8")
    }

    /// Verifies that a record renders as a single line carrying the
    /// timestamp, the five-character level, the target, and the message.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn renders_record_line() {
        let line = render(
            Level::Info,
            "janusgate::server",
            "2026-09-11T08:15:30.123Z",
            "listening on 0.0.0.0:8080",
        );

        assert_eq!(
            line,
            "2026-09-11T08:15:30.123Z INFO  janusgate::server: listening on 0.0.0.0:8080\n"
        );
    }

    /// Verifies that every level renders in exactly five characters so
    /// columns stay aligned.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn pads_every_level_to_five_characters() {
        let rendered = [
            (Level::Error, "ERROR"),
            (Level::Warn, "WARN "),
            (Level::Info, "INFO "),
            (Level::Debug, "DEBUG"),
            (Level::Trace, "TRACE"),
        ];

        for (level, expected) in rendered {
            let line = render(level, "t", "2026-09-11T08:15:30.123Z", "m");

            assert!(line.starts_with(&format!("2026-09-11T08:15:30.123Z {expected} t: m\n")));
        }
    }
}
