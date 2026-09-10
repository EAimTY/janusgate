//! Tests for the connection-pooled database abstraction.
//!
//! The tests live in a child module of `database` instead of an external
//! integration test because several contracts are only reachable white-box:
//! closing the pool, parking an idle checkout, and checking out the raw
//! connection that later poisons recycling have no public API surface.

use super::{Database, Error, Value, params};
use crate::test_support;
use std::{
    env,
    error::Error as StdError,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

/// SQL inserting one canonical provider row with positional parameters.
const INSERT_PROVIDER: &str = "INSERT INTO providers (name, model_prefix, type, credentials) \
     VALUES (?1, ?2, ?3, ?4)";

/// Credentials JSON payload shared by tests inserting provider rows.
const TEST_CREDENTIALS: &str = r#"{"apiKey":"test"}"#;

/// Pool size used by tests that do not exercise pool exhaustion.
const TEST_POOL_SIZE: usize = 4;

/// Pool timeout long enough that tests exercising unrelated behavior never
/// observe an acquisition timeout.
const TEST_POOL_TIMEOUT: Duration = Duration::from_secs(5);

/// RAII guard deleting a test's temporary file-system artifact when dropped,
/// so a database file or directory is removed on the success path, on any
/// error return, and during the unwind of a panicking test.
struct TempArtifactGuard {
    /// Path removed when this guard is dropped.
    path: PathBuf,

    /// Whether the path is a directory tree rather than a single file.
    is_directory: bool,
}

impl TempArtifactGuard {
    /// Builds the unique temporary database path via
    /// [`test_support::unique_temp_path`], guards it, and opens a database
    /// at it. The guard exists before the database opens, so the file is
    /// removed even when opening fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while opening the database.
    async fn open_database(
        max_connections: usize,
        pool_timeout: Duration,
    ) -> Result<(Database, Self), Box<dyn StdError>> {
        let mut path = test_support::unique_temp_path("test_database");

        path.set_extension("db");

        let guard = Self {
            path,
            is_directory: false,
        };

        let database =
            test_support::open_test_database_at(&guard.path, max_connections, pool_timeout).await?;

        Ok((database, guard))
    }

    /// Guards a single file for deletion on drop.
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            is_directory: false,
        }
    }

    /// Guards a directory tree for deletion on drop.
    fn directory(path: PathBuf) -> Self {
        Self {
            path,
            is_directory: true,
        }
    }

    /// The guarded path, for tests that reopen the same artifact.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempArtifactGuard {
    fn drop(&mut self) {
        let removal = if self.is_directory {
            fs::remove_dir_all(&self.path)
        } else {
            fs::remove_file(&self.path)
        };

        // Cleanup is best-effort: a leftover artifact in the system temporary
        // directory must not fail an otherwise passing test from `Drop`.
        let _ = removal;

        // The engine keeps a write-ahead log and a shared-memory sidecar
        // beside the database file; remove them too, still best-effort.
        for suffix in ["-wal", "-shm"] {
            let mut sidecar_path = self.path.as_os_str().to_os_string();

            sidecar_path.push(suffix);

            let _ = fs::remove_file(PathBuf::from(sidecar_path));
        }
    }
}

/// Inserts one canonical provider row and asserts the write reports exactly
/// one changed row.
///
/// # Panics
///
/// Panics if the insert does not report exactly one change.
///
/// # Errors
///
/// Returns any error encountered while executing the insert.
async fn insert_provider(
    database: &Database,
    name: &str,
    model_prefix: &str,
) -> Result<(), Box<dyn StdError>> {
    let changes = test_support::seed_minimal_provider(database, name, model_prefix, "test").await?;

    assert_eq!(changes, 1);

    Ok(())
}

/// Builds positional parameters inserting one canonical provider row.
fn provider_params(name: &str, model_prefix: &str) -> [Value; 4] {
    [
        name.into(),
        model_prefix.into(),
        "openai_compatible".into(),
        TEST_CREDENTIALS.into(),
    ]
}

/// Queries a single integer from the first column of a statement that must
/// produce at least one row.
///
/// # Errors
///
/// Returns an error if the query fails or produces no rows.
async fn query_single_integer(database: &Database, sql: &str) -> Result<i64, Box<dyn StdError>> {
    let rows = database.query(sql, ()).await?;

    let row = rows.first().ok_or("query returned no rows")?;

    Ok(row.get(0)?)
}

