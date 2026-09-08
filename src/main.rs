#![allow(dead_code)]

use deref_once_lock::OnceLock;
use host_port_pair::HostPortPair;
use json_strip_comments::StripComments;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use serde::Deserialize;
use std::{env, ffi::OsString, fs::File, panic, path::PathBuf, process};
use tokio as _;

/// Global read-only application configuration, initialized once during process startup
/// and accessible throughout the crate without synchronization overhead.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Root gateway configuration schema.
///
/// Every field is mandatory and unknown fields are strictly rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// Address and TCP port on which the gateway listens for plaintext HTTP connections.
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
}

/// Administrative management API configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagementApiConfig {
    /// Bearer-less administrative authorization token required by `/management-api/*`.
    token: String,
}

/// Outward client protocol configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiConfig {
    /// Shared API key for outward protocol endpoints under `/api/*`.
    key: String,
}

fn main() {
    // Install a process-terminating panic hook before any other operation to ensure
    // thread panics abort immediately instead of leaving the application in a degraded state.
    set_panic_hook();

    let config = parse_config(env::args_os());

    // SAFETY: CONFIG is initialized exactly once at application startup
    CONFIG.set(config).unwrap();
}

/// Parses command-line arguments, opens the designated configuration file,
/// strips comments, and deserializes the configuration.
fn parse_config(args: impl IntoIterator<Item = impl Into<OsString>>) -> Config {
    let mut parser = ArgumentsParser::from_iter(args);
    let mut config_path = None;

    // SAFETY: command-line argument syntax errors are fatal
    while let Some(argument) = parser.next().unwrap() {
        match argument {
            Argument::Short('c') | Argument::Long("config") => {
                // SAFETY: missing value for configuration option is fatal
                config_path = Some(PathBuf::from(parser.value().unwrap()));
            }
            // SAFETY: unrecognized command-line arguments are fatal
            _ => panic!("unexpected argument"),
        }
    }

    // SAFETY: missing configuration path argument is fatal
    let path = config_path.expect("missing argument CONFIG");

    // SAFETY: invalid configuration file path is fatal
    let file = File::options().read(true).open(&path).unwrap();

    // SAFETY: invalid configuration content is fatal
    serde_json::from_reader::<_, Config>(StripComments::new(file)).unwrap()
}

/// Configures a custom panic hook that terminates the entire process immediately.
///
/// By default, a panic on a background or worker thread in Rust only unwinds that specific
/// thread. In an asynchronous server, this leaves the remaining runtime operational while
/// services or connections may be permanently broken. This hook prints diagnostic panic info
/// to stderr via the standard hook and calls `process::exit(1)` to ensure fail-fast semantics.
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
