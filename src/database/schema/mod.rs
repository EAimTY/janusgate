//! Versioned database schema definitions and the startup migration workflow.
//!
//! The schema is versioned linearly: every modification to the database schema is a new version,
//! defined by one `version_x.rs` submodule whose only item is `const SCHEMA: Schema` and
//! registered in [`SCHEMAS`]. A version's `schema` field carries the complete DDL of that version
//! as hand-written SQL, and its `migration` field carries the SQL that transforms the previous
//! version's database into it.
//!
//! [`migrate`] runs at startup: it reads `PRAGMA user_version`, applies the first version's DDL to
//! a fresh database and walks forward from there, applying one migration per version, each
//! committed atomically together with its `user_version` advance — so a fresh database and an
//! outdated one both end at the latest version.
//!
//! Migration texts run under foreign key enforcement: the bootstrap connection enables
//! `PRAGMA foreign_keys` before the walk, and each text executes inside its own transaction, where
//! SQLite defines the pragma as a no-op (Turso currently accepting it mid-transaction is a
//! divergence from SQLite that may be aligned at any time). No migration may therefore rely on
//! disabling enforcement; every migration must be authored to hold under it statement by
//! statement — a table rebuild, for instance, orders its steps so referential integrity holds at
//! every intermediate state.

mod version_1;

use log::{debug, info};
#[cfg(test)]
use turso::Builder as TursoBuilder;
use turso::Connection;

/// One version of the SQLite database schema.
///
/// Both text fields are statement sequences in which every statement is terminated by `;`; the
/// workflow embeds them into its transaction batches.
struct Schema {
    /// The version's number, stored in `PRAGMA user_version`.
    ///
    /// Versions start at 1 and increase by exactly one per version; the ordering test enforces
    /// this over [`SCHEMAS`].
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

/// Every schema version, ordered by ascending version starting at 1 with no gaps.
///
/// [`migrate`] binary-searches this table by version; the contiguity invariant — which makes every
/// positive version at or below the latest resolvable — is enforced by tests.
const SCHEMAS: &[Schema] = &[version_1::SCHEMA];

/// Brings the schema of the database behind `connection` up to the latest known version, logging
/// progress under the database's `path`.
///
/// Delegates to [`bring_up_to_latest`] over [`SCHEMAS`]; see that function for the workflow.
///
/// # Panics
///
/// Panics under the same conditions as [`bring_up_to_latest`].
pub(super) async fn migrate(connection: &Connection, path: &str) {
    bring_up_to_latest(connection, SCHEMAS, path).await;
}

/// Walks `schemas` — schema versions in ascending order — forward from the database's stored
/// version, bringing the database behind `connection` to `schemas`' latest version and logging
/// progress under the database's `path`.
///
/// The current version is read from `PRAGMA user_version`. A fresh database (`0`) receives the
/// first version's DDL and joins the walk from there — so a fresh database ends at the latest
/// version, however many exist, through the same later migrations an outdated database applies.
/// A database at a known version applies the migrations of every later version in order; a
/// database already at the latest version does nothing. Each step — one schema or migration text
/// plus its `user_version` write — commits atomically, so a crash mid-chain leaves the database
/// on its last committed version and the next startup resumes from there.
///
/// The schema table is a parameter rather than [`SCHEMAS`] itself so tests can exercise the walk
/// over chains with more versions than the registry currently contains.
///
/// # Panics
///
/// Panics if reading `user_version`, applying a schema or migration batch, or advancing
/// `user_version` fails, or if the stored version is negative or newer than the latest known
/// version — a database this build cannot interpret.
async fn bring_up_to_latest(connection: &Connection, schemas: &[Schema], path: &str) {
    let mut current_version = query_user_version(connection).await;

    if current_version == 0 {
        // A fresh database receives the first version's DDL and joins the walk from there, exactly
        // like a database already at version 1 — whatever later versions exist still apply.
        let first = first_schema(schemas);
        apply(connection, first.schema, first.version, path).await;
        current_version = first.version;
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

/// Applies a schema or migration text together with its `user_version` advance in one atomic
/// transaction and logs the step.
///
/// The text and the version write must commit atomically: a crash that persisted only part of a
/// migration would leave the database between versions, which no build can resume.
///
/// # Panics
///
/// Panics if executing the batch fails.
async fn apply(connection: &Connection, text: &str, version: i64, path: &str) {
    let batch = format!("BEGIN IMMEDIATE;\n{text}PRAGMA user_version = {version};\nCOMMIT;",);

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

/// Opens a connection to a fresh in-memory test database, shared by the per-version schema
/// tests in the `version_x` modules and the workflow tests below.
///
/// # Panics
///
/// Panics if building or connecting to the in-memory database fails.
#[cfg(test)]
pub(super) async fn open_memory_connection() -> Connection {
    #[expect(
        clippy::expect_used,
        reason = "an in-memory test database always builds"
    )]
    let database = TursoBuilder::new_local(":memory:")
        .build()
        .await
        .expect("failed to build an in-memory test database");

    #[expect(
        clippy::expect_used,
        reason = "a fresh in-memory database always connects"
    )]
    let connection = database
        .connect()
        .expect("failed to connect to the in-memory test database");

