#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::{config::ConfigLoader, context::Context, database::Database, server::Server};
use deref_once_lock::OnceLock;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use std::{ffi::OsString, path::PathBuf};

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

/// Global application configuration loader, installed once during process startup, hot-reloaded by
/// its watcher task, and accessible throughout the crate without synchronization overhead.
static CONFIG: OnceLock<ConfigLoader> = OnceLock::new();

/// Runs the gateway: parses command-line arguments, loads and installs the global configuration,
/// initializes logging, spawns the configuration watcher, opens the database, and serves until the
/// process is terminated.
///
/// # Panics
///
/// Panics if command-line arguments are invalid, the configuration cannot be
/// loaded, or the global configuration has already been initialized.
pub async fn run(args: impl IntoIterator<Item = impl Into<OsString>>) -> ! {
    let config_path = parse_config_path(args);

    #[expect(
        clippy::expect_used,
        reason = "loading an invalid configuration at startup is fatal"
    )]
    let (config, watcher) = ConfigLoader::new(config_path).expect("cannot load the configuration");

    #[expect(
        clippy::unwrap_used,
        reason = "CONFIG is initialized exactly once at application startup"
    )]
    CONFIG.set(config).unwrap();

    logging::init();

    tokio::spawn(watcher);

    let database = CONFIG.database.clone();

    #[expect(
        clippy::expect_used,
        reason = "opening database at startup is fatal on failure"
    )]
    let database = Database::open(
        &database.path,
        database.max_connections,
        database.pool_timeout,
    )
    .await
    .expect("failed to open database");

    let context = Context {
        database,
        data_plane: http_client::data_plane(),
        control_plane: http_client::control_plane(),
    };

    Server::new(context).run().await
}

/// Parses command-line arguments, yielding the designated configuration file path.
///
/// # Panics
///
/// Panics if command-line arguments are invalid or the configuration path
/// is missing.
fn parse_config_path(args: impl IntoIterator<Item = impl Into<OsString>>) -> PathBuf {
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
                // Render the offending argument itself so the panic names the invocation element
                // that must be removed or corrected.
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
    config_path.expect("missing argument CONFIG")
}
