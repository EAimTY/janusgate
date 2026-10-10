//! Versioned database schema definitions and the startup migration workflow.
//!
//! The schema is versioned linearly: every modification to a database schema is a new version,
//! defined by one `version_x.rs` submodule whose only item is `const SCHEMA: Schema` and
//! registered in its registry — [`MAIN_SCHEMAS`] for the main database, [`API_LOGS_SCHEMAS`]
//! for the api-log files, selected by [`Registry`]. A version's `schema` field carries the
//! complete DDL of that version as hand-written SQL, and its `migration` field carries the SQL
//! that transforms the previous version's database into it.
//!
//! [`migrate`] runs when a database is opened: it reads `PRAGMA user_version`, applies the first
//! version's DDL to a fresh database and walks forward from there, applying one migration per
//! version, each committed atomically together with its `user_version` advance — so a fresh
//! database and an outdated one both end at the latest version of their registry.
//!
//! Each registry stamps its identity into the database header: version 1's SQL sets
//! `PRAGMA application_id` to the registry's fourcc — `JGMD` for the main database, `JGAL` for
//! an api-log file. Before walking an existing database, the bootstrap verifies the stamp, and
//! a mismatch — like an unknown stored version or a failed schema batch — answers the typed
//! schema-drift error ([`Error::SchemaDrift`]) instead of a panic, so a file opened under the
//! wrong registry fails its open rather than surfacing later as missing-table errors on first
//! query. A database file predating the stamp carries the default `0` and fails the same
//! verification — an unreleased development artifact the operator recreates. The main
//! database's startup open treats the error as fatal, while an api-log day file answering with
//! drift fails only its own request (`docs/design/storage.md`).
//!
//! Migration texts run under foreign key enforcement: the bootstrap connection enables
//! `PRAGMA foreign_keys` before the walk, and each text executes inside its own transaction, where
//! SQLite defines the pragma as a no-op (Turso currently accepting it mid-transaction is a
//! divergence from SQLite that may be aligned at any time). No migration may therefore rely on
//! disabling enforcement; every migration must be authored to hold under it statement by
//! statement — a table rebuild, for instance, orders its steps so referential integrity holds at
//! every intermediate state.

mod api_logs_version_1;
mod version_1;

use super::Error;
use log::{debug, info};
use turso::Connection;

/// One version of a SQLite database schema.
///
/// Both text fields are statement sequences in which every statement is terminated by `;`; the
/// workflow embeds them into its transaction batches.
struct Schema {
    /// The version's number, stored in `PRAGMA user_version`.
    ///
    /// Versions start at 1 and increase by exactly one per version, so every stored version
    /// resolves by binary search and every version is reachable by walking forward.
    version: i64,

    /// The complete DDL creating the version's schema from an empty database, hand-written
    /// in the version's module.
    schema: &'static str,

    /// The statements that transform a database of the previous version into this version. The
    /// workflow applies them together with the `user_version` advance inside one transaction.
    ///
    /// The empty string for version 1, which has no predecessor: a fresh database receives
    /// [`Schema::schema`] directly instead.
    ///
    /// The statements run under foreign key enforcement and must be authored to hold under it —
    /// see the module documentation; never rely on `PRAGMA foreign_keys` to disable enforcement
    /// mid-migration.
    migration: &'static str,
}

/// Every schema version of the main database, ordered by ascending version starting at 1 with no
/// gaps.
const MAIN_SCHEMAS: &[Schema] = &[version_1::SCHEMA];

/// Every schema version of an api-log file, ordered by ascending version starting at 1 with no
/// gaps. Api-log versions are immutable history: a column added later is a new version whose
/// migration existing files walk when opened, so they stay readable.
const API_LOGS_SCHEMAS: &[Schema] = &[api_logs_version_1::SCHEMA];

/// The `PRAGMA application_id` value stamped into the main database: the fourcc `JGMD` —
/// *JanusGate main database*.
const MAIN_APPLICATION_ID: i64 = 0x4a474d44;

/// The `PRAGMA application_id` value stamped into an api-log file: the fourcc `JGAL` —
/// *JanusGate api logs*.
const API_LOGS_APPLICATION_ID: i64 = 0x4a47414c;

/// The schema registry a database migrates under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Registry {
    /// The main control-plane database.
    Main,

    /// One api-log file of the day-partitioned api-log store (`docs/design/storage.md` *ApiLog
    /// Store*).
    ApiLogs,
}

