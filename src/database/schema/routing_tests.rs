//! Schema invariants for public routes, pool exposure, and durable account restrictions.

use super::{assert_constraint_violation, open_fresh, version_1, version_2};
use std::error::Error as StdError;

/// Bootstrap refuses versions beyond the registered schema history.
///
/// # Panics
///
/// Intentionally panics when bootstrap observes unregistered version three.
#[tokio::test]
#[should_panic(expected = "unknown schema version 3")]
async fn obsolete_development_version_is_refused() {
    #[expect(
        clippy::expect_used,
        reason = "the immutable initial schema must apply"
    )]
    let connection = open_fresh(version_1::SCHEMA.schema)
        .await
        .expect("fresh schema");
    #[expect(
        clippy::expect_used,
        reason = "the fixture stamps a positive schema version"
    )]
    connection
        .execute_batch("PRAGMA user_version = 3;")
        .await
        .expect("version stamp");
    super::migrate(&connection, super::Registry::Main, "development").await;
}

/// Seeds one physical model and one owning pool under the requested schema.
///
/// # Errors
///
/// Returns schema and fixture errors.
async fn inventory_schema(schema: &str) -> Result<turso::Connection, Box<dyn StdError>> {
    let connection = open_fresh(schema).await?;
    connection
        .execute_batch(
            r#"
        PRAGMA foreign_keys = ON;
        INSERT INTO provider_definitions
            (name, version, format, inference, credential, created_at)
            VALUES ('fixture', 1, 'openai_response', '{}', '{"type":"api_key"}', 1);
        INSERT INTO providers
            (name, provider_definition_name, provider_definition_version, credentials)
            VALUES ('A', 'fixture', 1, '{"type":"api_key","api_key":"a"}');
        INSERT INTO models (provider_id, upstream_id, discovery_state)
            VALUES (1, 'M', 'discovered');
        INSERT INTO provider_pools (name) VALUES ('coding');
        INSERT INTO provider_pool_members (pool_id, provider_id, priority) VALUES (1, 1, 7);
    "#,
        )
        .await?;
    Ok(connection)
}

/// Seeds immutable version-one routing constraints.
///
/// # Errors
///
/// Returns fixture errors.
async fn inventory() -> Result<turso::Connection, Box<dyn StdError>> {
    inventory_schema(version_1::SCHEMA.schema).await
}

/// Reads one aggregate or identity query whose fixture guarantees a row.
///
/// # Errors
///
/// Returns query and decoding errors.
///
/// # Panics
///
/// Panics if a guaranteed aggregate or newly inserted identity is absent.
async fn integer(
    connection: &turso::Connection,
    statement: &str,
) -> Result<i64, Box<dyn StdError>> {
    #[expect(
        clippy::expect_used,
        reason = "fixture identity or aggregate queries yield a row"
    )]
    Ok(connection
        .query(statement, ())
        .await?
        .next()
        .await?
        .expect("fixture row")
        .get(0)?)
}

/// Public routing identities never recycle a deleted accounting or affinity namespace.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if a deleted public identity is reused.
#[tokio::test]
async fn public_entity_identities_are_never_recycled() -> Result<(), Box<dyn StdError>> {
    let connection = inventory().await?;
    connection
        .execute_batch(
            "
        INSERT INTO provider_pools (name) VALUES ('discarded');
        DELETE FROM provider_pools WHERE name = 'discarded';
        INSERT INTO provider_pools (name) VALUES ('replacement');
        INSERT INTO model_routes (public_id, pool_id, upstream_id) VALUES ('discarded', 1, 'M');
        DELETE FROM model_routes WHERE public_id = 'discarded';
        INSERT INTO model_routes (public_id, pool_id, upstream_id) VALUES ('replacement', 1, 'M');
    ",
        )
        .await?;
    assert_eq!(
        integer(
            &connection,
            "SELECT id FROM provider_pools WHERE name = 'replacement'"
        )
        .await?,
        3
    );
    assert_eq!(
        integer(
            &connection,
            "SELECT id FROM model_routes WHERE public_id = 'replacement'"
        )
        .await?,
        2
    );
    Ok(())
}

