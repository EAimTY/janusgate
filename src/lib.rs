#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::{
    config::ConfigLoader,
    context::Context,
    database::{Database, schema::Registry},
    server::Server,
    storage::api_logs::ApiLogStore,
};
use deref_once_lock::OnceLock;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use std::{ffi::OsString, path::PathBuf};

mod accounting;
mod affinity;
mod api;
mod compaction;
mod config;
mod context;
mod database;
mod format;
mod gateway;
pub(crate) mod http_body;
mod http_client;
mod ids;
mod ir;
mod logging;
mod management_api;
mod provider;
mod routing;
mod routing_text;
mod security;
mod serde_helpers;
mod server;
mod sse;
mod storage;
#[cfg(test)]
mod test_support;
mod unix_time;

#[cfg(feature = "management-api-typescript-bindings")]
pub use management_api::generate_typescript_bindings;

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
        Registry::Main,
        database.max_connections,
        database.pool_timeout,
    )
    .await
    .expect("failed to open database");

    // The api-log store is constructed once at startup from the `api_logs` section; its
    // presence is the recording switch, and the section's absence is the empty store (README
    // §3 *ApiLog Store*).
    let api_log_store = CONFIG
        .api_logs
        .as_ref()
        .map(|api_logs| ApiLogStore::new(api_logs.path.clone(), api_logs.retention_days));

    // One control-plane client is built per startup, and every short upstream round-trip — the
    // context handlers' and the device poller's alike — shares its connection pool (README §4).
    let control_plane = http_client::control_plane();

    crate::affinity::initialize_key();
    let context = Context {
        database: database.clone(),
        api_log_store,
        affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: std::sync::Arc::new(
            crate::provider::quota::coordinator::Coordinator::default(),
        ),
        data_plane: http_client::data_plane(),
        control_plane: control_plane.clone(),
    };

    // The background device poller resumes every unexpired pending device flow from its stored
    // schedule and keeps polling newly started flows.
    provider::oauth::spawn_device_poller(database, control_plane);

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
