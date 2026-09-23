//! Schema version 1: the initial database layout.
//!
//! Version 1 creates every current object — the `providers`, `models`, `oauth_states`, `api_keys`,
//! and `api_logs` tables and the `idx_oauth_states_expires_at` and `idx_api_logs_started_at`
//! indexes — so its [`Schema::migration`]
//! is empty: there is no previous version to migrate from. A fresh database receives this
//! version's DDL directly, and every later version's migration transforms it.
//!
//! REFACTOR §7 exception, this once: the project is pre-launch, so this version is edited in
//! place — no new `version_x` module, no migration text, `SCHEMAS` stays at one version — gaining
//! the two new static-key provider types, the three new request-accounting endpoint values, and
//! the `/`-and-`:` exclusion on routing tokens (the Gemini URL path cannot address them).
//!
//! `api_logs` is request accounting: one row per dispatched inference request (README §3
//! *ApiLog*):
//!
//! - **Dispatch is the recording boundary**: a row exists exactly when the upstream HTTP request
//!   was issued (`send()` called). Everything before that instant — authentication, routing 404s,
//!   token refresh, upstream payload rendering failures — never reaches this table.
//! - `id` deliberately carries no `AUTOINCREMENT`: no identity reference points at `api_logs` rows
//!   — the id serves only as a discardable pagination cursor — so its number may be recycled,
//!   unlike the referenced identities the never-recycled rule protects (README §3 *Identity is
//!   never recycled*).
//! - No foreign keys: `api-keys:delete`, `providers:delete`, and `models:delete` neither cascade
//!   into nor block log rows. A deleted row's logs remain untouched and keep attributing by id —
//!   sound precisely because the referenced identities (`providers.id`, `api_keys.id`, `models.id`)
//!   are never recycled.
//! - No name snapshot: labels are resolved by the frontend through `api-keys:list`,
//!   `providers:list`, and `models:list`.
//! - The model is referenced by `model_id` — a bare `models.id` integer — for the same reason
//!   `api_key_id` and `provider_id` are bare integers: no FK, so a `models:delete` leaves the log
//!   row intact and attribution by id unchanged.
//! - `http_status` is `NULL` exactly when no response was ever produced to the client; the
//!   per-outcome decision rules live in the recording code (README §3 *ApiLog*).
//! - The five token columns are `NULL` when the upstream reported no usage; `output_tokens`
//!   includes `reasoning_tokens`.
//! - `started_at` (dispatch-start instant, microseconds) is the table's only secondary index:
//!   pagination reads order by `id` (rowid order comes for free) and aggregate reads bound their
//!   time window on this index.
//! - The requested reasoning effort and any content — prompt, completion, headers, upstream error
//!   bodies — are never recorded.
//!
//! Every integer identity a foreign reference can point at — `providers.id`, `api_keys.id`, and
//! `models.id` — is an `INTEGER PRIMARY KEY AUTOINCREMENT` key, so SQLite never reuses a deleted
//! row's number (README §3, *Identity is never recycled*). `oauth_states.id` carries AUTOINCREMENT
//! too so the rule holds for every row identity uniformly, even though nothing references it
//! today. The bookkeeping tables AUTOINCREMENT introduces — SQLite's `sqlite_sequence` and
//! Turso's `__turso_internal_*` sequence tables — are internal objects excluded from the
//! structural schema comparison.
//!
//! Machine-consumed text carries composition `CHECK`s (README §7): every routing token —
//! `providers.model_prefix` (empty means no prefix), `models.upstream_id`, `models.public_id`, a
//! present `models.id_override` — every bearer secret (`api_keys.key`), and every static-key
//! `providers.base_url` holds only printable ASCII without whitespace (`U+0021–U+007E`), matched
//! as `NOT GLOB '*[^!-~]*'`, with non-emptiness pinned wherever empty is not a state. The routing
//! tokens `providers.model_prefix`, `models.upstream_id`, and a present `models.id_override`
//! additionally reject `/` and `:` (`NOT GLOB '*[/:]*'`): the Gemini URL path cannot address them
//! (REFACTOR §7). Human labels (`providers.name`, `api_keys.name`) stay free-form behind their
//! non-empty `CHECK`s, and opaque upstream credential JSON is validated at the management layer
//! instead — its shape is not a column invariant.

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
    model_prefix TEXT NOT NULL CHECK (
        model_prefix NOT GLOB '*[^!-~]*' AND model_prefix NOT GLOB '*[/:]*'
    ),
    type TEXT NOT NULL CHECK (
        type IN (
            'openai_response',
            'openai_chat_completion',
            'gemini_generate_content',
            'gemini_interaction',
            'anthropic_messages',
            'codex_oauth',
            'antigravity_oauth'
        )
    ),
    base_url TEXT NULL CHECK (
        base_url IS NULL OR (base_url <> '' AND base_url NOT GLOB '*[^!-~]*')
    ),
    credentials TEXT NOT NULL,
    expires_at INTEGER NULL,
    refresh_started_at INTEGER NULL
) STRICT;

