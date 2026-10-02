//! Api-log schema version 1: the initial api-log file layout (README §3 *ApiLog Store*).
//!
//! Version 1 creates the `api_logs` table and its four indexes — moved verbatim from the main
//! schema's history into the api-log store's own registry — so its [`Schema::migration`] is
//! empty: there is no previous version to migrate from. A fresh api-log file receives this
//! version's DDL directly, and every later version's migration transforms it.
//!
//! The version's SQL opens by stamping the file's registry identity into `PRAGMA application_id`
//! — the fourcc `JGAL` — written by version 1's SQL itself and never at runtime. The bootstrap
//! verifies the stamp on any file that already carries a schema version, so a file opened under
//! the wrong registry — or a file predating the stamp, an unreleased development artifact the
//! operator recreates — fails at startup instead of at first query.
//!
//! `api_logs` is request accounting: one row per dispatched inference request:
//!
//! - **Dispatch is the recording boundary**: a row exists exactly when the upstream HTTP request
//!   was issued (`send()` called). Everything before that instant — authentication, routing 404s,
//!   token refresh, upstream payload rendering failures — never reaches this table.
//! - `id` deliberately carries no `AUTOINCREMENT`: no identity reference points at `api_logs` rows
//!   and the id is never published — it stays internal as the per-file tie-break of the list
//!   ordering (`ORDER BY started_at DESC, id DESC`), so its number may be recycled.
//! - No foreign keys: `api-keys:delete`, `providers:delete`, and `models:delete` neither cascade
//!   into nor block log rows. A deleted row's logs remain untouched and keep attributing by id —
//!   sound precisely because the referenced identities (`providers.id`, `api_keys.id`, `models.id`)
//!   are never recycled.
//! - No name snapshot: labels are resolved by the frontend through `api-keys:list`,
//!   `providers:list`, and `models:list`.
//! - The model is referenced by `model_id` — a bare `models.id` integer — for the same reason
//!   `api_key_id` and `provider_id` are bare integers: no FK, so a `models:delete` leaves the log
//!   row intact and attribution by id unchanged.
//! - `http_status` is `NULL` exactly when no response was ever produced to the client, and
//!   otherwise a status `CHECK`-pinned to 100-999 — the full `StatusCode` domain the gateway
//!   mirrors verbatim from the upstream response, so a recorded status is never rejected at write
//!   time; the per-outcome decision rules live in the recording code.
//! - `duration` counts whole milliseconds from the dispatch stamp to the recording instant; the
//!   recording code saturates a wall clock stepped backwards, and the schema `CHECK`-pins the
//!   result non-negative.
//! - The five token columns are `NULL` when the upstream reported no usage; `output_tokens`
//!   includes `reasoning_tokens`.
//! - `started_at` (dispatch-start instant, microseconds) is indexed alone
//!   (`idx_api_logs_started_at`) and in one composite index per attribution id
//!   (`idx_api_logs_api_key_started_at`, `idx_api_logs_provider_started_at`,
//!   `idx_api_logs_model_started_at`): the management filters pair a high-selectivity id with a
//!   time window, while the low-cardinality columns — `outcome`, `endpoint_type`, `is_stream`,
//!   `http_status` — are deliberately not indexed. List reads order by `started_at DESC, id DESC`.
//! - The requested reasoning effort and any content — prompt, completion, headers, upstream error
//!   bodies — are never recorded.

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

/// Complete DDL of version 1, applied to a fresh api-log file.
const SCHEMA_SQL: &str = r###"
PRAGMA application_id = 0x4A47414C;

CREATE TABLE api_logs (
    id INTEGER PRIMARY KEY NOT NULL,
    started_at INTEGER NOT NULL,
    api_key_id INTEGER NOT NULL,
    provider_id INTEGER NOT NULL,
    model_id INTEGER NOT NULL,
    endpoint_type TEXT NOT NULL CHECK (
        endpoint_type IN (
            'openai_chat_completions',
            'openai_responses',
            'openai_responses_compact',
            'gemini_generate_content',
            'gemini_interactions',
            'anthropic_messages'
        )
    ),
    is_stream INTEGER NOT NULL CHECK (is_stream IN (0, 1)),
    http_status INTEGER NULL CHECK (http_status IS NULL OR http_status BETWEEN 100 AND 999),
    outcome TEXT NOT NULL CHECK (
        outcome IN ('completed', 'client_aborted', 'upstream_error', 'internal')
    ),
    duration INTEGER NOT NULL CHECK (duration >= 0),
    input_tokens INTEGER NULL,
    output_tokens INTEGER NULL,
    cached_tokens INTEGER NULL,
    cache_write_tokens INTEGER NULL,
    reasoning_tokens INTEGER NULL
) STRICT;

CREATE INDEX idx_api_logs_started_at ON api_logs (started_at);

CREATE INDEX idx_api_logs_api_key_started_at ON api_logs (api_key_id, started_at);

CREATE INDEX idx_api_logs_provider_started_at ON api_logs (provider_id, started_at);