impl Registry {
    /// The registry's schema versions in ascending order.
    fn schemas(self) -> &'static [Schema] {
        match self {
            Self::Main => MAIN_SCHEMAS,
            Self::ApiLogs => API_LOGS_SCHEMAS,
        }
    }

    /// The value the registry's version-1 SQL stamps into `PRAGMA application_id`, and the value
    /// the bootstrap expects to find on an existing database.
    fn application_id(self) -> i64 {
        match self {
            Self::Main => MAIN_APPLICATION_ID,
            Self::ApiLogs => API_LOGS_APPLICATION_ID,
        }
    }

    /// The registry's name for startup diagnostics.
    fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::ApiLogs => "api_logs",
        }
    }
}

/// Brings the schema of the database behind `connection` up to the latest version of `registry`,
/// logging progress under the database's `path`.
///
/// Delegates to [`bring_up_to_latest`] over the registry's versions; see that function for the
/// workflow.
///
/// # Errors
///
/// Returns [`Error`] under the same conditions as [`bring_up_to_latest`]; a database this build
/// cannot interpret under `registry` answers [`Error::SchemaDrift`].
pub(super) async fn migrate(
    connection: &Connection,
    registry: Registry,
    path: &str,
) -> Result<(), Error> {
    bring_up_to_latest(connection, registry, registry.schemas(), path).await
}

/// Walks `schemas` — schema versions in ascending order — forward from the database's stored
/// version, bringing the database behind `connection` to the latest version and logging
/// progress under the database's `path`.
///
/// The current version is read from `PRAGMA user_version`. A fresh database (`0`) receives the
/// first version's DDL — which stamps the registry into `PRAGMA application_id` — and joins the
/// walk from there, so a fresh database ends at the latest version, however many exist, through
/// the same later migrations an outdated database applies. The fresh-database decision re-runs
/// inside the write-locked bootstrap transaction: of several opens bootstrapping one file at
/// once — an api-log write racing an api-log read — exactly one applies the DDL, and the others
/// join the version it committed, stamp verification included. Every migration step re-checks
/// the stored version inside its own transaction the same way, so two opens walking one
/// outdated file apply each migration exactly once between them. A database at a known version
/// first has its application id verified against `registry` — a mismatch, or the default `0` on
/// a file predating the stamp, answers drift — and then applies the migrations of every later
/// version in order; a database already at the latest version does nothing. Each step — one
/// schema or migration text plus its `user_version` write — commits atomically, so a crash
/// mid-chain leaves the database on its last committed version and the next startup resumes
/// from there.
///
/// The schema versions and the registry are parameters rather than [`MAIN_SCHEMAS`] or
/// [`API_LOGS_SCHEMAS`] directly, so one walk serves both registries without knowing which it
/// brings up.
///
/// # Errors
///
/// Returns [`Error`] if reading `user_version`, beginning or aborting a bootstrap transaction,
/// verifying the registry's application-id stamp, or applying a schema or migration batch fails.
/// A stored version negative or newer than the latest known, or an application id other than the
/// one `registry` stamps, answers [`Error::SchemaDrift`] — a database this build cannot
/// interpret.
async fn bring_up_to_latest(
    connection: &Connection,
    registry: Registry,
    schemas: &[Schema],
    path: &str,
) -> Result<(), Error> {
    let mut current_version = query_user_version(connection).await?;

    if current_version == 0 {
        // A fresh database receives the first version's DDL — stamping the registry — and joins
        // the walk from there, exactly like a database already at version 1 — whatever later
        // versions exist still apply.
        current_version = apply_first_version(connection, registry, schemas, path).await?;
    } else {
        // An existing database carries a schema version, so it must carry this registry's
        // application-id stamp; verify it before any DDL runs against the file.
        verify_application_id(connection, registry, path).await?;
    }

    match schemas.binary_search_by(|schema| schema.version.cmp(&current_version)) {
        Ok(index) => {
            for schema in schemas.iter().skip(index + 1) {
                apply(connection, schema.migration, schema.version, path).await?;
            }

            if current_version == last_schema(schemas).version {
                debug!("database {path} is at schema version {current_version}, the latest known");
            }
        }

        Err(_) => {
            // Versions are contiguous from 1, so a binary-search miss among positive versions
            // can only be a version newer than the latest this build knows. Stored file state
            // this build cannot interpret: typed drift, fatal at the main database's startup
            // open and request-scoped for an api-log day file (`docs/design/storage.md`).
            let latest_version = last_schema(schemas).version;

            return Err(Error::SchemaDrift {
                detail: format!(
                    "database {path} has unknown schema version {current_version}; this build \
                     knows versions 1..={latest_version}"
                ),
            });
        }
    }

    Ok(())
}

