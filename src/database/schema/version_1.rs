//! Schema version 1: the initial database layout.
//!
//! Version 1 creates every current object — the `gateway_database_registry` marker, the
//! `provider_definitions`, `providers`, `models`, `ongoing_oauth_flows`, and `api_keys` tables
//! and the `idx_ongoing_oauth_flows_expires_at` and
//! `idx_ongoing_oauth_flows_device_next_poll_at` indexes — so its [`Schema::migration`] is
//! empty: there is no previous version to migrate from. A fresh database receives this version's
//! DDL directly, and every later version's migration transforms it. The request-accounting table
//! is not part of this layout: `api_logs` lives in the day-partitioned shard files under its own
//! registry (the `shard_version_1` module, README §3 *ApiLog Shard Store*). A database predating
//! the shard store keeps its old in-main `api_logs` table as an unread orphan; historical rows
//! are not migrated.
//!
//! The `gateway_database_registry` marker stamps the file's registry: one row whose `registry`
//! column is `CHECK`-pinned to `main`, written by version 1's DDL itself and never at runtime.
//! The bootstrap verifies the stamp on any database that already carries a schema version, so a
//! file opened under the wrong registry — or a file predating the stamp, an unreleased
//! development artifact the operator recreates — fails at startup instead of at first query.
//!
//! `provider_definitions` holds the immutable, one-format connection recipes keyed by the
//! composite `(name, version)` primary key; `providers` holds account instances, each bound to
//! one exact definition version through a composite foreign key; `ongoing_oauth_flows` holds
//! unconsumed authorization flows, likewise bound to one exact definition version. Version
//! immutability is what lets a new definition version ship without changing how an existing
//! credential or in-flight flow is interpreted: definitions are never updated or deleted through
//! the management API, and `ON DELETE RESTRICT` additionally protects referenced versions at the
//! database boundary.
//!
//! `ongoing_oauth_flows` carries the whole flow lifecycle: its target (a provider to create, or
//! one to reauthorize), its grant kind with the grant-specific columns (the PKCE verifier of an
//! authorization-code flow; the device code, verification URIs, poll schedule, and exchange
//! checkpoint of a device flow), the working slots and base URL, the expiry, and the terminal
//! outcome. *Ongoing* means unconsumed: a terminal device result stays until the management
//! client retrieves it or the flow expires. The cross-column `CHECK`s pin the lifecycle states —
//! a `create` flow names its new provider, a `reauthorize` flow names its target provider, a
//! device flow carries its schedule exactly while it is pending, the exchange checkpoint of a
//! completed exchange exists exactly on a claimed pending device flow, a terminal outcome is
//! either a result provider or a failure pair, never both, and a terminal outcome exists only on
//! a device flow: an authorization-code flow is deleted by the atomic consume of its first
//! completion attempt, so its terminal state is never persisted.
//!
//! Every integer identity a foreign reference can point at — `providers.id`, `api_keys.id`, and
//! `models.id` — is an `INTEGER PRIMARY KEY AUTOINCREMENT` key, so SQLite never reuses a deleted
//! row's number (*Identity is never recycled*). `ongoing_oauth_flows.id` carries AUTOINCREMENT
//! too so the rule holds for every row identity uniformly, even though nothing references it
//! (a terminal flow exposes its result provider's id, a bare integer without a foreign key, so a
//! later provider deletion cannot strand the result row). The bookkeeping tables AUTOINCREMENT
//! introduces — SQLite's `sqlite_sequence` and Turso's `__turso_internal_*` sequence tables — are
//! internal objects excluded from the structural schema comparison.
//!
//! The JSON columns — `provider_definitions.inference`/`credential`/`provisioning`/`discovery`,
//! `providers.credentials`, and `ongoing_oauth_flows.slots`/`device_exchange_checkpoint` — carry
//! only `json_valid` checks: their shapes are descriptor contracts the management write boundary
//! enforces through Rust's concrete serde types, not column invariants. Text formats are
//! likewise enforced at the management or upstream-discovery ingestion boundary instead of by
//! column checks, so routing tokens, secrets, and base URLs validate in one place, before any
//! database write.

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
CREATE TABLE gateway_database_registry (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    registry TEXT NOT NULL CHECK (registry = 'main')
) STRICT;

INSERT INTO gateway_database_registry (id, registry) VALUES (1, 'main');

