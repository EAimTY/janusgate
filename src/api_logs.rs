//! Request accounting: the `api_logs` write path and read path (README §3 *ApiLog*).
//!
//! One row is one dispatched inference request — the instant the upstream HTTP request was issued
//! (`send()`). Everything before that instant (authentication, routing, token refresh, upstream
//! payload rendering) is never recorded, and no content — prompt, completion, headers, upstream
//! error bodies — ever reaches the table. The draft lifecycle ([`ApiLogDraft`]) and the read
//! surface live beside this write path in the same module as the table statements.
//!
//! Recording is best-effort by settled boundary, not by accident: a non-streaming client's
//! disconnect *after* the response has been handed to hyper is unobservable — that row records
//! `completed` with the status it rendered — and a connection that cancels the handler while it
//! still awaits the upstream response never reaches a finalization point, so it writes no row at
//! all. Those two classes of missed recording are the documented edge of the accounting
//! semantics (README §3 *ApiLog*).
//!
//! Storage failures reuse the provider storage error domain ([`Error`]), so a constraint
//! backstop classifies as a conflict on the management taxonomy with no new mapping, and a
//! recorded row's closed-vocabulary columns parse back as drift rather than as a client fault.

use crate::{
    database::{Database, Row, Value, params},
    ir::IrUsage,
    provider::storage::Error,
    unix_time,
};
use log::error;
use serde::{Deserialize, Serialize};

/// Column order of api log selects, consumed positionally by [`api_log_from_row`].
const API_LOG_COLUMNS: &str = "id, started_at, api_key_id, provider_id, model_id, \
     endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
     output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens";

/// The outward completion endpoint a request arrived on, as recorded in `api_logs.endpoint_type`.
///
/// The serde and TypeScript names are the storage names — the `CHECK` vocabulary of the column
/// and [`ApiLogEndpointType::as_str`] are one vocabulary (README §3 *ApiLog*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum ApiLogEndpointType {
    /// `POST /api/openai-v1/chat/completions`.
    #[serde(rename = "openai_chat_completions")]
    OpenaiChatCompletions,

    /// `POST /api/openai-v1/responses`.
    #[serde(rename = "openai_responses")]
    OpenaiResponses,
}

impl ApiLogEndpointType {
    /// Renders the storage spelling of the endpoint type — the exact string the schema `CHECK`
    /// accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiChatCompletions => "openai_chat_completions",
            Self::OpenaiResponses => "openai_responses",
        }
    }

    /// Parses the stored spelling back into the closed set; anything outside it is storage drift,
    /// because the column is written only through [`ApiLogEndpointType::as_str`] and its schema
    /// `CHECK` pins the vocabulary.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "openai_chat_completions" => Some(Self::OpenaiChatCompletions),
            "openai_responses" => Some(Self::OpenaiResponses),
            _ => None,
        }
    }
}

/// The recorded outcome of one dispatched request, as stored in `api_logs.outcome`.
///
/// The serde and TypeScript names are the storage names — the `CHECK` vocabulary of the column
/// and [`ApiLogOutcome::as_str`] are one vocabulary (README §3 *ApiLog*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum ApiLogOutcome {
    /// The response completed normally: `completed` + `200`.
    #[serde(rename = "completed")]
    Completed,

    /// The client disconnected: before the first enqueued frame `http_status` stays `NULL`,
    /// afterwards the already-declared `200` stands.
    #[serde(rename = "client_aborted")]
    ClientAborted,

    /// The upstream failed — a mirrored non-2xx status, a transport failure mapped to 502, or a
    /// mid-stream failure under an already-declared `200`.
    #[serde(rename = "upstream_error")]
    UpstreamError,

    /// A gateway-side failure under an already-declared `200`.
    #[serde(rename = "internal")]
    Internal,
}

impl ApiLogOutcome {
    /// Renders the storage spelling of the outcome — the exact string the schema `CHECK` accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ClientAborted => "client_aborted",
            Self::UpstreamError => "upstream_error",
            Self::Internal => "internal",
        }
    }

    /// Parses the stored spelling back into the closed set; see
    /// [`ApiLogEndpointType::parse`] for the drift classification.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "completed" => Some(Self::Completed),
            "client_aborted" => Some(Self::ClientAborted),
            "upstream_error" => Some(Self::UpstreamError),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

/// The bucketing dimension of the usage aggregation (README §7.1 `usage:query`): a closed,
/// unordered set — the type is the validation, so any other value is rejected while the request
/// payload parses. The serde and TypeScript names are the wire names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum UsageGroupBy {
    /// UTC day of `started_at`, rendered `YYYY-MM-DD`.
    #[serde(rename = "day")]
    Day,

    /// Routed model row id.
    #[serde(rename = "model")]
    Model,

    /// Owning provider row id.
    #[serde(rename = "provider")]
    Provider,

    /// Authenticating API key row id.
    #[serde(rename = "api_key")]
    ApiKey,
}

/// The bucket key of one aggregation row (README §7.1): the UTC day string when grouping by day,
/// or the grouped row id — model, provider, or API key — otherwise. The two JSON shapes are
/// distinct, so the closed sum parses back without a tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum UsageBucketKey {
    /// A UTC day in `YYYY-MM-DD`.
    Day(String),

    /// A grouped row id.
    Id(i64),
}

/// One accounting row about to be inserted: every column but the server-assigned `id`.
#[derive(Debug, Clone)]
pub(crate) struct ApiLogWrite {
    /// Dispatch-start instant in UTC microseconds; stamped the moment `send()` is called.
    pub(crate) started_at: i64,

    /// Row id of the authenticated gateway API key.
    pub(crate) api_key_id: i64,

    /// Row id of the routed provider.
    pub(crate) provider_id: i64,

    /// Row id of the routed model.
    pub(crate) model_id: i64,

    /// Outward completion endpoint the request arrived on.
    pub(crate) endpoint_type: ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    pub(crate) is_stream: bool,

