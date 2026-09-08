#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]
#![allow(dead_code)]

use crate::database::Database;
use deref_once_lock::OnceLock;
use host_port_pair::HostPortPair;
use json_strip_comments::StripComments;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use serde::Deserialize;
use std::{env, ffi::OsString, fs::File, panic, path::PathBuf, process};
use tokio::main;
#[cfg(test)]
use tokio::sync::OnceCell;

mod api;
mod database;
mod management_api;
mod server;

/// Global read-only application configuration, initialized once during process
/// startup and accessible throughout the crate without synchronization
/// overhead.
static CONFIG: OnceLock<Config> = OnceLock::new();

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
    max_connections: usize,
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
/// Panics if command-line arguments or configuration cannot be parsed, or if
/// the global configuration has already been initialized.
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
        reason = "opening database at startup is fatal on failure"
    )]
    let database = Database::open(
        &crate::CONFIG.database.path,
        crate::CONFIG.database.max_connections,
    )
    .await
    .expect("failed to open database");

    server::run(database).await
}

/// Initializes `CONFIG` for test execution if not already initialized.
///
/// # Panics
///
/// Panics if parsing the static test listen address fails.
#[cfg(test)]
pub(crate) fn init_test_config() {
    let temp_path = env::temp_dir().join(format!("janusgate_test_{}.db", process::id()));

    #[expect(
        clippy::unwrap_used,
        reason = "test host port pair parsing is infallible for valid literal"
    )]
    let listen = "127.0.0.1:8080".parse().unwrap();

    let _ = CONFIG.set(Config {
        listen,
        database: DatabaseConfig {
            path: temp_path,
            max_connections: 4,
        },
        management_api: ManagementApiConfig {
            token: "test_token".to_owned(),
        },
        api: ApiConfig {
            key: "test_key".to_owned(),
        },
    });
}

/// Returns a shared database instance for test execution.
///
/// # Panics
///
/// Panics if configuration or database initialization fails.
#[cfg(test)]
pub(crate) async fn test_database() -> Database {
    init_test_config();

    static DATABASE: OnceCell<Database> = OnceCell::const_new();

    DATABASE
        .get_or_init(|| async {
            #[expect(
                clippy::expect_used,
                reason = "test database initialization is fatal on failure"
            )]
            Database::open(
                &crate::CONFIG.database.path,
                crate::CONFIG.database.max_connections,
            )
            .await
            .expect("failed to open test database")
        })
        .await
        .clone()
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
    serde_json::from_reader::<_, Config>(StripComments::new(file)).unwrap()
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
    #[test]
    fn parses_config_example() {
        let _config = super::parse_config(["janusgate", "-c", "config.example.jsonc"]);
    }
}
