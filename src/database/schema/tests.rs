//! Schema registry tests: every registered version applies to a fresh file and walks forward to
//! the latest version, an earlier version's seed rows survive the walk unchanged, and a file this
//! build cannot interpret — an unknown stored version, a foreign or missing application-id stamp,
//! a wrong registry — answers the typed schema drift. The applied schema's declared constraint
//! behavior (foreign-key enforcement, `STRICT` typing) is exercised against the real database as
//! well.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or list is that test's failure report"
)]

use super::{
    API_LOGS_APPLICATION_ID, API_LOGS_SCHEMAS, MAIN_APPLICATION_ID, MAIN_SCHEMAS, Registry, Schema,
};
use crate::database::{
    Database, Error as DatabaseError, MAIN_POOL_WAIT, TursoError, main_pool_connections,
};
use std::{
    error::Error as StdError,
    path::{Path, PathBuf},
};
use turso::Builder as TursoBuilder;

/// Removes the guarded database file and its engine sidecar files on drop.
struct DatabaseFile(PathBuf);

impl Drop for DatabaseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);

        for suffix in ["-wal", "-shm", "-tshm"] {
            let mut sidecar = self.0.clone().into_os_string();
            sidecar.push(suffix);
            let _ = std::fs::remove_file(sidecar);
        }
    }
}

/// Builds the guarded path of a fresh test database file.
fn fresh_file(label: &str) -> DatabaseFile {
    DatabaseFile(std::env::temp_dir().join(format!(
        "janusgate-schema-test-{}-{label}.db",
        rand::random::<u64>()
    )))
}

/// Opens the file at `path` under `registry`: the pooled open whose bootstrap runs the full
/// verification-and-migration walk every test here drives.
async fn open(path: &Path, registry: Registry) -> Result<Database, DatabaseError> {
    Database::open(path, registry, main_pool_connections(), MAIN_POOL_WAIT).await
}

/// Stamps one pragma statement onto the existing database file at `path` through its own pooled
/// open, dropping the handle before returning so the next open reads the mutated header.
async fn stamp_pragma(
    path: &Path,
    registry: Registry,
    statement: &str,
) -> Result<(), Box<dyn StdError>> {
    let database = open(path, registry).await?;

    database.execute(statement, ()).await?;

    drop(database);

    Ok(())
}

/// Applies `schemas` up to and including `target_version` to the empty file at `path` through a
/// raw engine connection — one immediate transaction per version, exactly the shape the bootstrap
/// walk commits — then runs `seed_sql` against the resulting file when nonempty.
async fn build_seeded_file(
    path: &Path,
    schemas: &[Schema],
    target_version: i64,
    seed_sql: &str,
) -> Result<(), Box<dyn StdError>> {
    let path_str = path.to_str().expect("fixture paths are UTF-8");

    let database = TursoBuilder::new_local(path_str).build().await?;

    let connection = database.connect()?;

    for schema in schemas {
        if schema.version > target_version {
            break;
        }

        let text = if schema.version == 1 {
            schema.schema
        } else {
            schema.migration
        };

        connection
            .execute_batch(format!(
                "BEGIN IMMEDIATE; {text}PRAGMA user_version = {};\nCOMMIT;",
                schema.version
            ))
            .await?;
    }

    connection.execute_batch(seed_sql).await?;

    drop(connection);

    Ok(())
}

