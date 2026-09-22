//! Schema version 1: the initial database layout.
//!
//! Version 1 creates every current object — the `providers`, `models`, `oauth_states`, and
//! `api_keys` tables and the `idx_oauth_states_expires_at` index — so its [`Schema::migration`]
//! is empty: there is no previous version to migrate from. A fresh database receives this
//! version's DDL directly, and every later version's migration transforms it.
//!
//! Every integer identity a foreign reference can point at — `providers.id`, `api_keys.id`, and
//! `models.id` — is an `INTEGER PRIMARY KEY AUTOINCREMENT` key, so SQLite never reuses a deleted
//! row's number (README §3, *Identity is never recycled*). `oauth_states.id` carries AUTOINCREMENT
//! too so the rule holds for every row identity uniformly, even though nothing references it
//! today. The bookkeeping tables AUTOINCREMENT introduces — SQLite's `sqlite_sequence` and
//! Turso's `__turso_internal_*` sequence tables — are internal objects excluded from the
//! structural schema comparison.

use super::Schema;

/// The schema of version 1.
///
/// The DDL is hand-written SQL in a raw string literal: every statement is terminated by `;`,
/// the exact shape the migration workflow embeds into its transaction batches. Its validity is
/// enforced by tests: the `tests` module in this file applies it to a fresh database, and the
/// parent module walks every migration forward into its declared schema.
pub(super) const SCHEMA: Schema = Schema {
    version: 1,
    schema: SCHEMA_SQL,
    migration: MIGRATION_SQL,
};

/// Complete DDL of version 1, applied to a fresh database.
const SCHEMA_SQL: &str = r###"
CREATE TABLE providers (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL UNIQUE CHECK (name != ''),
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    model_prefix TEXT NOT NULL,
    type TEXT NOT NULL CHECK (
        type IN (
            'openai_response',
            'openai_chat_completion',
            'gemini_generate_content',
            'codex_oauth',
            'antigravity_oauth'
        )
    ),
    base_url TEXT NULL,
    credentials TEXT NOT NULL,
    expires_at INTEGER NULL,
    refresh_started_at INTEGER NULL
) STRICT;

CREATE TABLE models (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    provider_id INTEGER NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    upstream_id TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    discovery_state TEXT NOT NULL CHECK (
        discovery_state IN ('discovered', 'vanished', 'manual')
    ),
    public_id TEXT NOT NULL UNIQUE,
    id_override TEXT NULL,
    UNIQUE (provider_id, upstream_id)
) STRICT;

CREATE TABLE oauth_states (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('codex_oauth', 'antigravity_oauth')),
    state TEXT NOT NULL UNIQUE,
    verifier TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_oauth_states_expires_at ON oauth_states (expires_at);

CREATE TABLE api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL CHECK (name <> ''),
    key TEXT NOT NULL UNIQUE CHECK (key <> ''),
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;
"###;

/// Migration upgrading the previous version into version 1: empty, as version 1 has no
/// predecessor.
const MIGRATION_SQL: &str = "";

#[cfg(test)]
mod tests {
    use super::{super::open_memory_connection, SCHEMA};
    use std::error::Error as StdError;

    /// Verifies that this version's DDL applies cleanly to a fresh in-memory database.
    ///
    /// # Panics
    ///
    /// Panics if opening the in-memory database fails.
    #[tokio::test]
    async fn schema_applies_to_a_fresh_database() -> Result<(), Box<dyn StdError>> {
        open_memory_connection()
            .await
            .execute_batch(SCHEMA.schema)
            .await?;

        Ok(())
    }

    /// Verifies the AUTOINCREMENT non-recycling invariant for every integer identity a foreign
    /// reference can point at — the sole guard that deleting a table's highest-numbered row frees
    /// its number: inserting a fresh row afterwards must yield a new, higher number instead of
    /// reusing the deleted one (README §3, *Identity is never recycled*). Each table runs the
    /// cycle on its own fresh database.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if a row identity is missing or a recycled number is observed.
    #[tokio::test]
    async fn identities_are_never_recycled_after_deleting_the_highest_row()
    -> Result<(), Box<dyn StdError>> {
        cycle_api_key_identity().await?;
        cycle_provider_identity().await?;
        cycle_model_identity().await
    }