/// Verifies that opening a fresh database applies the canonical schema
/// atomically with the fingerprint write: `PRAGMA user_version` reports the
/// compile-time fingerprint, every canonical schema object exists, and
/// reopening the finished database takes the compatible-fingerprint path.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn open_applies_canonical_schema_to_fresh_database() -> Result<(), Box<dyn StdError>> {
    let (database, path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let user_version = query_single_integer(&database, "PRAGMA user_version").await?;

    assert_eq!(user_version, i64::from(super::DATABASE_SCHEMA_FINGERPRINT));

    let rows = database
        .query("SELECT name FROM sqlite_master ORDER BY name", ())
        .await?;

    let object_names: Vec<String> = rows
        .iter()
        .map(|row| row.get::<String>(0))
        .collect::<Result<Vec<_>, _>>()?;

    for expected_name in [
        "providers",
        "models",
        "oauth_states",
        "idx_oauth_states_expires_at",
    ] {
        assert!(
            object_names.iter().any(|name| name == expected_name),
            "missing schema object {expected_name}"
        );
    }

    let reopened_database =
        test_support::open_test_database_at(path_guard.path(), TEST_POOL_SIZE, TEST_POOL_TIMEOUT)
            .await?;

    let user_version = query_single_integer(&reopened_database, "PRAGMA user_version").await?;

    assert_eq!(user_version, i64::from(super::DATABASE_SCHEMA_FINGERPRINT));

    Ok(())
}

/// Verifies that opening a database whose stored fingerprint is incompatible
/// panics instead of executing against an unverified schema.
///
/// # Panics
///
/// Panics by contract when opening the tampered database.
#[tokio::test]
#[should_panic(expected = "schema fingerprint mismatch")]
async fn open_panics_on_incompatible_schema_fingerprint() {
    // The first open takes the fresh-database path; its failure is setup
    // breakage, so it fails the test directly.
    #[expect(
        clippy::expect_used,
        reason = "opening the fresh test database is fatal on failure"
    )]
    let (database, path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT)
            .await
            .expect("fresh test database should open");

    // Record a plausible but incompatible fingerprint.
    let tampered_version = super::DATABASE_SCHEMA_FINGERPRINT.wrapping_sub(1);

    #[expect(
        clippy::expect_used,
        reason = "tampering the fingerprint is fatal on failure"
    )]
    database
        .execute(&format!("PRAGMA user_version = {tampered_version}"), ())
        .await
        .expect("failed to tamper with user_version");

    drop(database);

    // The reopening panics by contract before any result can be produced; the
    // unwinding panic drops the guard and still removes the database file.
    let _ =
        test_support::open_test_database_at(path_guard.path(), TEST_POOL_SIZE, TEST_POOL_TIMEOUT)
            .await;
}

/// Verifies that opening a database creates every missing parent directory.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn open_creates_missing_parent_directories() -> Result<(), Box<dyn StdError>> {
    let directory = test_support::unique_temp_path("database_test_dir");

    let _directory_guard = TempArtifactGuard::directory(directory.clone());

    let path = directory.join("nested").join("gateway.db");

    let database =
        test_support::open_test_database_at(&path, TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let provider_count = query_single_integer(&database, "SELECT COUNT(*) FROM providers").await?;

    assert_eq!(provider_count, 0);

    Ok(())
}

/// Verifies that a database path whose parent is an ordinary file surfaces
/// the filesystem failure as [`Error::Io`].
///
/// # Panics
///
/// Panics if creating the blocking file fails.
#[tokio::test]
async fn open_reports_io_error_when_parent_is_a_file() -> Result<(), Box<dyn StdError>> {
    let blocker_path = test_support::unique_temp_path("database_test_blocker");

    let _blocker_guard = TempArtifactGuard::file(blocker_path.clone());

    fs::write(&blocker_path, b"")?;

    let result = Database::open(
        blocker_path.join("gateway.db"),
        test_support::pool_size(TEST_POOL_SIZE),
        TEST_POOL_TIMEOUT,
    )
    .await;

    assert!(matches!(result, Err(Error::Io(_))));

    Ok(())
}