    /// HTTP status delivered to the client; `NULL` exactly when no response was ever produced
    /// (README §3 *ApiLog*).
    pub(crate) http_status: Option<i64>,

    /// Recorded outcome of the request.
    pub(crate) outcome: ApiLogOutcome,

    /// Milliseconds between the dispatch stamp and the accounting instant.
    pub(crate) duration: i64,

    /// Tokens consumed by the input; `NULL` when the upstream reported no usage.
    pub(crate) input_tokens: Option<i64>,

    /// Tokens produced by the output, `reasoning_tokens` included; `NULL` without usage.
    pub(crate) output_tokens: Option<i64>,

    /// Input tokens served from the upstream prompt cache; `NULL` without that breakdown.
    pub(crate) cached_tokens: Option<i64>,

    /// Input tokens written to the upstream prompt cache; `NULL` without that breakdown.
    pub(crate) cache_write_tokens: Option<i64>,

    /// Output tokens spent on reasoning; `NULL` without that breakdown.
    pub(crate) reasoning_tokens: Option<i64>,
}

/// One dispatched request's accounting draft: every fact known before dispatch, carried through
/// [`crate::gateway::execute`] and closed by exactly one of the three finalization points.
///
/// The module's single invariant: **`started_at` is stamped ⇔ the request was dispatched with
/// accounting enabled**. Who stamps, who finalizes, and whether a row is written are all decided
/// by that one variable — [`ApiLogDraft::stamp`] reads the live `api_logs.is_enabled` switch, and
/// [`finalize_api_log`] writes if and only if a stamp is present.
#[derive(Debug, Clone)]
pub(crate) struct ApiLogDraft {
    /// Dispatch-start instant in UTC microseconds; `None` until [`ApiLogDraft::stamp`] runs at the
    /// `send()` call, and permanently `None` when accounting is disabled.
    pub(crate) started_at: Option<i64>,

    /// Row id of the authenticated gateway API key.
    pub(crate) api_key_id: i64,

    /// Row id of the routed provider.
    pub(crate) provider_id: i64,

    /// Row id of the routed model.
    pub(crate) model_id: i64,

    /// Outward completion endpoint the request arrived on.
    pub(crate) endpoint_type: ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    pub(crate) is_stream: bool,
}

impl ApiLogDraft {
    /// Marks the dispatch instant, called the moment before the upstream `send()` leaves: stamps
    /// `started_at` from the database clock when `api_logs.is_enabled` says so, read live from
    /// the global configuration so a reload applies to the very next dispatch. With the switch
    /// off the draft stays unstamped and no row is ever written for this request — this one call
    /// is the whole switch.
    pub(crate) fn stamp(&mut self) {
        if crate::CONFIG.api_logs.is_enabled {
            self.started_at = Some(unix_time::unix_now_micros());
        }
    }
}

/// Closes one dispatched request's accounting: builds the terminal write from `draft` plus the
/// finalization facts and inserts it. Every call site runs at exactly one of the three mutually
/// exclusive finalization points (README §3 *ApiLog*), so each dispatched request is recorded
/// exactly once.
///
/// This function never reports failure outward — an accounting INSERT failure must never enter
/// any HTTP error domain (README §8): it is logged at error level with the key, provider, and
/// model row ids for attribution and swallowed, leaving the served response and every later
/// database operation untouched. An unstamped draft — the request was never dispatched with
/// accounting enabled — performs no INSERT at all.
///
/// `duration` is the whole span from the dispatch stamp to this finalization instant, in
/// whole milliseconds: anything before dispatch (token refresh, payload rendering) is excluded by
/// construction. `usage` is the terminal usage report when one exists; without it the five token
/// columns stay `NULL`.
pub(crate) async fn finalize_api_log(
    database: &Database,
    draft: &ApiLogDraft,
    outcome: ApiLogOutcome,
    http_status: Option<i64>,
    usage: Option<IrUsage>,
) {
    let Some(started_at) = draft.started_at else {
        return;
    };

    // A wall clock stepped backwards must not produce a negative duration.
    let duration = unix_time::unix_now_micros().saturating_sub(started_at) / 1000;
    let (input_tokens, output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens) =
        token_columns(usage);

    let write = ApiLogWrite {
        started_at,
        api_key_id: draft.api_key_id,
        provider_id: draft.provider_id,
        model_id: draft.model_id,
        endpoint_type: draft.endpoint_type,
        is_stream: draft.is_stream,
        http_status,
        outcome,
        duration,
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_write_tokens,
        reasoning_tokens,
    };

    if let Err(failure) = record_api_log(database, write).await {
        error!(
            "failed to record api log for API key {} (provider {}, model {}): {failure}",
            draft.api_key_id, draft.provider_id, draft.model_id,
        );
    }
}

/// Lists accounting rows newest first: at most `limit` rows whose id is strictly below
/// `before_id` — the cursor a client derives from the previous page's last row — ordered by `id`
/// descending, so the rowid order doubles as the pagination order over the insertion history.
/// An absent `before_id` starts at the newest row.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_api_logs(
    database: &Database,
    limit: i64,
    before_id: Option<i64>,
) -> Result<Vec<ApiLog>, Error> {
    let (sql, values) = match before_id {
        Some(before_id) => (
            format!(
                "SELECT {API_LOG_COLUMNS} FROM api_logs WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
            ),
            vec![before_id, limit],
        ),

        None => (
            format!("SELECT {API_LOG_COLUMNS} FROM api_logs ORDER BY id DESC LIMIT ?1"),
            vec![limit],
        ),
    };

    let rows = database.query(sql, values).await?;

    rows.iter().map(api_log_from_row).collect()
}