/// The seed rows valid at main schema version `version`: one definition version, the provider
/// bound to it, and the provider's physical model. A new schema version extends this match with
/// its own seed statements, so the forward-migration walk keeps exercising row preservation from
/// every earlier version.
fn main_seed_sql(version: i64) -> &'static str {
    match version {
        1 => {
            r###"
            INSERT INTO provider_definitions (name, version, format, inference, credential,
                provisioning, discovery, compaction, created_at, quota)
            VALUES ('seed-definition', 1, 'openai_chat_completion',
                '{"url":[],"upstream_streaming":"match_client"}', '{"type":"api_key"}',
                NULL, NULL, NULL, 1728000000000000, NULL);

            INSERT INTO providers (name, provider_definition_name, provider_definition_version,
                is_enabled, base_url, credentials, model_prefix)
            VALUES ('seed-provider', 'seed-definition', 1, 1, NULL,
                '{"type":"api_key","api_key":"sk-seed"}', 'seed.');

            INSERT INTO models (provider_id, upstream_id, is_enabled, discovery_state)
            VALUES (1, 'seed-model', 1, 'discovered');
        "###
        }

        _ => "",
    }
}

/// The seed row valid at api-log schema version `version`. A new api-log schema version extends
/// this match with its own seed statements.
fn api_logs_seed_sql(version: i64) -> &'static str {
    match version {
        1 => {
            r###"
            INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, model_route_id,
                upstream_http_status, endpoint_type, is_stream, http_status, outcome, duration,
                input_tokens, output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens)
            VALUES (1728000000000000, 11, 22, 33, 44, 200, 'openai_chat_completions', 0, 200,
                'completed', 1500, 10, 20, NULL, NULL, NULL);
        "###
        }

        _ => "",
    }
}

/// Reads the single integer scalar the pragma or query produces.
async fn scalar(database: &Database, sql: &str) -> Result<i64, Box<dyn StdError>> {
    let rows = database.query(sql, ()).await?;

    let value = rows
        .first()
        .map(|row| row.get::<i64>(0))
        .transpose()?
        .expect("the probe yields its single scalar row");

    Ok(value)
}

/// Asserts the database carries `expected_tables` among its stored tables.
async fn assert_tables(
    database: &Database,
    expected_tables: &[&str],
) -> Result<(), Box<dyn StdError>> {
    let rows = database
        .query("SELECT name FROM sqlite_master WHERE type = 'table'", ())
        .await?;

    let mut names = Vec::new();

    for row in &rows {
        names.push(row.get::<String>(0)?);
    }

    for table in expected_tables {
        assert!(
            names.iter().any(|name| name == table),
            "table {table} is missing from the applied schema: {names:?}"
        );
    }

    Ok(())
}

/// The fresh-database walk body shared by both registries: one open brings the empty file to the
/// latest version, stamps the registry's application id, and stores every declared table.
async fn fresh_database_walks_to_latest(
    registry: Registry,
    schemas: &[Schema],
    application_id: i64,
    expected_tables: &[&str],
) -> Result<(), Box<dyn StdError>> {
    let latest = schemas.last().expect("the registry carries version 1");

    let file = fresh_file("fresh");

    let database = open(&file.0, registry).await?;

    assert_eq!(
        scalar(&database, "PRAGMA user_version").await?,
        latest.version,
        "a fresh database ends at the latest registered version"
    );

    assert_eq!(
        scalar(&database, "PRAGMA application_id").await?,
        application_id,
        "the registry's application-id stamp is written by the first version's DDL"
    );

    assert_tables(&database, expected_tables).await?;

    Ok(())
}

/// Every version of the main database applies to a fresh file and walks forward to the latest
/// version, and the starting version's seed rows survive the walk unchanged.
#[tokio::test]
async fn main_fresh_database_walks_to_latest_version() -> Result<(), Box<dyn StdError>> {
    fresh_database_walks_to_latest(
        Registry::Main,
        MAIN_SCHEMAS,
        MAIN_APPLICATION_ID,
        &[
            "provider_definitions",
            "providers",
            "models",
            "provider_pools",
            "provider_pool_members",
            "model_routes",
            "provider_quota_blocks",
            "ongoing_oauth_flows",
            "api_keys",
            "api_key_provider_defaults",
            "api_key_pool_defaults",
            "api_key_model_route_overrides",
        ],
    )
    .await
}