/// Verifies that a database path that is not valid UTF-8 is rejected with
/// [`Error::InvalidPath`] instead of being silently mangled.
///
/// # Panics
///
/// Panics if the non-UTF-8 path cannot be constructed.
#[tokio::test]
#[cfg(unix)]
async fn open_rejects_non_utf8_path() -> Result<(), Box<dyn StdError>> {
    // 0xff can never appear in valid UTF-8, so `to_str` must reject the path.
    let mut file_name = test_support::unique_temp_name("database_test_not_utf8").into_bytes();

    file_name.push(0xff);
    file_name.extend_from_slice(b".db");

    let path = env::temp_dir().join(OsStr::from_bytes(&file_name));

    let result = Database::open(
        path,
        test_support::pool_size(TEST_POOL_SIZE),
        TEST_POOL_TIMEOUT,
    )
    .await;

    assert!(matches!(result, Err(Error::InvalidPath(_))));

    Ok(())
}

/// Verifies that a database path that is not valid UTF-8 is rejected with
/// [`Error::InvalidPath`] before any filesystem mutation, so no parent
/// directory is created for the rejected path.
///
/// # Panics
///
/// Panics if the non-UTF-8 path cannot be constructed.
#[tokio::test]
#[cfg(unix)]
async fn open_rejects_non_utf8_path_without_creating_parents() -> Result<(), Box<dyn StdError>> {
    // 0xff can never appear in valid UTF-8, so `to_str` must reject the
    // parent directory name before any filesystem mutation can target it.
    // On Linux, `create_dir_all` would otherwise succeed on the invalid name,
    // making the missing validation observable.
    let mut directory_name =
        test_support::unique_temp_name("database_test_dir_not_utf8").into_bytes();

    directory_name.push(0xff);

    let directory = env::temp_dir().join(OsStr::from_bytes(&directory_name));

    let path = directory.join("gateway.db");

    let result = Database::open(
        path,
        test_support::pool_size(TEST_POOL_SIZE),
        TEST_POOL_TIMEOUT,
    )
    .await;

    assert!(matches!(result, Err(Error::InvalidPath(_))));

    // The rejected path must not have left its parent directory behind.
    assert!(!directory.exists());

    Ok(())
}

/// Verifies that `query` materializes every SQLite storage class: typed
/// parameters round-trip `INTEGER`, `REAL`, `TEXT`, and `BLOB` values through
/// a `STRICT` table, `NULL` maps to `Option::None` in both binding and
/// extraction, and a query matching no rows yields an empty vector.
///
/// # Panics
///
/// Panics if the scratch table, inserts, or value assertions fail.
#[tokio::test]
async fn query_materializes_typed_values_and_nulls() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    database
        .execute(
            "CREATE TABLE test_values (\
                integer_value INTEGER NOT NULL,\
                real_value REAL NOT NULL,\
                text_value TEXT NOT NULL,\
                blob_value BLOB NOT NULL,\
                nullable_value INTEGER\
            ) STRICT",
            (),
        )
        .await?;

    let changes = database
        .execute(
            "INSERT INTO test_values VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                42_i64,
                1.5_f64,
                "gateway",
                b"\x00\x01".as_slice(),
                Option::<i64>::None
            ],
        )
        .await?;

    assert_eq!(changes, 1);

    let rows = database
        .query(
            "SELECT integer_value, real_value, text_value, blob_value, nullable_value \
             FROM test_values",
            (),
        )
        .await?;

    assert_eq!(rows.len(), 1);

    let row = rows.first().ok_or("missing values row")?;

    assert_eq!(row.get_value(0)?, Value::Integer(42));
    assert_eq!(row.get_value(1)?, Value::Real(1.5));
    assert_eq!(row.get_value(2)?, Value::Text("gateway".to_string()));
    assert_eq!(row.get_value(3)?, Value::Blob(b"\x00\x01".to_vec()));
    assert_eq!(row.get_value(4)?, Value::Null);

    assert_eq!(row.get::<i64>(0)?, 42);
    assert_eq!(row.get::<Option<i64>>(4)?, None);

    let rows = database
        .query(
            "SELECT integer_value FROM test_values WHERE integer_value = ?1",
            params![7_i64],
        )
        .await?;

    assert!(rows.is_empty());

    Ok(())
}

