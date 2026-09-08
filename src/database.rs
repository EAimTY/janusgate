//! In-process SQLite database engine and connection-pooled abstraction backed
//! by Turso.

use std::{path::Path, ptr::NonNull, sync::Arc, time::Duration};
#[allow(unused_imports)]
pub use turso::{IntoParams, Params, Row, Rows, Value, named_params, params};

/// Non-zero 32-bit big-endian schema fingerprint computed at compile time.
const DATABASE_SCHEMA_FINGERPRINT: i32 = i32::from_be_bytes(*include_bytes!(concat!(
    env!("OUT_DIR"),
    "/database_schema_fingerprint"
)));

/// Canonical SQLite DDL schema embedded at compile time.
const DATABASE_SCHEMA: &str = include_str!("../DATABASE.sqlite");

/// Result type alias specialized for database operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Database operation, connection pool, and storage engine errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database path is not valid UTF-8.
    #[error("database path is not valid UTF-8: {0}")]
    InvalidPath(String),

    /// Underlying filesystem or I/O failure.
    #[error("database I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Database connection pool is closed.
    #[error("database connection pool is closed")]
    PoolClosed,

    /// Connection pool acquisition or creation timed out.
    #[error("database connection pool acquisition timed out")]
    PoolTimeout,

    /// Connection pool construction failed.
    #[error("failed to build database connection pool: {0}")]
    PoolBuild(String),

    /// SQLite constraint violation (e.g. unique, check, foreign key, or not
    /// null).
    #[error("database constraint violation: {0}")]
    ConstraintViolation(String),

    /// Database is busy or locked by another connection.
    #[error("database is busy or locked: {0}")]
    Busy(String),

    /// Database file is corrupt or malformed.
    #[error("database file is corrupt: {0}")]
    Corrupt(String),

    /// Attempted a write operation on a read-only database.
    #[error("database is read-only: {0}")]
    Readonly(String),

    /// Database disk storage is full.
    #[error("database disk storage is full: {0}")]
    DatabaseFull(String),

    /// Query returned no rows when at least one was expected.
    #[error("database query returned no rows")]
    QueryReturnedNoRows,

    /// Value type conversion between database and Rust type failed.
    #[error("database value conversion failed: {0}")]
    ConversionFailed(String),

    /// Database client or library misuse.
    #[error("database misuse: {0}")]
    Misuse(String),

    /// SQL statement execution or preparation error.
    #[error("database execution error: {0}")]
    ExecutionFailed(String),
}

impl From<turso::Error> for Error {
    fn from(error: turso::Error) -> Self {
        match error {
            turso::Error::Constraint(message) => Self::ConstraintViolation(message),
            turso::Error::Busy(message) | turso::Error::BusySnapshot(message) => {
                Self::Busy(message)
            }
            turso::Error::Corrupt(message) | turso::Error::NotAdb(message) => {
                Self::Corrupt(message)
            }
            turso::Error::Readonly(message) => Self::Readonly(message),
            turso::Error::DatabaseFull(message) => Self::DatabaseFull(message),
            turso::Error::QueryReturnedNoRows => Self::QueryReturnedNoRows,
            turso::Error::ConversionFailure(message) => Self::ConversionFailed(message),
            turso::Error::ToSqlConversionFailure(error) => {
                Self::ConversionFailed(error.to_string())
            }
            turso::Error::Misuse(message) => Self::Misuse(message),
            turso::Error::IoError(kind, message) => Self::Io(std::io::Error::new(kind, message)),
            turso::Error::Error(message) => Self::ExecutionFailed(message),
            other => Self::ExecutionFailed(other.to_string()),
        }
    }
}

impl From<deadpool::managed::PoolError<Error>> for Error {
    fn from(error: deadpool::managed::PoolError<Error>) -> Self {
        match error {
            deadpool::managed::PoolError::Backend(backend_error) => backend_error,
            deadpool::managed::PoolError::Closed => Self::PoolClosed,
            deadpool::managed::PoolError::Timeout(_) => Self::PoolTimeout,
            other => Self::PoolBuild(other.to_string()),
        }
    }
}

