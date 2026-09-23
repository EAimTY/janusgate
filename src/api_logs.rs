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
use vec1::Vec1;

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

    /// Gemini GenerateContent inbound surface (`POST /api/gemini-v1beta/models/...`).
    #[serde(rename = "gemini_generate_content")]
    GeminiGenerateContent,

    /// Gemini Interactions inbound surface (`POST /api/gemini-v1beta/interactions`).
    #[serde(rename = "gemini_interactions")]
    GeminiInteractions,

    /// Anthropic Messages inbound surface (`POST /api/anthropic-v1/messages`).
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
}

impl ApiLogEndpointType {
    /// Renders the storage spelling of the endpoint type — the exact string the schema `CHECK`
    /// accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiChatCompletions => "openai_chat_completions",
            Self::OpenaiResponses => "openai_responses",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::GeminiInteractions => "gemini_interactions",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }

    /// Parses the stored spelling back into the closed set; anything outside it is storage drift,
    /// because the column is written only through [`ApiLogEndpointType::as_str`] and its schema
    /// `CHECK` pins the vocabulary.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "openai_chat_completions" => Some(Self::OpenaiChatCompletions),
            "openai_responses" => Some(Self::OpenaiResponses),
            "gemini_generate_content" => Some(Self::GeminiGenerateContent),
            "gemini_interactions" => Some(Self::GeminiInteractions),
            "anthropic_messages" => Some(Self::AnthropicMessages),
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
/// unordered set — the element type is the validation, so any other value is rejected while the
/// request payload parses. The serde and TypeScript names are the wire names.
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

    /// Outward completion endpoint the request arrived on.
    #[serde(rename = "endpoint_type")]
    EndpointType,

    /// Recorded outcome of the request.
    #[serde(rename = "outcome")]
    Outcome,

    /// Whether the client requested a streamed response.
    #[serde(rename = "is_stream")]
    IsStream,

    /// HTTP status delivered to the client; the `NULL` rows form their own bucket.
    #[serde(rename = "http_status")]
    HttpStatus,
}

impl UsageGroupBy {
    /// Renders the wire spelling of the dimension — the exact string the request vocabulary
    /// carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Model => "model",
            Self::Provider => "provider",
            Self::ApiKey => "api_key",
            Self::EndpointType => "endpoint_type",
            Self::Outcome => "outcome",
            Self::IsStream => "is_stream",
            Self::HttpStatus => "http_status",
        }
    }

    /// The SQL key expression the dimension groups, selects, and orders by (README §7.1): the
    /// stored column itself, except the day dimension, whose expression derives the UTC day of
    /// the dispatch stamp.
    pub(crate) const fn expression(self) -> &'static str {
        match self {
            Self::Day => "date(started_at / 1000000, 'unixepoch')",
            Self::Model => "model_id",
            Self::Provider => "provider_id",
            Self::ApiKey => "api_key_id",
            Self::EndpointType => "endpoint_type",
            Self::Outcome => "outcome",
            Self::IsStream => "is_stream",
            Self::HttpStatus => "http_status",
        }
    }
}

/// The bucket key of one aggregation row (README §7.1): one value per requested dimension —
/// in request order on the wire — and `None` for every dimension the aggregation did not
/// request. `http_status` is `None` both for an unrequested dimension and for the bucket the
/// `NULL` rows form; the wire layer decides the field's presence from the requested
/// dimensions and renders that bucket as `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UsageBucketKey {
    /// The UTC day of `started_at`, `YYYY-MM-DD`.
    pub(crate) day: Option<String>,

    /// Grouped model row id.
    pub(crate) model_id: Option<i64>,

    /// Grouped provider row id.
    pub(crate) provider_id: Option<i64>,

    /// Grouped API key row id.
    pub(crate) api_key_id: Option<i64>,

    /// Grouped outward completion endpoint.
    pub(crate) endpoint_type: Option<ApiLogEndpointType>,

    /// Grouped recorded outcome.
    pub(crate) outcome: Option<ApiLogOutcome>,

    /// Grouped stream flag.
    pub(crate) is_stream: Option<bool>,

    /// Grouped HTTP status; `None` for the `NULL` rows' bucket.
    pub(crate) http_status: Option<i64>,
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