/// Target unions, public uniqueness, and deletion rules hold under enforced foreign keys.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if a malformed target or a forbidden deletion succeeds.
#[tokio::test]
async fn routes_enforce_targets_and_deletion_semantics() -> Result<(), Box<dyn StdError>> {
    let connection = inventory().await?;
    for target in [
        "NULL, NULL, NULL",
        "1, 1, 'M'",
        "NULL, 1, NULL",
        "1, NULL, 'M'",
    ] {
        assert_constraint_violation(
            connection
                .execute(
                    format!(
                        "INSERT INTO model_routes (public_id, model_id, pool_id, upstream_id) \
             VALUES ('invalid', {target})"
                    ),
                    (),
                )
                .await
                .map(|_| ()),
            "routing constraint",
        );
    }
    connection
        .execute_batch(
            "
        INSERT INTO model_routes (public_id, model_id) VALUES ('private-M', 1), ('alias-M', 1);
        INSERT INTO model_routes (public_id, pool_id, upstream_id)
            VALUES ('coding-M', 1, 'M'), ('future-N', 1, 'N');
        UPDATE models SET discovery_state = 'vanished' WHERE id = 1;
    ",
        )
        .await?;
    assert_constraint_violation(
        connection
            .execute(
                "INSERT INTO model_routes (public_id, model_id) VALUES ('private-M', 1)",
                (),
            )
            .await
            .map(|_| ()),
        "routing constraint",
    );
    assert_constraint_violation(
        connection
            .execute("DELETE FROM provider_pools WHERE id = 1", ())
            .await
            .map(|_| ()),
        "routing constraint",
    );
    assert_eq!(
        integer(&connection, "SELECT COUNT(*) FROM model_routes").await?,
        4
    );
    connection
        .execute("DELETE FROM models WHERE id = 1", ())
        .await?;
    assert_eq!(
        integer(&connection, "SELECT COUNT(*) FROM model_routes").await?,
        2
    );
    connection
        .execute("DELETE FROM providers WHERE id = 1", ())
        .await?;
    assert_eq!(
        integer(&connection, "SELECT COUNT(*) FROM provider_pool_members").await?,
        0
    );
    Ok(())
}

/// Quota restrictions belong to accounts and admit only future or unknown reset instants.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if quota constraints or cascading deletion differ from the contract.
#[tokio::test]
async fn blocks_enforce_resets_and_account_ownership() -> Result<(), Box<dyn StdError>> {
    let connection = inventory().await?;
    for reset in ["99", "100"] {
        assert_constraint_violation(
            connection
                .execute(
                    format!(
                        r#"INSERT INTO provider_quota_blocks
            VALUES (1, '["weekly"]', '{{"type":"all_models"}}', 100, {reset})"#
                    ),
                    (),
                )
                .await
                .map(|_| ()),
            "routing constraint",
        );
    }
    connection
        .execute_batch(
            r#"
        INSERT INTO provider_quota_blocks
            VALUES (1, '["weekly"]', '{"type":"all_models"}', 100, NULL);
        INSERT INTO provider_quota_blocks
            VALUES (1, '["daily"]', '{"type":"models","upstream_ids":["M"]}', 100, 101);
        DELETE FROM providers WHERE id = 1;
    "#,
        )
        .await?;
    assert_eq!(
        integer(&connection, "SELECT COUNT(*) FROM provider_quota_blocks").await?,
        0
    );
    Ok(())
}

/// Fresh and migrated version-two databases preserve IDs and unique automatic pool models.
///
/// # Errors
///
/// Returns schema, migration, fixture, and decoding errors.
///
/// # Panics
///
/// Panics if migration recycles a route ID or retains an automatic member prefix.
#[tokio::test]
async fn pool_exposure_schema_and_migration_preserve_owned_routes() -> Result<(), Box<dyn StdError>>
{
    for should_migrate in [false, true] {
        let schema = if should_migrate {
            version_1::SCHEMA.schema
        } else {
            version_2::SCHEMA.schema
        };
        let connection = inventory_schema(schema).await?;
        connection
            .execute_batch(
                "
            INSERT INTO model_routes (id, public_id, model_id, is_automatic)
                VALUES (1, 'A-M', 1, 1);
            INSERT INTO model_routes (id, public_id, pool_id, upstream_id)
                VALUES (100, 'authored', 1, 'M');
            DELETE FROM model_routes WHERE id = 100;
        ",
            )
            .await?;
        if should_migrate {
            connection
                .execute_batch(&format!("BEGIN; {} COMMIT;", version_2::SCHEMA.migration))
                .await?;
            assert!(
                integer(
                    &connection,
                    "SELECT id FROM model_routes WHERE pool_id = 1 AND is_automatic = 1"
                )
                .await?
                    > 100
            );
            #[expect(
                clippy::expect_used,
                reason = "migration created the pooled model entry"
            )]
            let name = connection
                .query(
                    "SELECT public_id FROM model_routes WHERE pool_id = 1 AND is_automatic = 1",
                    (),
                )
                .await?
                .next()
                .await?
                .expect("pool entry")
                .get::<String>(0)?;
            assert_eq!(name, "pool-1-M");
            assert_eq!(
                integer(
                    &connection,
                    "SELECT COUNT(*) FROM model_routes WHERE model_id = 1 AND is_automatic = 1"
                )
                .await?,
                0
            );
        } else {
            connection
                .execute(
                    "INSERT INTO model_routes (public_id, pool_id, upstream_id, is_automatic) \
                 VALUES ('pool-M', 1, 'M', 1)",
                    (),
                )
                .await?;
        }
        assert_constraint_violation(
            connection
                .execute(
                    "INSERT INTO model_routes (public_id, pool_id, upstream_id, is_automatic) \
             VALUES ('duplicate', 1, 'M', 1)",
                    (),
                )
                .await
                .map(|_| ()),
            "one automatic pool route per exact model",
        );
    }
    Ok(())
}
