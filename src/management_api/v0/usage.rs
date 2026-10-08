//! Management routes reading the request-accounting rows (`usage:list`, `usage:query`).
//!
//! Both reads answer `POST` with typed bodies (README §7.0 rule 6), share one filter type,
//! serve rows and aggregates that carry only the log's own facts — never a name, never content
//! (rule 7) — and are never gated by the `api_logs` write switch: existing api-log files stay
//! readable (README §3 *ApiLog*).

use super::{Error, Nullable, UsageFilter, route};
use crate::{
    accounting::UsageGroupBy,
    context::Context,
    storage::api_logs::{self, ApiLog, UsageAggregate, list_api_logs, query_usage},
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};
use vec1::Vec1;

route! {
    pub(super) ListUsage: {
        path: "/usage:list",
        method: http::Method::POST,
        request: ListUsageRequest,
        response: ListUsageResponse,
    }
}

route! {
    pub(super) QueryUsage: {
        path: "/usage:query",
        method: http::Method::POST,
        request: QueryUsageRequest,
        response: QueryUsageResponse,
    }
}

/// Page size `usage:list` uses when `limit` is absent (README §7.1).
const DEFAULT_LIMIT: i64 = 100;

/// Largest accepted `usage:list` page size; larger values are rejected with 400 (README §7.1).
const MAX_LIMIT: i64 = 1000;

/// Largest accepted length of one multi-value filter field (README §7.1): one cap shared by
/// every such field, bounding pathological requests — the management API may face untrusted
/// users, and every value becomes a bound SQL parameter.
const FILTER_VALUES_CAP: usize = 100;

/// One accounting row in its management wire shape (README §7.1): the row's sixteen published
/// columns — the rowid never appears, serving only as the per-file tie-break of the read
/// ordering. `http_status` and the five token columns mirror their nullable storage columns
/// (`T | null`, rule 2), and no field carries a name or content: labels are resolved through the
/// entity list routes (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct ApiLogResponse {
    /// Stable requested public route identity.
    model_route_id: i64,
    /// First data-plane upstream header status; null means no upstream response was received.
    upstream_http_status: Option<i64>,
    /// Dispatch-start instant in UTC microseconds; the `usage:list` pagination cursor.
    started_at: i64,

    /// Row id of the authenticated gateway API key.
    api_key_id: i64,

    /// Row id of the routed provider.
    provider_id: i64,

    /// Row id of the routed model.
    model_id: i64,

    /// Outward completion endpoint the request arrived on.
    endpoint_type: super::ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    is_stream: bool,

    /// HTTP status delivered to the client; `null` when no response was ever produced — the
    /// nullable mirror column (rule 2).
    http_status: Option<i64>,

    /// Recorded outcome of the request.
    outcome: super::ApiLogOutcome,

    /// Milliseconds between the dispatch stamp and the accounting instant.
    duration: i64,

    /// Tokens consumed by the input; `null` when the upstream reported no usage — the nullable
    /// mirror column (rule 2).
    input_tokens: Option<i64>,

    /// Tokens produced by the output, `reasoning_tokens` included; `null` without usage — the
    /// nullable mirror column (rule 2).
    output_tokens: Option<i64>,

    /// Input tokens served from the upstream prompt cache; `null` without that breakdown — the
    /// nullable mirror column (rule 2).
    cached_tokens: Option<i64>,

    /// Input tokens written to the upstream prompt cache; `null` without that breakdown — the
    /// nullable mirror column (rule 2).
    cache_write_tokens: Option<i64>,

    /// Output tokens spent on reasoning; `null` without that breakdown — the nullable mirror
    /// column (rule 2).
    reasoning_tokens: Option<i64>,
}

impl ApiLogResponse {
    /// Mirrors one stored row verbatim.
    fn from_log(log: &ApiLog) -> Self {
        Self {
            model_route_id: log.model_route_id,
            upstream_http_status: log.upstream_http_status,
            started_at: log.started_at,
            api_key_id: log.api_key_id,
            provider_id: log.provider_id,
            model_id: log.model_id,
            endpoint_type: log.endpoint_type,
            is_stream: log.is_stream,
            http_status: log.http_status,
            outcome: log.outcome,
            duration: log.duration,
            input_tokens: log.input_tokens,
            output_tokens: log.output_tokens,
            cached_tokens: log.cached_tokens,
            cache_write_tokens: log.cache_write_tokens,
            reasoning_tokens: log.reasoning_tokens,
        }
    }
}

