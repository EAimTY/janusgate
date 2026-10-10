//! In-process SQLite database engine and connection-pooled abstraction backed by Turso.

use deadpool::{
    Runtime,
    managed::{
        BuildError, Manager as PoolManager, Metrics, Object, Pool, PoolError, RecycleError,
        RecycleResult,
    },
};
use std::{fs, io::Error as IoError, num::NonZero, path::Path, time::Duration};
use turso::{Builder as TursoBuilder, Connection, Database as TursoDatabase};

#[rustfmt::skip]
pub(crate) use turso::{Error as TursoError, IntoParams, Row, Value, params};

pub(crate) mod schema;

/// Maximum duration a database write waits for the database file lock held by another connection
/// before failing.
///
/// SQLite-compatible engines lock the whole database file for writes, so this bounds how long
/// concurrent writers block each other instead of failing immediately with a busy error.
const CONNECTION_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The fixed main-database connection pool size; a performance choice, not configuration: the
/// engine serializes writers on the whole-file lock ([`CONNECTION_BUSY_TIMEOUT`]), so extra
/// connections serve only concurrent short reads — per-request authorization lookups, quota
/// checks, and management queries — for which eight leave ample headroom at negligible
/// in-process cost.
///
/// # Panics
///
/// Never: the literal 8 is nonzero, and the `expect` states that invariant.
pub(crate) fn main_pool_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 8 is nonzero")]
    let connections = NonZero::new(8).expect("8 is nonzero");

    connections
}

/// The fixed main-database pool wait, paired with [`CONNECTION_BUSY_TIMEOUT`]: a checkout whose
/// holders are parked on engine work bounded by that busy timeout almost never converts into
/// success by waiting longer.
pub(crate) const MAIN_POOL_WAIT: Duration = Duration::from_secs(5);

/// Database operation, connection pool, and storage engine errors.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// Database path is not valid UTF-8.
    #[error("database path is not valid UTF-8: {0}")]
    InvalidPath(String),

    /// The database file is not interpretable under its schema registry: it carries an
    /// unknown schema version, a foreign `PRAGMA application_id` stamp, or a schema batch
    /// that failed to apply. Stored file state, not a first-party contract violation: the
    /// main database's startup open turns it fatal, while an api-log day file answers it
    /// through the failing request instead (README §3).
    #[error("database schema drift: {detail}")]
    SchemaDrift {
        /// Human-readable drift detail naming the file, its stored state, and the expectation.
        detail: String,
    },

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

impl Error {
    /// Whether this failure is the engine's stale-snapshot busy error: a read-then-write
    /// transaction attempted its first write after a concurrent commit invalidated its read
    /// snapshot, and the engine refused the upgrade ("database snapshot is stale, rollback and
    /// retry"; busy timeouts never help). The transaction must be rolled back and the attempt
    /// retried, and the retry re-reads the newer state — so a read-then-write lease such as the
    /// token-refresh acquire classifies it as "not acquired" rather than as failed work.
    pub(crate) fn is_stale_snapshot(&self) -> bool {
        matches!(self, Self::Turso(TursoError::BusySnapshot(_)))
    }
}

/// In-process SQLite-compatible database client managing a pool of connections.
#[derive(Clone, Debug)]
pub(crate) struct Database {
    /// Connection pool managing reusable Turso connections.
    pool: Pool<Manager>,
}

/// Database transaction handle holding exclusive ownership of a pooled connection.
///
/// [`Transaction::commit`] or [`Transaction::rollback`] finishes the transaction and returns the
/// connection to the pool. Dropping an unfinished handle discards its connection instead: the
/// engine-level transaction on it is still open, and returning it to the pool would park the
/// database write lock until the connection's next checkout.
#[derive(Debug)]
pub(crate) struct Transaction {
    /// Pooled connection hosting the open transaction.
    ///
    /// This is an `Option` only so `Drop` can dismantle the checkout with [`Object::take`] without
    /// running the pool-return path.
    connection: Option<Object<Manager>>,

    /// Whether the transaction has been finished by commit or rollback.
    is_completed: bool,
}

/// Deadpool connection manager for Turso database connections.
#[derive(Debug)]
struct Manager {
    /// In-process Turso database handle.
    database: TursoDatabase,
}