impl From<deadpool::managed::BuildError> for Error {
    fn from(error: deadpool::managed::BuildError) -> Self {
        Self::PoolBuild(error.to_string())
    }
}

/// In-process SQLite-compatible database client managing a pool of connections.
#[derive(Clone, Debug)]
pub struct Database {
    /// Internal shared state holding the connection pool.
    inner: Arc<Inner>,
}

/// Internal database state containing the connection pool.
#[derive(Debug)]
struct Inner {
    /// Connection pool managing reusable Turso connections.
    pool: deadpool::managed::Pool<Manager>,
}

/// Deadpool connection manager for Turso database connections.
#[derive(Debug)]
struct Manager {
    /// In-process Turso database handle.
    database: turso::Database,
}

impl deadpool::managed::Manager for Manager {
    type Error = Error;
    type Type = turso::Connection;

    async fn create(&self) -> std::result::Result<Self::Type, Self::Error> {
        let connection = self.database.connect()?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update("foreign_keys", "ON").await?;
        Ok(connection)
    }

    async fn recycle(
        &self,
        connection: &mut Self::Type,
        _: &deadpool::managed::Metrics,
    ) -> deadpool::managed::RecycleResult<Self::Error> {
        if !connection.is_autocommit().map_err(Error::from)? {
            let mut statement = connection
                .prepare_cached("ROLLBACK")
                .await
                .map_err(Error::from)?;
            statement.execute(()).await.map_err(Error::from)?;
        }
        Ok(())
    }
}

/// Database transaction handle holding exclusive ownership of a pooled
/// connection.
#[derive(Debug)]
pub struct Transaction<'db> {
    /// Internal transaction state and connection ownership adapter.
    inner: TransactionInner<'db>,
}

/// Internal representation of an active database transaction.
#[derive(Debug)]
struct TransactionInner<'db> {
    /// Active Turso transaction handle.
    transaction: Option<turso::transaction::Transaction<'db>>,

    /// Pointer to the heap-allocated connection checkout.
    connection: NonNull<deadpool::managed::Object<Manager>>,
}

// SAFETY:
// `TransactionInner` exclusively owns the pooled connection object via
// `self.connection`. Both `turso::Transaction` and
// `deadpool::managed::Object<Manager>` are `Send`, allowing the transaction
// handle to be safely transferred across threads.
unsafe impl Send for TransactionInner<'_> {}

// SAFETY:
// Shared references to `TransactionInner` only expose read-only access to
// `turso::Transaction`, which implements `Sync`. The raw connection pointer is
// never dereferenced concurrently and is only reclaimed during `Drop` with
// `&mut self`.
unsafe impl Sync for TransactionInner<'_> {}

impl Drop for TransactionInner<'_> {
    fn drop(&mut self) {
        // Drop the underlying Turso transaction first to ensure any uncommitted
        // state executes its rollback drop semantics before the connection is
        // reclaimed.
        drop(self.transaction.take());

        // SAFETY:
        // 1. `self.connection` was created with `Box::leak` during transaction
        //    creation.
        // 2. The memory address remained stable throughout the transaction
        //    lifecycle.
        // 3. The underlying Turso transaction has been dropped prior to this
        //    operation.
        // 4. `self.connection` is reclaimed exactly once here in `Drop`.
        let _reclaimed = unsafe { Box::from_raw(self.connection.as_ptr()) };
    }
}

/// Prepares and executes a cached SQL statement against the given connection.
///
/// # Errors
///
/// Returns [`Error`] if preparing the statement or executing the query fails.
async fn execute(
    connection: &turso::Connection,
    sql: impl AsRef<str>,
    params: impl IntoParams,
) -> Result<u64> {
    let mut statement = connection.prepare_cached(sql).await?;
    let changes = statement.execute(params).await?;
    Ok(changes)
}

/// Prepares and queries a cached SQL statement against the given connection.
///
/// # Errors
///
/// Returns [`Error`] if preparing the statement or executing the query fails.
async fn query(
    connection: &turso::Connection,
    sql: impl AsRef<str>,
    params: impl IntoParams,
) -> Result<Rows> {
    let mut statement = connection.prepare_cached(sql).await?;
    let rows = statement.query(params).await?;
    Ok(rows)
}

