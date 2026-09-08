//! In-process SQLite database engine and connection-pooled abstraction backed
//! by Turso.

use deadpool::managed::{
    BuildError, Manager as PoolManager, Metrics, Object, Pool, PoolError, RecycleError,
    RecycleResult,
};
use std::{fs, io::Error as IoError, path::Path, ptr::NonNull, sync::Arc, time::Duration};
use turso::{
    Builder as TursoBuilder, Connection, Database as TursoDatabase, Error as TursoError,
    transaction::Transaction as TursoTransaction,
};

// The re-exported items double as the module's own imports: `Row` and
// `IntoParams` appear in this module's signatures.
#[rustfmt::skip]
#[allow(unused_imports)]
pub use turso::{named_params, params, IntoParams, Params, Row, Value};

/// Non-zero 32-bit big-endian schema fingerprint computed at compile time.
const DATABASE_SCHEMA_FINGERPRINT: i32 = i32::from_be_bytes(*include_bytes!(concat!(
    env!("OUT_DIR"),
    "/database_schema_fingerprint"
)));

/// Canonical SQLite DDL schema embedded at compile time.
const DATABASE_SCHEMA: &str = include_str!("../DATABASE.sqlite");

// `Transaction` handles are held across `.await` points in spawned gateway
// tasks, so `Send + Sync` is an API contract. This pins the auto traits: the
// manual `unsafe impl`s on `TransactionInner` are load-bearing because
// `NonNull` is unconditionally `!Send` and `!Sync`, and any refactor that
// breaks thread-safety must fail here, not at distant call sites.
const _: () = {
    const fn assert_send_sync<T>()
    where
        T: Send + Sync,
    {
    }

    assert_send_sync::<Transaction<'static>>();
};

/// Database operation, connection pool, and storage engine errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database path is not valid UTF-8.
    #[error("database path is not valid UTF-8: {0}")]
    InvalidPath(String),

    /// Underlying filesystem or I/O failure.
    #[error("database I/O error: {0}")]
    Io(#[from] IoError),

    /// Turso database engine error.
    #[error("database error: {0}")]
    Turso(#[from] TursoError),

    /// Database connection pool is closed.
    #[error("database connection pool is closed")]
    PoolClosed,

    /// Connection pool acquisition timed out.
    #[error("database connection pool acquisition timed out")]
    PoolTimeout,
}

/// In-process SQLite-compatible database client managing a pool of connections.
#[derive(Clone, Debug)]
pub struct Database {
    /// Internal shared state holding the connection pool.
    inner: Arc<Inner>,
}

/// Database transaction handle holding exclusive ownership of a pooled
/// connection.
#[derive(Debug)]
pub struct Transaction<'db> {
    /// Internal transaction state and connection ownership adapter.
    inner: TransactionInner<'db>,
}

/// Internal database state containing the connection pool.
#[derive(Debug)]
struct Inner {
    /// Connection pool managing reusable Turso connections.
    pool: Pool<Manager>,
}

/// Deadpool connection manager for Turso database connections.
#[derive(Debug)]
struct Manager {
    /// In-process Turso database handle.
    database: TursoDatabase,
}

