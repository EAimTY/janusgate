//! Fresh main schema invariants and domain constraints.

use super::{assert_constraint_violation, open_fresh, version_1::SCHEMA};
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
         provider_definition_version, credentials) \
         VALUES ('a', 'def', 1, '{\"type\":\"api_key\",\"api_key\":\"k\"}');",
        "INSERT INTO providers (name, provider_definition_name, \
         provider_definition_version, credentials) \
         VALUES ('b', 'def', 1, '{\"type\":\"api_key\",\"api_key\":\"k\"}');",
        "DELETE FROM providers WHERE name = 'b';",
        "INSERT INTO providers (name, provider_definition_name, \
         provider_definition_version, credentials) \
         VALUES ('c', 'def', 1, '{\"type\":\"api_key\",\"api_key\":\"k\"}');",
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
        (
            "compaction",
            "INSERT INTO provider_definitions (name, version, format, inference, credential, \
             created_at, compaction) VALUES ('badcompact', 1, 'openai_chat_completion', '{}', \
             '{}', 1, 'not json');",
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
             provider_definition_version, credentials) \
             VALUES ('p', 'unknown', 1, '{API_KEY_CREDENTIALS}');"
        ))
        .await;

    assert_constraint_violation(outcome, "FOREIGN KEY");

    let outcome = connection
        .execute_batch(&format!(
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, credentials) \
             VALUES ('p', 'def', 2, '{API_KEY_CREDENTIALS}');"
        ))
        .await;

    assert_constraint_violation(outcome, "FOREIGN KEY");

    connection
        .execute_batch(&format!(
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, credentials) \
             VALUES ('p', 'def', 1, '{API_KEY_CREDENTIALS}');"
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
             provider_definition_version, credentials) \
             VALUES ('p', 'def', 1, 'not json');",
        )
        .await;

    assert_constraint_violation(outcome, "credentials");

    // An arbitrary JSON object is storable — the descriptor shape is validated above it.
    connection
        .execute_batch(
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, credentials) \
             VALUES ('p', 'def', 1, '{}');",
        )
        .await?;

    Ok(())
}

/// Verifies the `models` capability-declaration column domains (README §3): the four JSON
/// columns accept any JSON or `NULL` and reject non-JSON, and every token column accepts a
/// positive integer or `NULL` and rejects a non-positive one — the domain the Rust write
/// boundary rests on, with the level vocabulary itself enforced above SQL.
///
/// # Errors
///
/// Returns an error if a statement fails.
///
/// # Panics
///
/// Panics if an out-of-domain write is not rejected as a constraint violation.
#[tokio::test]
async fn models_enforce_capability_column_domains() -> Result<(), Box<dyn StdError>> {
    let connection = open_fresh(SCHEMA.schema).await?;
    insert_probe_definition(&connection, "def").await?;
    connection
        .execute_batch(&format!(
            "INSERT INTO providers (name, provider_definition_name, \
             provider_definition_version, credentials) \
             VALUES ('p', 'def', 1, '{API_KEY_CREDENTIALS}');"
        ))
        .await?;

    // The base row every probe mutates: a capability-free manual model, every column `NULL`.
    connection
        .execute_batch(
            "INSERT INTO models (provider_id, upstream_id, discovery_state) \
             VALUES (1, 'm', 'manual');",
        )
        .await?;

    // Arbitrary JSON — including the empty array, the positive declaration of no levels —
    // is storable in both level columns: their element vocabulary is the Rust boundary's.
    // The compaction fact columns share the shape-free `json_valid` boundary: the closed
    // mechanism and support vocabularies are the Rust write boundary's, not SQL's.
    connection
        .execute_batch(
            "UPDATE models SET thinking_levels = '[]', thinking_levels_override = '[1]', \
             compaction_support = '{}', compaction_support_override = \
             '{\"messages_on_demand_compaction\": \"unknown\"}' \
             WHERE upstream_id = 'm';",
        )
        .await?;

    let probes = [
        (
            "thinking_levels",
            "UPDATE models SET thinking_levels = 'not json' WHERE upstream_id = 'm';",
        ),
        (
            "thinking_levels_override",
            "UPDATE models SET thinking_levels_override = 'not json' WHERE upstream_id = 'm';",
        ),
        (
            "compaction_support",
            "UPDATE models SET compaction_support = 'not json' WHERE upstream_id = 'm';",
        ),
        (
            "compaction_support_override",
            "UPDATE models SET compaction_support_override = 'not json' \
             WHERE upstream_id = 'm';",
        ),
        (
            "max_input_tokens",
            "UPDATE models SET max_input_tokens = 0 WHERE upstream_id = 'm';",
        ),
        (
            "max_output_tokens",
            "UPDATE models SET max_output_tokens = -1 WHERE upstream_id = 'm';",
        ),
        (
            "max_input_tokens_override",
            "UPDATE models SET max_input_tokens_override = 0 WHERE upstream_id = 'm';",
        ),
        (
            "max_output_tokens_override",
            "UPDATE models SET max_output_tokens_override = -1 WHERE upstream_id = 'm';",
        ),
    ];

    for (column, statement) in probes {
        let outcome = connection.execute_batch(statement).await;
        assert_constraint_violation(outcome, column);
    }

    // A positive token and the return to `NULL` — the unknown state — both write cleanly.
    connection
        .execute_batch(
            "UPDATE models SET max_input_tokens = 256000, max_output_tokens_override = 128000, \
             thinking_levels = NULL WHERE upstream_id = 'm';",
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
             provider_definition_version, credentials) \
             VALUES ('p', 'def', 1, '{API_KEY_CREDENTIALS}');"
        ))
        .await?;

    assert_identity_not_recycled(
        &connection,
        "INSERT INTO models (provider_id, upstream_id, discovery_state) \
         VALUES (1, 'm-1', 'manual');",
        "INSERT INTO models (provider_id, upstream_id, discovery_state) \
         VALUES (1, 'm-2', 'manual');",
        "DELETE FROM models WHERE upstream_id = 'm-2';",
        "INSERT INTO models (provider_id, upstream_id, discovery_state) \
         VALUES (1, 'm-3', 'manual');",
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
                ", 'new-provider', ''"
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
                 expires_at, provider_name, code_verifier) \
                 VALUES ('def', 1, 'st-p2', 'reauthorize', 'authorization_code', \
                 '{}', 1000, 'n', 'v');"
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
             expires_at, provider_name, code_verifier, device_code) \
             VALUES ('def', 1, 'st-g1', 'create', 'authorization_code', '{}', 1000, \
             'n', 'v', 'dc');",
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