CREATE TABLE models (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    provider_id INTEGER NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    upstream_id TEXT NOT NULL CHECK (
        upstream_id <> '' AND upstream_id NOT GLOB '*[^!-~]*'
        AND upstream_id NOT GLOB '*[/:]*'
    ),
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    discovery_state TEXT NOT NULL CHECK (
        discovery_state IN ('discovered', 'vanished', 'manual')
    ),
    public_id TEXT NOT NULL UNIQUE CHECK (
        public_id <> '' AND public_id NOT GLOB '*[^!-~]*'
    ),
    id_override TEXT NULL CHECK (
        id_override IS NULL OR (
            id_override <> '' AND id_override NOT GLOB '*[^!-~]*'
            AND id_override NOT GLOB '*[/:]*'
        )
    ),
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
    key TEXT NOT NULL UNIQUE CHECK (key <> '' AND key NOT GLOB '*[^!-~]*'),
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE api_logs (
    id INTEGER PRIMARY KEY NOT NULL,
    started_at INTEGER NOT NULL,
    api_key_id INTEGER NOT NULL,
    provider_id INTEGER NOT NULL,
    model_id INTEGER NOT NULL,
    endpoint_type TEXT NOT NULL CHECK (
        endpoint_type IN (
            'openai_chat_completions', 'openai_responses', 'gemini_generate_content',
            'gemini_interactions', 'anthropic_messages'
        )
    ),
    is_stream INTEGER NOT NULL CHECK (is_stream IN (0, 1)),
    http_status INTEGER NULL,
    outcome TEXT NOT NULL CHECK (
        outcome IN ('completed', 'client_aborted', 'upstream_error', 'internal')
    ),
    duration INTEGER NOT NULL,
    input_tokens INTEGER NULL,
    output_tokens INTEGER NULL,
    cached_tokens INTEGER NULL,
    cache_write_tokens INTEGER NULL,
    reasoning_tokens INTEGER NULL
) STRICT;

CREATE INDEX idx_api_logs_started_at ON api_logs (started_at);
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
             VALUES ('a', 'a-', 'openai_chat_completion', '{}');",
            "INSERT INTO providers (name, model_prefix, type, credentials) \
             VALUES ('b', 'b-', 'openai_chat_completion', '{}');",
            "DELETE FROM providers WHERE name = 'b';",
            "INSERT INTO providers (name, model_prefix, type, credentials) \
             VALUES ('c', 'c-', 'openai_chat_completion', '{}');",
            "SELECT MAX(id) FROM providers;",
        )
        .await
    }

    /// Verifies the `api_logs.endpoint_type` closed domain: an endpoint outside the five outward
    /// completion endpoints is rejected at write time by the schema `CHECK`. First-party write
    /// paths only ever insert parsed endpoint types, so the probe uses a raw statement.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the illegal endpoint type is not rejected as a constraint violation.
    #[tokio::test]
    async fn api_logs_reject_an_illegal_endpoint_type() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;

        let outcome = connection
            .execute_batch(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 endpoint_type, is_stream, outcome, duration) \
                 VALUES (1, 1, 1, 1, 'grpc', 0, 'completed', 1);",
            )
            .await;

        assert_constraint_violation(outcome, "endpoint_type");

        // The five legal endpoint values all write cleanly.
        for (index, endpoint) in [
            "openai_chat_completions",
            "openai_responses",
            "gemini_generate_content",
            "gemini_interactions",
            "anthropic_messages",
        ]
        .iter()
        .enumerate()
        {
            connection
                .execute_batch(&format!(
                    "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                     endpoint_type, is_stream, outcome, duration) \
                     VALUES ({index}, 1, 1, 1, '{endpoint}', 0, 'completed', 1);",
                ))
                .await?;
        }

        Ok(())
    }

    /// Verifies the `api_logs.outcome` closed domain: an outcome outside the four recorded
    /// outcomes is rejected at write time by the schema `CHECK`.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the illegal outcome is not rejected as a constraint violation.
    #[tokio::test]
    async fn api_logs_reject_an_illegal_outcome() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;

        let outcome = connection
            .execute_batch(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 endpoint_type, is_stream, outcome, duration) \
                 VALUES (1, 1, 1, 1, 'openai_responses', 0, 'retrying', 1);",
            )
            .await;

        assert_constraint_violation(outcome, "outcome");

        Ok(())
    }

    /// Verifies the `api_logs.is_stream` closed domain: a flag outside `0`/`1` is rejected at
    /// write time by the schema `CHECK`.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the illegal flag is not rejected as a constraint violation.
    #[tokio::test]
    async fn api_logs_reject_an_illegal_stream_flag() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;

        let outcome = connection
            .execute_batch(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 endpoint_type, is_stream, outcome, duration) \
                 VALUES (1, 1, 1, 1, 'openai_chat_completions', 2, 'completed', 1);",
            )
            .await;

        assert_constraint_violation(outcome, "is_stream");

        Ok(())
    }

    /// Verifies the routing-token composition `CHECK`s (README §7, REFACTOR §7): a model prefix,
    /// an upstream identifier, a public ID, or a present override carrying a space, CJK text, or a
    /// control character — or an empty identifier where empty is not a state — is rejected at
    /// write time. The routing columns `model_prefix`, `upstream_id`, and a present `id_override`
    /// additionally reject `/` and `:` (the Gemini URL path cannot address them). An empty prefix
    /// stays writable: it means no prefix. First-party write paths validate before the write, so
    /// the probes use raw statements.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if an unroutable value is not rejected as a constraint violation.
    #[tokio::test]
    async fn routing_tokens_reject_values_outside_printable_ascii() -> Result<(), Box<dyn StdError>>
    {
        let connection = open_fresh().await?;
        connection
            .execute_batch(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('p', 'p-', 'openai_chat_completion', '{}');",
            )
            .await?;

        // One attempt per probe: a rejected write leaves no row behind, so the public-ID UNIQUE
        // never collides across probes.
        let probes = [
            (
                "model_prefix",
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('q-%ID%', 'has space', 'openai_chat_completion', '{}');",
            ),
            (
                "model_prefix",
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('q-%ID%', '前缀', 'openai_chat_completion', '{}');",
            ),
            (
                "model_prefix",
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('q-%ID%', 'has/slash', 'openai_chat_completion', '{}');",
            ),
            (
                "model_prefix",
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('q-%ID%', 'has:colon', 'openai_chat_completion', '{}');",
            ),
            (
                "upstream_id",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
                 VALUES (1, 'has space', 'manual', 'p-has-%ID%');",
            ),
            (
                "upstream_id",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
                 VALUES (1, '', 'manual', 'p-empty-%ID%');",
            ),
            (
                "upstream_id",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
                 VALUES (1, '模型', 'manual', 'p-cjk-%ID%');",
            ),
            (
                "upstream_id",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
                 VALUES (1, 'with/slash-%ID%', 'manual', 'p-with-slash-%ID%');",
            ),
            (
                "upstream_id",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
                 VALUES (1, 'with:colon-%ID%', 'manual', 'p-with-colon-%ID%');",
            ),
            (
                "public_id",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id) \
                 VALUES (1, 'm-%ID%', 'manual', 'has space');",
            ),
            (
                "id_override",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id, \
                 id_override) VALUES (1, 'm-%ID%', 'manual', 'p-m-%ID%', 'has space');",
            ),
            (
                "id_override",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id, \
                 id_override) VALUES (1, 'm-%ID%', 'manual', 'p-m-%ID%', '');",
            ),
            (
                "id_override",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id, \
                 id_override) VALUES (1, 'm-%ID%', 'manual', 'p-m-%ID%', 'with/slash');",
            ),
            (
                "id_override",
                "INSERT INTO models (provider_id, upstream_id, discovery_state, public_id, \
                 id_override) VALUES (1, 'm-%ID%', 'manual', 'p-m-%ID%', 'with:colon');",
            ),
        ];

        for (index, (column, template)) in probes.iter().enumerate() {
            let statement = template.replace("%ID%", &index.to_string());
            let outcome = connection.execute_batch(&statement).await;
            assert_constraint_violation(outcome, column);
        }

        // An empty prefix means no prefix and stays writable.
        open_fresh()
            .await?
            .execute_batch(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('bare', '', 'openai_chat_completion', '{}');",
            )
            .await?;

        Ok(())
    }

    /// Verifies the bearer-secret and base-URL composition `CHECK`s (README §7): a gateway key
    /// carrying a space or CJK text, and a static-key base URL carrying either, are rejected at
    /// write time; a `NULL` base URL stays writable for OAuth rows.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if an unroutable value is not rejected as a constraint violation.
    #[tokio::test]
    async fn secrets_and_urls_reject_values_outside_printable_ascii()
    -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh().await?;

        for (index, key) in ["has space", "密钥", ""].iter().enumerate() {
            let outcome = connection
                .execute_batch(&format!(
                    "INSERT INTO api_keys (name, key, created_at) VALUES ('k-{index}', '{key}', 1);"
                ))
                .await;
            assert_constraint_violation(outcome, "key");
        }

        for (index, base_url) in ["has space", "https://例子.example"].iter().enumerate() {
            let outcome = connection
                .execute_batch(&format!(
                    "INSERT INTO providers (name, model_prefix, type, base_url, credentials) \
                     VALUES ('u-{index}', 'u-', 'openai_chat_completion', '{base_url}', '{{}}');"
                ))
                .await;
            assert_constraint_violation(outcome, "base_url");
        }

        // A `NULL` base URL is the OAuth state and stays writable.
        connection
            .execute_batch(
                "INSERT INTO providers (name, model_prefix, type, base_url, credentials) \
                 VALUES ('oauth', 'o-', 'codex_oauth', NULL, '{}');",
            )
            .await?;

        Ok(())
    }
    /// a missing-table or syntax failure, which would make the rejection meaningless.
    ///
    /// # Panics
    ///
    /// Panics when the write succeeds or fails with any classification but a constraint
    /// violation.
    #[expect(
        clippy::panic,
        reason = "a rejection outside the constraint classification fails the test loudly"
    )]
    fn assert_constraint_violation(outcome: Result<(), turso::Error>, column: &str) {
        match outcome {
            Err(turso::Error::Constraint(_)) => {}

            Err(other) => panic!("{column} rejection must be a constraint violation: {other}"),

            Ok(()) => panic!("{column} value outside its closed domain must be rejected"),
        }
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
                 VALUES ('p', 'p-', 'openai_chat_completion', '{}');",
            )
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
}