/// Verifies that the database schema matches [`DATABASE_SCHEMA_FINGERPRINT`],
/// applying the canonical schema if the database is newly created.
///
/// # Panics
///
/// Panics if connecting fails, setting pragmas fails, querying the fingerprint
/// fails, applying the schema fails, or if an incompatible schema fingerprint
/// is detected.
async fn bootstrap_schema(database: &turso::Database) {
    #[expect(
        clippy::expect_used,
        reason = "opening initial connection to verify schema is fatal on failure"
    )]
    let connection = database
        .connect()
        .expect("failed to connect to database for schema verification");

    #[expect(
        clippy::expect_used,
        reason = "configuring busy timeout on database connection is fatal on failure"
    )]
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("failed to set busy timeout");

    #[expect(
        clippy::expect_used,
        reason = "enabling foreign key constraints is fatal on failure"
    )]
    connection
        .pragma_update("foreign_keys", "ON")
        .await
        .expect("failed to enable foreign keys");

    match query_schema_fingerprint(&connection).await {
        DATABASE_SCHEMA_FINGERPRINT => {}

        0 => apply_canonical_schema(&connection).await,

        other => {
            #[expect(
                clippy::panic,
                reason = "incompatible database schema fingerprint is fatal"
            )]
            {
                panic!(
                    "database schema fingerprint mismatch: expected \
                     {DATABASE_SCHEMA_FINGERPRINT}, got {other}"
                );
            }
        }
    }
}

/// Retrieves the current schema fingerprint stored in `PRAGMA user_version`.
///
/// # Panics
///
/// Panics if executing `PRAGMA user_version` or extracting its value fails.
async fn query_schema_fingerprint(connection: &turso::Connection) -> i32 {
    #[expect(
        clippy::expect_used,
        reason = "querying PRAGMA user_version is fatal on failure"
    )]
    let mut rows = connection
        .query("PRAGMA user_version", ())
        .await
        .expect("failed to query user_version");

    #[expect(
        clippy::expect_used,
        reason = "PRAGMA user_version query always produces a row"
    )]
    let row = rows
        .next()
        .await
        .expect("failed to read user_version row")
        .expect("missing user_version row");

    #[expect(
        clippy::expect_used,
        reason = "user_version column is an integer value"
    )]
    let user_version: i64 = row
        .get(0)
        .expect("failed to get user_version integer value");

    #[expect(
        clippy::cast_possible_truncation,
        reason = "SQLite user_version is a 32-bit signed integer"
    )]
    let user_version = user_version as i32;

    user_version
}

/// Applies the canonical SQLite DDL schema in batch and records the
/// compile-time fingerprint in `PRAGMA user_version`.
///
/// # Panics
///
/// Panics if executing the schema DDL batch or updating `PRAGMA user_version`
/// fails.
async fn apply_canonical_schema(connection: &turso::Connection) {
    #[expect(
        clippy::expect_used,
        reason = "applying canonical schema DDL is fatal on failure"
    )]
    connection
        .execute_batch(DATABASE_SCHEMA)
        .await
        .expect("failed to apply database schema");

    let set_user_version_sql = format!("PRAGMA user_version = {DATABASE_SCHEMA_FINGERPRINT}");

    #[expect(
        clippy::expect_used,
        reason = "recording schema fingerprint is fatal on failure"
    )]
    connection
        .execute_batch(&set_user_version_sql)
        .await
        .expect("failed to store database schema fingerprint");
}