/// Internal representation of an active database transaction.
#[derive(Debug)]
struct TransactionInner<'db> {
    /// Active Turso transaction handle.
    transaction: Option<TursoTransaction<'db>>,

    /// Pointer to the heap-allocated connection checkout.
    connection: NonNull<Object<Manager>>,
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
    pub async fn open(path: impl AsRef<Path>, max_connections: usize) -> Result<Self, Error> {
        let path = path.as_ref();

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| Error::InvalidPath(path.to_string_lossy().into_owned()))?;

        let database = TursoBuilder::new_local(path_str).build().await?;

        bootstrap_schema(&database).await;

        let manager = Manager { database };

        let pool = match Pool::builder(manager).max_size(max_connections).build() {
            Ok(pool) => pool,
            Err(BuildError::NoRuntimeSpecified) => {
                // SAFETY: Deadpool runtime is not configured or required for
                // local connection pooling.
                unreachable!("deadpool runtime is not configured or required");
            }
        };

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
    pub async fn execute(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<u64, Error> {
        let connection = self.inner.pool.get().await?;

        execute(&connection, sql, params).await
    }

    /// Queries the database with parameters using a pooled connection and a
    /// cached prepared statement, returning all rows eagerly materialized.
    ///
    /// The statement always executes to completion before the pooled
    /// connection is released, so a result set can never outlive its
    /// connection checkout or interleave with another borrower's statements.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection, preparing the statement, or
    /// executing the query fails.
    pub async fn query(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, Error> {
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
    pub async fn transaction<'db>(&'db self) -> Result<Transaction<'db>, Error> {
        let connection = self.inner.pool.get().await?;

        let connection_box = Box::new(connection);
        let connection_mut: &'db mut Object<Manager> = Box::leak(connection_box);
        let non_null_connection = NonNull::from(&mut *connection_mut);

        let transaction = match connection_mut.transaction().await {
            Ok(txn) => txn,
            Err(err) => {
                // SAFETY:
                // Transaction creation failed, so the Turso transaction was not
                // created. We reclaim the leaked Box and return the connection
                // to the pool immediately.
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
    pub async fn execute(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<u64, Error> {
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
    /// statement, returning all rows eagerly materialized. The statement
    /// always executes to completion, leaving no unfinished statement state on
    /// the connection when the transaction commits or rolls back.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the query fails.
    ///
    /// # Panics
    ///
    /// Panics if the underlying transaction is no longer active, which violates
    /// an internal invariant.
    pub async fn query(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, Error> {
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
    ///
    /// # Panics
    ///
    /// Panics if the underlying transaction is no longer active, which violates
    /// an internal invariant.
    pub async fn commit(mut self) -> Result<(), Error> {
        #[expect(
            clippy::expect_used,
            reason = "transaction is active until commit consumes the handle"
        )]
        let transaction = self
            .inner
            .transaction
            .take()
            .expect("transaction is active until commit consumes the handle");

        transaction.commit().await?;

        Ok(())
    }

    /// Rolls back the transaction, discarding all changes and returning the
    /// pooled connection to the connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if rolling back the transaction fails.
    ///
    /// # Panics
    ///
    /// Panics if the underlying transaction is no longer active, which violates
    /// an internal invariant.
    pub async fn rollback(mut self) -> Result<(), Error> {
        #[expect(
            clippy::expect_used,
            reason = "transaction is active until rollback consumes the handle"
        )]
        let transaction = self
            .inner
            .transaction
            .take()
            .expect("transaction is active until rollback consumes the handle");

        transaction.rollback().await?;

        Ok(())
    }
}

impl PoolManager for Manager {
    type Error = TursoError;
    type Type = Connection;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        let connection = self.database.connect()?;

        configure_connection(&connection).await?;

        Ok(connection)
    }

    async fn recycle(
        &self,
        connection: &mut Self::Type,
        _: &Metrics,
    ) -> RecycleResult<Self::Error> {
        // Drain a trivial query so turso resolves any dangling transaction
        // marker through its own cleanup path (`Connection::query` first rolls
        // back an abandoned transaction and then clears the marker). A bare
        // statement-level ROLLBACK leaves the marker set and would permanently
        // poison the connection for future transactions.
        let mut rows = connection.query("SELECT 1", ()).await?;
        while rows.next().await?.is_some() {}

        // A connection that still reports an open transaction is in an unknown
        // state; failing here makes deadpool discard and replace it.
        if !connection.is_autocommit()? {
            return Err(RecycleError::Backend(TursoError::Misuse(
                "recycled connection still has an open transaction".to_string(),
            )));
        }

        Ok(())
    }
}

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
    connection: &Connection,
    sql: impl AsRef<str>,
    params: impl IntoParams,
) -> Result<u64, Error> {
    let mut statement = connection.prepare_cached(sql).await?;
    let changes = statement.execute(params).await?;

    Ok(changes)
}

/// Prepares a cached SQL statement against the given connection and drains the
/// result set to completion into an owned vector of rows.
///
/// Draining matters: an unfinished statement left on the connection would
/// corrupt or roll back whatever the next borrower of the pooled connection
/// executes.
///
/// # Errors
///
/// Returns [`Error`] if preparing the statement or executing the query fails.
async fn query(
    connection: &Connection,
    sql: impl AsRef<str>,
    params: impl IntoParams,
) -> Result<Vec<Row>, Error> {
    let mut statement = connection.prepare_cached(sql).await?;
    let mut rows = statement.query(params).await?;

    let mut result = Vec::new();

    while let Some(row) = rows.next().await? {
        result.push(row);
    }

    Ok(result)
}

/// Applies the pragmas and timeouts required by every JanusGate database
/// connection, whether pooled or used for schema bootstrap.
///
/// # Errors
///
/// Returns [`TursoError`] if configuring the busy timeout or the foreign key
/// pragma fails.
async fn configure_connection(connection: &Connection) -> Result<(), TursoError> {
    connection.busy_timeout(Duration::from_secs(5))?;

    connection.pragma_update("foreign_keys", "ON").await?;

    Ok(())
}

/// Verifies that the database schema matches [`DATABASE_SCHEMA_FINGERPRINT`],
/// applying the canonical schema if the database is newly created.
///
/// # Panics
///
/// Panics if connecting fails, setting pragmas fails, querying the fingerprint
/// fails, applying the schema fails, or if an incompatible schema fingerprint
/// is detected.
async fn bootstrap_schema(database: &TursoDatabase) {
    #[expect(
        clippy::expect_used,
        reason = "opening initial connection to verify schema is fatal on failure"
    )]
    let connection = database
        .connect()
        .expect("failed to connect to database for schema verification");

    #[expect(
        clippy::expect_used,
        reason = "configuring the schema verification connection is fatal on failure"
    )]
    configure_connection(&connection)
        .await
        .expect("failed to configure database connection");

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
async fn query_schema_fingerprint(connection: &Connection) -> i32 {
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
/// compile-time fingerprint in `PRAGMA user_version` inside a single
/// transaction.
///
/// The DDL and the fingerprint write must commit atomically: a crash that
/// persisted only part of the schema would leave `user_version` at zero, and
/// the next startup would reapply the DDL and fail on the existing tables.
///
/// # Panics
///
/// Panics if executing the schema batch fails.
async fn apply_canonical_schema(connection: &Connection) {
    // Header writes such as `PRAGMA user_version` participate in the
    // transaction, so wrapping the DDL in BEGIN/COMMIT makes the whole batch
    // all-or-nothing.
    let batch_sql = format!(
        "BEGIN IMMEDIATE;\n{}PRAGMA user_version = {DATABASE_SCHEMA_FINGERPRINT};\nCOMMIT;",
        DATABASE_SCHEMA
    );

    #[expect(
        clippy::expect_used,
        reason = "applying the canonical schema batch is fatal on failure"
    )]
    connection
        .execute_batch(batch_sql)
        .await
        .expect("failed to apply database schema");
}