/// Aggregates accounting rows into `selection`'s buckets: one bucket per distinct value of the
/// grouping dimension inside the time window and filters, ordered by that key ascending — an ISO
/// day string orders chronologically, a row id numerically. `requests` counts rows; each token
/// column sums with `NULL` excluded from the sum and an all-`NULL` bucket rendering `0`, so
/// unreported usage never inflates or deflates a total.
///
/// The window binds `started_at` directly, so an aggregation with any bound reads through
/// `idx_api_logs_started_at`; an empty selection aggregates the whole table.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn query_usage(
    database: &Database,
    selection: &UsageSelection,
) -> Result<Vec<UsageAggregate>, Error> {
    let key_expression = match selection.group_by {
        UsageGroupBy::Day => "date(started_at / 1000000, 'unixepoch')",
        UsageGroupBy::Model => "model_id",
        UsageGroupBy::Provider => "provider_id",
        UsageGroupBy::ApiKey => "api_key_id",
    };

    // Conditions and values build together, so every `?N` placeholder binds the value its clause
    // pushed: window bounds first (they ride the index), then the locating filters.
    let mut conditions: Vec<String> = Vec::new();
    let mut values: Vec<i64> = Vec::new();

    let mut push = |condition: &str, value: i64| {
        values.push(value);
        conditions.push(format!("{condition} ?{}", values.len()));
    };

    if let Some(from) = selection.from {
        push("started_at >=", from);
    }

    if let Some(to) = selection.to {
        push("started_at <", to);
    }

    if let Some(model_id) = selection.model_id {
        push("model_id =", model_id);
    }

    if let Some(provider_id) = selection.provider_id {
        push("provider_id =", provider_id);
    }

    if let Some(api_key_id) = selection.api_key_id {
        push("api_key_id =", api_key_id);
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT {key_expression}, COUNT(*), \
         COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), \
         COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(cache_write_tokens), 0), \
         COALESCE(SUM(reasoning_tokens), 0) \
         FROM api_logs{where_clause} \
         GROUP BY {key_expression} ORDER BY {key_expression}"
    );

    let rows = database.query(sql, values).await?;

    rows.iter()
        .map(|row| usage_aggregate_from_row(row, selection.group_by))
        .collect()
}

/// Parses an accounting row in [`API_LOG_COLUMNS`] order.
fn api_log_from_row(row: &Row) -> Result<ApiLog, Error> {
    Ok(ApiLog {
        id: row.get(0)?,
        started_at: row.get(1)?,
        api_key_id: row.get(2)?,
        provider_id: row.get(3)?,
        model_id: row.get(4)?,
        endpoint_type: endpoint_type_from_value(row.get_value(5)?)?,
        is_stream: row.get::<i64>(6)? != 0,
        http_status: row.get(7)?,
        outcome: outcome_from_value(row.get_value(8)?)?,
        duration: row.get(9)?,
        input_tokens: row.get(10)?,
        output_tokens: row.get(11)?,
        cached_tokens: row.get(12)?,
        cache_write_tokens: row.get(13)?,
        reasoning_tokens: row.get(14)?,
    })
}

/// Parses one aggregation row: the key in the grouping's own shape, then the count and the five
/// coalesced sums.
fn usage_aggregate_from_row(row: &Row, group_by: UsageGroupBy) -> Result<UsageAggregate, Error> {
    let key = match group_by {
        UsageGroupBy::Day => UsageBucketKey::Day(row.get(0)?),
        UsageGroupBy::Model | UsageGroupBy::Provider | UsageGroupBy::ApiKey => {
            UsageBucketKey::Id(row.get(0)?)
        }
    };

    Ok(UsageAggregate {
        key,
        requests: row.get(1)?,
        input_tokens: row.get(2)?,
        output_tokens: row.get(3)?,
        cached_tokens: row.get(4)?,
        cache_write_tokens: row.get(5)?,
        reasoning_tokens: row.get(6)?,
    })
}

/// Reads the `endpoint_type` column, failing with internal drift on anything outside the closed
/// vocabulary — see [`ApiLogEndpointType::parse`].
fn endpoint_type_from_value(value: Value) -> Result<ApiLogEndpointType, Error> {
    match value {
        Value::Text(text) => ApiLogEndpointType::parse(&text).ok_or_else(|| Error::Drift {
            message: format!("unknown api log endpoint type `{text}`"),
        }),

        _ => Err(Error::Drift {
            message: "endpoint_type column must be a text value".to_string(),
        }),
    }
}

/// Reads the `outcome` column; see [`ApiLogEndpointType::parse`] for the drift classification.
fn outcome_from_value(value: Value) -> Result<ApiLogOutcome, Error> {
    match value {
        Value::Text(text) => ApiLogOutcome::parse(&text).ok_or_else(|| Error::Drift {
            message: format!("unknown api log outcome `{text}`"),
        }),

        _ => Err(Error::Drift {
            message: "outcome column must be a text value".to_string(),
        }),
    }
}

/// The five token columns of one accounting row, in column order; all `NULL` when the upstream
/// reported no usage.
type TokenColumns = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// Maps a terminal usage report onto the five token columns: no usage yields five `NULL`s, and a
/// breakdown the upstream did not report stays `NULL` beside its reported totals. Upstream token
/// counts are third-party integers — one beyond `i64` saturates instead of failing the
/// accounting.
fn token_columns(usage: Option<IrUsage>) -> TokenColumns {
    let Some(usage) = usage else {
        return (None, None, None, None, None);
    };

    let stored = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);

    (
        Some(stored(usage.input_tokens)),
        Some(stored(usage.output_tokens)),
        usage.cached_tokens.map(stored),
        usage.cache_write_tokens.map(stored),
        usage.reasoning_tokens.map(stored),
    )
}