/// Verifies that the per-connection `PRAGMA foreign_keys = ON` configuration
/// is active on pooled connections: inserting a model row referencing a
/// missing provider fails, and the identical insert succeeds once the
/// referenced provider exists.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn foreign_keys_are_enforced_on_pooled_connections() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let orphaned_insert = database
        .execute(
            "INSERT INTO models (provider_id, upstream_name, name) VALUES (?1, ?2, ?3)",
            params![999_i64, "gpt-5", "ghost/gpt-5"],
        )
        .await;

    assert!(matches!(orphaned_insert, Err(Error::Turso(_))));

    insert_provider(&database, "fk_provider", "fk/").await?;

    let provider_id = query_single_integer(
        &database,
        "SELECT id FROM providers WHERE name = 'fk_provider'",
    )
    .await?;

    database
        .execute(
            "INSERT INTO models (provider_id, upstream_name, name) VALUES (?1, ?2, ?3)",
            params![provider_id, "gpt-5", "fk/gpt-5"],
        )
        .await?;

    Ok(())
}

/// Verifies that a SQLite constraint violation surfaces as [`Error::Turso`].
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn unique_constraint_violation_yields_turso_error() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    insert_provider(&database, "duplicate_provider", "dup/").await?;

    let duplicate_insert = database
        .execute(
            INSERT_PROVIDER,
            provider_params("duplicate_provider", "dup/"),
        )
        .await;

    assert!(matches!(duplicate_insert, Err(Error::Turso(_))));

    Ok(())
}

/// Verifies that the `providers.type` closed domain admits exactly the three
/// canonical provider type names and rejects every other value at the storage
/// layer.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
///
/// # Errors
///
/// Returns any error encountered while executing the inserts.
#[tokio::test]
async fn provider_type_check_rejects_unknown_values() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    for provider_type in ["openai_oauth", "antigravity_oauth", "openai_compatible"] {
        let name = format!("provider_{provider_type}");

        database
            .execute(
                INSERT_PROVIDER,
                params![name.as_str(), "", provider_type, TEST_CREDENTIALS],
            )
            .await?;
    }

    let rejected_insert = database
        .execute(
            INSERT_PROVIDER,
            params!["provider_unknown", "", "unknown", TEST_CREDENTIALS],
        )
        .await;

    assert!(matches!(rejected_insert, Err(Error::Turso(_))));

    Ok(())
}

/// Verifies that the `oauth_states.type` closed domain admits exactly the two
/// OAuth flow types and rejects the API-key-only provider type, which is a
/// valid `providers.type` but never an OAuth flow type.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
///
/// # Errors
///
/// Returns any error encountered while executing the inserts.
#[tokio::test]
async fn oauth_state_type_check_rejects_unknown_values() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    for flow_type in ["openai_oauth", "antigravity_oauth"] {
        let state = format!("state_{flow_type}");
        let verifier = format!("verifier_{flow_type}");

        database
            .execute(
                "INSERT INTO oauth_states (type, state, verifier, expires_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![flow_type, state.as_str(), verifier.as_str(), 0_i64],
            )
            .await?;
    }

    let rejected_insert = database
        .execute(
            "INSERT INTO oauth_states (type, state, verifier, expires_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params!["openai_compatible", "state_unknown", "", 0_i64],
        )
        .await;

    assert!(matches!(rejected_insert, Err(Error::Turso(_))));

    Ok(())
}

/// Verifies that the `providers.enabled` boolean closed domain admits exactly
/// 0 and 1 and rejects every other integer.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
///
/// # Errors
///
/// Returns any error encountered while executing the inserts.
#[tokio::test]
async fn provider_enabled_check_rejects_non_boolean_values() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    for enabled_value in [0_i64, 1_i64] {
        let name = format!("provider_enabled_{enabled_value}");

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials, enabled) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    name.as_str(),
                    "",
                    "openai_compatible",
                    TEST_CREDENTIALS,
                    enabled_value
                ],
            )
            .await?;
    }

    let rejected_insert = database
        .execute(
            "INSERT INTO providers (name, model_prefix, type, credentials, enabled) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "provider_non_boolean",
                "",
                "openai_compatible",
                TEST_CREDENTIALS,
                2_i64
            ],
        )
        .await;

    assert!(matches!(rejected_insert, Err(Error::Turso(_))));

    Ok(())
}