impl Database {
    /// Opens the in-process SQLite database at the specified path and
    /// initializes a connection pool with at most `max_connections`
    /// concurrent connections.
    ///
    /// Verifies the schema fingerprint stored in `PRAGMA user_version`. If the
    /// database is fresh (`user_version == 0`), applies the canonical schema
    /// and stores the compile-time schema fingerprint. If the fingerprint
    /// matches [`DATABASE_SCHEMA_FINGERPRINT`], initialization succeeds.
    /// Otherwise, panics.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if opening the database or creating the connection
    /// pool fails.
    ///
    /// # Panics
    ///
    /// Panics if database verification connection fails, schema application
    /// fails, or the schema fingerprint does not match
    /// [`DATABASE_SCHEMA_FINGERPRINT`].
    pub async fn open(path: impl AsRef<Path>, max_connections: usize) -> Result<Self> {
        let path = path.as_ref();

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| Error::InvalidPath(path.to_string_lossy().into_owned()))?;

        let database = turso::Builder::new_local(path_str).build().await?;

        bootstrap_schema(&database).await;

        let manager = Manager { database };

        let pool = deadpool::managed::Pool::builder(manager)
            .max_size(max_connections)
            .build()?;

        Ok(Self {
            inner: Arc::new(Inner { pool }),
        })
    }

    /// Executes a SQL statement with parameters using a pooled connection and
    /// cached prepared statement.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection, preparing the statement, or
    /// execution fails.
    pub async fn execute(&self, sql: impl AsRef<str>, params: impl IntoParams) -> Result<u64> {
        let connection = self.inner.pool.get().await?;
        execute(&connection, sql, params).await
    }

    /// Queries the database with parameters using a pooled connection and
    /// cached prepared statement.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection, preparing the statement, or
    /// executing the query fails.
    pub async fn query(&self, sql: impl AsRef<str>, params: impl IntoParams) -> Result<Rows> {
        let connection = self.inner.pool.get().await?;
        query(&connection, sql, params).await
    }

    /// Begins a new transaction, checking out a connection from the pool and
    /// retaining exclusive ownership of it for the lifetime of the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection or starting the transaction
    /// fails.
    pub async fn transaction<'db>(&'db self) -> Result<Transaction<'db>> {
        let connection = self.inner.pool.get().await?;

        let connection_box = Box::new(connection);
        let connection_mut: &'db mut deadpool::managed::Object<Manager> = Box::leak(connection_box);
        let non_null_connection = NonNull::from(&mut *connection_mut);

        let transaction = match connection_mut.transaction().await {
            Ok(txn) => txn,
            Err(err) => {
                // SAFETY:
                // Transaction creation failed, so the Turso transaction was not
                // created. We reclaim the leaked Box and return
                // the connection to the pool immediately.
                let _reclaimed = unsafe { Box::from_raw(non_null_connection.as_ptr()) };
                return Err(err.into());
            }
        };

        Ok(Transaction {
            inner: TransactionInner {
                transaction: Some(transaction),
                connection: non_null_connection,
            },
        })
    }
}