/// The shared filter of both management reads (README §7.1): every present field narrows the
/// rows both `usage:list` and `usage:query` consider. Values are never validated against stored
/// rows — an unknown id or a never-recorded status matches an empty set, because a filter
/// locates rather than addresses. Within one multi-value field the values OR (SQL `IN`);
/// fields compose with AND. `from` is an inclusive lower bound of `started_at` and `to` an
/// exclusive upper bound, in UTC microseconds. The multi-value fields are non-empty by
/// construction: the wire parser rejects an empty array before the filter exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UsageFilter {
    /// Inclusive lower bound of `started_at`, in UTC microseconds.
    pub(crate) from: Option<i64>,

    /// Exclusive upper bound of `started_at`, in UTC microseconds.
    pub(crate) to: Option<i64>,

    /// API key row ids a row may carry.
    pub(crate) api_key_id: Option<Vec1<i64>>,

    /// Provider row ids a row may carry.
    pub(crate) provider_id: Option<Vec1<i64>>,

    /// Model row ids a row may carry.
    pub(crate) model_id: Option<Vec1<i64>>,

    /// Outward completion endpoints a row may carry.
    pub(crate) endpoint_type: Option<Vec1<ApiLogEndpointType>>,

    /// Recorded outcomes a row may carry.
    pub(crate) outcome: Option<Vec1<ApiLogOutcome>>,

    /// Stream flag a row must carry.
    pub(crate) is_stream: Option<bool>,

    /// HTTP statuses a row may carry; a `NULL` status matches nothing, since SQL `NULL` never
    /// satisfies an `IN` list.
    pub(crate) http_status: Option<Vec1<i64>>,
}

/// Lists accounting rows newest first: at most `limit` rows inside `filter` whose id is
/// strictly below `before_id` — the cursor a client derives from the previous page's last row
/// — ordered by `id` descending, so the rowid order doubles as the pagination order over the
/// insertion history. An absent `before_id` starts at the newest row.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_api_logs(
    database: &Database,
    limit: i64,
    before_id: Option<i64>,
    filter: &UsageFilter,
) -> Result<Vec<ApiLog>, Error> {
    let (mut conditions, mut values) = filter_conditions(filter);

    // The cursor rides behind the filter fields: both compose with AND.
    if let Some(before_id) = before_id {
        push_bound(&mut conditions, &mut values, "id <", before_id);
    }

    values.push(Value::Integer(limit));

    let sql = format!(
        "SELECT {API_LOG_COLUMNS} FROM api_logs{} ORDER BY id DESC LIMIT ?{}",
        where_clause(&conditions),
        values.len(),
    );

    let rows = database.query(sql, values).await?;

    rows.iter().map(api_log_from_row).collect()
}