/// Verifies that the `models.enabled_override` boolean closed domain admits
/// exactly `NULL`, 0, and 1 and rejects every other integer.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
///
/// # Errors
///
/// Returns any error encountered while executing the inserts.
#[tokio::test]
async fn model_enabled_override_check_rejects_non_boolean() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    insert_provider(&database, "override_provider", "override/").await?;

    let provider_id = query_single_integer(
        &database,
        "SELECT id FROM providers WHERE name = 'override_provider'",
    )
    .await?;

    for (case_index, enabled_override_value) in [Option::<i64>::None, Some(0_i64), Some(1_i64)]
        .into_iter()
        .enumerate()
    {
        let upstream_name = format!("gpt-5-admitted-{case_index}");
        let name = format!("override/{upstream_name}");

        database
            .execute(
                "INSERT INTO models (provider_id, upstream_name, name, enabled_override) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    provider_id,
                    upstream_name.as_str(),
                    name.as_str(),
                    enabled_override_value
                ],
            )
            .await?;
    }

    let rejected_insert = database
        .execute(
            "INSERT INTO models (provider_id, upstream_name, name, enabled_override) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                provider_id,
                "gpt-5-rejected",
                "override/gpt-5-rejected",
                2_i64
            ],
        )
        .await;

    assert!(matches!(rejected_insert, Err(Error::Turso(_))));

    Ok(())
}

/// Verifies that committing a transaction persists its statements.
///
/// # Panics
///
/// Panics if transaction execution or an assertion fails.
#[tokio::test]
async fn transaction_commit_persists_changes() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let mut transaction = database.transaction().await?;

    transaction
        .execute(
            INSERT_PROVIDER,
            provider_params("commit_provider", "commit/"),
        )
        .await?;

    transaction.commit().await?;

    let committed_rows = query_single_integer(
        &database,
        "SELECT COUNT(*) FROM providers WHERE name = 'commit_provider'",
    )
    .await?;

    assert_eq!(committed_rows, 1);

    Ok(())
}

/// Verifies that an explicit rollback discards every statement of the
/// transaction and returns the connection to the pool healthy.
///
/// # Panics
///
/// Panics if transaction execution or an assertion fails.
#[tokio::test]
async fn transaction_rollback_discards_changes() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let mut transaction = database.transaction().await?;

    transaction
        .execute(
            INSERT_PROVIDER,
            provider_params("rollback_provider", "rollback/"),
        )
        .await?;

    transaction.rollback().await?;

    let rolled_back_rows = query_single_integer(
        &database,
        "SELECT COUNT(*) FROM providers WHERE name = 'rollback_provider'",
    )
    .await?;

    assert_eq!(rolled_back_rows, 0);

    // The rolled-back connection is reusable through the pool.
    insert_provider(&database, "after_rollback_provider", "after/").await?;

    Ok(())
}

/// Verifies that dropping an unfinished transaction discards its statements.
///
/// # Panics
///
/// Panics if transaction execution or an assertion fails.
#[tokio::test]
async fn transaction_drop_discards_uncommitted_changes() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    {
        let mut transaction = database.transaction().await?;

        transaction
            .execute(
                INSERT_PROVIDER,
                provider_params("dropped_provider", "dropped/"),
            )
            .await?;

        // Dropped here without commit or rollback.
    }

    let dropped_rows = query_single_integer(
        &database,
        "SELECT COUNT(*) FROM providers WHERE name = 'dropped_provider'",
    )
    .await?;

    assert_eq!(dropped_rows, 0);

    Ok(())
}

/// Verifies that dropping an unfinished transaction releases the engine-level
/// write lock immediately: an independently checked out connection can take
/// the write lock without stalling, and the dropped transaction's write stays
/// discarded.
///
/// # Panics
///
/// Panics if transaction execution or an assertion fails.
#[tokio::test]
async fn transaction_drop_releases_write_lock_immediately() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    // Park an idle connection so the pool cannot hand the dropped
    // transaction's connection back out during this test; a leaked write lock
    // would then stall the independent write below.
    let idle_checkout = database.pool.get().await?;

    {
        let mut transaction = database.transaction().await?;

        transaction
            .execute(
                INSERT_PROVIDER,
                provider_params("parked_provider", "parked/"),
            )
            .await?;

        // Dropped here with the engine-level write transaction still open.
    }

    idle_checkout.execute("BEGIN IMMEDIATE", ()).await?;

    idle_checkout
        .execute(
            INSERT_PROVIDER,
            provider_params("unparked_provider", "parked/"),
        )
        .await?;

    idle_checkout.execute("COMMIT", ()).await?;

    let rows = database
        .query(
            "SELECT name FROM providers WHERE model_prefix = ?1 ORDER BY name",
            params!["parked/"],
        )
        .await?;

    let names: Vec<String> = rows
        .iter()
        .map(|row| row.get::<String>(0))
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(names, ["unparked_provider"]);

    Ok(())
}

