#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::{config::Config, database::Database};
use deref_once_lock::OnceLock;
use json_strip_comments::StripComments;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use reqwest::Client;
use std::{env, ffi::OsString, fs::File, panic, path::PathBuf, process, time::Duration};
use tokio::main;

mod api;
mod config;
mod database;
mod error;
mod format;
mod gateway;
mod ids;
mod ir;
mod logging;
mod management_api;
mod provider;
mod security;
mod server;
mod sse;

mod unix_time;

/// Global read-only application configuration, initialized once during process
/// startup and accessible throughout the crate without synchronization
/// overhead.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Global shared HTTP client used for every upstream request, initialized
/// once during process startup.
static HTTP: OnceLock<Client> = OnceLock::new();

/// Maximum idle time between two consecutive reads of an upstream response.
///
/// reqwest resets this deadline after every successful read, so it never
/// bounds a slow-but-flowing response; it only aborts a stalled upstream
/// connection, which would otherwise park the request task forever.
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Deadline for establishing an upstream connection.
///
/// Connection setup never carries data to reset a read deadline, so it is
/// bounded separately from [`UPSTREAM_READ_TIMEOUT`].
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Application entry point.
///
/// # Panics
///
/// Panics if command-line arguments or configuration cannot be parsed or if
/// the global configuration or logger has already been initialized.
#[main]
async fn main() -> ! {
    // Install a process-terminating panic hook before any other operation to
    // ensure thread panics abort immediately instead of leaving the
    // application in a degraded state.
    set_panic_hook();

    let config = parse_config(env::args_os());

    #[expect(
        clippy::unwrap_used,
        reason = "CONFIG is initialized exactly once at application startup"
    )]
    CONFIG.set(config).unwrap();

    logging::init();

    #[expect(
        clippy::expect_used,
        reason = "building the HTTP client at startup is fatal on failure"
    )]
    let http = Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(UPSTREAM_READ_TIMEOUT)
        .build()
        .expect("failed to build HTTP client");

    #[expect(
        clippy::unwrap_used,
        reason = "HTTP is initialized exactly once at application startup"
    )]
    HTTP.set(http).unwrap();

    #[expect(
        clippy::expect_used,
        reason = "opening database at startup is fatal on failure"
    )]
    let database = Database::open(
        &crate::CONFIG.database.path,
        crate::CONFIG.database.max_connections,
        crate::CONFIG.database.pool_timeout,
    )
    .await
    .expect("failed to open database");

    server::run(database).await
}

/// Returns the global shared HTTP client.
///
/// # Panics
///
/// Panics when called before startup initialization, which only a startup
/// ordering bug can cause.
pub(crate) fn http() -> &'static Client {
    #[expect(
        clippy::expect_used,
        reason = "HTTP is initialized exactly once at application startup"
    )]
    HTTP.get().expect("HTTP client is initialized at startup")
}

/// Parses command-line arguments, opens the designated configuration file,
/// strips comments, and deserializes the configuration.
///
/// # Panics
///
/// Panics if command-line arguments are invalid, the configuration file cannot
/// be opened, or deserialization fails.
fn parse_config(args: impl IntoIterator<Item = impl Into<OsString>>) -> Config {
    let mut parser = ArgumentsParser::from_iter(args);
    let mut config_path = None;

    #[expect(
        clippy::unwrap_used,
        reason = "command-line argument syntax errors are fatal"
    )]
    while let Some(argument) = parser.next().unwrap() {
        match argument {
            #[expect(
                clippy::unwrap_used,
                reason = "missing value for configuration option is fatal"
            )]
            Argument::Short('c') | Argument::Long("config") => {
                config_path = Some(PathBuf::from(parser.value().unwrap()));
            }

            #[expect(
                clippy::panic,
                reason = "unrecognized command-line arguments are fatal"
            )]
            _ => {
                // Render the offending argument itself so the panic names the
                // invocation element that must be removed or corrected.
                let rendered = match &argument {
                    Argument::Short(short) => format!("-{short}"),
                    Argument::Long(long) => format!("--{long}"),
                    Argument::Value(value) => value.to_string_lossy().into_owned(),
                };

                panic!("unexpected command-line argument {rendered:?}");
            }
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "missing configuration path argument is fatal"
    )]
    let path = config_path.expect("missing argument CONFIG");

    #[expect(clippy::panic, reason = "unopenable configuration file is fatal")]
    let file = File::options()
        .read(true)
        .open(&path)
        .unwrap_or_else(|error| panic!("cannot open configuration file {path:?}: {error}"));

    #[expect(clippy::panic, reason = "unparsable configuration content is fatal")]
    let config: Config = serde_json::from_reader(StripComments::new(file))
        .unwrap_or_else(|error| panic!("cannot parse configuration file {path:?}: {error}"));

    config
}

/// Configures a custom panic hook that terminates the entire process
/// immediately.
///
/// By default, a panic on a background or worker thread in Rust only unwinds
/// that specific thread. In an asynchronous server, this leaves the remaining
/// runtime operational while services or connections may be permanently broken.
/// This hook prints diagnostic panic info to stderr via the standard hook and
/// calls `process::exit(1)` to ensure fail-fast semantics.
fn set_panic_hook() {
    let hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Print the panic location, payload, and backtrace to stderr.
        hook(panic_info);

        // Terminate the process immediately with a failure status code.
        process::exit(1);
    }));
}

#[cfg(test)]
mod tests {
    /// Verifies that `config.example.jsonc` parses successfully into a valid
    /// [`Config`](crate::config::Config).
    ///
    /// # Panics
    ///
    /// Panics if parsing or validation of `config.example.jsonc` fails.
    #[test]
    fn parses_example_configuration() {
        let _config = super::parse_config([
            "janusgate",
            "-c",
            concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.jsonc"),
        ]);
    }
}
