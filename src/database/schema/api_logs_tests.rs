//! Fresh api-log schema invariants, indexes, and closed storage domains.

use super::{api_logs_version_1::SCHEMA, assert_constraint_violation, open_fresh};
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
             model_route_id, \
             endpoint_type, is_stream, outcome, duration) \
             VALUES (1, 1, 1, 1, 1, 'grpc', 0, 'completed', 1);",
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
                 model_route_id, \
                 endpoint_type, is_stream, outcome, duration) \
                 VALUES ({index}, 1, 1, 1, 1, '{endpoint}', 0, 'completed', 1);",
            ))
            .await?;
    }

    Ok(())
}

/// Verifies the `api_logs` index set on a fresh api-log file: exactly the five declared indexes
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
            "idx_api_logs_model_route_started_at",
            "idx_api_logs_model_started_at",
            "idx_api_logs_provider_started_at",
            "idx_api_logs_started_at",
        ],
        "the api_logs index set is exactly the five documented indexes"
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
            "idx_api_logs_model_route_started_at" => &["model_route_id", "started_at"],
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
             model_route_id, \
             endpoint_type, is_stream, outcome, duration) \
             VALUES (1, 1, 1, 1, 1, 'openai_responses', 0, 'retrying', 1);",
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
             model_route_id, \
             endpoint_type, is_stream, outcome, duration) \
             VALUES (1, 1, 1, 1, 1, 'openai_chat_completions', 2, 'completed', 1);",
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
             model_route_id, \
             endpoint_type, is_stream, outcome, duration) \
             VALUES (1, 1, 1, 1, 1, 'openai_responses', 0, 'completed', -1);",
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
                 model_route_id, \
                 endpoint_type, is_stream, http_status, outcome, duration) \
                 VALUES (1, 1, 1, 1, 1, 'openai_responses', 0, {status}, 'completed', 1);",
            ))
            .await;

        assert_constraint_violation(outcome, "http_status");
    }

    // The range boundaries and the explicit `NULL` state all write cleanly.
    for (index, status) in ["100", "999", "NULL"].iter().enumerate() {
        connection
            .execute_batch(&format!(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 model_route_id, \
                 endpoint_type, is_stream, http_status, outcome, duration) \
                 VALUES ({index}, 1, 1, 1, 1, 'openai_responses', 0, {status}, \
                 'completed', 1);",
            ))
            .await?;
    }

    Ok(())
}
