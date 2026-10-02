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
//! an api-log file. Before walking an existing database, the bootstrap verifies the stamp and
//! panics on a mismatch, so a file opened under the wrong registry fails at startup instead of
//! surfacing later as missing-table errors on first query. A database file predating the stamp
//! carries the default `0` and fails the same startup verification — an unreleased development
//! artifact the operator recreates.
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
mod version_2;

use log::{debug, info};
#[cfg(test)]
use std::error::Error as StdError;
use turso::Connection;
#[cfg(test)]
use turso::{Builder as TursoBuilder, Database as TursoDatabase};

/// One version of a SQLite database schema.
///
/// Both text fields are statement sequences in which every statement is terminated by `;`; the
/// workflow embeds them into its transaction batches.
struct Schema {
    /// The version's number, stored in `PRAGMA user_version`.
    ///
    /// Versions start at 1 and increase by exactly one per version; the ordering test enforces
    /// this over both registries.
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
const MAIN_SCHEMAS: &[Schema] = &[version_1::SCHEMA, version_2::SCHEMA];

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

    /// One api-log file of the day-partitioned api-log store (README §3 *ApiLog Store*).
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
/// # Panics
///
/// Panics under the same conditions as [`bring_up_to_latest`].
pub(super) async fn migrate(connection: &Connection, registry: Registry, path: &str) {
    bring_up_to_latest(connection, registry, registry.schemas(), path).await;
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
/// a file predating the stamp, is fatal — and then applies the migrations of every later version
/// in order; a database already at the latest version does nothing. Each step — one schema or
/// migration text plus its `user_version` write — commits atomically, so a crash mid-chain
/// leaves the database on its last committed version and the next startup resumes from there.
///
/// The schema versions and the registry are parameters rather than [`MAIN_SCHEMAS`],
/// [`API_LOGS_SCHEMAS`], and their [`Registry`] values themselves so tests can exercise the walk
/// over chains with more versions than a registry currently contains; a synthetic chain carries
/// the application-id stamp in its version 1 exactly like a real registry.
///
/// # Panics
///
/// Panics if reading `user_version`, beginning or aborting a bootstrap transaction, verifying
/// the registry's application-id stamp, applying a schema or migration batch, or advancing
/// `user_version` fails, if the stored version is negative or newer than the latest known
/// version — a database this build cannot interpret — or if the database's application id is not
/// the one `registry` stamps.
async fn bring_up_to_latest(
    connection: &Connection,
    registry: Registry,
    schemas: &[Schema],
    path: &str,
) {
    let mut current_version = query_user_version(connection).await;

    if current_version == 0 {
        // A fresh database receives the first version's DDL — stamping the registry — and joins
        // the walk from there, exactly like a database already at version 1 — whatever later
        // versions exist still apply.
        current_version = apply_first_version(connection, registry, schemas, path).await;
    } else {
        // An existing database carries a schema version, so it must carry this registry's
        // application-id stamp; verify it before any DDL runs against the file.
        verify_application_id(connection, registry, path).await;
    }

    match schemas.binary_search_by(|schema| schema.version.cmp(&current_version)) {
        Ok(index) => {
            for schema in schemas.iter().skip(index + 1) {
                apply(connection, schema.migration, schema.version, path).await;
            }

            if current_version == last_schema(schemas).version {
                debug!("database {path} is at schema version {current_version}, the latest known");
            }
        }

        Err(_) => {
            // Versions are contiguous from 1, so a binary-search miss among positive versions
            // can only be a version newer than the latest this build knows.
            let latest_version = last_schema(schemas).version;

            #[expect(
                clippy::panic,
                reason = "an unknown schema version is fatal: this build cannot interpret \
                          the database"
            )]
            {
                panic!(
                    "database {path} has unknown schema version {current_version}; this build \
                     knows versions 1..={latest_version}"
                );
            }
        }
    }
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
/// its empty transaction and verifies that database's application id — a mismatch stays fatal —
/// returning the found version to continue the walk from.
///
/// # Panics
///
/// Panics if beginning or aborting the transaction, reading `user_version`, applying the DDL
/// batch, or verifying the application id fails, or if the database's application id is not the
/// one `registry` stamps.
async fn apply_first_version(
    connection: &Connection,
    registry: Registry,
    schemas: &[Schema],
    path: &str,
) -> i64 {
    begin_write_transaction(connection).await;

    if query_user_version(connection).await == 0 {
        let first = first_schema(schemas);
        apply_batch(connection, first.schema, first.version, path).await;

        first.version
    } else {
        let raced_version = query_user_version(connection).await;

        rollback_write_transaction(connection).await;
        verify_application_id(connection, registry, path).await;

        raced_version
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
/// # Panics
///
/// Panics if beginning or aborting the transaction, reading `user_version`, or executing the
/// batch fails.
async fn apply(connection: &Connection, text: &str, version: i64, path: &str) {
    begin_write_transaction(connection).await;

    if query_user_version(connection).await >= version {
        rollback_write_transaction(connection).await;
        debug!(
            "database {path} already carries schema version {version}; a racing bootstrap \
                applied its migration"
        );

        return;
    }

    apply_batch(connection, text, version, path).await;
}

/// Executes the version batch — the schema or migration text plus the `user_version` advance —
/// on the caller's open bootstrap transaction, committing it, and logs the step.
///
/// # Panics
///
/// Panics if executing the batch fails.
async fn apply_batch(connection: &Connection, text: &str, version: i64, path: &str) {
    let batch = format!("{text}PRAGMA user_version = {version};\nCOMMIT;",);

    #[expect(
        clippy::expect_used,
        reason = "applying a schema version batch is fatal on failure"
    )]
    connection
        .execute_batch(batch)
        .await
        .expect("failed to apply a database schema version");

    info!("brought database {path} to schema version {version}");
}

/// Begins the bootstrap's immediate transaction, taking the database's write lock so the
/// caller's version reads and DDL decisions hold against every concurrent writer of the file.
///
/// # Panics
///
/// Panics if beginning the transaction fails.
async fn begin_write_transaction(connection: &Connection) {
    #[expect(
        clippy::expect_used,
        reason = "beginning a schema bootstrap transaction is fatal on failure"
    )]
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .await
        .expect("failed to begin a database schema bootstrap transaction");
}