CREATE TABLE provider_definitions (
    name TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    format TEXT NOT NULL CHECK (
        format IN (
            'openai_response',
            'openai_chat_completion',
            'gemini_generate_content',
            'gemini_interaction',
            'anthropic_messages'
        )
    ),
    inference TEXT NOT NULL CHECK (json_valid(inference)),
    credential TEXT NOT NULL CHECK (json_valid(credential)),
    provisioning TEXT NULL CHECK (provisioning IS NULL OR json_valid(provisioning)),
    discovery TEXT NULL CHECK (discovery IS NULL OR json_valid(discovery)),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (name, version)
) STRICT;

CREATE TABLE providers (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL UNIQUE,
    provider_definition_name TEXT NOT NULL,
    provider_definition_version INTEGER NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    model_prefix TEXT NOT NULL,
    base_url TEXT NULL,
    credentials TEXT NOT NULL CHECK (json_valid(credentials)),
    FOREIGN KEY (provider_definition_name, provider_definition_version)
        REFERENCES provider_definitions (name, version) ON DELETE RESTRICT
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

CREATE TABLE ongoing_oauth_flows (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    provider_definition_name TEXT NOT NULL,
    provider_definition_version INTEGER NOT NULL,
    state_token TEXT NOT NULL UNIQUE,
    purpose TEXT NOT NULL CHECK (purpose IN ('create', 'reauthorize')),
    provider_name TEXT NULL,
    model_prefix TEXT NULL,
    base_url TEXT NULL,
    target_provider_id INTEGER NULL CHECK (
        target_provider_id IS NULL OR target_provider_id > 0
    ),
    grant_type TEXT NOT NULL CHECK (grant_type IN ('authorization_code', 'device')),
    slots TEXT NOT NULL CHECK (json_valid(slots)),
    code_verifier TEXT NULL,
    device_code TEXT NULL,
    device_user_code TEXT NULL,
    device_verification_uri TEXT NULL,
    device_verification_uri_complete TEXT NULL,
    device_poll_interval_seconds INTEGER NULL CHECK (
        device_poll_interval_seconds IS NULL OR device_poll_interval_seconds > 0
    ),
    device_next_poll_at INTEGER NULL,
    device_poll_started_at INTEGER NULL,
    device_exchange_checkpoint TEXT NULL CHECK (
        device_exchange_checkpoint IS NULL OR json_valid(device_exchange_checkpoint)
    ),
    outcome TEXT NULL CHECK (outcome IN ('succeeded', 'failed')),
    result_provider_id INTEGER NULL CHECK (
        result_provider_id IS NULL OR result_provider_id > 0
    ),
    failure_code TEXT NULL,
    failure_message TEXT NULL,
    expires_at INTEGER NOT NULL,
    FOREIGN KEY (provider_definition_name, provider_definition_version)
        REFERENCES provider_definitions (name, version) ON DELETE RESTRICT,
    CHECK (
        (purpose = 'create' AND provider_name IS NOT NULL AND model_prefix IS NOT NULL
         AND target_provider_id IS NULL)
        OR (purpose = 'reauthorize' AND provider_name IS NULL AND model_prefix IS NULL
            AND target_provider_id IS NOT NULL)
    ),
    CHECK (
        (grant_type = 'authorization_code' AND device_code IS NULL
         AND device_user_code IS NULL AND device_verification_uri IS NULL
         AND device_verification_uri_complete IS NULL AND device_poll_interval_seconds IS NULL
         AND device_next_poll_at IS NULL AND device_poll_started_at IS NULL)
        OR (grant_type = 'device' AND code_verifier IS NULL
            AND device_code IS NOT NULL AND device_user_code IS NOT NULL
            AND (device_verification_uri IS NOT NULL
                 OR device_verification_uri_complete IS NOT NULL)
            AND device_poll_interval_seconds IS NOT NULL
            AND ((outcome IS NULL AND device_next_poll_at IS NOT NULL)
                 OR (outcome IS NOT NULL AND device_next_poll_at IS NULL)))
    ),
    CHECK (outcome IS NULL OR device_poll_started_at IS NULL),
    CHECK (outcome IS NULL OR grant_type = 'device'),
    CHECK (device_exchange_checkpoint IS NULL OR grant_type = 'device'),
    CHECK (device_exchange_checkpoint IS NULL OR device_poll_started_at IS NOT NULL),
    CHECK (outcome IS NULL OR device_exchange_checkpoint IS NULL),
    CHECK (
        (outcome IS NULL AND result_provider_id IS NULL
         AND failure_code IS NULL AND failure_message IS NULL)
        OR (outcome = 'succeeded' AND result_provider_id IS NOT NULL
            AND failure_code IS NULL AND failure_message IS NULL)
        OR (outcome = 'failed' AND result_provider_id IS NULL
            AND failure_code IS NOT NULL AND failure_message IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_ongoing_oauth_flows_expires_at ON ongoing_oauth_flows (expires_at);

CREATE INDEX idx_ongoing_oauth_flows_device_next_poll_at
    ON ongoing_oauth_flows (device_next_poll_at);

CREATE TABLE api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL,
    key TEXT NOT NULL UNIQUE,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;
"###;

/// Migration upgrading the previous version into version 1: empty, as version 1 has no
/// predecessor.
const MIGRATION_SQL: &str = "";

#[cfg(test)]
mod tests {
    use super::{
        super::{assert_constraint_violation, open_fresh},
        SCHEMA,
    };
    use std::error::Error as StdError;

    /// The `credentials` JSON of a minimal static-key provider row, for raw statement probes.
    const API_KEY_CREDENTIALS: &str = r#"{"type":"api_key","api_key":"k"}"#;

    /// Verifies that this version's DDL applies cleanly to a fresh in-memory database.
    ///
    /// # Errors
    ///
    /// Returns an error if the DDL fails to apply.
    ///
    /// # Panics
    ///
    /// Panics if connecting to the in-memory database fails.
    #[tokio::test]
    async fn schema_applies_to_a_fresh_database() -> Result<(), Box<dyn StdError>> {
        open_fresh(SCHEMA.schema).await?;

        Ok(())
    }

    /// Inserts one definition row usable as the foreign target of provider probes.
    ///
    /// # Errors
    ///
    /// Returns an error if the insert fails.
    async fn insert_probe_definition(
        connection: &turso::Connection,
        name: &str,
    ) -> Result<(), Box<dyn StdError>> {
        connection
            .execute_batch(&format!(
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('{name}', 1, 'openai_chat_completion', \
                 '{{}}', '{{\"type\":\"api_key\"}}', 1);"
            ))
            .await?;

        Ok(())
    }

    /// Verifies the AUTOINCREMENT non-recycling invariant for every integer identity a foreign
    /// reference can point at — the sole guard that deleting a table's highest-numbered row frees
    /// its number: inserting a fresh row afterwards must yield a new, higher number instead of
    /// reusing the deleted one. Each table runs the cycle on its own fresh database.
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
        let connection = open_fresh(SCHEMA.schema).await?;

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
        let connection = open_fresh(SCHEMA.schema).await?;
        insert_probe_definition(&connection, "def").await?;

        assert_identity_not_recycled(
            &connection,
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, model_prefix, credentials) \
             VALUES ('a', 'def', 1, 'a-', '{\"type\":\"api_key\",\"api_key\":\"k\"}');",
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, model_prefix, credentials) \
             VALUES ('b', 'def', 1, 'b-', '{\"type\":\"api_key\",\"api_key\":\"k\"}');",
            "DELETE FROM providers WHERE name = 'b';",
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, model_prefix, credentials) \
             VALUES ('c', 'def', 1, 'c-', '{\"type\":\"api_key\",\"api_key\":\"k\"}');",
            "SELECT MAX(id) FROM providers;",
        )
        .await
    }

    /// Verifies the `provider_definitions` column domains: the format enum rejects a foreign
    /// format, the version range rejects zero and negatives, and the composite primary key
    /// rejects a second row of one `(name, version)` — while the same name carries a second
    /// version.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if an out-of-domain write is not rejected as a constraint violation.
    #[tokio::test]
    async fn provider_definitions_reject_out_of_domain_columns() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;
        insert_probe_definition(&connection, "def").await?;

        let probes = [
            (
                "format",
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('fmt', 1, 'bogus_format', '{}', '{}', 1);",
            ),
            (
                "version",
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('v0', 0, 'openai_chat_completion', '{}', '{}', 1);",
            ),
            (
                "version",
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('vneg', -1, 'openai_chat_completion', '{}', '{}', 1);",
            ),
            (
                "PRIMARY KEY (name, version)",
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('def', 1, 'openai_chat_completion', '{}', '{}', 1);",
            ),
            (
                "inference",
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('badjson', 1, 'openai_chat_completion', 'not json', '{}', 1);",
            ),
        ];

        for (column, statement) in probes {
            let outcome = connection.execute_batch(statement).await;
            assert_constraint_violation(outcome, column);
        }

        // The same name under a second version is exactly the versioning model.
        connection
            .execute_batch(
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('def', 2, 'anthropic_messages', '{}', '{}', 1);",
            )
            .await?;

        Ok(())
    }

    /// Verifies the `providers` foreign key and its restrict behavior: a provider row cannot
    /// reference an unknown definition version, and a referenced version cannot be deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if an integrity violation is not rejected.
    #[tokio::test]
    async fn providers_enforce_the_definition_foreign_key() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;

        // Foreign keys are enforced on the migration bootstrap connection and every runtime
        // connection; the probe enables them explicitly like the runtime does.
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .await?;

        insert_probe_definition(&connection, "def").await?;

        let outcome = connection
            .execute_batch(&format!(
                "INSERT INTO providers (name, provider_definition_name, \
                 provider_definition_version, model_prefix, credentials) \
                 VALUES ('p', 'unknown', 1, 'p-', '{API_KEY_CREDENTIALS}');"
            ))
            .await;

        assert_constraint_violation(outcome, "FOREIGN KEY");

        let outcome = connection
            .execute_batch(&format!(
                "INSERT INTO providers (name, provider_definition_name, \
                 provider_definition_version, model_prefix, credentials) \
                 VALUES ('p', 'def', 2, 'p-', '{API_KEY_CREDENTIALS}');"
            ))
            .await;

        assert_constraint_violation(outcome, "FOREIGN KEY");

        connection
            .execute_batch(&format!(
                "INSERT INTO providers (name, provider_definition_name, \
                 provider_definition_version, model_prefix, credentials) \
                 VALUES ('p', 'def', 1, 'p-', '{API_KEY_CREDENTIALS}');"
            ))
            .await?;

        // A referenced version is protected even from a later version's presence.
        connection
            .execute_batch(
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('def', 2, 'openai_chat_completion', '{}', '{}', 1);",
            )
            .await?;

        let outcome = connection
            .execute_batch("DELETE FROM provider_definitions WHERE name = 'def' AND version = 1;")
            .await;

        assert_constraint_violation(outcome, "FOREIGN KEY");

        Ok(())
    }

    /// Verifies the `providers.credentials` JSON-syntax check: only valid JSON is stored; the
    /// payload's shape is the management boundary's contract, not a column invariant.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if invalid JSON is accepted.
    #[tokio::test]
    async fn providers_reject_malformed_credential_json() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;
        insert_probe_definition(&connection, "def").await?;

        let outcome = connection
            .execute_batch(
                "INSERT INTO providers (name, provider_definition_name, \
                 provider_definition_version, model_prefix, credentials) \
                 VALUES ('p', 'def', 1, 'p-', 'not json');",
            )
            .await;

        assert_constraint_violation(outcome, "credentials");

        // An arbitrary JSON object is storable — the descriptor shape is validated above it.
        connection
            .execute_batch(
                "INSERT INTO providers (name, provider_definition_name, \
                 provider_definition_version, model_prefix, credentials) \
                 VALUES ('p', 'def', 1, 'p-', '{}');",
            )
            .await?;

        Ok(())
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
        let connection = open_fresh(SCHEMA.schema).await?;
        insert_probe_definition(&connection, "def").await?;
        connection
            .execute_batch(&format!(
                "INSERT INTO providers (name, provider_definition_name, \
                 provider_definition_version, model_prefix, credentials) \
                 VALUES ('p', 'def', 1, 'p-', '{API_KEY_CREDENTIALS}');"
            ))
            .await?;

        assert_identity_not_recycled(
            &connection,
            "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
             VALUES (1, 'm-1', 'manual', 'p-m-1');",
            "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
             VALUES (1, 'm-2', 'manual', 'p-m-2');",
            "DELETE FROM models WHERE upstream_id = 'm-2';",
            "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
             VALUES (1, 'm-3', 'manual', 'p-m-3');",
            "SELECT MAX(id) FROM models;",
        )
        .await
    }

    /// Verifies the cross-column lifecycle `CHECK`s of `ongoing_oauth_flows` (README §3): a
    /// `create` flow names its new provider, a `reauthorize` flow names its target provider, a
    /// device flow carries its schedule exactly while it is pending, the exchange checkpoint of
    /// a completed exchange exists exactly on a claimed pending device flow, a terminal outcome
    /// is either a result provider or a failure pair, never both, and a terminal outcome exists
    /// only on a device flow — an authorization-code flow is deleted by the atomic consume of
    /// its first completion attempt, so its terminal state is never persisted — while every
    /// conforming state combination writes cleanly.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if an illegal state combination is accepted.
    #[tokio::test]
    async fn ongoing_oauth_flows_enforce_their_lifecycle_states() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;
        insert_probe_definition(&connection, "def").await?;

        /// The shared prefix of a legal insert: definition reference, state token, purpose, and
        /// grant kind.
        fn insert_prefix(id: u32, purpose: &str, grant: &str) -> String {
            format!(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at{purpose_fields}{grant_fields}) VALUES ('def', 1, 'st-{id}', \
                 '{purpose}', '{grant}', '{{}}', 1000{purpose_values}{grant_values})",
                purpose_fields = if purpose == "create" {
                    ", provider_name, model_prefix"
                } else {
                    ", target_provider_id"
                },
                purpose_values = if purpose == "create" {
                    ", 'new-provider', 'p-'"
                } else {
                    ", 7"
                },
                grant_fields = if grant == "authorization_code" {
                    ", code_verifier"
                } else {
                    ", device_code, device_user_code, device_verification_uri, \
                     device_poll_interval_seconds, device_next_poll_at"
                },
                grant_values = if grant == "authorization_code" {
                    ", 'verifier'"
                } else {
                    ", 'dc', 'uc', 'https://verify.example', 5, 500"
                },
            )
        }

        // Every conforming pending state combination writes cleanly.
        connection
            .execute_batch(&insert_prefix(1, "create", "authorization_code"))
            .await?;
        connection
            .execute_batch(&insert_prefix(2, "reauthorize", "device"))
            .await?;

        // A device flow that reached a terminal outcome carries no poll schedule anymore but
        // keeps its device code columns, which identify the flow's grant.
        connection
            .execute_batch(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at, target_provider_id, device_code, device_user_code, \
                 device_verification_uri, device_poll_interval_seconds, outcome, \
                 result_provider_id) \
                 VALUES ('def', 1, 'st-3', 'reauthorize', 'device', '{}', 1000, 7, 'dc', 'uc', \
                 'https://verify.example', 5, 'succeeded', 7);",
            )
            .await?;

        let purpose_probes = [
            (
                "purpose",
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                     provider_definition_version, state_token, purpose, grant_type, slots, \
                     expires_at, target_provider_id, code_verifier) \
                     VALUES ('def', 1, 'st-p1', 'create', 'authorization_code', \
                     '{}', 1000, 7, 'v');"
                    .to_string(),
            ),
            (
                "purpose",
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                     provider_definition_version, state_token, purpose, grant_type, slots, \
                     expires_at, provider_name, model_prefix, code_verifier) \
                     VALUES ('def', 1, 'st-p2', 'reauthorize', 'authorization_code', \
                     '{}', 1000, 'n', 'p-', 'v');"
                    .to_string(),
            ),
        ];

        for (column, statement) in purpose_probes {
            let outcome = connection.execute_batch(&statement).await;
            assert_constraint_violation(outcome, column);
        }

        // An authorization-code flow carrying device columns.
        let outcome = connection
            .execute_batch(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at, provider_name, model_prefix, code_verifier, device_code) \
                 VALUES ('def', 1, 'st-g1', 'create', 'authorization_code', '{}', 1000, \
                 'n', 'p-', 'v', 'dc');",
            )
            .await;

        assert_constraint_violation(outcome, "grant_type");

        // A device flow without its device code.
        let outcome = connection
            .execute_batch(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at, target_provider_id) \
                 VALUES ('def', 1, 'st-g2', 'reauthorize', 'device', '{}', 1000, 7);",
            )
            .await;

        assert_constraint_violation(outcome, "grant_type");

        // A device flow without a verification URI of either spelling.
        let outcome = connection
            .execute_batch(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at, target_provider_id, device_code, device_user_code, \
                 device_poll_interval_seconds, device_next_poll_at) \
                 VALUES ('def', 1, 'st-g3', 'reauthorize', 'device', '{}', 1000, 7, \
                 'dc', 'uc', 5, 500);",
            )
            .await;

        assert_constraint_violation(outcome, "grant_type");

        // A pending device flow without its next poll instant.
        let outcome = connection
            .execute_batch(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at, target_provider_id, device_code, device_user_code, \
                 device_verification_uri, device_poll_interval_seconds) \
                 VALUES ('def', 1, 'st-g4', 'reauthorize', 'device', '{}', 1000, 7, \
                 'dc', 'uc', 'https://v', 5);",
            )
            .await;

        assert_constraint_violation(outcome, "grant_type");

        // A terminal device flow carrying a next-poll instant: the schedule lives exactly while
        // the flow is pending, so the succeeded row must reject it.
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_next_poll_at = 600 \
                 WHERE state_token = 'st-3';",
            )
            .await;

        assert_constraint_violation(outcome, "grant_type");

        // A terminal outcome without either its result provider or its failure pair.
        let outcome = connection
            .execute_batch("UPDATE ongoing_oauth_flows SET outcome = 'failed' WHERE id = 3;")
            .await;

        assert_constraint_violation(outcome, "outcome");

        // A terminal outcome on an authorization-code flow: the flow is deleted by the atomic
        // consume of its first completion attempt, so a persisted terminal outcome is a device
        // flow's alone. The probe satisfies every other terminal-outcome state, isolating the
        // grant coupling.
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET outcome = 'succeeded', result_provider_id = 7 \
                 WHERE id = 1;",
            )
            .await;

        assert_constraint_violation(outcome, "outcome");

        // A succeeded outcome carrying a failure message.
        let outcome = connection
            .execute_batch("UPDATE ongoing_oauth_flows SET failure_message = 'x' WHERE id = 3;")
            .await;

        assert_constraint_violation(outcome, "outcome");

        // A non-positive poll interval.
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_poll_interval_seconds = 0 WHERE id = 2;",
            )
            .await;

        assert_constraint_violation(outcome, "device_poll_interval_seconds");

        // A failed outcome carrying its gateway-owned failure pair writes cleanly, and a
        // non-positive id column is rejected.
        connection
            .execute_batch(
                "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
                 provider_definition_version, state_token, purpose, grant_type, slots, \
                 expires_at, target_provider_id, device_code, device_user_code, \
                 device_verification_uri, device_poll_interval_seconds, outcome, failure_code, \
                 failure_message) \
                 VALUES ('def', 1, 'st-f1', 'reauthorize', 'device', '{}', 1000, 7, 'dc', 'uc', \
                 'https://v', 5, 'failed', 'expired_token', 'the device code expired');",
            )
            .await?;

        // The same schedule exclusion under the failed outcome spelling.
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_next_poll_at = 600 \
                 WHERE state_token = 'st-f1';",
            )
            .await;

        assert_constraint_violation(outcome, "grant_type");

        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET target_provider_id = 0 \
                 WHERE state_token = 'st-f1';",
            )
            .await;

        assert_constraint_violation(outcome, "target_provider_id");

        // An exchange checkpoint on an unclaimed flow: the checkpoint records a completed
        // exchange whose commit a crashed claimant still owes, so it exists exactly while the
        // claim that produced it is held (README §3).
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_exchange_checkpoint = \
                 '{\"access_token\":\"t\"}' WHERE state_token = 'st-2';",
            )
            .await;

        assert_constraint_violation(outcome, "device_exchange_checkpoint");

        // The claim stamps first, so the following checkpoint probes isolate the column.
        connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_poll_started_at = 600 \
                 WHERE state_token = 'st-2';",
            )
            .await?;

        // A non-JSON checkpoint: only JSON syntax is a column invariant; the payload's shape is
        // the poller's serde contract, not a schema check. A rejected statement leaves the row
        // untouched, so the claim survives for the conforming write below.
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_exchange_checkpoint = 'not json' \
                 WHERE state_token = 'st-2';",
            )
            .await;

        assert_constraint_violation(outcome, "device_exchange_checkpoint");

        // A claimed pending device flow carrying its checkpoint is exactly the state a crash
        // between the exchange and the commit leaves behind: it writes cleanly.
        connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_exchange_checkpoint = \
                 '{\"access_token\":\"t\"}' WHERE state_token = 'st-2';",
            )
            .await?;

        // A terminal row carrying a checkpoint: every terminal persist clears the checkpoint
        // together with the claim and the schedule, so a checkpoint past the outcome is drift.
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_exchange_checkpoint = \
                 '{\"access_token\":\"t\"}' WHERE state_token = 'st-3';",
            )
            .await;

        assert_constraint_violation(outcome, "device_exchange_checkpoint");

        // A checkpoint on an authorization-code flow: the checkpoint is a device-poller fact —
        // the synchronous authorization-code completion consumes its row at the first
        // completion attempt and holds no claim to resume (README §3).
        let outcome = connection
            .execute_batch(
                "UPDATE ongoing_oauth_flows SET device_exchange_checkpoint = \
                 '{\"access_token\":\"t\"}' WHERE state_token = 'st-1';",
            )
            .await;

        assert_constraint_violation(outcome, "device_exchange_checkpoint");

        Ok(())
    }
}