/// Every version of the api-log registry applies to a fresh file and walks forward to the latest
/// version.
#[tokio::test]
async fn api_logs_fresh_database_walks_to_latest_version() -> Result<(), Box<dyn StdError>> {
    fresh_database_walks_to_latest(
        Registry::ApiLogs,
        API_LOGS_SCHEMAS,
        API_LOGS_APPLICATION_ID,
        &["api_logs"],
    )
    .await
}

/// Every registered main version applies to a fresh file and a database at that version walks
/// forward to the latest, with the version's seed rows surviving the walk unchanged.
#[tokio::test]
async fn main_every_version_walks_forward_preserving_rows() -> Result<(), Box<dyn StdError>> {
    every_version_walks_forward_preserving_rows(
        Registry::Main,
        MAIN_SCHEMAS,
        main_seed_sql,
        "SELECT COUNT(*) FROM models WHERE upstream_id = 'seed-model'",
    )
    .await
}

/// Every registered api-log version applies to a fresh file and a database at that version walks
/// forward to the latest, with the version's seed row surviving the walk unchanged.
#[tokio::test]
async fn api_logs_every_version_walks_forward_preserving_rows() -> Result<(), Box<dyn StdError>> {
    every_version_walks_forward_preserving_rows(
        Registry::ApiLogs,
        API_LOGS_SCHEMAS,
        api_logs_seed_sql,
        "SELECT COUNT(*) FROM api_logs WHERE outcome = 'completed'",
    )
    .await
}

/// The forward-walk body shared by both registries: for every version, a file built at that
/// version through the registry's own texts and seeded with representative rows opens under the
/// registry, ends at the latest version, and still answers the seed probe.
async fn every_version_walks_forward_preserving_rows(
    registry: Registry,
    schemas: &[Schema],
    seed_sql: fn(i64) -> &'static str,
    seed_probe: &str,
) -> Result<(), Box<dyn StdError>> {
    let latest = schemas.last().expect("the registry carries version 1");

    for schema in schemas {
        let file = fresh_file("walk");

        build_seeded_file(&file.0, schemas, schema.version, seed_sql(schema.version)).await?;

        let database = open(&file.0, registry).await?;

        assert_eq!(
            scalar(&database, "PRAGMA user_version").await?,
            latest.version,
            "the walk from version {} reaches the latest version",
            schema.version
        );

        assert_eq!(
            scalar(&database, "PRAGMA application_id").await?,
            registry.application_id(),
            "the walk from version {} keeps the registry stamp",
            schema.version
        );

        assert_eq!(
            scalar(&database, seed_probe).await?,
            1,
            "the version-{} seed row survives the walk",
            schema.version
        );
    }

    Ok(())
}

/// A main database stored at a version newer than this build knows answers the typed schema
/// drift at open.
#[tokio::test]
async fn unknown_stored_version_answers_schema_drift() -> Result<(), Box<dyn StdError>> {
    let file = fresh_file("unknown-version");

    open(&file.0, Registry::Main).await?;

    stamp_pragma(&file.0, Registry::Main, "PRAGMA user_version = 999").await?;

    let outcome = open(&file.0, Registry::Main).await;

    assert!(
        matches!(
            &outcome,
            Err(DatabaseError::SchemaDrift { detail })
                if detail.contains("unknown schema version 999")
        ),
        "expected typed schema drift for the unknown version, got {outcome:?}"
    );

    Ok(())
}

/// A negative stored version is outside the contiguous positive version chain and answers the
/// typed schema drift.
#[tokio::test]
async fn negative_stored_version_answers_schema_drift() -> Result<(), Box<dyn StdError>> {
    let file = fresh_file("negative-version");

    open(&file.0, Registry::Main).await?;

    stamp_pragma(&file.0, Registry::Main, "PRAGMA user_version = -1").await?;

    let outcome = open(&file.0, Registry::Main).await;

    assert!(
        matches!(
            &outcome,
            Err(DatabaseError::SchemaDrift { detail })
                if detail.contains("unknown schema version -1")
        ),
        "expected typed schema drift for the negative version, got {outcome:?}"
    );

    Ok(())
}