/// Verifies that the pool serves new transactions after a transaction handle
/// was dropped unfinished, proving the discarded checkout is replaced rather
/// than reused.
///
/// # Panics
///
/// Panics if transaction execution or an assertion fails.
#[tokio::test]
async fn transaction_drop_allows_new_transactions() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    {
        let mut transaction = database.transaction().await?;

        transaction
            .execute(
                INSERT_PROVIDER,
                provider_params("poisoned_provider", "poisoned/"),
            )
            .await?;

        // Dropped here without commit or rollback.
    }

    let mut transaction = database.transaction().await?;

    transaction
        .execute(
            INSERT_PROVIDER,
            provider_params("recovered_provider", "recovered/"),
        )
        .await?;

    transaction.commit().await?;

    let recovered_rows = query_single_integer(
        &database,
        "SELECT COUNT(*) FROM providers WHERE name = 'recovered_provider'",
    )
    .await?;

    assert_eq!(recovered_rows, 1);

    Ok(())
}

/// Verifies that a failed statement inside a transaction leaves the
/// transaction intact: prior statements stay discardable through rollback,
/// and the connection returns to the pool healthy.
///
/// # Panics
///
/// Panics if transaction execution or an assertion fails.
#[tokio::test]
async fn transaction_survives_failed_statement_and_rollback() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let mut transaction = database.transaction().await?;

    transaction
        .execute(
            INSERT_PROVIDER,
            provider_params("doomed_provider", "doomed/"),
        )
        .await?;

    let foreign_key_violation = transaction
        .execute(
            "INSERT INTO models (provider_id, upstream_name, name) VALUES (?1, ?2, ?3)",
            params![999_i64, "gpt-5", "doomed/gpt-5"],
        )
        .await;

    assert!(matches!(foreign_key_violation, Err(Error::Turso(_))));

    transaction.rollback().await?;

    let doomed_rows = query_single_integer(
        &database,
        "SELECT COUNT(*) FROM providers WHERE name = 'doomed_provider'",
    )
    .await?;

    assert_eq!(doomed_rows, 0);

    let model_rows = query_single_integer(&database, "SELECT COUNT(*) FROM models").await?;

    assert_eq!(model_rows, 0);

    insert_provider(&database, "healthy_provider", "healthy/").await?;

    Ok(())
}

/// Verifies that checking out from a fully occupied pool fails with
/// [`Error::PoolTimeout`] once the configured timeout elapses, and that the
/// pool recovers once a connection is released.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn exhausted_pool_times_out_then_recovers() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(1, Duration::from_millis(50)).await?;

    let transaction = database.transaction().await?;

    let exhausted_checkout = database.query("SELECT 1", ()).await;

    assert!(matches!(exhausted_checkout, Err(Error::PoolTimeout)));

    transaction.commit().await?;

    database.query("SELECT 1", ()).await?;

    Ok(())
}

/// Verifies that every public entry point reports [`Error::PoolClosed`] once
/// the connection pool has been closed.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn closed_pool_reports_pool_closed_for_every_entry_point() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    database.pool.close();

    assert!(matches!(
        database.execute("SELECT 1", ()).await,
        Err(Error::PoolClosed)
    ));

    assert!(matches!(
        database.query("SELECT 1", ()).await,
        Err(Error::PoolClosed)
    ));

    assert!(matches!(
        database.transaction().await,
        Err(Error::PoolClosed)
    ));

    Ok(())
}