/// One stored accounting row: all fifteen columns exactly as stored — the row the management
/// read surface mirrors (README §3 *ApiLog*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiLog {
    /// Row identifier; the pagination cursor of `usage:list`, in insertion order.
    pub(crate) id: i64,

    /// Dispatch-start instant in UTC microseconds.
    pub(crate) started_at: i64,

    /// Row id of the authenticated gateway API key.
    pub(crate) api_key_id: i64,

    /// Row id of the routed provider.
    pub(crate) provider_id: i64,

    /// Row id of the routed model.
    pub(crate) model_id: i64,

    /// Outward completion endpoint the request arrived on.
    pub(crate) endpoint_type: ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    pub(crate) is_stream: bool,

    /// HTTP status delivered to the client; `NULL` when no response was ever produced.
    pub(crate) http_status: Option<i64>,

    /// Recorded outcome of the request.
    pub(crate) outcome: ApiLogOutcome,

    /// Milliseconds between the dispatch stamp and the accounting instant.
    pub(crate) duration: i64,

    /// Tokens consumed by the input; `NULL` when the upstream reported no usage.
    pub(crate) input_tokens: Option<i64>,

    /// Tokens produced by the output, `reasoning_tokens` included; `NULL` without usage.
    pub(crate) output_tokens: Option<i64>,

    /// Input tokens served from the upstream prompt cache; `NULL` without that breakdown.
    pub(crate) cached_tokens: Option<i64>,

    /// Input tokens written to the upstream prompt cache; `NULL` without that breakdown.
    pub(crate) cache_write_tokens: Option<i64>,

    /// Output tokens spent on reasoning; `NULL` without that breakdown.
    pub(crate) reasoning_tokens: Option<i64>,
}

/// One aggregation bucket of `usage:query` (README §7.1): the bucket key, the row count, and the
/// token sums — `NULL` token columns never participate in a sum, so a bucket whose rows all
/// reported no usage sums to `0`, and every sum renders as a plain number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageAggregate {
    /// Bucket key: the UTC day, or the grouped row id.
    pub(crate) key: UsageBucketKey,

    /// Number of accounting rows in the bucket.
    pub(crate) requests: i64,

    /// Summed input tokens.
    pub(crate) input_tokens: i64,

    /// Summed output tokens.
    pub(crate) output_tokens: i64,

    /// Summed cached input tokens.
    pub(crate) cached_tokens: i64,

    /// Summed cache-write input tokens.
    pub(crate) cache_write_tokens: i64,

    /// Summed reasoning output tokens.
    pub(crate) reasoning_tokens: i64,
}

/// The `usage:query` selection: one bucketing plus the absent-keeps filters, each `None` when the
/// field was absent. A filter locates rather than addresses: an unknown id naturally yields an
/// empty result, and `from` is inclusive while `to` is exclusive over `started_at`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UsageSelection {
    /// Bucketing dimension.
    pub(crate) group_by: UsageGroupBy,

    /// Inclusive lower bound of `started_at`, in UTC microseconds.
    pub(crate) from: Option<i64>,

    /// Exclusive upper bound of `started_at`, in UTC microseconds.
    pub(crate) to: Option<i64>,

    /// Model row id filter.
    pub(crate) model_id: Option<i64>,

    /// Provider row id filter.
    pub(crate) provider_id: Option<i64>,

    /// API key row id filter.
    pub(crate) api_key_id: Option<i64>,
}