/// Applies the first version's DDL to a database whose stored version read `0`, returning the
/// version the database carries afterwards for the walk to continue from.
///
/// The fresh-database decision races any concurrent bootstrap of the same file: before either
/// connection commits, both read `0` from `PRAGMA user_version`, and the loser re-applying the
/// DDL would die on the first `CREATE TABLE`. The decision therefore re-runs inside the
/// write-locked bootstrap transaction — [`begin_write_transaction`] serializes it against every
/// other writer of the file: a connection still reading `0` inside the transaction is the
/// bootstrapper and applies the DDL; one finding the version a racing bootstrap committed aborts
/// its empty transaction and verifies that database's application id — a mismatch still answers
/// drift — returning the found version to continue the walk from.
///
/// # Errors
///
/// Returns [`Error`] if beginning or aborting the transaction, reading `user_version`, applying
/// the DDL batch, or verifying the application id fails; a database whose application id is not
/// the one `registry` stamps answers [`Error::SchemaDrift`].
async fn apply_first_version(
    connection: &Connection,
    registry: Registry,
    schemas: &[Schema],
    path: &str,
) -> Result<i64, Error> {
    begin_write_transaction(connection).await?;

    if query_user_version(connection).await? == 0 {
        let first = first_schema(schemas);
        apply_batch(connection, first.schema, first.version, path).await?;

        Ok(first.version)
    } else {
        let raced_version = query_user_version(connection).await?;

        rollback_write_transaction(connection).await?;
        verify_application_id(connection, registry, path).await?;

        Ok(raced_version)
    }
}

/// Applies a schema or migration text together with its `user_version` advance in one atomic
/// transaction and logs the step.
///
/// The text and the version write must commit atomically: a crash that persisted only part of a
/// migration would leave the database between versions, which no build can resume.
///
/// Like the fresh-bootstrap decision in [`apply_first_version`], the walk's own version decision
/// re-runs inside the write-locked bootstrap transaction: the caller selected this migration from
/// a `user_version` read taken before any lock was held, and a racing bootstrap of the same
/// outdated file can commit this very migration first — two opens walking one file, an api-log
/// write racing an api-log read, serialize on the write lock here. A connection finding the
/// version at or
/// past `version` inside the transaction aborts its empty transaction and joins the racing walk's
/// outcome; the stored version can only have moved forward since the caller's read, so
/// at-or-past the target means this migration is already applied.
///
/// # Errors
///
/// Returns [`Error`] if beginning or aborting the transaction, reading `user_version`, or
/// executing the batch fails.
async fn apply(connection: &Connection, text: &str, version: i64, path: &str) -> Result<(), Error> {
    begin_write_transaction(connection).await?;

    if query_user_version(connection).await? >= version {
        rollback_write_transaction(connection).await?;
        debug!(
            "database {path} already carries schema version {version}; a racing bootstrap \
                applied its migration"
        );

        return Ok(());
    }

    apply_batch(connection, text, version, path).await
}

/// Executes the version batch — the schema or migration text plus the `user_version` advance —
/// on the caller's open bootstrap transaction, committing it, and logs the step.
///
/// # Errors
///
/// Returns [`Error::SchemaDrift`] if executing the batch fails. Every batch failure classifies
/// as drift — an engine error during the DDL or the `COMMIT` no less than a refusal by the
/// file's stored objects — because a partially applied batch leaves the file uninterpretable,
/// which is the condition drift names.
async fn apply_batch(
    connection: &Connection,
    text: &str,
    version: i64,
    path: &str,
) -> Result<(), Error> {
    let batch = format!("{text}PRAGMA user_version = {version};\nCOMMIT;",);

    connection
        .execute_batch(batch)
        .await
        .map_err(|failure| Error::SchemaDrift {
            detail: format!(
                "failed to apply a database schema version {version} to database {path}: \
                 {failure}"
            ),
        })?;

    info!("brought database {path} to schema version {version}");

    Ok(())
}