/// Aggregates accounting rows into `selection`'s buckets: one bucket per distinct combination
/// of the requested dimensions inside the filter, ordered by the key expressions in request
/// order under SQLite's ordering — `NULL` first, days chronological, ids numeric. An empty
/// grouping aggregates everything into one total bucket: an aggregate query without `GROUP
/// BY` answers with exactly one row, so a selection matching no rows is one zeroed bucket,
/// never an empty list. `requests` counts rows; each token column sums with `NULL` excluded
/// from the sum and an all-`NULL` bucket rendering `0`, so unreported usage never inflates or
/// deflates a total.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn query_usage(
    database: &Database,
    selection: &UsageSelection,
) -> Result<Vec<UsageAggregate>, Error> {
    let key_expressions: Vec<&str> = selection
        .group_by
        .iter()
        .map(|dimension| dimension.expression())
        .collect();
    let key_list = key_expressions.join(", ");

    // Conditions and values build together, so every `?N` placeholder binds the value its
    // clause pushed: window bounds first (they ride the indexes), then the locating filters.
    let (conditions, values) = filter_conditions(&selection.filter);

    let aggregates = "COUNT(*), \
         COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), \
         COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(cache_write_tokens), 0), \
         COALESCE(SUM(reasoning_tokens), 0)";

    // No dimensions, no key columns, no grouping, no ordering: the single total bucket.
    let (select_keys, grouping, ordering) = if key_expressions.is_empty() {
        (String::new(), String::new(), String::new())
    } else {
        (
            format!("{key_list}, "),
            format!(" GROUP BY {key_list}"),
            format!(" ORDER BY {key_list}"),
        )
    };

    let sql = format!(
        "SELECT {select_keys}{aggregates} FROM api_logs{}{grouping}{ordering}",
        where_clause(&conditions),
    );

    let rows = database.query(sql, values).await?;

    rows.iter()
        .map(|row| usage_aggregate_from_row(row, &selection.group_by))
        .collect()
}

/// Builds the WHERE clause both management reads share (README §7.1): conditions and values
/// grow together, so every `?N` placeholder binds the value its clause pushed — the window
/// bounds first, then each present filter field.
fn filter_conditions(filter: &UsageFilter) -> (Vec<String>, Vec<Value>) {
    let mut conditions: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();

    if let Some(from) = filter.from {
        push_bound(&mut conditions, &mut values, "started_at >=", from);
    }

    if let Some(to) = filter.to {
        push_bound(&mut conditions, &mut values, "started_at <", to);
    }

    if let Some(ids) = &filter.api_key_id {
        push_in_condition(
            &mut conditions,
            &mut values,
            "api_key_id",
            &ids.iter().map(|id| Value::Integer(*id)).collect::<Vec<_>>(),
        );
    }

    if let Some(ids) = &filter.provider_id {
        push_in_condition(
            &mut conditions,
            &mut values,
            "provider_id",
            &ids.iter().map(|id| Value::Integer(*id)).collect::<Vec<_>>(),
        );
    }

    if let Some(ids) = &filter.model_id {
        push_in_condition(
            &mut conditions,
            &mut values,
            "model_id",
            &ids.iter().map(|id| Value::Integer(*id)).collect::<Vec<_>>(),
        );
    }

    if let Some(types) = &filter.endpoint_type {
        push_in_condition(
            &mut conditions,
            &mut values,
            "endpoint_type",
            &types
                .iter()
                .map(|kind| Value::Text(kind.as_str().to_string()))
                .collect::<Vec<_>>(),
        );
    }

    if let Some(outcomes) = &filter.outcome {
        push_in_condition(
            &mut conditions,
            &mut values,
            "outcome",
            &outcomes
                .iter()
                .map(|outcome| Value::Text(outcome.as_str().to_string()))
                .collect::<Vec<_>>(),
        );
    }

    if let Some(is_stream) = filter.is_stream {
        push_bound(
            &mut conditions,
            &mut values,
            "is_stream =",
            i64::from(is_stream),
        );
    }

    if let Some(statuses) = &filter.http_status {
        // SQL `NULL` never satisfies an `IN` list, so the never-answered rows match nothing
        // here by the engine's own semantics (README §7.1).
        push_in_condition(
            &mut conditions,
            &mut values,
            "http_status",
            &statuses
                .iter()
                .map(|status| Value::Integer(*status))
                .collect::<Vec<_>>(),
        );
    }

    (conditions, values)
}

/// Appends one scalar condition binding one value: the placeholder is numbered by the value's
/// position, so it binds exactly what this clause pushed.
fn push_bound(conditions: &mut Vec<String>, values: &mut Vec<Value>, condition: &str, value: i64) {
    values.push(Value::Integer(value));
    conditions.push(format!("{condition} ?{}", values.len()));
}