impl Database {
    /// Opens the in-process SQLite database at the specified path, creating the file and its
    /// parent directory when absent, and initializes a connection pool with at most
    /// `max_connections` concurrent connections, waiting at most `pool_wait` for a free
    /// connection when every connection is checked out. The `registry` names the schema version
    /// registry the database migrates under — the main control-plane schema, or the api_logs
    /// schema for api-log files.
    ///
    /// Before the pool is created, the schema version stored in `PRAGMA user_version` is verified
    /// and the database is migrated forward to the latest schema version of the registry: a fresh
    /// database receives the initial schema, an outdated one is migrated step by step, and a
    /// database this build cannot interpret — an unknown version, a foreign registry's
    /// application-id stamp, or a failed schema batch — fails with [`Error::SchemaDrift`]. See
    /// the [`schema`] module for the versioning and migration design.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if opening the database, creating the connection pool, or bringing the
    /// schema up to the registry's latest version fails; a database file this build cannot
    /// interpret answers [`Error::SchemaDrift`].
    pub(crate) async fn open(
        path: impl AsRef<Path>,
        registry: schema::Registry,
        max_connections: NonZero<usize>,
        pool_wait: Duration,
    ) -> Result<Self, Error> {
        let path = path.as_ref();

        // Validate before mutating the filesystem, so an invalid path never
        // leaves freshly created parent directories behind.
        let path_str = database_path(path)?;

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        Self::open_pooled(path_str, registry, max_connections, pool_wait).await
    }

    /// Opens the existing database at the specified path without creating anything: a missing
    /// file is `Ok(None)` — the empty partition a missing api-log file reads as (README §3
    /// *ApiLog Store*) — and neither the file nor its parent directory is ever created.
    /// Otherwise the open is exactly [`Database::open`]: the same pool sizing, wait, schema
    /// verification, and forward migration, so an outdated file a read opens first walks to
    /// the latest schema version before it serves.
    ///
    /// The existence check sits directly before the engine open, and the engine's open itself is
    /// create-on-open: a file that still vanishes between the two is replaced by a fresh, empty
    /// database, which the read answers as the empty partition the vanished file became. The
    /// residue is an empty schema'd file at the path, which retention expires like any other
    /// api-log file when a sweep next runs.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the path is not valid UTF-8, opening the database, creating the
    /// connection pool, or bringing the schema up to the registry's latest version fails; a
    /// database file this build cannot interpret answers [`Error::SchemaDrift`].
    pub(crate) async fn open_existing(
        path: impl AsRef<Path>,
        registry: schema::Registry,
        max_connections: NonZero<usize>,
        pool_wait: Duration,
    ) -> Result<Option<Self>, Error> {
        let path_str = database_path(path.as_ref())?;

        if !path.as_ref().exists() {
            return Ok(None);
        }

        Self::open_pooled(path_str, registry, max_connections, pool_wait)
            .await
            .map(Some)
    }

