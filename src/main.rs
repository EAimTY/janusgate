#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::database::Database;
use deref_once_lock::OnceLock;
use host_port_pair::HostPortPair;
use json_strip_comments::StripComments;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use log::LevelFilter;
use reqwest::Client;
use serde::Deserialize;
use serde_with::{As, DurationMilliSeconds};
use std::{
    env, ffi::OsString, fs::File, num::NonZero, panic, path::PathBuf, process, time::Duration,
};
use tokio::main;

mod api;
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

#[cfg(test)]
mod test_support;

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

/// Root gateway configuration schema.
///
/// Every field is mandatory and unknown fields are strictly rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// Minimum severity a log record must carry to be emitted to standard
    /// error.
    log_level: LevelFilter,

    /// Address and TCP port on which the gateway listens for plaintext HTTP
    /// connections.
    listen: HostPortPair,

    /// In-process SQLite database configuration.
    database: DatabaseConfig,

    /// Administrative management API configuration.
    management_api: ManagementApiConfig,

    /// Outward client API configuration.
    api: ApiConfig,
}

/// In-process SQLite database configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseConfig {
    /// File path to the in-process SQLite database file.
    path: PathBuf,

    /// Maximum number of concurrent database connections in the connection
    /// pool.
    max_connections: NonZero<usize>,

    /// Maximum time in milliseconds to wait for a free connection from the
    /// connection pool when every connection is checked out.
    #[serde(with = "As::<DurationMilliSeconds>")]
    pool_timeout: Duration,
}

/// Administrative management API configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagementApiConfig {
    /// Bearer-less administrative authorization token required by
    /// `/management-api/*`.
    token: String,
}

/// Outward client protocol configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiConfig {
    /// Shared API key for outward protocol endpoints under `/api/*`.
    key: String,
}

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
    use log::LevelFilter;
    use std::{ffi::OsString, path::PathBuf};

    /// Replaces the global configuration for router-level tests; only the
    /// first call per process succeeds and later calls are ignored.
    pub(crate) fn set_config_for_tests(config: super::Config) {
        let _ = crate::CONFIG.set(config);
    }

    /// Renders a configuration file body carrying the given log level and
    /// secrets.
    fn config_body(log_level: &str, api_key: &str, management_token: &str) -> String {
        let head = format!(
            r#"{{"log_level":"{log_level}","listen":"127.0.0.1:0","database":{{"path":"test.db","#,
        );
        let database = r#""max_connections":1,"pool_timeout":1}"#;
        let management = format!(r#","management_api":{{"token":"{management_token}"}}"#);
        let api = format!(r#","api":{{"key":"{api_key}"}}}}"#);

        format!("{head}{database}{management}{api}")
    }

    /// A temporary configuration file deleted on drop, including through
    /// panic unwinding of `#[should_panic]` tests.
    struct TempConfig(PathBuf);

    impl TempConfig {
        /// Writes `body` to a uniquely named temporary configuration file.
        ///
        /// # Panics
        ///
        /// Panics if the file cannot be written.
        fn write(name: &str, body: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("janusgate-{}-{name}.jsonc", std::process::id()));

            #[expect(clippy::expect_used, reason = "a write failure fails the test")]
            std::fs::write(&path, body).expect("temporary configuration should be writable");

            Self(path)
        }

        /// Parses the temporary configuration file.
        ///
        /// # Panics
        ///
        /// Panics if parsing or validation fails.
        fn parse(&self) -> super::Config {
            let arguments: Vec<OsString> = vec![
                "janusgate".into(),
                "-c".into(),
                self.0.clone().into_os_string(),
            ];

            super::parse_config(arguments)
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Verifies that valid configuration parses, carrying the configured
    /// log level.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_valid_configuration() {
        let temporary = TempConfig::write("valid-secrets", &config_body("info", "key", "token"));

        let config = temporary.parse();

        assert_eq!(config.log_level, LevelFilter::Info);
    }

    /// Verifies that log levels parse case-insensitively.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_log_level_case_insensitively() {
        let temporary = TempConfig::write("upper-log-level", &config_body("WARN", "key", "token"));

        let config = temporary.parse();

        assert_eq!(config.log_level, LevelFilter::Warn);
    }

    /// Verifies that a configuration file missing the log level is fatal
    /// with a panic naming the offending field.
    ///
    /// # Panics
    ///
    /// Panics as required by the assertion.
    #[test]
    #[should_panic(expected = "missing field `log_level`")]
    fn names_missing_log_level_field() {
        let temporary = TempConfig::write(
            "missing-log-level",
            concat!(
                r#"{"listen":"127.0.0.1:0","database":{"path":"test.db","#,
                r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token"},"#,
                r#""api":{"key":"key"}}"#,
            ),
        );

        let _config = temporary.parse();
    }

    /// Verifies that an unknown log level is fatal with a panic naming the
    /// offending value.
    ///
    /// # Panics
    ///
    /// Panics as required by the assertion.
    #[test]
    #[should_panic(expected = "unknown variant `bogus`")]
    fn names_unknown_log_level_value() {
        let temporary = TempConfig::write("bogus-log-level", &config_body("bogus", "key", "token"));

        let _config = temporary.parse();
    }

    /// Verifies that an unopenable configuration file is fatal with a panic
    /// naming the offending path.
    ///
    /// # Panics
    ///
    /// Panics as required by the assertion.
    #[test]
    #[should_panic(expected = "janusgate-unopenable-configuration.jsonc")]
    fn names_unopenable_configuration_file() {
        let _config = super::parse_config([
            "janusgate",
            "-c",
            "janusgate-unopenable-configuration.jsonc",
        ]);
    }

    /// Verifies that an unparsable configuration file is fatal with a panic
    /// naming the offending path.
    ///
    /// # Panics
    ///
    /// Panics as required by the assertion.
    #[test]
    #[should_panic(expected = "unparsable-configuration.jsonc")]
    fn names_unparsable_configuration_file() {
        let temporary = TempConfig::write("unparsable-configuration", "{");

        let _config = temporary.parse();
    }

    /// Verifies that an unrecognized command-line argument is fatal with a
    /// panic naming the offending argument.
    ///
    /// # Panics
    ///
    /// Panics as required by the assertion.
    #[test]
    #[should_panic(expected = "unexpected command-line argument \"--bogus\"")]
    fn names_unexpected_command_line_argument() {
        let _config = super::parse_config(["janusgate", "--bogus"]);
    }
}
