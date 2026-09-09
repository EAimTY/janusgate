#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::database::Database;
use deref_once_lock::OnceLock;
use host_port_pair::HostPortPair;
use json_strip_comments::StripComments;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
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

/// Secret values that [`config.example.jsonc`](config.example.jsonc) ships
/// as placeholders.
///
/// They must never reach a running deployment: a gateway started with a
/// known placeholder secret is open to everyone.
const EXAMPLE_PLACEHOLDER_SECRETS: [&str; 2] = ["CHANGE_ME_API_KEY", "CHANGE_ME_MANAGEMENT_TOKEN"];

/// Maximum idle time between two consecutive reads of an upstream response.
///
/// reqwest resets this deadline after every successful read, so it never
/// bounds a slow-but-flowing response; it only aborts a stalled upstream
/// connection, which would otherwise park the request task forever.
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Root gateway configuration schema.
///
/// Every field is mandatory and unknown fields are strictly rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
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
/// Panics if command-line arguments or configuration cannot be parsed, if
/// a configured secret is unsafe to deploy, or if the global configuration
/// has already been initialized.
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

    #[expect(
        clippy::expect_used,
        reason = "building the HTTP client at startup is fatal on failure"
    )]
    let http = Client::builder()
        .connect_timeout(Duration::from_secs(10))
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
/// strips comments, and deserializes and validates the configuration.
///
/// # Panics
///
/// Panics if command-line arguments are invalid, the configuration file cannot
/// be opened, deserialization fails, or a configured secret is unsafe to
/// deploy.
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
            _ => panic!("unexpected argument"),
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "missing configuration path argument is fatal"
    )]
    let path = config_path.expect("missing argument CONFIG");

    #[expect(
        clippy::unwrap_used,
        reason = "invalid configuration file path is fatal"
    )]
    let file = File::options().read(true).open(&path).unwrap();

    #[expect(clippy::unwrap_used, reason = "invalid configuration content is fatal")]
    let config: Config = serde_json::from_reader(StripComments::new(file)).unwrap();

    validate_secrets(&config);

    config
}

/// Rejects secret configuration values that would fail inbound authentication
/// open: empty, whitespace-only, or still set to the example placeholder.
///
/// An empty shared secret authorizes requests presenting the same empty
/// value, so it is a fatal configuration violation rather than a runtime
/// condition.
///
/// # Panics
///
/// Panics when any secret field is unsafe to deploy.
fn validate_secrets(config: &Config) {
    for (field, value) in [
        ("management_api.token", config.management_api.token.as_str()),
        ("api.key", config.api.key.as_str()),
    ] {
        if value.trim().is_empty() {
            invalid_secret(field, "must be a non-empty string");
        }

        if EXAMPLE_PLACEHOLDER_SECRETS.contains(&value) {
            invalid_secret(
                field,
                "is still the config.example.jsonc placeholder; replace it with a real secret",
            );
        }
    }
}

/// Reports an unsafe configured secret and aborts the process.
///
/// # Panics
///
/// Always: the caller has already established that the secret is unsafe to
/// deploy, and an unsafe secret must fail startup instead of failing
/// authentication open.
#[expect(
    clippy::panic,
    reason = "an unsafe configured secret is a fatal configuration violation"
)]
fn invalid_secret(field: &str, problem: &str) -> ! {
    panic!("configuration secret `{field}` {problem}");
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
    use std::{ffi::OsString, path::PathBuf};

    /// Replaces the global configuration for router-level tests; only the
    /// first call per process succeeds and later calls are ignored.
    pub(crate) fn set_config_for_tests(config: super::Config) {
        let _ = crate::CONFIG.set(config);
    }

    /// Renders a configuration file body carrying the given secrets.
    fn config_body(api_key: &str, management_token: &str) -> String {
        let database = concat!(
            r#"{"listen":"127.0.0.1:0","database":{"path":"test.db","max_connections":1,"#,
            r#""pool_timeout":1}"#
        );
        let management = format!(r#","management_api":{{"token":"{management_token}"}}"#);
        let api = format!(r#","api":{{"key":"{api_key}"}}}}"#);

        format!("{database}{management}{api}")
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

    /// Verifies that non-empty operator secrets parse.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_non_empty_secrets() {
        let temporary = TempConfig::write("valid-secrets", &config_body("key", "token"));

        let _config = temporary.parse();
    }

    /// Verifies that an empty `api.key` is fatal instead of authorizing
    /// empty bearer tokens.
    #[test]
    #[should_panic(expected = "`api.key`")]
    fn rejects_empty_api_key() {
        let temporary = TempConfig::write("empty-api-key", &config_body("", "token"));

        let _config = temporary.parse();
    }

    /// Verifies that a whitespace-only `api.key` is fatal.
    #[test]
    #[should_panic(expected = "must be a non-empty string")]
    fn rejects_whitespace_only_api_key() {
        let temporary = TempConfig::write("blank-api-key", &config_body("   ", "token"));

        let _config = temporary.parse();
    }

    /// Verifies that an empty `management_api.token` is fatal instead of
    /// authorizing empty authorization headers.
    #[test]
    #[should_panic(expected = "`management_api.token`")]
    fn rejects_empty_management_token() {
        let temporary = TempConfig::write("empty-token", &config_body("key", ""));

        let _config = temporary.parse();
    }

    /// Verifies that an example placeholder `api.key` is fatal.
    #[test]
    #[should_panic(expected = "`api.key`")]
    fn rejects_placeholder_api_key() {
        let temporary = TempConfig::write(
            "placeholder-api-key",
            &config_body("CHANGE_ME_API_KEY", "token"),
        );

        let _config = temporary.parse();
    }

    /// Verifies that an example placeholder `management_api.token` is fatal.
    #[test]
    #[should_panic(expected = "`management_api.token`")]
    fn rejects_placeholder_management_token() {
        let temporary = TempConfig::write(
            "placeholder-token",
            &config_body("key", "CHANGE_ME_MANAGEMENT_TOKEN"),
        );

        let _config = temporary.parse();
    }

    /// Verifies that the shipped example configuration, which still carries
    /// its placeholder secrets, is fatal to run: deserialization must
    /// succeed so the panic can only originate from secret validation.
    #[test]
    #[should_panic(expected = "placeholder")]
    fn rejects_config_example_placeholders() {
        let _config = super::parse_config(["janusgate", "-c", "config.example.jsonc"]);
    }
}
