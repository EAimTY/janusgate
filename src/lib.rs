#![doc = include_str!("../README.md")]
#![recursion_limit = "256"]

use crate::{
    config::ConfigLoader,
    context::Context,
    database::{Database, MAIN_POOL_WAIT, main_pool_connections, schema::Registry},
    server::Server,
    storage::api_logs::ApiLogStore,
};
use deref_once_lock::OnceLock;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use std::{ffi::OsString, path::PathBuf};

mod access_control;
mod accounting;
mod affinity;
mod api;
mod compaction;
mod compaction_model;
mod config;
mod context;
mod database;
mod format;
mod gateway;
mod http_body;
mod http_client;
mod ids;
mod ir;
mod logging;
mod management_api;
mod provider;
mod router_fallback;
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
mod vocabulary;

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
/// loaded, the global configuration has already been initialized, or the main database cannot be
/// opened or brought up to the latest schema version — a schema-drifted main database file is
/// fatal at startup, unlike an api-log day file, whose drift fails only its own request
/// (`docs/design/storage.md`).
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

    #[expect(
        clippy::expect_used,
        reason = "opening database at startup is fatal on failure"
    )]
    let database = Database::open(
        &CONFIG.database.path,
        Registry::Main,
        main_pool_connections(),
        MAIN_POOL_WAIT,
    )
    .await
    .expect("failed to open database");

    // The api-log store is constructed once at startup from the resolved `api.logs` section;
    // `api.logs.is_enabled` is the live recording switch the write path reads per dispatched
    // request, while the store's directory is consumed here, so a changed path applies after
    // restart (`docs/design/storage.md` *ApiLog Store*).
    let api_log_store =
        ApiLogStore::new(CONFIG.api.logs.path.clone(), CONFIG.api.logs.retention_days);

    // One control-plane client is built per startup, and every short upstream round-trip — the
    // context handlers' and the device poller's alike — shares its connection pool
    // (`docs/reference/configuration.md`).
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