    connection
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, error::Error as StdError};
    use turso_parser::{
        ast::{
            Cmd, ColumnConstraint, ColumnDefinition, CreateTableBody, ForeignKeyClause,
            IndexedColumn, NullsOrder, RefAct, RefArg, SortOrder, SortedColumn, Stmt,
            TableConstraint, Type as AstType,
        },
        parser::Parser,
    };

    /// Verifies that [`SCHEMAS`] is non-empty, starts at version 1, and increases by exactly one
    /// per version — the invariant that makes every positive version at or below the latest
    /// resolvable by binary search and every version reachable by walking forward.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn schema_versions_are_contiguous_from_one() {
        assert_eq!(
            first_schema(SCHEMAS).version,
            1,
            "schema versions must start at 1",
        );

        for (previous, schema) in SCHEMAS.iter().zip(SCHEMAS.iter().skip(1)) {
            assert_eq!(
                schema.version,
                previous.version + 1,
                "schema versions must increase by exactly one",
            );
        }
    }

    /// Verifies that version 1's migration is empty: a fresh database receives the first
    /// version's DDL directly and joins the walk from there, so there is no previous version
    /// to migrate from.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn first_migration_is_empty() {
        assert_eq!(
            first_schema(SCHEMAS).migration,
            "",
            "version 1 has no predecessor, so its migration must be empty",
        );
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
        let first = first_schema(SCHEMAS);
        let connection = open_memory_connection().await;
        connection.execute_batch(first.schema).await?;
        assert_declared_schema(&connection, first).await?;

        for (previous, schema) in SCHEMAS.iter().zip(SCHEMAS.iter().skip(1)) {
            let connection = open_memory_connection().await;
            connection.execute_batch(previous.schema).await?;
            connection.execute_batch(schema.migration).await?;
            assert_declared_schema(&connection, schema).await?;
        }

        Ok(())
    }

    /// Verifies the full startup workflow on a fresh database: [`migrate`] brings `user_version`
    /// to the latest version, and a second run over the now-current database is a no-op.
    ///
    /// # Panics
    ///
    /// Panics if the workflow fails or `user_version` does not reach the latest version.
    #[tokio::test]
    async fn migrate_bringup_is_complete_and_idempotent() -> Result<(), Box<dyn StdError>> {
        let connection = open_memory_connection().await;

        migrate(&connection, ":memory:").await;
        assert_eq!(
            query_user_version(&connection).await,
            last_schema(SCHEMAS).version,
        );

        migrate(&connection, ":memory:").await;
        assert_eq!(
            query_user_version(&connection).await,
            last_schema(SCHEMAS).version,
        );

        Ok(())
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

        let schemas = [
            Schema {
                version: 1,
                schema: "CREATE TABLE synthetic (value INTEGER) STRICT;\n",
                migration: "",
            },
            Schema {
                version: 2,
                schema: "CREATE TABLE synthetic (value INTEGER, label TEXT) STRICT;\n",
                migration: "ALTER TABLE synthetic ADD COLUMN label TEXT;\n",
            },
        ];

        bring_up_to_latest(&connection, &schemas, ":memory:").await;
        assert_eq!(
            query_user_version(&connection).await,
            last_schema(&schemas).version,
        );
        assert_declared_schema(&connection, last_schema(&schemas)).await?;

        bring_up_to_latest(&connection, &schemas, ":memory:").await;
        assert_eq!(
            query_user_version(&connection).await,
            last_schema(&schemas).version,
        );

        Ok(())
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
    /// # Errors
    ///
    /// Returns an error if querying `sqlite_schema` or parsing a stored object's DDL fails.
    async fn database_object_lines(
        connection: &Connection,
    ) -> Result<BTreeSet<String>, Box<dyn StdError>> {
        let mut rows = connection
            .query(
                "SELECT sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
                (),
            )
            .await?;

        let mut lines = BTreeSet::new();

        while let Some(row) = rows.next().await? {
            let sql: String = row.get(0)?;
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