    /// Builds the pooled database over an already-validated path string: engine open, schema
    /// bootstrap, and connection pool construction — the body shared by the creating
    /// [`Database::open`] and the refuse-to-create [`Database::open_existing`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if opening the database, creating the connection pool, or bringing the
    /// schema up to the registry's latest version fails; a database file this build cannot
    /// interpret answers [`Error::SchemaDrift`].
    async fn open_pooled(
        path_str: &str,
        registry: schema::Registry,
        max_connections: NonZero<usize>,
        pool_wait: Duration,
    ) -> Result<Self, Error> {
        let database = TursoBuilder::new_local(path_str).build().await?;

        bootstrap_schema(&database, registry, path_str).await?;

        let manager = Manager { database };

        let pool = match Pool::builder(manager)
            .max_size(max_connections.get())
            .wait_timeout(Some(pool_wait))
            .runtime(Runtime::Tokio1)
            .build()
        {
            Ok(pool) => pool,

            #[expect(
                clippy::unreachable,
                reason = "Deadpool requires a runtime only when timeouts are configured, and the \
                          wait timeout is always paired with the explicit Tokio runtime above"
            )]
            Err(BuildError::NoRuntimeSpecified) => {
                unreachable!("deadpool wait timeout is always paired with a runtime")
            }
        };

        Ok(Self { pool })
    }

    /// Executes a SQL statement with parameters using a pooled connection and cached prepared
    /// statement.
    ///
    /// The statement must not produce rows; use [`Database::query`] for row-returning statements.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection, preparing the statement, or execution fails.
    pub(crate) async fn execute(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<u64, Error> {
        let connection = self.pool.get().await?;

        execute(&connection, sql, params).await
    }

    /// Queries the database with parameters using a pooled connection and a cached prepared
    /// statement, returning all rows eagerly materialized.
    ///
    /// The statement always executes to completion before the pooled connection is released, so a
    /// result set can never outlive its connection checkout or interleave with another borrower's
    /// statements.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection, preparing the statement, or executing the query
    /// fails.
    pub(crate) async fn query(
        &self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, Error> {
        let connection = self.pool.get().await?;

        query(&connection, sql, params).await
    }

    /// Begins a new transaction, checking out a connection from the pool and retaining exclusive
    /// ownership of it for the lifetime of the transaction.
    ///
    /// Dropping the returned handle without finishing it discards the checked out connection; see
    /// [`Transaction`]..
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if acquiring a connection or starting the transaction fails.
    pub(crate) async fn transaction(&self) -> Result<Transaction, Error> {
        let connection = self.pool.get().await?;

        execute(&connection, "BEGIN DEFERRED", ()).await?;

        Ok(Transaction {
            connection: Some(connection),
            is_completed: false,
        })
    }
}

impl Transaction {
    /// Executes a SQL statement within this transaction using a cached prepared statement.
    ///
    /// The statement must not produce rows; use [`Transaction::query`] for row-returning
    /// statements.
    ///
    /// Exclusive access to the handle is required (`&mut self`), so statements within one
    /// transaction can never run concurrently or interleave on the checked out connection.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub(crate) async fn execute(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<u64, Error> {
        execute(self.connection(), sql, params).await
    }

    /// Queries the database within this transaction using a cached prepared statement, returning
    /// all rows eagerly materialized. The statement always executes to completion, leaving no
    /// unfinished statement state on the connection when the transaction commits or rolls back.
    ///
    /// Exclusive access to the handle is required (`&mut self`), so statements within one
    /// transaction can never run concurrently or interleave on the checked out connection.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if preparing or executing the query fails.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub(crate) async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, Error> {
        query(self.connection(), sql, params).await
    }

    /// Commits the transaction, applying all changes and returning the pooled connection to the
    /// connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if committing the transaction fails. A failed commit discards the pooled
    /// connection instead of returning it, because the transaction state on it is unresolved.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub(crate) async fn commit(mut self) -> Result<(), Error> {
        self.execute("COMMIT", ()).await?;

        self.is_completed = true;

        Ok(())
    }

    /// Rolls back the transaction, discarding all changes and returning the pooled connection to
    /// the connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if rolling back the transaction fails. A failed rollback discards the
    /// pooled connection instead of returning it, because the transaction state on it is
    /// unresolved.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    pub(crate) async fn rollback(mut self) -> Result<(), Error> {
        self.execute("ROLLBACK", ()).await?;

        self.is_completed = true;

        Ok(())
    }

    /// Returns the checked out connection hosting the open transaction.
    ///
    /// # Panics
    ///
    /// Panics if the transaction connection has already been dismantled, which
    /// violates an internal invariant.
    #[expect(
        clippy::expect_used,
        reason = "connection is held until Drop dismantles the checkout"
    )]
    fn connection(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("connection is held until Drop dismantles the checkout")
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

impl From<PoolError<TursoError>> for Error {
    fn from(error: PoolError<TursoError>) -> Self {
        match error {
            PoolError::Backend(error) => Self::Turso(error),
            PoolError::Closed => Self::PoolClosed,
            PoolError::Timeout(_) => Self::PoolTimeout,

            #[expect(
                clippy::unreachable,
                reason = "the pool attaches a Tokio runtime and installs no hooks, making these \
                          error variants unreachable"
            )]
            PoolError::NoRuntimeSpecified | PoolError::PostCreateHook(_) => {
                unreachable!("deadpool runtime is attached and no hooks are installed")
            }
        }
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

/// Validates that a database path is UTF-8, which the engine's local-file open requires. Both
/// open modes run this before anything is created, so an invalid path never leaves freshly
/// created parent directories behind.
///
/// # Errors
///
/// Returns [`Error::InvalidPath`] for a path that is not valid UTF-8.
fn database_path(path: &Path) -> Result<&str, Error> {
    path.to_str()
        .ok_or_else(|| Error::InvalidPath(path.to_string_lossy().into_owned()))
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

/// Prepares a cached SQL statement against the given connection and drains the result set to
/// completion into an owned vector of rows.
///
/// Draining matters: an unfinished statement left on the connection would corrupt or roll back
/// whatever the next borrower of the pooled connection executes.
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

/// Applies the pragmas and timeouts required by every JanusGate database connection, whether pooled
/// or used for schema bootstrap. Foreign key enforcement is therefore also on while the schema
/// migration workflow runs; see the [`schema`] module's documentation for the constraint this puts
/// on migration texts.
///
/// # Errors
///
/// Returns [`TursoError`] if configuring the busy timeout or the foreign key pragma fails.
async fn configure_connection(connection: &Connection) -> Result<(), TursoError> {
    connection.busy_timeout(CONNECTION_BUSY_TIMEOUT)?;

    connection.pragma_update("foreign_keys", "ON").await?;

    Ok(())
}

/// Brings the database schema up to the latest version of `registry` before the connection pool is
/// created, logging progress under the database's path. See the [`schema`] module for the
/// versioning and migration design.
///
/// # Errors
///
/// Returns [`Error`] if connecting the bootstrap connection, configuring it, or running the
/// migration workflow fails; a database file this build cannot interpret under `registry`
/// answers [`Error::SchemaDrift`].
async fn bootstrap_schema(
    database: &TursoDatabase,
    registry: schema::Registry,
    path: &str,
) -> Result<(), Error> {
    let connection = database.connect()?;

    configure_connection(&connection).await?;

    schema::migrate(&connection, registry, path).await
}