CREATE INDEX idx_api_logs_model_started_at ON api_logs (model_id, started_at);
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

    /// Verifies the `api_logs.endpoint_type` closed domain: an endpoint outside the six outward
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
        let connection = open_fresh(SCHEMA.schema).await?;

        let outcome = connection
            .execute_batch(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 endpoint_type, is_stream, outcome, duration) \
                 VALUES (1, 1, 1, 1, 'grpc', 0, 'completed', 1);",
            )
            .await;

        assert_constraint_violation(outcome, "endpoint_type");

        // The six legal endpoint values all write cleanly.
        for (index, endpoint) in [
            "openai_chat_completions",
            "openai_responses",
            "openai_responses_compact",
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

    /// Verifies the `api_logs` index set on a fresh api-log file: exactly the four declared indexes
    /// exist, each carrying its documented columns in order — the time index alone, and one
    /// composite per attribution id pairing the id with the dispatch time.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement or query fails.
    ///
    /// # Panics
    ///
    /// Panics if an index is missing, unexpected, or carries different columns than declared.
    #[tokio::test]
    async fn api_logs_carry_the_documented_indexes() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;

        let mut rows = connection
            .query(
                "SELECT name, sql FROM sqlite_schema WHERE type = 'index' \
                 AND tbl_name = 'api_logs' ORDER BY name",
                (),
            )
            .await?;

        let mut indexes = Vec::new();

        while let Some(row) = rows.next().await? {
            let name: String = row.get(0)?;
            let sql: String = row.get(1)?;

            indexes.push((name, sql));
        }

        assert_eq!(
            indexes
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [
                "idx_api_logs_api_key_started_at",
                "idx_api_logs_model_started_at",
                "idx_api_logs_provider_started_at",
                "idx_api_logs_started_at",
            ],
            "the api_logs index set is exactly the four documented indexes"
        );

        // The index columns and their order read back from the stored DDL, parsed like the
        // canonical schema comparison parses it.
        for (name, sql) in &indexes {
            let indexed_columns = indexed_columns(sql)?;

            #[expect(
                clippy::panic,
                reason = "an unexpected stored index fails the test loudly"
            )]
            let expected: &[&str] = match name.as_str() {
                "idx_api_logs_started_at" => &["started_at"],
                "idx_api_logs_api_key_started_at" => &["api_key_id", "started_at"],
                "idx_api_logs_provider_started_at" => &["provider_id", "started_at"],
                "idx_api_logs_model_started_at" => &["model_id", "started_at"],
                other => panic!("unexpected api_logs index {other}"),
            };

            assert_eq!(&indexed_columns, expected, "columns of {name}");
        }

        Ok(())
    }

    /// Parses one `CREATE INDEX` statement and renders its indexed columns in declared order.
    ///
    /// # Errors
    ///
    /// Returns an error if the statement fails to parse or is not a `CREATE INDEX`.
    ///
    /// # Panics
    ///
    /// Panics if an indexed column is an expression rather than a plain column reference, which
    /// the schema never declares.
    fn indexed_columns(sql: &str) -> Result<Vec<String>, Box<dyn StdError>> {
        use turso_parser::{
            ast::{Cmd, Expr, Stmt},
            parser::Parser,
        };

        let mut parser = Parser::new(sql.as_bytes());

        let statement = match parser.next() {
            Some(Ok(Cmd::Stmt(Stmt::CreateIndex { columns, .. }))) => columns,
            _ => return Err("expected one CREATE INDEX statement".into()),
        };

        #[expect(
            clippy::panic,
            reason = "the schema declares plain column indexes only"
        )]
        let names = statement
            .iter()
            .map(|column| match &*column.expr {
                Expr::Id(name) => name.to_string(),
                other => panic!("index declares an expression column: {other:?}"),
            })
            .collect::<Vec<String>>();

        Ok(names)
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
        let connection = open_fresh(SCHEMA.schema).await?;

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
        let connection = open_fresh(SCHEMA.schema).await?;

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

    /// Verifies the `api_logs.duration` domain: a negative duration is rejected at write time by
    /// the schema `CHECK` — the recording code saturates a wall clock stepped backwards, so a
    /// negative value can only be a bug.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if the negative duration is not rejected as a constraint violation.
    #[tokio::test]
    async fn api_logs_reject_a_negative_duration() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;

        let outcome = connection
            .execute_batch(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 endpoint_type, is_stream, outcome, duration) \
                 VALUES (1, 1, 1, 1, 'openai_responses', 0, 'completed', -1);",
            )
            .await;

        assert_constraint_violation(outcome, "duration");

        Ok(())
    }

    /// Verifies the `api_logs.http_status` closed domain: a status outside the mirrored
    /// `StatusCode` domain 100-999 is rejected at write time by the schema `CHECK`, the
    /// boundaries write cleanly, and `NULL` — the no-response-was-produced state — stays legal.
    ///
    /// # Errors
    ///
    /// Returns an error if a statement fails.
    ///
    /// # Panics
    ///
    /// Panics if an illegal status is not rejected as a constraint violation.
    #[tokio::test]
    async fn api_logs_reject_an_illegal_http_status() -> Result<(), Box<dyn StdError>> {
        let connection = open_fresh(SCHEMA.schema).await?;

        for status in [99, 1000] {
            let outcome = connection
                .execute_batch(&format!(
                    "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                     endpoint_type, is_stream, http_status, outcome, duration) \
                     VALUES (1, 1, 1, 1, 'openai_responses', 0, {status}, 'completed', 1);",
                ))
                .await;

            assert_constraint_violation(outcome, "http_status");
        }

        // The range boundaries and the explicit `NULL` state all write cleanly.
        for (index, status) in ["100", "999", "NULL"].iter().enumerate() {
            connection
                .execute_batch(&format!(
                    "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                     endpoint_type, is_stream, http_status, outcome, duration) \
                     VALUES ({index}, 1, 1, 1, 'openai_responses', 0, {status}, \
                     'completed', 1);",
                ))
                .await?;
        }

        Ok(())
    }
}