/// Strictly parsed `usage:list` request (README §7.1): every field is absent-keeps (rule 2).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListUsageRequest {
    /// Page size; absent means the default of 100, and a present value outside `1..=1000` is
    /// rejected with 400.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    limit: Option<i64>,

    /// Exclusive upper cursor bound: only strictly older rows — `started_at < before`, in UTC
    /// microseconds — are listed. Absent starts at the newest row; the next page's cursor is the
    /// last returned row's `started_at`, which the client derives and the response never echoes
    /// (rule 7).
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    before: Option<i64>,

    /// The shared filter (README §7.1); absent lists every row.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    filter: Option<UsageFilter>,
}

/// The `usage:list` success body (README §7.1): rows ordered by `started_at` descending, the
/// per-file rowid breaking ties.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListUsageResponse {
    /// One page of accounting rows, newest first.
    api_logs: Vec<ApiLogResponse>,
}

/// Strictly parsed `usage:query` request (README §7.1): `group_by` is required and is a set of
/// closed-vocabulary dimensions — the element type is the validation — and `filter` is
/// absent-keeps (rule 2).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct QueryUsageRequest {
    /// Bucketing dimensions in request order; `[]` aggregates one total bucket. A repeated
    /// dimension is rejected with 400 naming the field.
    group_by: Vec<UsageGroupBy>,

    /// The shared filter (README §7.1); absent filters nothing.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    filter: Option<UsageFilter>,
}

/// The composite bucket key of one aggregation row (README §7.1): one field per requested
/// dimension, exactly the requested ones — a fixed object serialized in declaration order, while
/// the request order drives the bucket list's ordering. The enums use their
/// recorded vocabularies; `http_status` renders `null` for the bucket the never-answered rows
/// form — its nullable mirror (rule 2).
#[derive(Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct UsageBucketKey {
    /// The grouped public route identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    model_route_id: Option<i64>,
    /// The UTC day of `started_at`, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    day: Option<String>,

    /// The grouped model row id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    model_id: Option<i64>,

    /// The grouped provider row id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    provider_id: Option<i64>,

    /// The grouped API key row id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    api_key_id: Option<i64>,

    /// The grouped outward completion endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    endpoint_type: Option<super::ApiLogEndpointType>,

    /// The grouped recorded outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    outcome: Option<super::ApiLogOutcome>,

    /// The grouped stream flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    is_stream: Option<bool>,

    /// The grouped HTTP status; `null` for the `NULL` rows' bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "number | null", optional)
    )]
    http_status: Option<Nullable<i64>>,
}

impl UsageBucketKey {
    /// Builds the wire key from the read layer's key: a field rides exactly when its dimension
    /// was requested — the wire layer, not the key itself, knows which dimensions are in play.
    fn from_domain(key: &api_logs::UsageBucketKey, group_by: &[UsageGroupBy]) -> Self {
        let mut wire = Self::default();

        for dimension in group_by {
            match dimension {
                UsageGroupBy::Day => wire.day = key.day.clone(),
                UsageGroupBy::Model => wire.model_id = key.model_id,
                UsageGroupBy::ModelRoute => wire.model_route_id = key.model_route_id,
                UsageGroupBy::Provider => wire.provider_id = key.provider_id,
                UsageGroupBy::ApiKey => wire.api_key_id = key.api_key_id,
                UsageGroupBy::EndpointType => wire.endpoint_type = key.endpoint_type,
                UsageGroupBy::Outcome => wire.outcome = key.outcome,
                UsageGroupBy::IsStream => wire.is_stream = key.is_stream,
                // The never-answered rows' bucket renders `null` — the nullable mirror (rule 2).
                UsageGroupBy::HttpStatus => wire.http_status = Some(Nullable(key.http_status)),
            }
        }

        wire
    }
}

/// One `usage:query` bucket (README §7.1): the log's own numeric facts under its composite
/// grouping key — `requests` counts rows and every token is a `NULL`-excluding sum rendered as
/// a number. No name rides along (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct UsageBucket {
    /// Bucket key: exactly the requested dimensions; the request order drives the bucket
    /// ordering.
    key: UsageBucketKey,

    /// Number of accounting rows in the bucket.
    requests: i64,

    /// Summed input tokens.
    input_tokens: i64,

    /// Summed output tokens.
    output_tokens: i64,

    /// Summed cached input tokens.
    cached_tokens: i64,

    /// Summed cache-write input tokens.
    cache_write_tokens: i64,

    /// Summed reasoning output tokens.
    reasoning_tokens: i64,
}

impl UsageBucket {
    /// Mirrors one aggregation row; the key carries exactly `group_by`'s dimensions.
    fn from_aggregate(aggregate: &UsageAggregate, group_by: &[UsageGroupBy]) -> Self {
        Self {
            key: UsageBucketKey::from_domain(&aggregate.key, group_by),
            requests: aggregate.requests,
            input_tokens: aggregate.input_tokens,
            output_tokens: aggregate.output_tokens,
            cached_tokens: aggregate.cached_tokens,
            cache_write_tokens: aggregate.cache_write_tokens,
            reasoning_tokens: aggregate.reasoning_tokens,
        }
    }
}