/// A file carrying a foreign application-id stamp answers the typed schema drift under its own
/// registry: the stamp names the file's registry identity and a disagreement means the file
/// belongs elsewhere.
#[tokio::test]
async fn foreign_application_id_answers_schema_drift() -> Result<(), Box<dyn StdError>> {
    let file = fresh_file("foreign-stamp");

    open(&file.0, Registry::Main).await?;

    stamp_pragma(&file.0, Registry::Main, "PRAGMA application_id = 305419896").await?;

    let outcome = open(&file.0, Registry::Main).await;

    assert!(
        matches!(
            &outcome,
            Err(DatabaseError::SchemaDrift { detail })
                if detail.contains("application_id")
        ),
        "expected typed schema drift for the foreign stamp, got {outcome:?}"
    );

    Ok(())
}

/// A file carrying a schema version but the default application id — the pre-stamp development
/// artifact — answers the typed schema drift: the stamp is written by version 1's own SQL and a
/// versioned file without it predates this registry.
#[tokio::test]
async fn unstamped_versioned_file_answers_schema_drift() -> Result<(), Box<dyn StdError>> {
    let file = fresh_file("pre-stamp");

    open(&file.0, Registry::Main).await?;

    stamp_pragma(&file.0, Registry::Main, "PRAGMA application_id = 0").await?;

    let outcome = open(&file.0, Registry::Main).await;

    assert!(
        matches!(&outcome, Err(DatabaseError::SchemaDrift { .. })),
        "expected typed schema drift for the pre-stamp file, got {outcome:?}"
    );

    Ok(())
}

/// A main-registry file opened under the api-log registry — and an api-log file opened under the
/// main registry — both answer the typed schema drift instead of surfacing as missing tables at
/// first query.
#[tokio::test]
async fn wrong_registry_answers_schema_drift() -> Result<(), Box<dyn StdError>> {
    let main_file = fresh_file("main-as-api-logs");

    open(&main_file.0, Registry::Main).await?;

    let main_as_api_logs = open(&main_file.0, Registry::ApiLogs).await;

    assert!(
        matches!(&main_as_api_logs, Err(DatabaseError::SchemaDrift { .. })),
        "expected typed schema drift for the main file under the api-log registry, \
         got {main_as_api_logs:?}"
    );

    let api_logs_file = fresh_file("api-logs-as-main");

    open(&api_logs_file.0, Registry::ApiLogs).await?;

    let api_logs_as_main = open(&api_logs_file.0, Registry::Main).await;

    assert!(
        matches!(&api_logs_as_main, Err(DatabaseError::SchemaDrift { .. })),
        "expected typed schema drift for the api-log file under the main registry, \
         got {api_logs_as_main:?}"
    );

    Ok(())
}