/// Aborts the caller's empty bootstrap transaction — the raced fresh-bootstrap loser's only
/// outcome — leaving the racing bootstrap's committed schema untouched.
///
/// # Panics
///
/// Panics if aborting the transaction fails.
async fn rollback_write_transaction(connection: &Connection) {
    #[expect(
        clippy::expect_used,
        reason = "aborting the empty schema bootstrap transaction is fatal on failure"
    )]
    connection
        .execute_batch("ROLLBACK;")
        .await
        .expect("failed to abort a database schema bootstrap transaction");
}

/// Retrieves the schema version stored in `PRAGMA user_version`.
///
/// # Panics
///
/// Panics if executing the pragma query or extracting its value fails.
async fn query_user_version(connection: &Connection) -> i64 {
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
        reason = "the PRAGMA user_version query always produces a row"
    )]
    let row = rows
        .next()
        .await
        .expect("failed to read the user_version row")
        .expect("missing user_version row");

    #[expect(
        clippy::expect_used,
        reason = "the user_version column is an integer value"
    )]
    row.get(0)
        .expect("failed to get the user_version integer value")
}

/// Verifies that the database behind `connection` carries `registry`'s `PRAGMA application_id`
/// stamp, panicking with the stored and the expected value named when they disagree.
///
/// The stamp is written by the registry's own version-1 SQL and never again, so the stored value
/// is the file's registry identity; a disagreement means the file belongs to the other registry
/// — or predates the stamp, carrying the default `0` — and no statement may run against it.
///
/// # Panics
///
/// Panics if reading the pragma fails, or if the stored application id is not the one `registry`
/// stamps.
async fn verify_application_id(connection: &Connection, registry: Registry, path: &str) {
    let stored = query_application_id(connection).await;
    let expected = registry.application_id();

    if stored != expected {
        #[expect(
            clippy::panic,
            reason = "an application-id mismatch is fatal: the file is another registry's \
                      database, or predates the stamp"
        )]
        {
            panic!(
                "database {path} carries the application_id {stored:#x}, but the {name} registry \
                 stamps {expected:#x}; the main and api-log registries live in separate files",
                name = registry.name(),
            );
        }
    }
}

/// Retrieves the application id stored in `PRAGMA application_id`.
///
/// # Panics
///
/// Panics if executing the pragma query or extracting its value fails.
async fn query_application_id(connection: &Connection) -> i64 {
    #[expect(
        clippy::expect_used,
        reason = "reading the application id of a versioned database is fatal on failure"
    )]
    let mut rows = connection
        .query("PRAGMA application_id", ())
        .await
        .expect("failed to query the database's application id");

    #[expect(
        clippy::expect_used,
        reason = "the PRAGMA application_id query always produces a row"
    )]
    let row = rows
        .next()
        .await
        .expect("failed to read the application id row")
        .expect("missing application id row");

    #[expect(
        clippy::expect_used,
        reason = "the application id column is an integer value"
    )]
    row.get(0)
        .expect("failed to get the application id integer value")
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

/// Opens a fresh in-memory test database, for tests that need several connections to one
/// database — the bootstrap-race staging below.
///
/// # Panics
///
/// Panics if building the in-memory database fails.
#[cfg(test)]
async fn open_memory_database() -> TursoDatabase {
    #[expect(
        clippy::expect_used,
        reason = "an in-memory test database always builds"
    )]
    TursoBuilder::new_local(":memory:")
        .build()
        .await
        .expect("failed to build an in-memory test database")
}

