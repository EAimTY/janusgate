//! Shared test support for test builds: unique temporary paths, isolated
//! guarded test databases, canonical provider and model seed rows, request
//! bodies, and global configuration initialization.
//!
//! Every test receives its own file-system artifacts; the only process-wide
//! shared state is the global configuration, which [`init_config`] installs
//! exactly once per process.

use crate::{
    database::{Database, params},
    format::UpstreamFormat,
    provider::{Credentials, NewProvider, ProviderType, storage},
};
use axum::body::Bytes;
use std::{
    env,
    error::Error as StdError,
    fs,
    num::NonZero,
    path::{Path, PathBuf},
    process,
    sync::{
        Once,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Monotonic counter making every generated test artifact path unique within
/// the process.
static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// RAII guard deleting a test's temporary database file when dropped, so the
/// file is removed on the success path, on any error return, and during the
/// unwind of a panicking test.
pub(crate) struct TempDatabaseGuard {
    /// Path removed when this guard is dropped.
    path: PathBuf,
}

impl TempDatabaseGuard {
    /// Opens an isolated test database at a unique temporary path with the
    /// given pool size and acquisition timeout and guards the path for
    /// deletion on drop. Every call yields its own database file; the path is
    /// never shared across tests. The guard exists before the database opens,
    /// so the file is removed even when opening fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while opening the database.
    pub(crate) async fn open_database(
        max_connections: usize,
        pool_timeout: Duration,
    ) -> Result<(Database, Self), Box<dyn StdError>> {
        let mut path = unique_temp_path("test_database");

        path.set_extension("db");

        let guard = Self { path };

        let database = open_test_database_at(&guard.path, max_connections, pool_timeout).await?;

        Ok((database, guard))
    }
}

/// Opens an isolated test database at the given path with the given pool size
/// and acquisition timeout, so tests that reopen one on-disk database or
/// exercise path-dependent behavior can name the path themselves.
///
/// # Errors
///
/// Returns any error encountered while opening the database.
pub(crate) async fn open_test_database_at(
    path: &Path,
    max_connections: usize,
    pool_timeout: Duration,
) -> Result<Database, Box<dyn StdError>> {
    let database = Database::open(path, pool_size(max_connections), pool_timeout).await?;

    Ok(database)
}

/// Inserts one enabled `openai_compatible` provider row through the storage
/// layer, carrying the chat-completion protocol, an example upstream base
/// URL, and the given API key credentials, returning its row identifier.
///
/// # Errors
///
/// Returns any error encountered while inserting the row.
pub(crate) async fn seed_compatible_provider(
    database: &Database,
    name: &str,
    api_key: &str,
) -> Result<i64, Box<dyn StdError>> {
    storage::insert_provider(
        database,
        &NewProvider {
            name,
            model_prefix: "",
            provider_type: ProviderType::OpenAiCompatible,
            protocol: Some(UpstreamFormat::OpenAiChatCompletion),
            base_url: Some("https://upstream.example"),
            credentials: &Credentials::ApiKey(api_key.to_string()),
            expires_at: None,
            is_enabled: true,
        },
    )
    .await?;

    let rows = database
        .query("SELECT id FROM providers WHERE name = ?1", params![name])
        .await?;

    Ok(rows.first().ok_or("missing provider row")?.get(0)?)
}

/// Inserts one enabled `openai_compatible` provider row carrying only its
/// name, model prefix, and API-key credentials; protocol and base URL stay
/// unset. Returns the reported row change count.
///
/// # Errors
///
/// Returns any error encountered while inserting the row.
pub(crate) async fn seed_minimal_provider(
    database: &Database,
    name: &str,
    model_prefix: &str,
    api_key: &str,
) -> Result<u64, Box<dyn StdError>> {
    let credentials = format!(r#"{{"apiKey":"{api_key}"}}"#);

    let changes = database
        .execute(
            "INSERT INTO providers (name, model_prefix, type, credentials) \
             VALUES (?1, ?2, 'openai_compatible', ?3)",
            params![name, model_prefix, credentials],
        )
        .await?;

    Ok(changes)
}

/// Inserts one model row of the provider named `provider_name`, joined
/// through the provider's name, with the given upstream and public names.
///
/// # Errors
///
/// Returns any error encountered while inserting the row.
pub(crate) async fn seed_model(
    database: &Database,
    provider_name: &str,
    upstream_name: &str,
    name: &str,
) -> Result<(), Box<dyn StdError>> {
    database
        .execute(
            "INSERT INTO models (provider_id, upstream_name, name) \
             SELECT id, ?2, ?3 FROM providers WHERE name = ?1",
            params![provider_name, upstream_name, name],
        )
        .await?;

    Ok(())
}

/// Writes a throwaway configuration file at a unique temporary path and
/// installs it as the global test configuration exactly once per process;
/// later calls are ignored, mirroring the process-global configuration.
///
/// # Panics
///
/// Panics if the configuration file cannot be written, or if the
/// configuration fails to parse or secret validation rejects it.
pub(crate) fn init_config() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let path = unique_temp_path("test_config").with_extension("jsonc");

        #[expect(
            clippy::expect_used,
            reason = "a configuration write failure fails the test"
        )]
        fs::write(
            &path,
            r#"{
                "log_level": "info",
                "listen": "127.0.0.1:1",
                "database": {"path": "irrelevant.db", "max_connections": 1, "pool_timeout": 1},
                "management_api": {"token": "test-mgmt"},
                "api": {"key": "test-api-key"}
            }"#,
        )
        .expect("config file should write");

        crate::tests::set_config_for_tests(crate::parse_config([
            "janusgate".to_string(),
            "-c".to_string(),
            path.to_string_lossy().to_string(),
        ]));
    });
}

/// Builds a JSON request body from a string.
pub(crate) fn body(json: &str) -> Bytes {
    Bytes::from(json.as_bytes().to_vec())
}

/// Converts a pool size into a non-zero connection count.
///
/// # Panics
///
/// Panics if the pool size is zero.
#[expect(
    clippy::expect_used,
    reason = "every call site passes a non-zero pool size"
)]
pub(crate) fn pool_size(connections: usize) -> NonZero<usize> {
    NonZero::new(connections).expect("pool size is non-zero")
}

/// Returns a unique file-system entry name carrying the given `prefix`:
/// `janusgate_{prefix}_{process id}_{counter}`.
///
/// The process id and a monotonically increasing counter make tests running
/// concurrently within one process, and reruns after a crashed process,
/// never collide on a file-system entry.
pub(crate) fn unique_temp_name(prefix: &str) -> String {
    let unique_id = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("janusgate_{prefix}_{}_{unique_id}", process::id())
}

/// Returns a unique file-system entry path inside the system temporary
/// directory carrying the given `prefix`.
pub(crate) fn unique_temp_path(prefix: &str) -> PathBuf {
    env::temp_dir().join(unique_temp_name(prefix))
}

impl Drop for TempDatabaseGuard {
    fn drop(&mut self) {
        // Cleanup is best-effort: a leftover artifact in the system temporary
        // directory must not fail an otherwise passing test from `Drop`.
        let _ = fs::remove_file(&self.path);

        // The engine keeps a write-ahead log and a shared-memory sidecar
        // beside the database file; remove them too, still best-effort.
        for suffix in ["-wal", "-shm"] {
            let mut sidecar_path = self.path.as_os_str().to_os_string();

            sidecar_path.push(suffix);

            let _ = fs::remove_file(PathBuf::from(sidecar_path));
        }
    }
}