/// Inserts one accounting row — a single synchronous statement on a pooled connection, the same
/// pool and semantics as every other first-party write. Recording is deliberately unbatched and
/// unsynchronized: one small INSERT per dispatched request (README §3 *ApiLog*).
///
/// # Errors
///
/// Returns [`Error`] when the insert fails — including a schema `CHECK` violation, which
/// classifies as [`Error::Constraint`].
pub(crate) async fn record_api_log(database: &Database, write: ApiLogWrite) -> Result<(), Error> {
    database
        .execute(
            "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
             endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
             output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                write.started_at,
                write.api_key_id,
                write.provider_id,
                write.model_id,
                write.endpoint_type.as_str(),
                i64::from(write.is_stream),
                write.http_status,
                write.outcome.as_str(),
                write.duration,
                write.input_tokens,
                write.output_tokens,
                write.cached_tokens,
                write.cache_write_tokens,
                write.reasoning_tokens,
            ],
        )
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api_keys::{self, NewApiKey},
        database::{Database, Value},
        provider::{
            Credentials, NewProvider, ProviderType,
            storage::{self, ModelCreate},
        },
    };
    use std::{error::Error as StdError, num::NonZero, path::PathBuf, time::Duration};

    /// Renders one value through the published wire vocabulary; every rendered body of this crate
    /// is a typed payload whose serialization is total.
    ///
    /// # Panics
    ///
    /// Panics if the typed payload fails to serialize, which cannot occur.
    fn wire<T: Serialize>(value: &T) -> serde_json::Value {
        #[expect(
            clippy::expect_used,
            reason = "every wire payload is a typed value whose serialization is total"
        )]
        let rendered = serde_json::to_value(value).expect("wire payloads serialize");

        rendered
    }

    /// Reads one column of a rendered row by position.
    ///
    /// # Panics
    ///
    /// Panics if the row carries fewer columns than the schema declares.
    fn column(row: &[Value], index: usize) -> &Value {
        #[expect(
            clippy::indexing_slicing,
            reason = "the schema pins every row at fifteen columns"
        )]
        &row[index]
    }

    /// File guard cleaning up the temporary SQLite test database on drop.
    struct TempDbGuard(PathBuf);

    impl Drop for TempDbGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Creates an initialized temporary SQLite test database.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database fails.
    async fn create_test_database() -> (Database, TempDbGuard) {
        let db_path = std::env::temp_dir().join(format!(
            "janusgate-apilogs-test-{}.db",
            rand::random::<u64>()
        ));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let max_connections = NonZero::new(2).unwrap();
        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, max_connections, Duration::from_secs(5))
            .await
            .unwrap();

        (database, TempDbGuard(db_path))
    }

    /// The complete row used by the round-trip test: every nullable column populated and the
    /// non-default endpoint, outcome, and stream flag set.
    fn full_write() -> ApiLogWrite {
        ApiLogWrite {
            started_at: 1_700_000_000_000_000,
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            endpoint_type: ApiLogEndpointType::OpenaiResponses,
            is_stream: true,
            http_status: Some(200),
            outcome: ApiLogOutcome::ClientAborted,
            duration: 1234,
            input_tokens: Some(11),
            output_tokens: Some(22),
            cached_tokens: Some(3),
            cache_write_tokens: Some(4),
            reasoning_tokens: Some(5),
        }
    }

    /// Reads every column of every stored row, in insertion order, as raw values the test asserts
    /// positionally.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    async fn read_all(database: &Database) -> Result<Vec<Vec<crate::database::Value>>, Error> {
        let rows = database
            .query(
                "SELECT id, started_at, api_key_id, provider_id, model_id, endpoint_type, \
                 is_stream, http_status, outcome, duration, input_tokens, output_tokens, \
                 cached_tokens, cache_write_tokens, reasoning_tokens \
                 FROM api_logs ORDER BY id",
                (),
            )
            .await?;

        Ok(rows
            .iter()
            .map(|row| {
                (0..row.column_count())
                    .map(|index| row.get_value(index))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Verifies the write path column by column: a fully populated row round-trips exactly as
    /// written — the closed-vocabulary columns stored through `as_str`, the stream flag as
    /// `0`/`1` — and a row whose seven nullable columns are all `NULL` reads back as `NULL`
    /// rather than a default. The `as_str` spellings are asserted equal to the serde vocabulary,
    /// because the storage string and the published wire string are one vocabulary.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a stored column diverges from the written fixture.
    #[tokio::test]
    async fn records_rows_reading_back_every_column() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        record_api_log(&database, full_write()).await?;

        let sparse = ApiLogWrite {
            started_at: 2,
            api_key_id: 1,
            provider_id: 1,
            model_id: 1,
            endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
            is_stream: false,
            http_status: None,
            outcome: ApiLogOutcome::Internal,
            duration: 0,
            input_tokens: None,
            output_tokens: None,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        };

        record_api_log(&database, sparse).await?;

        let rows = read_all(&database).await?;
        assert_eq!(rows.len(), 2);

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly two rows")]
        let full = rows.first().unwrap();
        assert_eq!(column(full, 0), &Value::Integer(1));
        assert_eq!(column(full, 1), &Value::Integer(1_700_000_000_000_000));
        assert_eq!(column(full, 2), &Value::Integer(3));
        assert_eq!(column(full, 3), &Value::Integer(5));
        assert_eq!(column(full, 4), &Value::Integer(7));
        assert_eq!(
            column(full, 5),
            &Value::Text("openai_responses".to_string())
        );
        assert_eq!(column(full, 6), &Value::Integer(1));
        assert_eq!(column(full, 7), &Value::Integer(200));
        assert_eq!(column(full, 8), &Value::Text("client_aborted".to_string()));
        assert_eq!(column(full, 9), &Value::Integer(1234));
        assert_eq!(column(full, 10), &Value::Integer(11));
        assert_eq!(column(full, 11), &Value::Integer(22));
        assert_eq!(column(full, 12), &Value::Integer(3));
        assert_eq!(column(full, 13), &Value::Integer(4));
        assert_eq!(column(full, 14), &Value::Integer(5));

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly two rows")]
        let sparse = rows.get(1).unwrap();
        assert_eq!(column(sparse, 0), &Value::Integer(2));
        assert_eq!(
            column(sparse, 5),
            &Value::Text("openai_chat_completions".to_string())
        );
        assert_eq!(column(sparse, 6), &Value::Integer(0));
        assert_eq!(column(sparse, 7), &Value::Null);
        assert_eq!(column(sparse, 8), &Value::Text("internal".to_string()));

        for index in 10..15 {
            assert_eq!(column(sparse, index), &Value::Null);
        }

        // One vocabulary: the storage spelling and the published wire spelling never drift apart.
        for endpoint_type in [
            ApiLogEndpointType::OpenaiChatCompletions,
            ApiLogEndpointType::OpenaiResponses,
        ] {
            assert_eq!(
                wire(&endpoint_type),
                serde_json::Value::String(endpoint_type.as_str().to_string()),
            );
        }

        for outcome in [
            ApiLogOutcome::Completed,
            ApiLogOutcome::ClientAborted,
            ApiLogOutcome::UpstreamError,
            ApiLogOutcome::Internal,
        ] {
            assert_eq!(
                wire(&outcome),
                serde_json::Value::String(outcome.as_str().to_string()),
            );
        }

        Ok(())
    }

    /// Verifies that a schema `CHECK` rejection surfacing through this module's write path
    /// classifies as a storage constraint violation — the same classification the management
    /// taxonomy maps to a conflict — rather than as a generic engine failure. First-party writes
    /// only ever insert parsed enums, so each probe bypasses the types with a raw statement.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a rejection classifies as anything but a storage constraint error.
    #[tokio::test]
    async fn classifies_check_rejections_as_constraint_errors() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let base = "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                    endpoint_type, is_stream, outcome, duration) VALUES (1, 1, 1, 1, ";

        let rejected = [
            format!("{base}'grpc', 0, 'completed', 1);"),
            format!("{base}'openai_responses', 0, 'retrying', 1);"),
            format!("{base}'openai_responses', 2, 'completed', 1);"),
        ];

        for statement in rejected {
            let outcome = database.execute(&statement, ()).await;

            #[expect(
                clippy::expect_used,
                reason = "the schema CHECK rejects every out-of-domain value"
            )]
            let error = outcome.expect_err("an out-of-domain value must violate the schema CHECK");

            assert!(
                matches!(Error::from(error), Error::Constraint { .. }),
                "a schema CHECK violation must classify as a storage constraint error"
            );
        }

        Ok(())
    }

    /// Verifies the no-FK attribution invariant (README §3 *ApiLog*): deleting the gateway API
    /// key, the model, and the provider a recorded row references — through their first-party
    /// delete paths, the provider's cascade included — leaves the log row byte-for-byte intact,
    /// still attributing by the never-recycled ids it stores.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a delete cascades into the log row or any column diverges from the fixture.
    #[tokio::test]
    async fn keeps_rows_intact_after_referenced_rows_are_deleted() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) = create_test_database().await;

        let api_key = api_keys::insert_api_key(
            &database,
            &NewApiKey {
                name: "service",
                key: "sk-live",
                is_enabled: true,
            },
        )
        .await?;

        let provider = storage::insert_provider(
            &database,
            &NewProvider {
                name: "acme",
                model_prefix: "acme/",
                provider_type: ProviderType::OpenAiChatCompletion,
                base_url: Some("http://upstream.example"),
                credentials: &Credentials::ApiKey("secret".to_string()),
                expires_at: None,
                is_enabled: true,
            },
        )
        .await?;

        let model = storage::insert_model_checked(
            &database,
            ModelCreate {
                provider_id: provider.id,
                upstream_id: "gpt-4",
                id_override: None,
                is_enabled: true,
            },
        )
        .await?;

        record_api_log(
            &database,
            ApiLogWrite {
                started_at: 1_000,
                api_key_id: api_key.id,
                provider_id: provider.id,
                model_id: model.model.id,
                endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
                is_stream: false,
                http_status: Some(200),
                outcome: ApiLogOutcome::Completed,
                duration: 10,
                input_tokens: Some(1),
                output_tokens: Some(2),
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
        )
        .await?;

        assert!(api_keys::delete_api_key(&database, api_key.id).await?);
        storage::delete_model_checked(&database, model.model.id).await?;
        assert!(storage::delete_provider(&database, provider.id).await?);

        let rows = read_all(&database).await?;

        assert_eq!(rows.len(), 1);

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        let row = rows.first().unwrap();
        assert_eq!(column(row, 0), &Value::Integer(1));
        assert_eq!(column(row, 1), &Value::Integer(1_000));
        assert_eq!(column(row, 2), &Value::Integer(api_key.id));
        assert_eq!(column(row, 3), &Value::Integer(provider.id));
        assert_eq!(column(row, 4), &Value::Integer(model.model.id));
        assert_eq!(column(row, 7), &Value::Integer(200));
        assert_eq!(column(row, 10), &Value::Integer(1));
        assert_eq!(column(row, 11), &Value::Integer(2));

        Ok(())
    }

    /// Verifies the best-effort accounting boundary (README §8): finalizing against a database
    /// whose `api_logs` table is gone neither panics nor surfaces anywhere, and the database keeps
    /// serving reads and writes of other tables afterwards. The unstamped-draft path — the
    /// switch-off shape — performs no INSERT and returns without touching the database.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if the swallowed recording failure surfaces anywhere or a later operation fails.
    #[tokio::test]
    async fn swallows_recording_failures_and_keeps_the_database_working()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let draft = ApiLogDraft {
            started_at: Some(1_000),
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            endpoint_type: ApiLogEndpointType::OpenaiResponses,
            is_stream: false,
        };

        database.execute("DROP TABLE api_logs", ()).await?;

        // The failure is swallowed: this call returns no Result, and nothing below panics.
        finalize_api_log(
            &database,
            &draft,
            ApiLogOutcome::Completed,
            Some(200),
            Some(crate::ir::IrUsage::default()),
        )
        .await;

        // An unstamped draft never reaches the statement at all, table present or not.
        let unstamped = ApiLogDraft {
            started_at: None,
            ..draft.clone()
        };

        finalize_api_log(
            &database,
            &unstamped,
            ApiLogOutcome::Completed,
            Some(200),
            None,
        )
        .await;

        // Reads and writes of the rest of the database are unaffected.
        let api_key = api_keys::insert_api_key(
            &database,
            &NewApiKey {
                name: "still-works",
                key: "sk-after",
                is_enabled: true,
            },
        )
        .await?;

        assert_eq!(
            api_keys::find_api_key_by_id(&database, api_key.id).await?,
            Some(api_key)
        );

        Ok(())
    }

    /// One read-fixture write: fixed identity and terminal facts, caller-chosen dispatch time and
    /// token columns (`[input, output, cached, cache_write, reasoning]`).
    fn write(
        started_at: i64,
        api_key_id: i64,
        provider_id: i64,
        model_id: i64,
        tokens: [Option<i64>; 5],
    ) -> ApiLogWrite {
        let [
            input_tokens,
            output_tokens,
            cached_tokens,
            cache_write_tokens,
            reasoning_tokens,
        ] = tokens;

        ApiLogWrite {
            started_at,
            api_key_id,
            provider_id,
            model_id,
            endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
            is_stream: false,
            http_status: Some(200),
            outcome: ApiLogOutcome::Completed,
            duration: 5,
            input_tokens,
            output_tokens,
            cached_tokens,
            cache_write_tokens,
            reasoning_tokens,
        }
    }

    /// Verifies `usage:list` pagination (README §7.1): rows order by `id` descending, the limit
    /// bounds each page, `before_id` is strictly older — the cursor row itself never reappears —
    /// and a cursor at or below the oldest row yields the empty final page.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a page diverges from the documented order or cursor semantics.
    #[tokio::test]
    async fn paginates_newest_first_with_strictly_older_cursors() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        for stamp in 1..=5 {
            record_api_log(&database, write(stamp * 1_000, 1, 1, 1, [None; 5])).await?;
        }

        let ids = |logs: &[ApiLog]| logs.iter().map(|log| log.id).collect::<Vec<i64>>();

        // The default page lists every row, newest first.
        let page = list_api_logs(&database, 100, None).await?;
        assert_eq!(ids(&page), [5, 4, 3, 2, 1]);

        // The limit bounds the page, order unchanged.
        let page = list_api_logs(&database, 2, None).await?;
        assert_eq!(ids(&page), [5, 4]);

        // before_id is strict: the cursor row is excluded, only older rows follow.
        let page = list_api_logs(&database, 100, Some(4)).await?;
        assert_eq!(ids(&page), [3, 2, 1]);

        // A limit smaller than the remainder keeps walking strictly older.
        let page = list_api_logs(&database, 1, Some(3)).await?;
        assert_eq!(ids(&page), [2]);

        // A cursor at the oldest row is the empty final page.
        let page = list_api_logs(&database, 100, Some(1)).await?;
        assert!(page.is_empty());

        Ok(())
    }

    /// Verifies `usage:query` end to end over a hand-checked fixture (README §7.1): all four
    /// groupings bucket and order by their own key, sums exclude `NULL` tokens while still
    /// counting their row — an all-`NULL` bucket sums to `0` — the window is inclusive from and
    /// exclusive to, and the locating filters narrow naturally, an unknown id yielding the empty
    /// set instead of an error.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if any bucket, sum, window edge, or filter diverges from the documented semantics.
    #[tokio::test]
    async fn aggregates_four_groupings_over_filters_and_windows() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        const DAY_MICROS: i64 = 86_400_000_000;
        let first_day = 1_700_000_000_000_000; // 2023-11-14 UTC
        let second_day = first_day + DAY_MICROS; // 2023-11-15 UTC

        record_api_log(
            &database,
            write(
                first_day + 1,
                1,
                1,
                1,
                [Some(10), Some(20), Some(1), Some(2), Some(3)],
            ),
        )
        .await?;

        // Reported no usage at all: counted everywhere, summing to zero.
        record_api_log(&database, write(first_day + 2, 2, 1, 2, [None; 5])).await?;

        // Totals without a breakdown: the breakdown columns stay out of the sums.
        record_api_log(
            &database,
            write(
                second_day + 1,
                1,
                2,
                1,
                [Some(5), Some(6), None, None, None],
            ),
        )
        .await?;

        record_api_log(
            &database,
            write(
                second_day + 2,
                1,
                1,
                1,
                [Some(1), Some(2), Some(3), Some(4), Some(5)],
            ),
        )
        .await?;

        let base = UsageSelection {
            group_by: UsageGroupBy::Day,
            from: None,
            to: None,
            model_id: None,
            provider_id: None,
            api_key_id: None,
        };

        // Grouping by UTC day: ISO keys ascending, NULL-excluding sums, the usage-less row still
        // counted.
        let buckets = query_usage(&database, &UsageSelection { ..base }).await?;
        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey::Day("2023-11-14".to_string()),
                    requests: 2,
                    input_tokens: 10,
                    output_tokens: 20,
                    cached_tokens: 1,
                    cache_write_tokens: 2,
                    reasoning_tokens: 3,
                },
                UsageAggregate {
                    key: UsageBucketKey::Day("2023-11-15".to_string()),
                    requests: 2,
                    input_tokens: 6,
                    output_tokens: 8,
                    cached_tokens: 3,
                    cache_write_tokens: 4,
                    reasoning_tokens: 5,
                },
            ]
        );

        // Grouping by model: numeric keys ascending; model 2's single usage-less row sums to 0.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: UsageGroupBy::Model,
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey::Id(1),
                    requests: 3,
                    input_tokens: 16,
                    output_tokens: 28,
                    cached_tokens: 4,
                    cache_write_tokens: 6,
                    reasoning_tokens: 8,
                },
                UsageAggregate {
                    key: UsageBucketKey::Id(2),
                    requests: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]
        );

        // Grouping by provider: the partial-breakdown row contributes only its totals.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: UsageGroupBy::Provider,
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey::Id(1),
                    requests: 3,
                    input_tokens: 11,
                    output_tokens: 22,
                    cached_tokens: 4,
                    cache_write_tokens: 6,
                    reasoning_tokens: 8,
                },
                UsageAggregate {
                    key: UsageBucketKey::Id(2),
                    requests: 1,
                    input_tokens: 5,
                    output_tokens: 6,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]
        );

        // Grouping by API key.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: UsageGroupBy::ApiKey,
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey::Id(1),
                    requests: 3,
                    input_tokens: 16,
                    output_tokens: 28,
                    cached_tokens: 4,
                    cache_write_tokens: 6,
                    reasoning_tokens: 8,
                },
                UsageAggregate {
                    key: UsageBucketKey::Id(2),
                    requests: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]
        );

        // The window is inclusive at `from` — the row stamped exactly at the bound is in — and
        // exclusive at `to`.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                from: Some(second_day),
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(UsageBucketKey::Day("2023-11-15".to_string()), 2)]
        );

        let buckets = query_usage(
            &database,
            &UsageSelection {
                from: Some(first_day + 2),
                to: Some(second_day),
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(UsageBucketKey::Day("2023-11-14".to_string()), 1)]
        );

        // An empty window is an empty aggregation, not an error.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                from: Some(second_day),
                to: Some(second_day),
                ..base
            },
        )
        .await?;

        assert!(buckets.is_empty());

        // An unknown id is an empty set: filters locate, they never 404 (README §7.0).
        let buckets = query_usage(
            &database,
            &UsageSelection {
                model_id: Some(999),
                ..base
            },
        )
        .await?;

        assert!(buckets.is_empty());

        // A filter narrows the grouping: model 1 only, still bucketed by day.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                model_id: Some(1),
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![
                (UsageBucketKey::Day("2023-11-14".to_string()), 1),
                (UsageBucketKey::Day("2023-11-15".to_string()), 2),
            ]
        );

        // Filters compose with each other, the window, and the grouping.
        let buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: UsageGroupBy::Provider,
                from: Some(second_day),
                model_id: Some(1),
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey::Id(1),
                    requests: 1,
                    input_tokens: 1,
                    output_tokens: 2,
                    cached_tokens: 3,
                    cache_write_tokens: 4,
                    reasoning_tokens: 5,
                },
                UsageAggregate {
                    key: UsageBucketKey::Id(2),
                    requests: 1,
                    input_tokens: 5,
                    output_tokens: 6,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]
        );

        let buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: UsageGroupBy::Model,
                api_key_id: Some(2),
                ..base
            },
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(UsageBucketKey::Id(2), 1)]
        );

        Ok(())
    }

    /// Performance-and-correctness sanity over a ten-thousand-row fixture (README §3 *ApiLog*
    /// coverage): both read paths stay correct — newest-first pagination over the whole table and
    /// aggregations whose recomputed sums match the generating formula exactly — and the
    /// aggregation's time-window predicate still reads through `idx_api_logs_started_at`, asserted
    /// through `EXPLAIN QUERY PLAN`. If Turso's plan output ever stops naming the index, this
    /// assertion fails loudly instead of silently weakening to a mere-existence check. No timing
    /// assertion exists: CI jitter makes wall-clock bounds meaningless, and the write side is a
    /// single INSERT by contract rather than by benchmark.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a page, bucket, sum, or the query plan diverges from its contract.
    #[tokio::test]
    async fn serves_a_large_fixture_correctly_with_the_window_index()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        const ROWS: i64 = 10_000;
        const DAY_MICROS: i64 = 86_400_000_000;
        let first_day = 1_700_000_000_000_000;
        let second_day = first_day + DAY_MICROS;

        // One statement over `ROWS` value tuples: rows alternate days, cycle three models, and
        // report no usage every fifth row.
        let mut fixture = String::from(
            "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
             endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
             output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens) VALUES ",
        );

        for index in 0..ROWS {
            let day = if index % 2 == 0 {
                first_day
            } else {
                second_day
            };
            let model_id = index % 3 + 1;

            let (input_tokens, output_tokens) = if index % 5 == 0 {
                ("NULL".to_string(), "NULL".to_string())
            } else {
                (format!("{}", 10 + index), format!("{}", 5 + index))
            };

            if index > 0 {
                fixture.push(',');
            }

            fixture.push_str(&format!(
                "({}, 9, 9, {}, 'openai_chat_completions', 0, 200, 'completed', 5, {}, {}, \
                 NULL, NULL, NULL)",
                day + index,
                model_id,
                input_tokens,
                output_tokens,
            ));
        }

        database.execute(fixture, ()).await?;

        // Both read paths agree with the generating formula.
        let first_page = list_api_logs(&database, 100, None).await?;
        let page_ids = first_page.iter().map(|log| log.id).collect::<Vec<i64>>();

        #[expect(clippy::unwrap_used, reason = "the fixture wrote ten thousand rows")]
        let first_id = *page_ids.first().unwrap();

        assert_eq!(first_id, ROWS);

        for pair in page_ids.windows(2) {
            let [newer, older] = pair else {
                break;
            };

            assert!(newer > older, "pages order newest first");
        }

        #[expect(clippy::unwrap_used, reason = "the page holds one hundred rows")]
        let cursor = *page_ids.last().unwrap();

        let second_page = list_api_logs(&database, 100, Some(cursor)).await?;
        let second_ids = second_page.iter().map(|log| log.id).collect::<Vec<i64>>();

        assert_eq!(
            second_ids.first(),
            Some(&(cursor - 1)),
            "the next page continues where the cursor left off"
        );

        let tail = list_api_logs(&database, 100, Some(3)).await?;
        assert_eq!(tail.iter().map(|log| log.id).collect::<Vec<i64>>(), [2, 1]);

        let base = UsageSelection {
            group_by: UsageGroupBy::Day,
            from: None,
            to: None,
            model_id: None,
            provider_id: None,
            api_key_id: None,
        };

        let day_buckets = query_usage(&database, &base).await?;
        let day_requests = day_buckets
            .iter()
            .map(|bucket| (bucket.key.clone(), bucket.requests))
            .collect::<Vec<_>>();

        assert_eq!(
            day_requests,
            vec![
                (UsageBucketKey::Day("2023-11-14".to_string()), ROWS / 2),
                (UsageBucketKey::Day("2023-11-15".to_string()), ROWS / 2),
            ]
        );

        // Recompute the per-model counts and token sums from the generating formula and require
        // the aggregation to match digit for digit.
        let expected_requests =
            |model_id: i64| (0..ROWS).filter(|index| index % 3 + 1 == model_id).count() as i64;
        let expected_sum = |model_id: i64, base_offset: i64| -> i64 {
            (0..ROWS)
                .filter(|index| index % 3 + 1 == model_id && index % 5 != 0)
                .map(|index| base_offset + index)
                .sum()
        };

        let model_buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: UsageGroupBy::Model,
                ..base
            },
        )
        .await?;
        let model_keys = model_buckets
            .iter()
            .map(|bucket| bucket.key.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            model_keys,
            [
                UsageBucketKey::Id(1),
                UsageBucketKey::Id(2),
                UsageBucketKey::Id(3)
            ]
        );

        for (bucket, model_id) in model_buckets.iter().zip(1..=3) {
            assert_eq!(bucket.requests, expected_requests(model_id));
            assert_eq!(bucket.input_tokens, expected_sum(model_id, 10));
            assert_eq!(bucket.output_tokens, expected_sum(model_id, 5));
        }

        // A one-sided window reads only the second day through the index.
        let window_buckets = query_usage(
            &database,
            &UsageSelection {
                from: Some(second_day),
                ..base
            },
        )
        .await?;

        assert_eq!(
            window_buckets
                .iter()
                .map(|bucket| (bucket.key.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(UsageBucketKey::Day("2023-11-15".to_string()), ROWS / 2)]
        );

        // The window predicate reads through the started_at index.
        let plan = database
            .query(
                "EXPLAIN QUERY PLAN SELECT id FROM api_logs \
                 WHERE started_at >= ?1 AND started_at < ?2",
                params![second_day, second_day + 1],
            )
            .await?;

        let details = plan
            .iter()
            .map(|row| row.get::<String>(3))
            .collect::<Result<Vec<String>, _>>()?;

        assert!(
            details
                .iter()
                .any(|detail| detail.contains("idx_api_logs_started_at")),
            "the usage window must read through idx_api_logs_started_at; plan: {details:?}"
        );

        Ok(())
    }
}