impl Transaction<'_> {
    /// Executes a SQL statement within this transaction using a cached prepared
    /// statement.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the underlying transaction is no longer active, which violates
    /// an internal invariant.
    pub async fn execute(&self, sql: impl AsRef<str>, params: impl IntoParams) -> Result<u64> {
        #[expect(
            clippy::expect_used,
            reason = "transaction is active while Transaction handle is held"
        )]
        let transaction = self
            .inner
            .transaction
            .as_ref()
            .expect("transaction is active while Transaction handle is held");

        execute(transaction, sql, params).await
    }

    /// Queries the database within this transaction using a cached prepared
    /// statement.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the query fails.
    ///
    /// # Panics
    ///
    /// Panics if the underlying transaction is no longer active, which violates
    /// an internal invariant.
    pub async fn query(&self, sql: impl AsRef<str>, params: impl IntoParams) -> Result<Rows> {
        #[expect(
            clippy::expect_used,
            reason = "transaction is active while Transaction handle is held"
        )]
        let transaction = self
            .inner
            .transaction
            .as_ref()
            .expect("transaction is active while Transaction handle is held");

        query(transaction, sql, params).await
    }

    /// Commits the transaction, applying all changes and returning the pooled
    /// connection to the connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if committing the transaction fails.
    pub async fn commit(mut self) -> Result<()> {
        if let Some(transaction) = self.inner.transaction.take() {
            transaction.commit().await?;
        }
        Ok(())
    }

    /// Rolls back the transaction, discarding all changes and returning the
    /// pooled connection to the connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if rolling back the transaction fails.
    pub async fn rollback(mut self) -> Result<()> {
        if let Some(transaction) = self.inner.transaction.take() {
            transaction.rollback().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        error::Error as StdError,
        result::Result as StdResult,
        sync::atomic::{AtomicU64, Ordering},
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Creates a unique isolated database instance for test execution.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database fails.
    async fn create_isolated_test_database() -> Database {
        let unique_id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "janusgate_isolated_test_{}_{unique_id}.db",
            std::process::id()
        ));
        #[expect(
            clippy::expect_used,
            reason = "test database creation is fatal on failure"
        )]
        Database::open(&path, 4)
            .await
            .expect("failed to open isolated test database")
    }

    /// Verifies that initializing the database creates the schema and stores
    /// the fingerprint.
    ///
    /// # Panics
    ///
    /// Panics if database initialization or querying fails.
    #[tokio::test]
    async fn database_initialization() -> StdResult<(), Box<dyn StdError>> {
        let unique_id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "janusgate_isolated_test_{}_{unique_id}.db",
            std::process::id()
        ));
        let database = Database::open(&path, 4).await?;

        let mut rows = database.query("PRAGMA user_version", ()).await?;
        let row = rows.next().await?.ok_or("missing user_version row")?;
        let version: i64 = row.get(0)?;
        assert_eq!(version, DATABASE_SCHEMA_FINGERPRINT as i64);

        // Verify reopening the database succeeds and reports the same
        // fingerprint.
        let reopened_database = Database::open(&path, 4).await?;
        let mut rows = reopened_database.query("PRAGMA user_version", ()).await?;
        let row = rows.next().await?.ok_or("missing user_version row")?;
        let version: i64 = row.get(0)?;
        assert_eq!(version, DATABASE_SCHEMA_FINGERPRINT as i64);

        let _ = std::fs::remove_file(path);
        Ok(())
    }

    /// Verifies that `execute` and `query` work with prepared statements and
    /// parameter binding.
    ///
    /// # Panics
    ///
    /// Panics if query assertions fail.
    #[tokio::test]
    async fn database_execute_and_query() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_exec_provider",
                    "test/",
                    "openai_compatible",
                    "{\"apiKey\":\"test\"}"
                ],
            )
            .await?;

        let mut rows = database
            .query(
                "SELECT name, model_prefix, type FROM providers WHERE name = ?1",
                params!["test_exec_provider"],
            )
            .await?;

        let row = rows.next().await?.ok_or("expected provider row")?;
        let name: String = row.get(0)?;
        let prefix: String = row.get(1)?;
        let provider_type: String = row.get(2)?;

        assert_eq!(name, "test_exec_provider");
        assert_eq!(prefix, "test/");
        assert_eq!(provider_type, "openai_compatible");

        Ok(())
    }

    /// Verifies that transactions successfully commit mutations.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_commit() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        let transaction = database.transaction().await?;
        transaction
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_tx_commit",
                    "tx/",
                    "openai_compatible",
                    "{\"apiKey\":\"tx\"}"
                ],
            )
            .await?;
        transaction.commit().await?;

        let mut rows = database
            .query(
                "SELECT name FROM providers WHERE name = ?1",
                params!["test_tx_commit"],
            )
            .await?;
        assert!(rows.next().await?.is_some());

        Ok(())
    }

    /// Verifies that transactions roll back mutations when explicitly rolled
    /// back.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_rollback() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        let transaction = database.transaction().await?;
        transaction
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_tx_rollback",
                    "tx/",
                    "openai_compatible",
                    "{\"apiKey\":\"tx\"}"
                ],
            )
            .await?;
        transaction.rollback().await?;

        let mut rows = database
            .query(
                "SELECT name FROM providers WHERE name = ?1",
                params!["test_tx_rollback"],
            )
            .await?;
        assert!(rows.next().await?.is_none());

        Ok(())
    }

    /// Verifies that transactions automatically roll back if dropped without
    /// committing.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_drop_rolls_back() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        {
            let transaction = database.transaction().await?;
            transaction
                .execute(
                    "INSERT INTO providers (name, model_prefix, type, credentials) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        "test_tx_drop",
                        "tx/",
                        "openai_compatible",
                        "{\"apiKey\":\"tx\"}"
                    ],
                )
                .await?;
            // Transaction dropped here without calling commit()
        }

        let mut rows = database
            .query(
                "SELECT name FROM providers WHERE name = ?1",
                params!["test_tx_drop"],
            )
            .await?;
        assert!(rows.next().await?.is_none());

        Ok(())
    }

    /// Verifies that multiple statements in a transaction execute and commit
    /// atomically.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_multiple_statements() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        let transaction = database.transaction().await?;
        transaction
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_multi_1",
                    "m1/",
                    "openai_compatible",
                    "{\"apiKey\":\"m1\"}"
                ],
            )
            .await?;
        transaction
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_multi_2",
                    "m2/",
                    "openai_compatible",
                    "{\"apiKey\":\"m2\"}"
                ],
            )
            .await?;
        transaction.commit().await?;

        let mut rows = database
            .query(
                "SELECT COUNT(*) FROM providers WHERE name IN (?1, ?2)",
                params!["test_multi_1", "test_multi_2"],
            )
            .await?;
        let row = rows.next().await?.ok_or("missing count row")?;
        let count: i64 = row.get(0)?;
        assert_eq!(count, 2);

        Ok(())
    }

    /// Verifies that query execution works with named parameters.
    ///
    /// # Panics
    ///
    /// Panics if named parameter query execution or assertions fail.
    #[tokio::test]
    async fn database_query_named_params() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_named_params",
                    "np/",
                    "openai_compatible",
                    "{\"apiKey\":\"np\"}"
                ],
            )
            .await?;

        let mut rows = database
            .query(
                "SELECT model_prefix FROM providers WHERE name = :name",
                named_params! { ":name": "test_named_params" },
            )
            .await?;
        let row = rows.next().await?.ok_or("expected provider row")?;
        let prefix: String = row.get(0)?;
        assert_eq!(prefix, "np/");

        Ok(())
    }

    /// Verifies concurrent executions and queries across pooled connections.
    ///
    /// # Panics
    ///
    /// Panics if task joins fail or query results do not match.
    #[tokio::test]
    async fn concurrent_pooled_access() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        let mut join_handles = Vec::new();
        for i in 0..10 {
            let db = database.clone();
            join_handles.push(tokio::spawn(async move {
                let mut rows = db.query("SELECT 1 + ?1", params![i]).await?;
                let row = rows
                    .next()
                    .await?
                    .ok_or_else(|| Error::Misuse("missing result row".to_owned()))?;
                let val: i64 = row.get(0)?;
                assert_eq!(val, 1 + i);
                Ok::<(), Error>(())
            }));
        }

        for handle in join_handles {
            handle.await??;
        }

        Ok(())
    }

    /// Verifies that constraint violations produce
    /// `Error::ConstraintViolation`.
    ///
    /// # Panics
    ///
    /// Panics if duplicate insertion unexpectedly succeeds.
    #[tokio::test]
    async fn error_constraint_violation() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "unique_provider",
                    "u/",
                    "openai_compatible",
                    "{\"apiKey\":\"u\"}"
                ],
            )
            .await?;

        let duplicate_result = database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "unique_provider",
                    "u/",
                    "openai_compatible",
                    "{\"apiKey\":\"u\"}"
                ],
            )
            .await;

        assert!(matches!(
            duplicate_result,
            Err(Error::ConstraintViolation(_))
        ));
        Ok(())
    }

    /// Verifies that invalid SQL statements produce `Error::ExecutionFailed`.
    ///
    /// # Panics
    ///
    /// Panics if syntax error query unexpectedly succeeds.
    #[tokio::test]
    async fn error_execution_failed() -> StdResult<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        let invalid_result = database.execute("SYNTAX ERROR IN SQL", ()).await;

        assert!(matches!(invalid_result, Err(Error::ExecutionFailed(_))));
        Ok(())
    }
}