/// Begins the bootstrap's immediate transaction, taking the database's write lock so the
/// caller's version reads and DDL decisions hold against every concurrent writer of the file.
///
/// # Errors
///
/// Returns [`Error`] if beginning the transaction fails.
async fn begin_write_transaction(connection: &Connection) -> Result<(), Error> {
    connection.execute_batch("BEGIN IMMEDIATE;").await?;

    Ok(())
}

/// Aborts the caller's empty bootstrap transaction — the raced fresh-bootstrap loser's only
/// outcome — leaving the racing bootstrap's committed schema untouched.
///
/// # Errors
///
/// Returns [`Error`] if aborting the transaction fails.
async fn rollback_write_transaction(connection: &Connection) -> Result<(), Error> {
    connection.execute_batch("ROLLBACK;").await?;

    Ok(())
}

/// Retrieves the schema version stored in `PRAGMA user_version`.
///
/// # Errors
///
/// Returns [`Error`] if executing the pragma query or extracting its value fails.
///
/// # Panics
///
/// Panics if the pragma query yields no row: `PRAGMA user_version` always produces exactly one
/// row, an engine-language guarantee independent of file state.
async fn query_user_version(connection: &Connection) -> Result<i64, Error> {
    let mut rows = connection.query("PRAGMA user_version", ()).await?;

    #[expect(
        clippy::expect_used,
        reason = "the PRAGMA user_version query always produces a row, an engine-language \
                  guarantee independent of file state"
    )]
    let row = rows.next().await?.expect("missing user_version row");

    Ok(row.get(0)?)
}

/// Verifies that the database behind `connection` carries `registry`'s `PRAGMA application_id`
/// stamp, answering drift with the stored and the expected value named when they disagree.
///
/// The stamp is written by the registry's own version-1 SQL and never again, so the stored value
/// is the file's registry identity; a disagreement means the file belongs to the other registry
/// — or predates the stamp, carrying the default `0` — and no statement may run against it.
///
/// # Errors
///
/// Returns [`Error`] if reading the pragma fails, or [`Error::SchemaDrift`] if the stored
/// application id is not the one `registry` stamps.
async fn verify_application_id(
    connection: &Connection,
    registry: Registry,
    path: &str,
) -> Result<(), Error> {
    let stored = query_application_id(connection).await?;
    let expected = registry.application_id();

    if stored != expected {
        // Stored file state this build cannot interpret: typed drift, fatal at the main
        // database's startup open and request-scoped for an api-log day file
        // (`docs/design/storage.md`).
        return Err(Error::SchemaDrift {
            detail: format!(
                "database {path} carries the application_id {stored:#x}, but the {name} registry \
                 stamps {expected:#x}; the main and api-log registries live in separate files",
                name = registry.name(),
            ),
        });
    }

    Ok(())
}

/// Retrieves the application id stored in `PRAGMA application_id`.
///
/// # Errors
///
/// Returns [`Error`] if executing the pragma query or extracting its value fails.
///
/// # Panics
///
/// Panics if the pragma query yields no row: `PRAGMA application_id` always produces exactly one
/// row, an engine-language guarantee independent of file state.
async fn query_application_id(connection: &Connection) -> Result<i64, Error> {
    let mut rows = connection.query("PRAGMA application_id", ()).await?;

    #[expect(
        clippy::expect_used,
        reason = "the PRAGMA application_id query always produces a row, an engine-language \
                  guarantee independent of file state"
    )]
    let row = rows.next().await?.expect("missing application id row");

    Ok(row.get(0)?)
}

/// Returns the first schema version of `schemas`.
///
/// # Panics
///
/// Panics if `schemas` is empty — the registry always contains at least version 1, and every
/// other table the workflow sees follows the same shape.
fn first_schema(schemas: &[Schema]) -> &Schema {
    #[expect(
        clippy::expect_used,
        reason = "the schema registry always contains version 1, so a first version exists"
    )]
    schemas
        .first()
        .expect("the schema registry always contains version 1")
}

/// Returns the last schema version of `schemas`.
///
/// # Panics
///
/// Panics if `schemas` is empty — the registry always contains at least version 1, and every
/// other table the workflow sees follows the same shape.
fn last_schema(schemas: &[Schema]) -> &Schema {
    #[expect(
        clippy::expect_used,
        reason = "the schema registry always contains version 1, so a latest version exists"
    )]
    schemas
        .last()
        .expect("the schema registry always contains version 1")
}