// SAFETY:
// `TransactionInner` exclusively owns the pooled connection object via
// `self.connection`. Both `TursoTransaction` and `Object<Manager>` are `Send`,
// allowing the transaction handle to be safely transferred across threads.
unsafe impl Send for TransactionInner<'_> {}

// SAFETY:
// Shared references to `TransactionInner` only expose read-only access to
// `TursoTransaction`, which implements `Sync`. The raw connection pointer is
// never dereferenced concurrently and is only reclaimed during `Drop` with
// `&mut self`.
unsafe impl Sync for TransactionInner<'_> {}

impl From<PoolError<TursoError>> for Error {
    fn from(error: PoolError<TursoError>) -> Self {
        match error {
            PoolError::Backend(error) => Self::Turso(error),
            PoolError::Closed => Self::PoolClosed,
            PoolError::Timeout(_) => Self::PoolTimeout,
            PoolError::NoRuntimeSpecified | PoolError::PostCreateHook(_) => {
                // SAFETY: Deadpool runtime and hooks are not configured on this
                // pool instance, making these error variants unreachable.
                unreachable!("deadpool runtime and hooks are not configured");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DATABASE_SCHEMA_FINGERPRINT, Database, Error, params};
    use std::{
        env,
        error::Error as StdError,
        fs, process,
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
        let path = env::temp_dir().join(format!(
            "janusgate_isolated_test_{}_{unique_id}.db",
            process::id()
        ));

        #[expect(
            clippy::expect_used,
            reason = "test database creation is fatal on failure"
        )]
        Database::open(&path, 4)
            .await
            .expect("failed to open isolated test database")
    }

