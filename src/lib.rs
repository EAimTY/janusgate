#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::{config::Config, context::Context, database::Database};
use deref_once_lock::OnceLock;
use json_strip_comments::StripComments;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use std::{ffi::OsString, fs::File, path::PathBuf};

mod api;
mod config;
mod context;
mod database;
mod format;
mod gateway;
mod http_client;
mod ids;
mod ir;
mod logging;
pub mod management_api;
mod provider;
mod security;
mod server;
mod sse;
mod unix_time;

/// Global read-only application configuration, initialized once during process
/// startup and accessible throughout the crate without synchronization
/// overhead.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Runs the gateway: parses command-line arguments, loads and installs the
/// global configuration, initializes logging, opens the database, and serves
/// until the process is terminated.
///
/// # Panics
///
/// Panics if command-line arguments or configuration cannot be parsed or if
/// the global configuration or logger has already been initialized.
pub async fn run(args: impl IntoIterator<Item = impl Into<OsString>>) -> ! {
    let config = parse_config(args);

    #[expect(
        clippy::unwrap_used,
        reason = "CONFIG is initialized exactly once at application startup"
    )]
    CONFIG.set(config).unwrap();

    logging::init();

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

    let context = Context {
        database,
        data_plane: http_client::data_plane(),
        control_plane: http_client::control_plane(),
    };

    server::run(context).await
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

    #[expect(
        clippy::expect_used,
        reason = "an unopenable configuration file is fatal"
    )]
    let file = File::options()
        .read(true)
        .open(&path)
        .expect("cannot open the configuration file");

    #[expect(
        clippy::expect_used,
        reason = "unparsable configuration content is fatal"
    )]
    let config: Config = serde_json::from_reader(StripComments::new(file))
        .expect("cannot parse the configuration file");

    config
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