/// The `usage:query` success body (README §7.1): buckets ordered by the key expressions in
/// request order, `NULL` first.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct QueryUsageResponse {
    /// One bucket per distinct requested-dimension combination inside the filter.
    buckets: Vec<UsageBucket>,
}

/// Resolves the requested page size: an absent `limit` is [`DEFAULT_LIMIT`], a present one must
/// lie in `1..=MAX_LIMIT` (inclusive), and any other value is a 400 rejection naming the field.
///
/// # Errors
///
/// Returns [`Error`] categorized as invalid request (400) when `limit` is out of range.
fn resolve_limit(limit: Option<i64>) -> Result<i64, Error> {
    match limit {
        None => Ok(DEFAULT_LIMIT),

        Some(value) if (1..=MAX_LIMIT).contains(&value) => Ok(value),

        Some(_) => Err(Error::Validation {
            param: "limit",
            message: format!("`limit` must be between 1 and {MAX_LIMIT}"),
        }),
    }
}

/// Validates the `group_by` set: the element type's parsing already bounds the vocabulary, so
/// the one extra rule is repetition — the same dimension twice is ambiguous for both the key
/// shape and the ordering (README §7.1).
///
/// # Errors
///
/// Returns [`Error`] categorized as invalid request (400) naming the repeated dimension.
fn resolve_group_by(group_by: Vec<UsageGroupBy>) -> Result<Vec<UsageGroupBy>, Error> {
    for (index, dimension) in group_by.iter().enumerate() {
        if group_by
            .iter()
            .take(index)
            .any(|earlier| earlier == dimension)
        {
            return Err(Error::Validation {
                param: "group_by",
                message: format!("`group_by` repeats `{}`", dimension.as_str()),
            });
        }
    }

    Ok(group_by)
}

/// Validates the shared filter, which is now the one storage-tier vocabulary type re-exported
/// by this version: the parsing already rejected `null`, unknown fields, and empty arrays, so
/// the one wire-level rule left is the per-field value cap (README §7.1).
///
/// # Errors
///
/// Returns [`Error`] categorized as invalid request (400) naming the first over-cap field.
fn resolve_filter(filter: Option<UsageFilter>) -> Result<UsageFilter, Error> {
    let Some(filter) = filter else {
        return Ok(UsageFilter::default());
    };

    let capped = [
        (
            "filter.api_key_id",
            filter.api_key_id.as_ref().map(Vec1::len),
        ),
        (
            "filter.provider_id",
            filter.provider_id.as_ref().map(Vec1::len),
        ),
        ("filter.model_id", filter.model_id.as_ref().map(Vec1::len)),
        (
            "filter.model_route_id",
            filter.model_route_id.as_ref().map(Vec1::len),
        ),
        (
            "filter.endpoint_type",
            filter.endpoint_type.as_ref().map(Vec1::len),
        ),
        ("filter.outcome", filter.outcome.as_ref().map(Vec1::len)),
        (
            "filter.http_status",
            filter.http_status.as_ref().map(Vec1::len),
        ),
    ];

    for (field, length) in capped {
        if length.is_some_and(|length| length > FILTER_VALUES_CAP) {
            return Err(Error::Validation {
                param: field,
                message: format!("`{field}` carries more than {FILTER_VALUES_CAP} values"),
            });
        }
    }

    Ok(filter)
}

/// Handles `POST /usage:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ListUsageRequest>, Error>,
) -> Result<Json<ListUsageResponse>, Error> {
    let limit = resolve_limit(request.limit)?;
    let filter = resolve_filter(request.filter)?;

    let logs = list_api_logs(
        context.api_log_store.as_ref(),
        limit,
        request.before,
        &filter,
    )
    .await?;

    Ok(Json(ListUsageResponse {
        api_logs: logs.iter().map(ApiLogResponse::from_log).collect(),
    }))
}

/// Handles `POST /usage:query`.
pub(super) async fn query(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<QueryUsageRequest>, Error>,
) -> Result<Json<QueryUsageResponse>, Error> {
    let group_by = resolve_group_by(request.group_by)?;
    let filter = resolve_filter(request.filter)?;

    let aggregates = query_usage(
        context.api_log_store.as_ref(),
        &api_logs::UsageSelection {
            group_by: group_by.clone(),
            filter,
        },
    )
    .await?;

    Ok(Json(QueryUsageResponse {
        buckets: aggregates
            .iter()
            .map(|aggregate| UsageBucket::from_aggregate(aggregate, &group_by))
            .collect(),
    }))
}