    /// Verifies that initializing the database creates the complete schema and
    /// stores the fingerprint.
    ///
    /// # Panics
    ///
    /// Panics if database initialization or querying fails.
    #[tokio::test]
    async fn database_initialization() -> Result<(), Box<dyn StdError>> {
        let unique_id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = env::temp_dir().join(format!(
            "janusgate_isolated_test_{}_{unique_id}.db",
            process::id()
        ));
        let database = Database::open(&path, 4).await?;

        let rows = database.query("PRAGMA user_version", ()).await?;
        let row = rows.first().ok_or("missing user_version row")?;
        let version: i64 = row.get(0)?;

        assert_eq!(version, DATABASE_SCHEMA_FINGERPRINT as i64);

        // Verify the schema batch applied every schema object.
        let rows = database
            .query("SELECT name FROM sqlite_master ORDER BY name", ())
            .await?;

        let names: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String>(0))
            .collect::<Result<Vec<_>, _>>()?;

        for expected in [
            "models",
            "oauth_states",
            "providers",
            "idx_oauth_states_expires_at",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing schema object {expected}"
            );
        }

        // Verify reopening the database succeeds and reports the same
        // fingerprint.
        let reopened_database = Database::open(&path, 4).await?;
        let rows = reopened_database.query("PRAGMA user_version", ()).await?;
        let row = rows.first().ok_or("missing user_version row")?;
        let version: i64 = row.get(0)?;

        assert_eq!(version, DATABASE_SCHEMA_FINGERPRINT as i64);

        let _ = fs::remove_file(path);

        Ok(())
    }

    /// Verifies that transactions successfully commit mutations.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_commit() -> Result<(), Box<dyn StdError>> {
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

        let rows = database
            .query(
                "SELECT name FROM providers WHERE name = ?1",
                params!["test_tx_commit"],
            )
            .await?;

        assert_eq!(rows.len(), 1);

        Ok(())
    }

    /// Verifies that transactions automatically roll back if dropped without
    /// committing.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_drop_rolls_back() -> Result<(), Box<dyn StdError>> {
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

        let rows = database
            .query(
                "SELECT name FROM providers WHERE name = ?1",
                params!["test_tx_drop"],
            )
            .await?;

        assert!(rows.is_empty());

        Ok(())
    }

    /// Verifies that a connection which hosted a dropped transaction can still
    /// host new transactions after being recycled by the pool.
    ///
    /// # Panics
    ///
    /// Panics if transaction execution or query assertions fail.
    #[tokio::test]
    async fn transaction_drop_then_new_transaction() -> Result<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        {
            let transaction = database.transaction().await?;

            transaction
                .execute(
                    "INSERT INTO providers (name, model_prefix, type, credentials) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        "test_tx_poison",
                        "tx/",
                        "openai_compatible",
                        "{\"apiKey\":\"tx\"}"
                    ],
                )
                .await?;
            // Transaction dropped here, leaving the connection flagged for
            // rollback by turso's dangling-transaction cleanup.
        }

        let transaction = database.transaction().await?;

        transaction
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "test_tx_after_drop",
                    "tx/",
                    "openai_compatible",
                    "{\"apiKey\":\"tx\"}"
                ],
            )
            .await?;

        transaction.commit().await?;

        let rows = database
            .query(
                "SELECT name FROM providers WHERE name = ?1",
                params!["test_tx_after_drop"],
            )
            .await?;

        assert_eq!(rows.len(), 1);

        Ok(())
    }

    /// Verifies concurrent executions and queries across pooled connections.
    ///
    /// # Panics
    ///
    /// Panics if task joins fail or query results do not match.
    #[tokio::test]
    async fn concurrent_pooled_access() -> Result<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        let mut join_handles = Vec::new();

        for i in 0..10 {
            let db = database.clone();

            join_handles.push(tokio::spawn(async move {
                let rows = db.query("SELECT 1 + ?1", params![i]).await?;
                let row = rows.first().ok_or("missing result row")?;
                let val: i64 = row.get(0)?;

                assert_eq!(val, 1 + i);

                Ok::<(), Box<dyn StdError + Send + Sync>>(())
            }));
        }

        for handle in join_handles {
            handle.await?.map_err(|err| err as Box<dyn StdError>)?;
        }

        Ok(())
    }

    /// Verifies that identical queries issued concurrently through a
    /// single-connection pool each observe complete, uncorrupted results,
    /// proving statements never interleave on a checked-out connection.
    ///
    /// # Panics
    ///
    /// Panics if task joins fail or query results do not match.
    #[tokio::test]
    async fn concurrent_identical_queries_single_connection() -> Result<(), Box<dyn StdError>> {
        let unique_id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = env::temp_dir().join(format!(
            "janusgate_isolated_test_{}_{unique_id}.db",
            process::id()
        ));

        let database = Database::open(&path, 1).await?;

        for suffix in ["a", "b"] {
            database
                .execute(
                    "INSERT INTO providers (name, model_prefix, type, credentials) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        format!("single_conn_{suffix}"),
                        "single/",
                        "openai_compatible",
                        "{\"apiKey\":\"s\"}"
                    ],
                )
                .await?;
        }

        let mut join_handles = Vec::new();

        for _ in 0..8 {
            let db = database.clone();

            join_handles.push(tokio::spawn(async move {
                let rows = db
                    .query(
                        "SELECT name FROM providers WHERE model_prefix = ?1 ORDER BY name",
                        params!["single/"],
                    )
                    .await?;

                let names: Vec<String> = rows
                    .iter()
                    .map(|row| row.get::<String>(0))
                    .collect::<Result<Vec<_>, _>>()?;

                assert_eq!(names.len(), 2);
                assert_eq!(names, ["single_conn_a", "single_conn_b"]);

                Ok::<(), Box<dyn StdError + Send + Sync>>(())
            }));
        }

        for handle in join_handles {
            handle.await?.map_err(|err| err as Box<dyn StdError>)?;
        }

        let _ = fs::remove_file(path);

        Ok(())
    }

    /// Verifies that executing against a closed connection pool returns
    /// `Error::PoolClosed`.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database fails.
    #[tokio::test]
    async fn error_pool_closed() -> Result<(), Box<dyn StdError>> {
        let database = create_isolated_test_database().await;

        database.inner.pool.close();

        let result = database.execute("SELECT 1", ()).await;

        assert!(matches!(result, Err(Error::PoolClosed)));

        Ok(())
    }

    /// Verifies that SQLite constraint violations produce `Error::Turso`.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn error_turso() -> Result<(), Box<dyn StdError>> {
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

        assert!(matches!(duplicate_result, Err(Error::Turso(_))));

        Ok(())
    }
}