    /// Runs one identity cycle: insert two rows, delete the higher-numbered one, insert again, and
    /// require the fresh row's number to exceed both earlier numbers.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement or identity query fails.
    ///
    /// # Panics
    ///
    /// Panics if the fresh row reuses the deleted number.
    async fn assert_identity_not_recycled(
        connection: &turso::Connection,
        insert_first: &str,
        insert_second: &str,
        delete_second: &str,
        insert_third: &str,
        select_max: &str,
    ) -> Result<(), Box<dyn StdError>> {
        connection.execute_batch(insert_first).await?;
        connection.execute_batch(insert_second).await?;

        let high = highest_identity(connection, select_max).await?;

        connection.execute_batch(delete_second).await?;
        connection.execute_batch(insert_third).await?;

        let fresh = highest_identity(connection, select_max).await?;

        assert!(
            fresh > high,
            "a fresh row must never reuse a deleted row's identity: {fresh} <= {high}"
        );

        Ok(())
    }

    /// Reads the table's current highest identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or produces no row.
    async fn highest_identity(
        connection: &turso::Connection,
        select_max: &str,
    ) -> Result<i64, Box<dyn StdError>> {
        let mut rows = connection.query(select_max, ()).await?;
        let row = rows.next().await?.ok_or("identity query produced no row")?;

        Ok(row.get(0)?)
    }

    /// Opens a fresh database carrying this version's schema.
    ///
    /// # Errors
    ///
    /// Returns an error if the DDL fails to apply.
    async fn open_fresh() -> Result<turso::Connection, Box<dyn StdError>> {
        let connection = open_memory_connection().await;
        connection.execute_batch(SCHEMA.schema).await?;

        Ok(connection)
    }

    /// Runs the cycle over `api_keys`.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement or identity query fails.
    ///
    /// # Panics
    ///
    /// Panics if the identity is recycled.
    async fn cycle_api_key_identity() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;

        assert_identity_not_recycled(
            &connection,
            "INSERT INTO api_keys (name, key, created_at) VALUES ('a', 'k-a', 1);",
            "INSERT INTO api_keys (name, key, created_at) VALUES ('b', 'k-b', 1);",
            "DELETE FROM api_keys WHERE name = 'b';",
            "INSERT INTO api_keys (name, key, created_at) VALUES ('c', 'k-c', 1);",
            "SELECT MAX(id) FROM api_keys;",
        )
        .await
    }

    /// Runs the cycle over `providers`.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement or identity query fails.
    ///
    /// # Panics
    ///
    /// Panics if the identity is recycled.
    async fn cycle_provider_identity() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;

        assert_identity_not_recycled(
            &connection,
            "INSERT INTO providers (name, model_prefix, type, credentials) \
             VALUES ('a', 'a/', 'openai_chat_completion', '{}');",
            "INSERT INTO providers (name, model_prefix, type, credentials) \
             VALUES ('b', 'b/', 'openai_chat_completion', '{}');",
            "DELETE FROM providers WHERE name = 'b';",
            "INSERT INTO providers (name, model_prefix, type, credentials) \
             VALUES ('c', 'c/', 'openai_chat_completion', '{}');",
            "SELECT MAX(id) FROM providers;",
        )
        .await
    }

    /// Runs the cycle over `models`, the row identities a management request and every
    /// row-addressed write address.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement or identity query fails.
    ///
    /// # Panics
    ///
    /// Panics if the identity is recycled.
    async fn cycle_model_identity() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;
        connection
            .execute_batch(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('p', 'p/', 'openai_chat_completion', '{}');",
            )
            .await?;

        assert_identity_not_recycled(
            &connection,
            "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
             VALUES (1, 'm-1', 'manual', 'p/m-1');",
            "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
             VALUES (1, 'm-2', 'manual', 'p/m-2');",
            "DELETE FROM models WHERE upstream_id = 'm-2';",
            "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
             VALUES (1, 'm-3', 'manual', 'p/m-3');",
            "SELECT MAX(id) FROM models;",
        )
        .await
    }
}