/// Appends one `column IN (…)` condition: a placeholder per value, numbered in order, so the
/// clause binds exactly the values it pushed. The list is non-empty by construction — the
/// wire parser rejects an empty array — so the `IN` never degenerates.
fn push_in_condition(
    conditions: &mut Vec<String>,
    values: &mut Vec<Value>,
    column: &str,
    list: &[Value],
) {
    let placeholders = list
        .iter()
        .enumerate()
        .map(|(index, _)| format!("?{}", values.len() + index + 1))
        .collect::<Vec<_>>()
        .join(", ");

    values.extend_from_slice(list);
    conditions.push(format!("{column} IN ({placeholders})"));
}

/// Joins the built conditions into the WHERE clause text; empty when no condition applies.
fn where_clause(conditions: &[String]) -> String {
    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
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

/// Parses one aggregation row: one key column per requested dimension, in request order, then
/// the count and the five coalesced sums.
fn usage_aggregate_from_row(row: &Row, group_by: &[UsageGroupBy]) -> Result<UsageAggregate, Error> {
    let mut key = UsageBucketKey::default();

    for (index, dimension) in group_by.iter().enumerate() {
        match dimension {
            UsageGroupBy::Day => key.day = Some(row.get(index)?),
            UsageGroupBy::Model => key.model_id = Some(row.get(index)?),
            UsageGroupBy::Provider => key.provider_id = Some(row.get(index)?),
            UsageGroupBy::ApiKey => key.api_key_id = Some(row.get(index)?),
            UsageGroupBy::EndpointType => {
                key.endpoint_type = Some(endpoint_type_from_value(row.get_value(index)?)?);
            }
            UsageGroupBy::Outcome => {
                key.outcome = Some(outcome_from_value(row.get_value(index)?)?);
            }
            UsageGroupBy::IsStream => key.is_stream = Some(row.get::<i64>(index)? != 0),
            // The status column is the one nullable key column: its `NULL` bucket reads as
            // `None` and renders `null` on the wire.
            UsageGroupBy::HttpStatus => key.http_status = row.get(index)?,
        }
    }

    Ok(UsageAggregate {
        key,
        requests: row.get(group_by.len())?,
        input_tokens: row.get(group_by.len() + 1)?,
        output_tokens: row.get(group_by.len() + 2)?,
        cached_tokens: row.get(group_by.len() + 3)?,
        cache_write_tokens: row.get(group_by.len() + 4)?,
        reasoning_tokens: row.get(group_by.len() + 5)?,
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
    /// Bucket key: one value per requested dimension.
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

/// The `usage:query` selection: the requested bucketing dimensions in request order — empty
/// aggregates one total bucket — plus the shared filter (README §7.1).
#[derive(Debug, Clone)]
pub(crate) struct UsageSelection {
    /// Bucketing dimensions, deduplicated and closed-vocabulary by the request validation.
    pub(crate) group_by: Vec<UsageGroupBy>,

    /// The filter both management reads share.
    pub(crate) filter: UsageFilter,
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
            ApiLogEndpointType::GeminiGenerateContent,
            ApiLogEndpointType::GeminiInteractions,
            ApiLogEndpointType::AnthropicMessages,
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
                model_prefix: "acme-",
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

    /// Builds one non-empty filter list from a fixture slice.
    ///
    /// # Panics
    ///
    /// Panics on an empty slice, which no test fixture constructs.
    fn list1<T: Clone>(values: &[T]) -> Vec1<T> {
        #[expect(clippy::unwrap_used, reason = "test fixtures are non-empty")]
        Vec1::try_from_vec(values.to_vec()).unwrap()
    }

    /// Verifies `usage:list` pagination (README §7.1): rows order by `id` descending, the limit
    /// bounds each page, `before_id` is strictly older — the cursor row itself never reappears —
    /// a cursor at or below the oldest row yields the empty final page, and the filter composes
    /// with the cursor: only rows inside the filter continue a filtered page.
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

        // Two rows of a second model, so the model filter has pages of its own.
        record_api_log(&database, write(6_000, 1, 1, 2, [None; 5])).await?;
        record_api_log(&database, write(7_000, 1, 1, 2, [None; 5])).await?;

        let ids = |logs: &[ApiLog]| logs.iter().map(|log| log.id).collect::<Vec<i64>>();
        let no_filter = UsageFilter::default();
        let model_two = UsageFilter {
            model_id: Some(list1(&[2])),
            ..UsageFilter::default()
        };

        // The default page lists every row, newest first.
        let page = list_api_logs(&database, 100, None, &no_filter).await?;
        assert_eq!(ids(&page), [7, 6, 5, 4, 3, 2, 1]);

        // The limit bounds the page, order unchanged.
        let page = list_api_logs(&database, 2, None, &no_filter).await?;
        assert_eq!(ids(&page), [7, 6]);

        // before_id is strict: the cursor row is excluded, only older rows follow.
        let page = list_api_logs(&database, 100, Some(6), &no_filter).await?;
        assert_eq!(ids(&page), [5, 4, 3, 2, 1]);

        // A limit smaller than the remainder keeps walking strictly older.
        let page = list_api_logs(&database, 1, Some(5), &no_filter).await?;
        assert_eq!(ids(&page), [4]);

        // A cursor at the oldest row is the empty final page.
        let page = list_api_logs(&database, 100, Some(1), &no_filter).await?;
        assert!(page.is_empty());

        // The filter narrows the stream to its own rows, newest first.
        let page = list_api_logs(&database, 100, None, &model_two).await?;
        assert_eq!(ids(&page), [7, 6]);

        // A filtered page continues through the filter: cursor and filter compose.
        let page = list_api_logs(&database, 100, Some(7), &model_two).await?;
        assert_eq!(ids(&page), [6]);

        let page = list_api_logs(&database, 100, Some(6), &model_two).await?;
        assert!(page.is_empty());

        // A filter the fixture satisfies only below the cursor composes the same way.
        let model_one = UsageFilter {
            model_id: Some(list1(&[1])),
            ..UsageFilter::default()
        };
        let page = list_api_logs(&database, 100, Some(6), &model_one).await?;
        assert_eq!(ids(&page), [5, 4, 3, 2, 1]);

        Ok(())
    }

    /// Verifies `usage:query` end to end over a hand-checked fixture (README §7.1): every
    /// dimension buckets and orders by its own key, dimension sets compose the key and the
    /// ordering in request order, the empty set aggregates one total bucket — zero rows
    /// included — `NULL` statuses form their own bucket, sums exclude `NULL` tokens while still
    /// counting their row, the window is inclusive from and exclusive to, and the filter
    /// locates: within a field values OR, across fields they AND, and an unknown id yields the
    /// empty set instead of an error.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if any bucket, sum, window edge, or filter diverges from the documented semantics.
    #[tokio::test]
    async fn aggregates_by_dimension_sets_over_the_shared_filter() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) = create_test_database().await;

        const DAY_MICROS: i64 = 86_400_000_000;
        let first_day = 1_700_000_000_000_000; // 2023-11-14 UTC
        let second_day = first_day + DAY_MICROS; // 2023-11-15 UTC

        // Four rows spanning both days, two models, two providers, two keys, two endpoints,
        // both stream flags, three outcomes, and one `NULL` status (recorded no usage at all).
        let fixture = [
            (
                first_day + 1,
                1,
                1,
                1,
                ApiLogEndpointType::OpenaiChatCompletions,
                false,
                Some(200),
                ApiLogOutcome::Completed,
                [Some(10), Some(20), Some(1), Some(2), Some(3)],
            ),
            (
                first_day + 2,
                2,
                1,
                2,
                ApiLogEndpointType::OpenaiChatCompletions,
                false,
                None,
                ApiLogOutcome::ClientAborted,
                [None; 5],
            ),
            (
                second_day + 1,
                1,
                2,
                1,
                ApiLogEndpointType::OpenaiResponses,
                true,
                Some(200),
                ApiLogOutcome::Completed,
                [Some(5), Some(6), None, None, None],
            ),
            (
                second_day + 2,
                1,
                1,
                1,
                ApiLogEndpointType::OpenaiChatCompletions,
                true,
                Some(500),
                ApiLogOutcome::UpstreamError,
                [Some(1), Some(2), Some(3), Some(4), Some(5)],
            ),
        ];

        for (
            started_at,
            api_key_id,
            provider_id,
            model_id,
            endpoint_type,
            is_stream,
            http_status,
            outcome,
            tokens,
        ) in fixture
        {
            let [
                input_tokens,
                output_tokens,
                cached_tokens,
                cache_write_tokens,
                reasoning_tokens,
            ] = tokens;

            record_api_log(
                &database,
                ApiLogWrite {
                    started_at,
                    api_key_id,
                    provider_id,
                    model_id,
                    endpoint_type,
                    is_stream,
                    http_status,
                    outcome,
                    duration: 5,
                    input_tokens,
                    output_tokens,
                    cached_tokens,
                    cache_write_tokens,
                    reasoning_tokens,
                },
            )
            .await?;
        }

        let selection =
            |group_by: Vec<UsageGroupBy>, filter: UsageFilter| UsageSelection { group_by, filter };

        let no_filter = UsageFilter::default();

        // Grouping by UTC day: ISO keys ascending, NULL-excluding sums, the usage-less row
        // still counted.
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Day], no_filter.clone()),
        )
        .await?;
        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey {
                        day: Some("2023-11-14".to_string()),
                        ..UsageBucketKey::default()
                    },
                    requests: 2,
                    input_tokens: 10,
                    output_tokens: 20,
                    cached_tokens: 1,
                    cache_write_tokens: 2,
                    reasoning_tokens: 3,
                },
                UsageAggregate {
                    key: UsageBucketKey {
                        day: Some("2023-11-15".to_string()),
                        ..UsageBucketKey::default()
                    },
                    requests: 2,
                    input_tokens: 6,
                    output_tokens: 8,
                    cached_tokens: 3,
                    cache_write_tokens: 4,
                    reasoning_tokens: 5,
                },
            ]
        );

        // The empty dimension set aggregates one total bucket whose key is empty.
        let buckets = query_usage(&database, &selection(vec![], no_filter.clone())).await?;
        assert_eq!(
            buckets,
            vec![UsageAggregate {
                key: UsageBucketKey::default(),
                requests: 4,
                input_tokens: 16,
                output_tokens: 28,
                cached_tokens: 4,
                cache_write_tokens: 6,
                reasoning_tokens: 8,
            }]
        );

        // A total bucket over an empty selection is one zeroed bucket, never an empty list.
        let unknown = UsageFilter {
            model_id: Some(list1(&[999])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(&database, &selection(vec![], unknown.clone())).await?;
        assert_eq!(
            buckets,
            vec![UsageAggregate {
                key: UsageBucketKey::default(),
                requests: 0,
                input_tokens: 0,
                output_tokens: 0,
                cached_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            }]
        );

        // The same filter over a real grouping yields no bucket at all: a filter locates,
        // never 404s (README §7.0).
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Model], unknown.clone()),
        )
        .await?;
        assert!(buckets.is_empty());

        // A composite key carries exactly the requested dimensions, and the requested order
        // drives the ordering: model-major here.
        let buckets = query_usage(
            &database,
            &selection(
                vec![UsageGroupBy::Model, UsageGroupBy::Day],
                no_filter.clone(),
            ),
        )
        .await?;

        let keys = buckets
            .iter()
            .map(|bucket| bucket.key.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                UsageBucketKey {
                    model_id: Some(1),
                    day: Some("2023-11-14".to_string()),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    model_id: Some(1),
                    day: Some("2023-11-15".to_string()),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    model_id: Some(2),
                    day: Some("2023-11-14".to_string()),
                    ..UsageBucketKey::default()
                },
            ]
        );

        // The reversed request orders day-major instead: the request order is the ordering.
        let buckets = query_usage(
            &database,
            &selection(
                vec![UsageGroupBy::Day, UsageGroupBy::Model],
                no_filter.clone(),
            ),
        )
        .await?;

        let keys = buckets
            .iter()
            .map(|bucket| bucket.key.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                UsageBucketKey {
                    day: Some("2023-11-14".to_string()),
                    model_id: Some(1),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    day: Some("2023-11-14".to_string()),
                    model_id: Some(2),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    day: Some("2023-11-15".to_string()),
                    model_id: Some(1),
                    ..UsageBucketKey::default()
                },
            ]
        );

        // The `NULL` status rows form their own bucket, and the enum dimension orders by its
        // stored spelling; SQLite orders `NULL` first.
        let buckets = query_usage(
            &database,
            &selection(
                vec![UsageGroupBy::Outcome, UsageGroupBy::HttpStatus],
                no_filter.clone(),
            ),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.outcome, bucket.key.http_status, bucket.requests))
                .collect::<Vec<_>>(),
            vec![
                (Some(ApiLogOutcome::ClientAborted), None, 1),
                (Some(ApiLogOutcome::Completed), Some(200), 2),
                (Some(ApiLogOutcome::UpstreamError), Some(500), 1),
            ]
        );

        // Every remaining dimension buckets by its own column.
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::EndpointType], no_filter.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.endpoint_type, bucket.requests))
                .collect::<Vec<_>>(),
            vec![
                (Some(ApiLogEndpointType::OpenaiChatCompletions), 3),
                (Some(ApiLogEndpointType::OpenaiResponses), 1),
            ]
        );

        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::IsStream], no_filter.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.is_stream, bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(false), 2), (Some(true), 2)]
        );

        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Provider], no_filter.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.provider_id, bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(1), 3), (Some(2), 1)]
        );

        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::ApiKey], no_filter.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.api_key_id, bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(1), 3), (Some(2), 1)]
        );

        // Within one field the values OR — model 2 here, the unknown id contributing nothing.
        let either = UsageFilter {
            model_id: Some(list1(&[2, 999])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Model], either.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.model_id, bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(2), 1)]
        );

        // Across fields the filters AND: models 1 or 2 together with the completed outcome.
        let filtered = UsageFilter {
            model_id: Some(list1(&[1, 2])),
            outcome: Some(list1(&[ApiLogOutcome::Completed])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Model], filtered.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.model_id, bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(1), 2)]
        );

        // The enum, stream, status, key, and provider filters each narrow on their own column.
        let filtered = UsageFilter {
            endpoint_type: Some(list1(&[ApiLogEndpointType::OpenaiResponses])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(&database, &selection(vec![], filtered)).await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| bucket.requests)
                .collect::<Vec<_>>(),
            [1]
        );

        let filtered = UsageFilter {
            is_stream: Some(true),
            ..UsageFilter::default()
        };
        let buckets = query_usage(&database, &selection(vec![], filtered)).await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| bucket.requests)
                .collect::<Vec<_>>(),
            [2]
        );

        // `NULL` statuses match no `IN` list: only the two answered 200s count.
        let filtered = UsageFilter {
            http_status: Some(list1(&[200])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(&database, &selection(vec![], filtered)).await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| bucket.requests)
                .collect::<Vec<_>>(),
            [2]
        );

        // Filters compose to an empty set — no provider-1 responses exist — and the empty
        // selection's total bucket counts zero, never an error nor an empty list.
        let filtered = UsageFilter {
            provider_id: Some(list1(&[1])),
            endpoint_type: Some(list1(&[ApiLogEndpointType::OpenaiResponses])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(&database, &selection(vec![], filtered)).await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| bucket.requests)
                .collect::<Vec<_>>(),
            [0]
        );

        // The window is inclusive at `from` — the row stamped exactly at the bound is in — and
        // exclusive at `to`.
        let windowed = UsageFilter {
            from: Some(second_day),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Day], windowed.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.day.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some("2023-11-15".to_string()), 2)]
        );

        let windowed = UsageFilter {
            from: Some(first_day + 2),
            to: Some(second_day),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Day], windowed.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.day.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some("2023-11-14".to_string()), 1)]
        );

        // An empty window is an empty aggregation with a real grouping, not an error.
        let windowed = UsageFilter {
            from: Some(second_day),
            to: Some(second_day),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Day], windowed.clone()),
        )
        .await?;

        assert!(buckets.is_empty());

        // Filters compose with the window and the grouping: the second day's completed rows.
        let composed = UsageFilter {
            from: Some(second_day),
            outcome: Some(list1(&[ApiLogOutcome::Completed])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            &database,
            &selection(vec![UsageGroupBy::Provider], composed.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.provider_id, bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(2), 1)]
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
        let no_filter = UsageFilter::default();
        let first_page = list_api_logs(&database, 100, None, &no_filter).await?;
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

        let second_page = list_api_logs(&database, 100, Some(cursor), &no_filter).await?;
        let second_ids = second_page.iter().map(|log| log.id).collect::<Vec<i64>>();

        assert_eq!(
            second_ids.first(),
            Some(&(cursor - 1)),
            "the next page continues where the cursor left off"
        );

        let tail = list_api_logs(&database, 100, Some(3), &no_filter).await?;
        assert_eq!(tail.iter().map(|log| log.id).collect::<Vec<i64>>(), [2, 1]);

        let day_selection = UsageSelection {
            group_by: vec![UsageGroupBy::Day],
            filter: UsageFilter::default(),
        };

        let day_buckets = query_usage(&database, &day_selection).await?;
        let day_requests = day_buckets
            .iter()
            .map(|bucket| (bucket.key.day.clone(), bucket.requests))
            .collect::<Vec<_>>();

        assert_eq!(
            day_requests,
            vec![
                (Some("2023-11-14".to_string()), ROWS / 2),
                (Some("2023-11-15".to_string()), ROWS / 2),
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
                group_by: vec![UsageGroupBy::Model],
                filter: UsageFilter::default(),
            },
        )
        .await?;
        let model_keys = model_buckets
            .iter()
            .map(|bucket| bucket.key.model_id)
            .collect::<Vec<_>>();

        assert_eq!(model_keys, [Some(1), Some(2), Some(3)]);

        for (bucket, model_id) in model_buckets.iter().zip(1..=3) {
            assert_eq!(bucket.requests, expected_requests(model_id));
            assert_eq!(bucket.input_tokens, expected_sum(model_id, 10));
            assert_eq!(bucket.output_tokens, expected_sum(model_id, 5));
        }

        // A one-sided window reads only the second day through the index.
        let window_buckets = query_usage(
            &database,
            &UsageSelection {
                group_by: vec![UsageGroupBy::Day],
                filter: UsageFilter {
                    from: Some(second_day),
                    ..UsageFilter::default()
                },
            },
        )
        .await?;

        assert_eq!(
            window_buckets
                .iter()
                .map(|bucket| (bucket.key.day.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some("2023-11-15".to_string()), ROWS / 2)]
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

        // An id filter paired with the time window reads through the matching composite index
        // (README §3 *ApiLog*): the filter shape the management surface issues most.
        let filtered_plan = database
            .query(
                "EXPLAIN QUERY PLAN SELECT id FROM api_logs \
                 WHERE model_id IN (?1, ?2) AND started_at >= ?3 AND started_at < ?4",
                params![1, 2, second_day, second_day + 1],
            )
            .await?;

        let filtered_details = filtered_plan
            .iter()
            .map(|row| row.get::<String>(3))
            .collect::<Result<Vec<String>, _>>()?;

        assert!(
            filtered_details
                .iter()
                .any(|detail| detail.contains("idx_api_logs_model_started_at")),
            "the filtered usage window must read through idx_api_logs_model_started_at; \
             plan: {filtered_details:?}"
        );

        Ok(())
    }
}