/// Opens a connection to a fresh in-memory test database, shared by the per-version schema
/// tests in the `version_x` modules and the workflow tests below.
///
/// # Panics
///
/// Panics if building or connecting to the in-memory database fails.
#[cfg(test)]
pub(crate) async fn open_memory_connection() -> Connection {
    #[expect(
        clippy::expect_used,
        reason = "a fresh in-memory database always connects"
    )]
    open_memory_database()
        .await
        .connect()
        .expect("failed to connect to the in-memory test database")
}

/// Opens a connection to a fresh in-memory test database carrying `schema` — the shared setup of
/// the per-version tests in the `version_x` modules, each passing its own `SCHEMA.schema`.
///
/// # Errors
///
/// Returns an error if the DDL fails to apply.
#[cfg(test)]
pub(crate) async fn open_fresh(schema: &str) -> Result<Connection, Box<dyn StdError>> {
    let connection = open_memory_connection().await;
    connection.execute_batch(schema).await?;

    Ok(connection)
}

/// Asserts that a rejected write failed with a constraint violation — never a missing-table or
/// syntax failure, which would make the rejection meaningless.
///
/// # Panics
///
/// Panics when the write succeeds or fails with any classification but a constraint violation.
#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "a rejection outside the constraint classification fails the test loudly"
)]
pub(crate) fn assert_constraint_violation(outcome: Result<(), turso::Error>, column: &str) {
    match outcome {
        Err(turso::Error::Constraint(_)) => {}

        Err(other) => panic!("{column} rejection must be a constraint violation: {other}"),

        Ok(()) => panic!("{column} value outside its closed domain must be rejected"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, error::Error as StdError, time::Duration};
    use turso_parser::{
        ast::{
            Cmd, ColumnConstraint, ColumnDefinition, CreateTableBody, ForeignKeyClause,
            IndexedColumn, NullsOrder, RefAct, RefArg, SortOrder, SortedColumn, Stmt,
            TableConstraint, Type as AstType,
        },
        parser::Parser,
    };

    /// Verifies that every registry is non-empty, starts at version 1, and increases by exactly
    /// one per version — the invariant that makes every positive version at or below the latest
    /// resolvable by binary search and every version reachable by walking forward.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn schema_versions_are_contiguous_from_one() {
        for schemas in [MAIN_SCHEMAS, API_LOGS_SCHEMAS] {
            assert_eq!(
                first_schema(schemas).version,
                1,
                "schema versions must start at 1",
            );

            for (previous, schema) in schemas.iter().zip(schemas.iter().skip(1)) {
                assert_eq!(
                    schema.version,
                    previous.version + 1,
                    "schema versions must increase by exactly one",
                );
            }
        }
    }

    /// Verifies that every registry's version 1 migration is empty: a fresh database receives the
    /// first version's DDL directly and joins the walk from there, so there is no previous
    /// version to migrate from.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn first_migration_is_empty() {
        for schemas in [MAIN_SCHEMAS, API_LOGS_SCHEMAS] {
            assert_eq!(
                first_schema(schemas).migration,
                "",
                "version 1 has no predecessor, so its migration must be empty",
            );
        }
    }

    /// Verifies that every version's declared schema is exactly what a database built for it
    /// holds: version 1's DDL applies cleanly to a fresh database, and for every later version,
    /// applying its predecessor's DDL followed by its migration to a fresh database yields its
    /// declared schema. Each version builds on its own fresh database, so versions never leak
    /// state into each other. This is the contract [`migrate`] relies on: any database, however
    /// far behind, only ever walks forward through texts this test applies.
    ///
    /// # Panics
    ///
    /// Panics if the resulting database schema does not match the declared schema of its
    /// version.
    ///
    /// # Errors
    ///
    /// Returns an error if a schema or migration text fails to apply, or a stored object's
    /// DDL fails to parse.
    #[tokio::test]
    async fn migrations_produce_their_target_schemas() -> Result<(), Box<dyn StdError>> {
        for schemas in [MAIN_SCHEMAS, API_LOGS_SCHEMAS] {
            let first = first_schema(schemas);
            let connection = open_memory_connection().await;
            connection.execute_batch(first.schema).await?;
            assert_declared_schema(&connection, first).await?;

            for (previous, schema) in schemas.iter().zip(schemas.iter().skip(1)) {
                let connection = open_memory_connection().await;
                connection.execute_batch(previous.schema).await?;
                connection.execute_batch(schema.migration).await?;
                assert_declared_schema(&connection, schema).await?;
            }
        }

        Ok(())
    }

    /// The quota migration preserves immutable definitions and provider foreign keys under
    /// enforcement, and its complete DDL applies independently to a fresh database.
    ///
    /// # Panics
    ///
    /// Panics if preserved bindings, null quota, or structural equivalence diverges.
    ///
    /// # Errors
    ///
    /// Returns database or canonicalization errors.
    #[tokio::test]
    async fn quota_migration_preserves_bound_rows_under_foreign_keys()
    -> Result<(), Box<dyn StdError>> {
        let connection = open_memory_connection().await;
        connection.execute_batch(version_1::SCHEMA.schema).await?;
        connection
            .execute_batch(
                r#"
            PRAGMA foreign_keys = ON;
            INSERT INTO provider_definitions
                (name, version, format, inference, credential, created_at)
                VALUES ('bound', 1, 'openai_response', '{}', '{"type":"api_key"}', 42);
            INSERT INTO providers
                (name, provider_definition_name, provider_definition_version, model_prefix,
                 credentials, is_enabled)
                VALUES ('provider', 'bound', 1, '', '{"type":"api_key","api_key":"secret"}', 0);
            BEGIN;
        "#,
            )
            .await?;
        connection
            .execute_batch(version_2::SCHEMA.migration)
            .await?;
        connection.execute_batch("COMMIT;").await?;
        #[expect(
            clippy::unwrap_used,
            reason = "the seeded provider and definition survive migration"
        )]
        let row = connection
            .query(
                "SELECT p.provider_definition_version, p.is_enabled, d.created_at, d.quota \
             FROM providers p JOIN provider_definitions d \
             ON d.name = p.provider_definition_name AND d.version = p.provider_definition_version",
                (),
            )
            .await?
            .next()
            .await?
            .unwrap();
        assert_eq!(row.get::<i64>(0)?, 1);
        assert_eq!(row.get::<i64>(1)?, 0);
        assert_eq!(row.get::<i64>(2)?, 42);
        assert_eq!(row.get::<Option<String>>(3)?, None);
        assert_declared_schema(&connection, &version_2::SCHEMA).await?;
        let fresh = open_fresh(version_2::SCHEMA.schema).await?;
        assert_declared_schema(&fresh, &version_2::SCHEMA).await?;
        Ok(())
    }

    /// Verifies the full startup workflow on a fresh database for each registry: [`migrate`]
    /// brings `user_version` to the registry's latest version, and a second run over the
    /// now-current database is a no-op. The second run exercises the application-id verification
    /// on an existing database: the stamp the fresh run wrote must satisfy the same registry.
    ///
    /// # Panics
    ///
    /// Panics if the workflow fails or `user_version` does not reach the latest version.
    #[tokio::test]
    async fn migrate_bringup_is_complete_and_idempotent() -> Result<(), Box<dyn StdError>> {
        for registry in [Registry::Main, Registry::ApiLogs] {
            let connection = open_memory_connection().await;

            migrate(&connection, registry, ":memory:").await;
            assert_eq!(
                query_user_version(&connection).await,
                last_schema(registry.schemas()).version,
            );

            migrate(&connection, registry, ":memory:").await;
            assert_eq!(
                query_user_version(&connection).await,
                last_schema(registry.schemas()).version,
            );
        }

        Ok(())
    }

    /// Builds the synthetic two-version main-registry chain the walk tests exercise: version 1
    /// stamps the registry's application id exactly like a real registry's version 1 — so stamp
    /// verification sees what a real stamped database carries — and version 2 adds a column to
    /// the synthetic table.
    fn synthetic_schemas() -> [Schema; 2] {
        [
            Schema {
                version: 1,
                schema: "PRAGMA application_id = 0x4A474D44;\n\
                         CREATE TABLE synthetic (value INTEGER) STRICT;\n",
                migration: "",
            },
            Schema {
                version: 2,
                schema: "PRAGMA application_id = 0x4A474D44;\n\
                         CREATE TABLE synthetic (value INTEGER, label TEXT) STRICT;\n",
                migration: "ALTER TABLE synthetic ADD COLUMN label TEXT;\n",
            },
        ]
    }

    /// Verifies that a fresh database is brought to the latest version through the whole chain —
    /// the first version's DDL, then every later migration — over a synthetic two-version chain,
    /// because the real registry holds a single version and cannot exercise the walk past it. A
    /// second run over the now-current database is a no-op.
    ///
    /// # Panics
    ///
    /// Panics if the workflow fails, the database does not end at the latest version's declared
    /// schema, or the second run changes anything.
    #[tokio::test]
    async fn fresh_database_walks_the_whole_chain_to_the_latest_version()
    -> Result<(), Box<dyn StdError>> {
        let connection = open_memory_connection().await;
        let schemas = synthetic_schemas();

        bring_up_to_latest(&connection, Registry::Main, &schemas, ":memory:").await;
        assert_eq!(
            query_user_version(&connection).await,
            last_schema(&schemas).version,
        );
        assert_declared_schema(&connection, last_schema(&schemas)).await?;

        bring_up_to_latest(&connection, Registry::Main, &schemas, ":memory:").await;
        assert_eq!(
            query_user_version(&connection).await,
            last_schema(&schemas).version,
        );

        Ok(())
    }

    /// Verifies the fresh-bootstrap race: a concurrent bootstrap commits version 1 — DDL, the
    /// application-id stamp, and version advance — into the database while
    /// [`bring_up_to_latest`], whose initial version read already returned 0, waits on the write
    /// lock. The raced bootstrap must join the committed version before walking to the latest
    /// version, instead of re-applying the DDL:
    /// two opens bootstrapping one file at once — an api-log write racing an api-log read —
    /// would otherwise both apply the first version, and the duplicate `CREATE TABLE` of the
    /// first table kills the loser.
    ///
    /// # Panics
    ///
    /// Panics if the workflow fails or the database does not end at the registry's latest version.
    #[tokio::test]
    async fn a_bootstrap_racing_a_committed_fresh_bootstrap_joins_its_version()
    -> Result<(), Box<dyn StdError>> {
        let database = open_memory_database().await;
        let racer = connect_staging(&database).await;
        let raced = connect_staging(&database).await;

        // The racer holds the write lock over written-but-uncommitted version 1, so the raced
        // bootstrap's initial version read still sees 0.
        stage(&racer, "BEGIN IMMEDIATE;").await;
        stage(&racer, version_1::SCHEMA.schema).await;
        stage(&racer, "PRAGMA user_version = 1;").await;

        // The commit lands while the raced bootstrap waits on the lock inside its own
        // transaction, after its initial read.
        tokio::join!(
            async {
                bring_up_to_latest(&raced, Registry::Main, MAIN_SCHEMAS, ":memory:").await;
            },
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                stage(&racer, "COMMIT;").await;
            },
        );

        assert_eq!(
            query_user_version(&raced).await,
            last_schema(MAIN_SCHEMAS).version
        );

        Ok(())
    }

    /// Verifies the migration-walk race: a concurrent bootstrap commits the version-2 migration —
    /// migration text and version advance — into a version-1 database while
    /// [`bring_up_to_latest`], whose initial version read already returned 1, waits on the write
    /// lock. The raced walk must re-check `user_version` inside its own transaction and join the
    /// committed version instead of re-applying the migration: two opens walking one outdated
    /// file at once — an api-log write racing an api-log read — would otherwise both select the
    /// same migration from the stale outer read, and the loser's duplicate `ALTER TABLE` dies in
    /// [`apply_batch`].
    ///
    /// # Panics
    ///
    /// Panics if the workflow fails, the database does not end at version 2, or the database does
    /// not carry version 2's declared schema.
    #[tokio::test]
    async fn a_bootstrap_racing_a_committed_migration_joins_its_version()
    -> Result<(), Box<dyn StdError>> {
        let schemas = synthetic_schemas();
        let database = open_memory_database().await;
        let racer = connect_staging(&database).await;
        let raced = connect_staging(&database).await;

        // The staging database sits at stamped version 1 — exactly what an outdated file carries.
        stage(&racer, first_schema(&schemas).schema).await;
        stage(&racer, "PRAGMA user_version = 1;").await;

        // The racer holds the write lock over staged-but-uncommitted version 2, so the raced
        // bootstrap's initial version read still sees 1 and selects the same migration.
        stage(&racer, "BEGIN IMMEDIATE;").await;
        stage(&racer, last_schema(&schemas).migration).await;
        stage(&racer, "PRAGMA user_version = 2;").await;

        // The commit lands while the raced walk waits on the lock inside its own transaction,
        // after its initial read and stamp verification.
        tokio::join!(
            async {
                bring_up_to_latest(&raced, Registry::Main, &schemas, ":memory:").await;
            },
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                stage(&racer, "COMMIT;").await;
            },
        );

        assert_eq!(query_user_version(&raced).await, 2);
        assert_declared_schema(&raced, last_schema(&schemas)).await?;

        Ok(())
    }

    /// Verifies that the loser of the fresh-bootstrap race still verifies the application id: a
    /// racer committing the api_logs registry's stamped version 1 into a database the bootstrap
    /// is bringing up under the main registry is fatal, naming the opened registry — the race
    /// must not bypass the startup stamp verification.
    ///
    /// # Panics
    ///
    /// Panics if the bootstrap does not panic.
    #[tokio::test]
    #[should_panic(expected = "but the main registry stamps")]
    async fn a_bootstrap_racing_a_foreign_registry_fresh_bootstrap_is_fatal() {
        let database = open_memory_database().await;
        let racer = connect_staging(&database).await;
        let raced = connect_staging(&database).await;

        stage(&racer, "BEGIN IMMEDIATE;").await;
        stage(&racer, api_logs_version_1::SCHEMA.schema).await;
        stage(&racer, "PRAGMA user_version = 1;").await;

        tokio::join!(
            async {
                bring_up_to_latest(&raced, Registry::Main, MAIN_SCHEMAS, ":memory:").await;
            },
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                stage(&racer, "COMMIT;").await;
            },
        );
    }

    /// Connects to the shared staging database of the bootstrap-race tests, with the busy wait
    /// the production bootstrap connection configures — the staged racing transaction holds the
    /// write lock, and the raced bootstrap's transaction begin must wait out its commit.
    ///
    /// # Panics
    ///
    /// Panics if connecting or configuring the connection fails.
    async fn connect_staging(database: &TursoDatabase) -> Connection {
        #[expect(
            clippy::expect_used,
            reason = "a fresh in-memory staging database always connects"
        )]
        let connection = database
            .connect()
            .expect("failed to connect to the staging database");

        #[expect(
            clippy::expect_used,
            reason = "the staging busy timeout always applies"
        )]
        connection
            .busy_timeout(Duration::from_secs(5))
            .expect("failed to configure the staging connection's busy timeout");

        connection
    }

    /// Executes one statement of a staged bootstrap-race scenario.
    ///
    /// # Panics
    ///
    /// Panics if executing the staging statement fails — a broken staging is a test bug, and the
    /// failure is the report.
    async fn stage(connection: &Connection, statement: &str) {
        #[expect(
            clippy::expect_used,
            reason = "a broken staging is a test bug, and the failure is the report"
        )]
        connection
            .execute_batch(statement)
            .await
            .expect("the bootstrap-race staging applies");
    }

    /// Stamps an in-memory database by hand: applies `schema` — DDL plus its `application_id`
    /// stamp — and advances `user_version` to 1, reproducing the database an existing file
    /// carries.
    ///
    /// # Panics
    ///
    /// Panics if the DDL or the `user_version` write fails.
    async fn open_stamped_memory_database(schema: &str) -> Connection {
        let connection = open_memory_connection().await;

        #[expect(
            clippy::expect_used,
            reason = "a registry's schema always applies to a fresh in-memory test database"
        )]
        connection
            .execute_batch(schema)
            .await
            .expect("a registry's schema applies to a fresh in-memory test database");

        #[expect(
            clippy::expect_used,
            reason = "the user_version write always succeeds on a fresh in-memory test database"
        )]
        connection
            .execute_batch("PRAGMA user_version = 1;")
            .await
            .expect("the user_version write succeeds on a fresh in-memory test database");

        connection
    }

    /// Verifies the application-id fatality in the api_logs-under-main direction: a database
    /// stamped for the api_logs registry panics when the bootstrap brings it up under the main
    /// registry, naming the opened registry — instead of passing validation and failing later
    /// with a missing-table error on first query.
    ///
    /// # Panics
    ///
    /// Panics if the bootstrap does not panic.
    #[tokio::test]
    #[should_panic(expected = "but the main registry stamps")]
    async fn an_api_log_stamped_database_is_fatal_under_the_main_registry() {
        let connection = open_stamped_memory_database(api_logs_version_1::SCHEMA.schema).await;

        migrate(&connection, Registry::Main, ":memory:").await;
    }

    /// Verifies the application-id fatality in the mirror direction: a database stamped for the
    /// main registry panics when the bootstrap brings it up under the api_logs registry.
    ///
    /// # Panics
    ///
    /// Panics if the bootstrap does not panic.
    #[tokio::test]
    #[should_panic(expected = "but the api_logs registry stamps")]
    async fn a_main_stamped_database_is_fatal_under_the_api_logs_registry() {
        let connection = open_stamped_memory_database(version_1::SCHEMA.schema).await;

        migrate(&connection, Registry::ApiLogs, ":memory:").await;
    }

    /// Verifies that a database carrying a schema version but no application-id stamp — a file
    /// predating the stamp, from before this change — fails the startup verification like any
    /// other database this build cannot interpret; the operator recreates the file.
    ///
    /// # Panics
    ///
    /// Panics if the bootstrap does not panic.
    #[tokio::test]
    #[should_panic(expected = "but the main registry stamps")]
    async fn a_database_predating_the_application_id_stamp_is_fatal() {
        let connection = open_memory_connection().await;

        // The era's layout without the stamp: one table, a stamped version, and the default
        // application id.
        #[expect(
            clippy::expect_used,
            reason = "the synthetic pre-stamp layout always applies to a fresh test database"
        )]
        connection
            .execute_batch(
                "CREATE TABLE pre_stamp (value INTEGER) STRICT;
                 PRAGMA user_version = 1;",
            )
            .await
            .expect("the pre-stamp layout applies to a fresh test database");

        migrate(&connection, Registry::Main, ":memory:").await;
    }

    /// Asserts that the database's current schema objects match the declared schema of `schema`.
    ///
    /// # Panics
    ///
    /// Panics if the comparison fails.
    async fn assert_declared_schema(
        connection: &Connection,
        schema: &Schema,
    ) -> Result<(), Box<dyn StdError>> {
        assert_eq!(
            database_object_lines(connection).await?,
            ddl_object_lines(schema.schema)?,
            "database schema does not match the declared schema of version {}",
            schema.version,
        );

        Ok(())
    }

    /// Renders a DDL text into its canonical object lines.
    ///
    /// # Errors
    ///
    /// Returns an error if the text fails to parse.
    fn ddl_object_lines(ddl: &str) -> Result<BTreeSet<String>, Box<dyn StdError>> {
        let mut lines = BTreeSet::new();

        for command in Parser::new(ddl.as_bytes()) {
            match command? {
                Cmd::Stmt(statement) => lines.extend(statement_lines(&statement)),
                Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. } => {
                    return Err("EXPLAIN is not a schema statement".into());
                }
            }
        }

        Ok(lines)
    }

    /// Reads the database's schema objects and renders them into canonical object lines.
    ///
    /// Internal bookkeeping objects are excluded: SQLite's own `sqlite_%` tables and Turso's
    /// `__turso_internal_*` bookkeeping tables (which its AUTOINCREMENT implementation
    /// materializes beside the declared tables), neither of which the declared DDL spells.
    ///
    /// # Errors
    ///
    /// Returns an error if querying `sqlite_schema` or parsing a stored object's DDL fails.
    async fn database_object_lines(
        connection: &Connection,
    ) -> Result<BTreeSet<String>, Box<dyn StdError>> {
        let mut rows = connection
            .query("SELECT name, sql FROM sqlite_schema ORDER BY name", ())
            .await?;

        let mut lines = BTreeSet::new();

        while let Some(row) = rows.next().await? {
            let name: String = row.get(0)?;

            if name.starts_with("sqlite_") || name.starts_with("__turso_internal_") {
                continue;
            }

            let sql: String = row.get(1)?;
            lines.extend(statement_lines(&parse_single_statement(&sql)?));
        }

        Ok(lines)
    }

    /// Parses exactly one SQL statement.
    ///
    /// # Errors
    ///
    /// Returns an error if the text fails to parse or carries more than one statement.
    fn parse_single_statement(sql: &str) -> Result<Stmt, Box<dyn StdError>> {
        let mut parser = Parser::new(sql.as_bytes());

        let statement = match parser.next() {
            Some(Ok(Cmd::Stmt(statement))) => statement,
            Some(Ok(Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. })) => {
                return Err("expected a DDL statement".into());
            }
            Some(Err(error)) => return Err(Box::new(error)),
            None => return Err("expected one statement, found none".into()),
        };

        if parser.next().is_some() {
            return Err("expected one statement, found more".into());
        }

        Ok(statement)
    }

    /// Renders one parsed statement into its canonical object lines.
    ///
    /// The lines describe the statement semantically, not textually: constraint order inside a
    /// column or table definition is irrelevant (each constraint renders as its own line, and the
    /// returned set orders them), an explicit `NULL` constraint is indistinguishable from an
    /// absent one, and declared types compare case-insensitively. Everything else — object names,
    /// column order, expressions, foreign keys, index definitions — compares exactly.
    ///
    /// # Panics
    ///
    /// Panics on statements outside the schema's vocabulary (`CREATE VIEW`, `CREATE TRIGGER`,
    /// generated columns, `CREATE TABLE ... AS SELECT`); the comparison grows when the schema does.
    fn statement_lines(statement: &Stmt) -> Vec<String> {
        match statement {
            Stmt::CreateTable {
                tbl_name,
                body:
                    CreateTableBody::ColumnsAndConstraints {
                        columns,
                        constraints,
                        options,
                    },
                ..
            } => {
                let table = tbl_name.name.to_string();

                let mut lines = vec![format!(
                    "table {table} strict={} without-rowid={}",
                    options.strict_text.is_some(),
                    options.without_rowid_text.is_some(),
                )];

                for (position, column) in columns.iter().enumerate() {
                    lines.extend(column_lines(&table, position, column));
                }

                for constraint in constraints {
                    lines.extend(table_constraint_lines(&table, &constraint.constraint));
                }

                lines
            }

            Stmt::CreateIndex {
                unique,
                idx_name,
                tbl_name,
                columns,
                where_clause,
                ..
            } => {
                let mut line = format!(
                    "index {} table {tbl_name} unique={unique} columns=({})",
                    idx_name.name,
                    columns
                        .iter()
                        .map(sorted_column_text)
                        .collect::<Vec<String>>()
                        .join(", "),
                );

                if let Some(predicate) = where_clause {
                    line.push_str(&format!(" where={predicate}"));
                }

                vec![line]
            }

            // The application-id pragma is header state, not a schema object: the structural
            // comparison covers stored objects only, and the header's stamp is never compared.
            Stmt::Pragma { .. } => Vec::new(),

            _ => {
                #[expect(
                    clippy::panic,
                    reason = "a statement outside the schema's vocabulary fails the comparison \
                              loudly; the canonicalizer grows with the schema"
                )]
                {
                    panic!("unsupported schema statement: {statement:?}");
                }
            }
        }
    }

    /// Renders one column definition, including its ordinal position — column order is semantic —
    /// into a main line plus one line per value-carrying or check constraint.
    ///
    /// # Panics
    ///
    /// Panics on generated columns; see [`statement_lines`].
    fn column_lines(table: &str, position: usize, column: &ColumnDefinition) -> Vec<String> {
        let name = column.col_name.to_string();

        let mut line = format!(
            "column[{position}] {table}.{name} type={}",
            column
                .col_type
                .as_ref()
                .map_or("<none>".to_string(), canonical_type),
        );

        let mut is_primary_key = false;
        let mut is_autoincrement = false;
        let mut is_not_null = false;
        let mut is_unique = false;
        let mut lines = Vec::new();

        for constraint in &column.constraints {
            match &constraint.constraint {
                ColumnConstraint::PrimaryKey { auto_increment, .. } => {
                    is_primary_key = true;
                    is_autoincrement = *auto_increment;
                }

                // An explicit `NULL` is the default state and contributes nothing; `NOT NULL` is
                // the constraint. Turso's ALTER TABLE rewriting also drops explicit `NULL`
                // constraints from the stored schema text, so treating them as absent keeps a
                // migrated database comparable to its declared schema.
                ColumnConstraint::NotNull { nullable, .. } => {
                    is_not_null |= !nullable;
                }

                ColumnConstraint::Unique(_) => is_unique = true,

                ColumnConstraint::Check { expr, .. } => {
                    lines.push(format!("column-check {table}.{name} {expr}"));
                }

                ColumnConstraint::Default(expr) => {
                    lines.push(format!("column-default {table}.{name} {expr}"));
                }

                ColumnConstraint::Collate { collation_name } => {
                    lines.push(format!("column-collate {table}.{name} {collation_name}",));
                }

                ColumnConstraint::ForeignKey { clause, .. } => {
                    lines.push(format!(
                        "column-foreign-key {table}.{name} {}",
                        foreign_key_clause_text(clause),
                    ));
                }

                ColumnConstraint::Generated { .. } => {
                    #[expect(
                        clippy::panic,
                        reason = "a generated column is not part of the gateway schema; the \
                                  canonicalizer grows with the schema"
                    )]
                    {
                        panic!("generated columns are not part of the gateway schema");
                    }
                }
            }
        }

        if is_primary_key {
            line.push_str(" primary-key");
        }

        if is_autoincrement {
            line.push_str(" autoincrement");
        }

        if is_not_null {
            line.push_str(" not-null");
        }

        if is_unique {
            line.push_str(" unique");
        }

        lines.insert(0, line);

        lines
    }

    /// Renders one table-level constraint into its canonical line.
    fn table_constraint_lines(table: &str, constraint: &TableConstraint) -> Vec<String> {
        match constraint {
            TableConstraint::PrimaryKey { columns, .. } => vec![format!(
                "table-primary-key {table} ({})",
                sorted_columns_text(columns),
            )],

            TableConstraint::Unique { columns, .. } => vec![format!(
                "table-unique {table} ({})",
                sorted_columns_text(columns),
            )],

            TableConstraint::Check { expr, .. } => {
                vec![format!("table-check {table} {expr}")]
            }

            TableConstraint::ForeignKey {
                columns, clause, ..
            } => vec![format!(
                "table-foreign-key {table} ({}) {}",
                columns
                    .iter()
                    .map(|column| column.col_name.to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
                foreign_key_clause_text(clause),
            )],
        }
    }

    /// Renders a foreign-key clause canonically: the referenced table and columns plus the sorted
    /// referential actions.
    fn foreign_key_clause_text(clause: &ForeignKeyClause) -> String {
        let mut actions = clause
            .args
            .iter()
            .map(|arg| match arg {
                RefArg::OnDelete(action) => format!("on-delete {}", ref_action_text(action)),
                RefArg::OnInsert(action) => format!("on-insert {}", ref_action_text(action)),
                RefArg::OnUpdate(action) => format!("on-update {}", ref_action_text(action)),
                RefArg::Match(name) => format!("match {name}"),
            })
            .collect::<Vec<String>>();

        actions.sort();

        format!(
            "references {}({}) {}",
            clause.tbl_name,
            clause
                .columns
                .iter()
                .map(|column: &IndexedColumn| column.col_name.to_string())
                .collect::<Vec<String>>()
                .join(", "),
            actions.join(" "),
        )
    }

    /// Renders a referential action.
    fn ref_action_text(action: &RefAct) -> &'static str {
        match action {
            RefAct::SetNull => "set-null",
            RefAct::SetDefault => "set-default",
            RefAct::Cascade => "cascade",
            RefAct::Restrict => "restrict",
            RefAct::NoAction => "no-action",
        }
    }

    /// Joins sorted columns — the column list of a table-level key constraint.
    fn sorted_columns_text(columns: &[SortedColumn]) -> String {
        columns
            .iter()
            .map(sorted_column_text)
            .collect::<Vec<String>>()
            .join(", ")
    }

    /// Renders one sorted column — an expression with optional sort and nulls ordering.
    fn sorted_column_text(column: &SortedColumn) -> String {
        let mut text = column.expr.to_string();

        if let Some(order) = &column.order {
            text.push_str(match order {
                SortOrder::Asc => " asc",
                SortOrder::Desc => " desc",
            });
        }

        if let Some(nulls) = &column.nulls {
            text.push_str(match nulls {
                NullsOrder::First => " nulls-first",
                NullsOrder::Last => " nulls-last",
            });
        }

        text
    }

    /// Renders a declared type canonically: uppercased name, with a size suffix when present.
    fn canonical_type(ty: &AstType) -> String {
        let mut text = ty.name.to_ascii_uppercase();

        if let Some(size) = &ty.size {
            text.push_str(&format!("({size:?})"));
        }

        text
    }
}