/// Verifies that a pooled connection recycled with an open transaction is
/// discarded rather than reused: the uncommitted write trapped in the leaked
/// transaction never becomes visible, and the pool keeps serving requests on
/// a replacement connection.
///
/// The leak is produced through the documented misuse path — a raw `BEGIN`
/// issued through the statement API on a checked out connection.
///
/// # Panics
///
/// Panics if database initialization or an assertion fails.
#[tokio::test]
async fn recycle_discards_connection_leaking_open_transaction() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) = TempArtifactGuard::open_database(1, TEST_POOL_TIMEOUT).await?;

    let leaked_checkout = database.pool.get().await?;

    leaked_checkout.execute("BEGIN IMMEDIATE", ()).await?;

    leaked_checkout
        .execute(
            INSERT_PROVIDER,
            provider_params("leaked_provider", "leaked/"),
        )
        .await?;

    // Returned to the pool with the transaction and its uncommitted write
    // still open.
    drop(leaked_checkout);

    let visible_rows = query_single_integer(&database, "SELECT COUNT(*) FROM providers").await?;

    assert_eq!(visible_rows, 0);

    // The replacement connection keeps the pool operational.
    insert_provider(&database, "replacement_provider", "replacement/").await?;

    Ok(())
}

/// Verifies that concurrent tasks share the pool safely: writers spread over
/// the pooled connections each persist exactly their own row while
/// concurrent readers always observe complete, coherent snapshots.
///
/// # Panics
///
/// Panics if a task fails or the final row count does not match the writers.
#[tokio::test]
async fn concurrent_mixed_workload_across_pooled_connections() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) =
        TempArtifactGuard::open_database(TEST_POOL_SIZE, TEST_POOL_TIMEOUT).await?;

    let mut join_handles = Vec::new();

    for writer_index in 0..8 {
        let database = database.clone();

        join_handles.push(tokio::spawn(async move {
            database
                .execute(
                    INSERT_PROVIDER,
                    provider_params(
                        &format!("concurrent_provider_{writer_index}"),
                        "concurrent/",
                    ),
                )
                .await?;

            Ok::<(), Box<dyn StdError + Send + Sync>>(())
        }));
    }

    for _ in 0..8 {
        let database = database.clone();

        join_handles.push(tokio::spawn(async move {
            let rows = database
                .query(
                    "SELECT name FROM providers WHERE model_prefix = ?1 ORDER BY name",
                    params!["concurrent/"],
                )
                .await?;

            let names: Vec<String> = rows
                .iter()
                .map(|row| row.get::<String>(0))
                .collect::<Result<Vec<_>, _>>()?;

            // A reader may observe any committed prefix of the writers, but
            // never a torn or oversized snapshot.
            assert!(names.len() <= 8);

            let is_sorted = names
                .windows(2)
                .all(|pair| matches!(pair, [first, second] if first < second));

            assert!(is_sorted);

            assert!(
                names
                    .iter()
                    .all(|name| name.starts_with("concurrent_provider_"))
            );

            Ok::<(), Box<dyn StdError + Send + Sync>>(())
        }));
    }

    for handle in join_handles {
        handle.await?.map_err(|err| err as Box<dyn StdError>)?;
    }

    let written_rows = query_single_integer(
        &database,
        "SELECT COUNT(*) FROM providers WHERE model_prefix = 'concurrent/'",
    )
    .await?;

    assert_eq!(written_rows, 8);

    Ok(())
}

/// Verifies that identical queries issued concurrently through a
/// single-connection pool each observe the complete result set, proving
/// statements on a checked out connection never interleave.
///
/// # Panics
///
/// Panics if a task fails or its result set does not match.
#[tokio::test]
async fn concurrent_identical_queries_single_connection() -> Result<(), Box<dyn StdError>> {
    let (database, _path_guard) = TempArtifactGuard::open_database(1, TEST_POOL_TIMEOUT).await?;

    insert_provider(&database, "shared_provider_a", "shared/").await?;

    insert_provider(&database, "shared_provider_b", "shared/").await?;

    let mut join_handles = Vec::new();

    for _ in 0..8 {
        let database = database.clone();

        join_handles.push(tokio::spawn(async move {
            let rows = database
                .query(
                    "SELECT name FROM providers WHERE model_prefix = ?1 ORDER BY name",
                    params!["shared/"],
                )
                .await?;

            let names: Vec<String> = rows
                .iter()
                .map(|row| row.get::<String>(0))
                .collect::<Result<Vec<_>, _>>()?;

            assert_eq!(names, ["shared_provider_a", "shared_provider_b"]);

            Ok::<(), Box<dyn StdError + Send + Sync>>(())
        }));
    }

    for handle in join_handles {
        handle.await?.map_err(|err| err as Box<dyn StdError>)?;
    }

    Ok(())
}
