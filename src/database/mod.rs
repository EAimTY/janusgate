//! In-process SQLite database engine and connection-pooled abstraction backed
//! by Turso.

use deadpool::{
    Runtime,
    managed::{
        BuildError, Manager as PoolManager, Metrics, Object, Pool, PoolError, RecycleError,
        RecycleResult,
    },
};
use std::{fs, io::Error as IoError, num::NonZero, path::Path, sync::Arc, time::Duration};
use turso::{Builder as TursoBuilder, Connection, Database as TursoDatabase, Error as TursoError};

#[cfg(test)]
mod tests;

// The re-exported items double as the module's own imports: `Row` and
// `IntoParams` appear in this module's signatures.
#[allow(unused_imports)]
pub use turso::{IntoParams, Params, Row, Value, named_params, params, params_from_iter};

/// Non-zero 32-bit big-endian schema fingerprint computed at compile time.
const DATABASE_SCHEMA_FINGERPRINT: i32 = i32::from_be_bytes(*include_bytes!(concat!(
    env!("OUT_DIR"),
    "/database_schema_fingerprint"
)));

/// Canonical SQLite DDL schema embedded at compile time.
const DATABASE_SCHEMA: &str = include_str!("SCHEMA.sqlite");

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
///
/// [`Transaction::commit`] or [`Transaction::rollback`] finishes the
/// transaction and returns the connection to the pool. Dropping an unfinished
/// handle discards its connection instead: the engine-level transaction on it
/// is still open, and returning it to the pool would park the database write
/// lock until the connection's next checkout.
#[derive(Debug)]
pub struct Transaction {
    /// Pooled connection hosting the open transaction.
    ///
    /// This is an `Option` only so `Drop` can dismantle the checkout with
    /// [`Object::take`] without running the pool-return path.
    connection: Option<Object<Manager>>,

    /// Whether the transaction has been finished by commit or rollback.
    is_completed: bool,
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

impl Database {
    /// Opens the in-process SQLite database at the specified path and
    /// initializes a connection pool with at most `max_connections` concurrent
    /// connections, waiting at most `pool_timeout` for a free connection when
    /// every connection is checked out.
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
    pub async fn open(
        path: impl AsRef<Path>,
        max_connections: NonZero<usize>,
        pool_timeout: Duration,
    ) -> Result<Self, Error> {
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

        let pool = match Pool::builder(manager)
            .max_size(max_connections.get())
            .wait_timeout(Some(pool_timeout))
            .runtime(Runtime::Tokio1)
            .build()
        {
            Ok(pool) => pool,
            Err(BuildError::NoRuntimeSpecified) => {
                // SAFETY: Deadpool requires a runtime only when timeouts are
                // configured, and the wait timeout is always paired with the
                // explicit Tokio runtime above.
                unreachable!("deadpool wait timeout is always paired with a runtime");
            }
        };

        Ok(Self {
            inner: Arc::new(Inner { pool }),
        })
    }

    /// Executes a SQL statement with parameters using a pooled connection and
    /// cached prepared statement.
    ///
    /// The statement must not produce rows; use [`Database::query`] for
    /// row-returning statements.
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
    /// Dropping the returned handle without finishing it discards the checked
    /// out connection; see [`Transaction`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection or starting the transaction
    /// fails.
    pub async fn transaction(&self) -> Result<Transaction, Error> {
        let connection = self.inner.pool.get().await?;

        execute(&connection, "BEGIN DEFERRED", ()).await?;

        Ok(Transaction {
            connection: Some(connection),
            is_completed: false,
        })
    }
}

impl Transaction {
    /// Executes a SQL statement within this transaction using a cached prepared
    /// statement.
    ///
    /// The statement must not produce rows; use [`Transaction::query`] for
    /// row-returning statements.
    ///
    /// Exclusive access to the handle is required (`&mut self`), so statements
    /// within one transaction can never run concurrently or interleave on the
    /// checked out connection.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub async fn execute(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<u64, Error> {
        #[expect(
            clippy::expect_used,
            reason = "connection is held until Drop dismantles the checkout"
        )]
        let connection = self
            .connection
            .as_ref()
            .expect("connection is held until Drop dismantles the checkout");

        execute(connection, sql, params).await
    }

    /// Queries the database within this transaction using a cached prepared
    /// statement, returning all rows eagerly materialized. The statement
    /// always executes to completion, leaving no unfinished statement state on
    /// the connection when the transaction commits or rolls back.
    ///
    /// Exclusive access to the handle is required (`&mut self`), so statements
    /// within one transaction can never run concurrently or interleave on the
    /// checked out connection.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the query fails.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, Error> {
        #[expect(
            clippy::expect_used,
            reason = "connection is held until Drop dismantles the checkout"
        )]
        let connection = self
            .connection
            .as_ref()
            .expect("connection is held until Drop dismantles the checkout");

        query(connection, sql, params).await
    }

    /// Commits the transaction, applying all changes and returning the pooled
    /// connection to the connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if committing the transaction fails. A failed commit
    /// discards the pooled connection instead of returning it, because the
    /// transaction state on it is unresolved.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub async fn commit(mut self) -> Result<(), Error> {
        self.execute("COMMIT", ()).await?;

        self.is_completed = true;

        Ok(())
    }

    /// Rolls back the transaction, discarding all changes and returning the
    /// pooled connection to the connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if rolling back the transaction fails. A failed
    /// rollback discards the pooled connection instead of returning it, because
    /// the transaction state on it is unresolved.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub async fn rollback(mut self) -> Result<(), Error> {
        self.execute("ROLLBACK", ()).await?;

        self.is_completed = true;

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
        // An unfinished transaction never reaches the pool: dropping its
        // `Transaction` handle discards the connection outright. A connection
        // that still reports an open transaction here therefore stems from
        // misuse such as a raw `BEGIN` issued through the query API. Its state
        // is unknown; failing here makes deadpool discard and replace it.
        if !connection.is_autocommit()? {
            return Err(RecycleError::Backend(TursoError::Misuse(
                "recycled connection still has an open transaction".to_string(),
            )));
        }

        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.is_completed {
            return;
        }

        // The engine-level transaction on this connection is still open, and
        // its rollback would only run at the connection's next pool checkout.
        // Returning it to the pool would park the database write lock until
        // then, so dismantle the checkout and drop the connection outright,
        // releasing the lock immediately.
        if let Some(connection) = self.connection.take() {
            drop(Object::take(connection));
        }
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

impl From<PoolError<TursoError>> for Error {
    fn from(error: PoolError<TursoError>) -> Self {
        match error {
            PoolError::Backend(error) => Self::Turso(error),
            PoolError::Closed => Self::PoolClosed,
            PoolError::Timeout(_) => Self::PoolTimeout,
            PoolError::NoRuntimeSpecified | PoolError::PostCreateHook(_) => {
                // SAFETY: The pool attaches a Tokio runtime and installs no
                // hooks, making these error variants unreachable.
                unreachable!("deadpool runtime is attached and no hooks are installed");
            }
        }
    }
}