/// Foreign keys are enforced on the applied schema: a child row with a missing parent is
/// refused, an `ON DELETE RESTRICT` parent refuses deletion while referenced, and `ON DELETE
/// CASCADE` deletions remove the children.
#[tokio::test]
async fn foreign_keys_are_enforced_on_the_applied_schema() -> Result<(), Box<dyn StdError>> {
    let file = fresh_file("foreign-keys");

    let database = open(&file.0, Registry::Main).await?;

    for statement in [
        "INSERT INTO provider_definitions (name, version, format, inference, credential, \
             provisioning, discovery, compaction, created_at, quota) \
             VALUES ('def', 1, 'openai_chat_completion', \
             '{\"url\":[],\"upstream_streaming\":\"match_client\"}', '{\"type\":\"api_key\"}', \
             NULL, NULL, NULL, 1, NULL)",
        "INSERT INTO providers (name, provider_definition_name, provider_definition_version, \
             is_enabled, base_url, credentials, model_prefix) \
             VALUES ('acct', 'def', 1, 1, NULL, \
             '{\"type\":\"api_key\",\"api_key\":\"k\"}', '')",
    ] {
        database.execute(statement, ()).await?;
    }

    // A model referencing an absent provider is refused.
    let orphan = database
        .execute(
            "INSERT INTO models (provider_id, upstream_id, discovery_state) \
             VALUES (9999, 'orphan', 'discovered')",
            (),
        )
        .await;

    assert!(
        matches!(
            &orphan,
            Err(DatabaseError::Turso(TursoError::Constraint(_)))
        ),
        "expected a foreign-key constraint failure for the orphan model, got {orphan:?}"
    );

    // The referenced definition version refuses deletion (`ON DELETE RESTRICT`).
    let restricted = database
        .execute("DELETE FROM provider_definitions WHERE name = 'def'", ())
        .await;

    assert!(
        matches!(
            &restricted,
            Err(DatabaseError::Turso(TursoError::Constraint(_)))
        ),
        "expected a restrict failure for the referenced definition, got {restricted:?}"
    );

    // Deleting the provider cascades its model, its route, its pool membership, its quota
    // blocks, and its key-policy rules.
    for statement in [
        "INSERT INTO models (provider_id, upstream_id, discovery_state) \
             VALUES (1, 'm', 'discovered')",
        "INSERT INTO model_routes (public_id, is_enabled, model_id, pool_id, upstream_id) \
             VALUES ('cascade.route', 1, 1, NULL, NULL)",
        "INSERT INTO provider_pools (name, is_enabled, model_prefix) VALUES ('pool', 1, '')",
        "INSERT INTO provider_pool_members (pool_id, provider_id, priority) VALUES (1, 1, 0)",
        "INSERT INTO provider_quota_blocks (provider_id, restriction_key, applies_to, \
             observed_at, blocked_until) \
             VALUES (1, '[\"key\"]', '{\"type\":\"all_models\"}', 1, 2)",
        "INSERT INTO api_keys (name, key, created_at) VALUES ('key', 'secret', 1)",
        "INSERT INTO api_key_provider_defaults (api_key_id, provider_id, access) \
             VALUES (1, 1, 'allow')",
        "DELETE FROM providers WHERE id = 1",
    ] {
        database.execute(statement, ()).await?;
    }

    assert_eq!(
        scalar(&database, "SELECT COUNT(*) FROM models").await?,
        0,
        "the provider deletion cascades its models"
    );

    assert_eq!(
        scalar(&database, "SELECT COUNT(*) FROM model_routes").await?,
        0,
        "the provider deletion cascades its direct routes"
    );

    assert_eq!(
        scalar(&database, "SELECT COUNT(*) FROM provider_pool_members").await?,
        0,
        "the provider deletion cascades its pool memberships"
    );

    assert_eq!(
        scalar(&database, "SELECT COUNT(*) FROM provider_quota_blocks").await?,
        0,
        "the provider deletion cascades its quota blocks"
    );

    assert_eq!(
        scalar(&database, "SELECT COUNT(*) FROM api_key_provider_defaults").await?,
        0,
        "the provider deletion cascades its key-policy rules"
    );

    Ok(())
}

/// `STRICT` tables reject a value of the wrong storage type.
#[tokio::test]
async fn strict_tables_reject_foreign_storage_types() -> Result<(), Box<dyn StdError>> {
    let file = fresh_file("strict");

    let database = open(&file.0, Registry::Main).await?;

    let mismatched = database
        .execute(
            "INSERT INTO provider_pools (name, is_enabled, model_prefix)
             VALUES ('pool', 'not-an-integer', '')",
            (),
        )
        .await;

    assert!(
        mismatched.is_err(),
        "a STRICT integer column must reject text: {mismatched:?}"
    );

    Ok(())
}
