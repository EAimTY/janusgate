//! Management routes reading the request-accounting rows (`usage:list`, `usage:query`).
//!
//! Both reads answer `POST` with typed bodies (README §7.0 rule 6), share one filter type,
//! serve rows and aggregates that carry only the log's own facts — never a name, never content
//! (rule 7) — and are never gated by the `api_logs` write switch: existing shards stay readable
//! (README §3 *ApiLog*).

use super::{Error, Nullable, route};
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
    pub ListUsage: {
        path: "/usage:list",
        method: http::Method::POST,
        request: ListUsageRequest,
        response: ListUsageResponse,
    }
}

route! {
    pub QueryUsage: {
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

/// One accounting row in its management wire shape (README §7.1): the row's fourteen published
/// columns — the rowid never appears, serving only as the per-shard tie-break of the read
/// ordering. `http_status` and the five token columns mirror their nullable storage columns
/// (`T | null`, rule 2), and no field carries a name or content: labels are resolved through the
/// entity list routes (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ApiLogResponse {
    /// Dispatch-start instant in UTC microseconds; the `usage:list` pagination cursor.
    pub started_at: i64,

    /// Row id of the authenticated gateway API key.
    pub api_key_id: i64,

    /// Row id of the routed provider.
    pub provider_id: i64,

    /// Row id of the routed model.
    pub model_id: i64,

    /// Outward completion endpoint the request arrived on.
    pub endpoint_type: super::ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    pub is_stream: bool,

    /// HTTP status delivered to the client; `null` when no response was ever produced — the
    /// nullable mirror column (rule 2).
    pub http_status: Option<i64>,

    /// Recorded outcome of the request.
    pub outcome: super::ApiLogOutcome,

    /// Milliseconds between the dispatch stamp and the accounting instant.
    pub duration: i64,

    /// Tokens consumed by the input; `null` when the upstream reported no usage — the nullable
    /// mirror column (rule 2).
    pub input_tokens: Option<i64>,

    /// Tokens produced by the output, `reasoning_tokens` included; `null` without usage — the
    /// nullable mirror column (rule 2).
    pub output_tokens: Option<i64>,

    /// Input tokens served from the upstream prompt cache; `null` without that breakdown — the
    /// nullable mirror column (rule 2).
    pub cached_tokens: Option<i64>,

    /// Input tokens written to the upstream prompt cache; `null` without that breakdown — the
    /// nullable mirror column (rule 2).
    pub cache_write_tokens: Option<i64>,

    /// Output tokens spent on reasoning; `null` without that breakdown — the nullable mirror
    /// column (rule 2).
    pub reasoning_tokens: Option<i64>,
}

impl ApiLogResponse {
    /// Mirrors one stored row verbatim.
    fn from_log(log: &ApiLog) -> Self {
        Self {
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

/// The shared filter of both management reads (README §7.1): every present field narrows the
/// rows the read considers — values within one field OR (SQL `IN`), fields compose with AND.
/// Every field is absent-keeps (rule 2), and the multi-value fields are non-empty arrays: the
/// `Vec1` parsing rejects an empty array with 400 before the filter exists. Values are never
/// validated against stored rows — an unknown id or a never-recorded status matches an empty
/// set, because a filter locates rather than addresses (rule 7).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct UsageFilter {
    /// Inclusive lower bound of `started_at`, in UTC microseconds; absent sets no bound.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub from: Option<i64>,

    /// Exclusive upper bound of `started_at`, in UTC microseconds; absent sets no bound.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub to: Option<i64>,

    /// API key row ids a row may carry; absent lists every key. Non-emptiness is a server-side
    /// guarantee the plain array binding does not restate.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub api_key_id: Option<Vec1<i64>>,

    /// Provider row ids a row may carry; absent lists every provider.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub provider_id: Option<Vec1<i64>>,

    /// Model row ids a row may carry; absent lists every model.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub model_id: Option<Vec1<i64>>,

    /// Outward completion endpoints a row may carry; absent lists every endpoint.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<ApiLogEndpointType>", optional)
    )]
    pub endpoint_type: Option<Vec1<super::ApiLogEndpointType>>,

    /// Recorded outcomes a row may carry; absent lists every outcome.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<ApiLogOutcome>", optional)
    )]
    pub outcome: Option<Vec1<super::ApiLogOutcome>>,

    /// Stream flag a row must carry; absent lists both.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub is_stream: Option<bool>,

    /// HTTP statuses a row may carry; absent lists every status. A `NULL` status — a client
    /// disconnected before the first frame — matches no list, and there is no direct way to
    /// select those rows; `outcome` reaches them indirectly (README §7.1).
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub http_status: Option<Vec1<i64>>,
}

/// Strictly parsed `usage:list` request (README §7.1): every field is absent-keeps (rule 2).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListUsageRequest {
    /// Page size; absent means the default of 100, and a present value outside `1..=1000` is
    /// rejected with 400.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub limit: Option<i64>,

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
    pub before: Option<i64>,

    /// The shared filter (README §7.1); absent lists every row.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub filter: Option<UsageFilter>,
}

/// The `usage:list` success body (README §7.1): rows ordered by `started_at` descending, the
/// per-shard rowid breaking ties.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListUsageResponse {
    /// One page of accounting rows, newest first.
    pub api_logs: Vec<ApiLogResponse>,
}

/// Strictly parsed `usage:query` request (README §7.1): `group_by` is required and is a set of
/// closed-vocabulary dimensions — the element type is the validation — and `filter` is
/// absent-keeps (rule 2).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct QueryUsageRequest {
    /// Bucketing dimensions in request order; `[]` aggregates one total bucket. A repeated
    /// dimension is rejected with 400 naming the field.
    pub group_by: Vec<UsageGroupBy>,

    /// The shared filter (README §7.1); absent filters nothing.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub filter: Option<UsageFilter>,
}

/// The composite bucket key of one aggregation row (README §7.1): one field per requested
/// dimension, exactly the requested ones, in request order on the wire. The enums use their
/// recorded vocabularies; `http_status` renders `null` for the bucket the never-answered rows
/// form — its nullable mirror (rule 2).
#[derive(Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct UsageBucketKey {
    /// The UTC day of `started_at`, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub day: Option<String>,

    /// The grouped model row id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub model_id: Option<i64>,

    /// The grouped provider row id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_id: Option<i64>,

    /// The grouped API key row id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub api_key_id: Option<i64>,

    /// The grouped outward completion endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub endpoint_type: Option<super::ApiLogEndpointType>,

    /// The grouped recorded outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub outcome: Option<super::ApiLogOutcome>,

    /// The grouped stream flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub is_stream: Option<bool>,

    /// The grouped HTTP status; `null` for the `NULL` rows' bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "number | null", optional)
    )]
    pub http_status: Option<Nullable<i64>>,
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
pub struct UsageBucket {
    /// Bucket key: exactly the requested dimensions, in request order.
    pub key: UsageBucketKey,

    /// Number of accounting rows in the bucket.
    pub requests: i64,

    /// Summed input tokens.
    pub input_tokens: i64,

    /// Summed output tokens.
    pub output_tokens: i64,

    /// Summed cached input tokens.
    pub cached_tokens: i64,

    /// Summed cache-write input tokens.
    pub cache_write_tokens: i64,

    /// Summed reasoning output tokens.
    pub reasoning_tokens: i64,
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
pub struct QueryUsageResponse {
    /// One bucket per distinct requested-dimension combination inside the filter.
    pub buckets: Vec<UsageBucket>,
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

/// Validates the shared filter and converts it into the read layer's own type: the parsing
/// already rejected `null`, unknown fields, and empty arrays, so the one wire-level rule left
/// is the per-field value cap (README §7.1).
///
/// # Errors
///
/// Returns [`Error`] categorized as invalid request (400) naming the first over-cap field.
fn resolve_filter(filter: Option<UsageFilter>) -> Result<api_logs::UsageFilter, Error> {
    let Some(filter) = filter else {
        return Ok(api_logs::UsageFilter::default());
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

    Ok(api_logs::UsageFilter {
        from: filter.from,
        to: filter.to,
        api_key_id: filter.api_key_id,
        provider_id: filter.provider_id,
        model_id: filter.model_id,
        endpoint_type: filter.endpoint_type,
        outcome: filter.outcome,
        is_stream: filter.is_stream,
        http_status: filter.http_status,
    })
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

#[cfg(test)]
mod tests {
    use super::{
        ApiLogResponse, FILTER_VALUES_CAP, ListUsageRequest, QueryUsageRequest, UsageBucket,
        UsageFilter, resolve_filter, resolve_group_by, resolve_limit,
    };
    use crate::{
        accounting::{ApiLogEndpointType, ApiLogOutcome, UsageGroupBy},
        management_api::v0::Error,
    };
    use vec1::Vec1;

    /// Builds one non-empty list for the multi-value filter fields.
    ///
    /// # Panics
    ///
    /// Panics on an empty slice, which no test constructs.
    fn list1<T: Clone>(values: &[T]) -> Vec1<T> {
        #[expect(clippy::unwrap_used, reason = "test fixtures are non-empty")]
        Vec1::try_from_vec(values.to_vec()).unwrap()
    }

    /// Verifies the two request payloads strictly (README §7.0 rule 2, §7.1): `group_by` is a
    /// required set over the closed dimension vocabulary, `filter` is absent-keeps at every
    /// level — the object itself and each of its fields — an unknown field never parses in
    /// either, a multi-value field rejects an empty array, and the enum fields parse only their
    /// recorded vocabularies.
    ///
    /// # Panics
    ///
    /// Panics if a payload parses or rejects outside its contract.
    #[test]
    fn parses_the_usage_payloads_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let listed: ListUsageRequest =
            serde_json::from_str(r#"{"limit":50,"before":7,"filter":{"model_id":[3]}}"#).unwrap();

        assert_eq!(listed.limit, Some(50));
        assert_eq!(listed.before, Some(7));
        assert_eq!(
            listed.filter.map(|filter| filter.model_id),
            Some(Some(list1(&[3])))
        );

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let defaulted: ListUsageRequest = serde_json::from_str(r#"{}"#).unwrap();

        assert_eq!(defaulted.limit, None);
        assert_eq!(defaulted.before, None);
        assert_eq!(defaulted.filter, None);

        // Absent-keeps: an explicit null is rejected, an unknown field is rejected.
        assert!(serde_json::from_str::<ListUsageRequest>(r#"{"limit":null}"#).is_err());
        assert!(serde_json::from_str::<ListUsageRequest>(r#"{"filter":null}"#).is_err());
        assert!(
            serde_json::from_str::<ListUsageRequest>(r#"{"oldest":true}"#).is_err(),
            "the request payload is strictly parsed"
        );
        // The retired row-id cursor is gone: its field is unknown now.
        assert!(serde_json::from_str::<ListUsageRequest>(r#"{"before_id":7}"#).is_err());

        // The filter is strict at its own level too: unknown fields and present-but-null
        // fields are rejected inside it.
        assert!(serde_json::from_str::<ListUsageRequest>(r#"{"filter":{"bogus":1}}"#).is_err());
        assert!(
            serde_json::from_str::<ListUsageRequest>(r#"{"filter":{"model_id":null}}"#).is_err()
        );

        // A multi-value field rejects an empty array, and the enum fields parse only their
        // closed vocabularies.
        assert!(serde_json::from_str::<ListUsageRequest>(r#"{"filter":{"model_id":[]}}"#).is_err());
        assert!(
            serde_json::from_str::<ListUsageRequest>(r#"{"filter":{"outcome":["retrying"]}}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<ListUsageRequest>(
                r#"{"filter":{"endpoint_type":["openai_responses"],"is_stream":true}}"#
            )
            .is_ok()
        );

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let queried: QueryUsageRequest =
            serde_json::from_str(r#"{"group_by":["day","model"],"filter":{"from":5}}"#).unwrap();

        assert_eq!(
            queried.group_by,
            vec![UsageGroupBy::Day, UsageGroupBy::Model]
        );
        assert_eq!(queried.filter.map(|filter| filter.from), Some(Some(5)));

        // The empty dimension set is legal: one total bucket.
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let total: QueryUsageRequest = serde_json::from_str(r#"{"group_by":[]}"#).unwrap();

        assert!(total.group_by.is_empty());
        assert_eq!(total.filter, None);

        // `group_by` is required, must be an array, and its element type is the validation.
        assert!(serde_json::from_str::<QueryUsageRequest>(r#"{}"#).is_err());
        assert!(serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":null}"#).is_err());
        assert!(serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":"day"}"#).is_err());
        assert!(serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":["hour"]}"#).is_err());
        assert!(
            serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":["day"],"up":1}"#).is_err()
        );
        // The old top-level filter fields are gone: an unknown field now.
        assert!(
            serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":["day"],"from":1}"#).is_err()
        );
    }

    /// Verifies the `limit` boundary (README §7.1): absent resolves to the default, both range
    /// edges pass, and anything outside `1..=1000` — zero or a thousand and one — classifies as
    /// the invalid-request category (400).
    ///
    /// # Panics
    ///
    /// Panics if a boundary resolves or classifies outside its contract.
    #[test]
    fn enforces_the_limit_boundary() {
        #[expect(clippy::unwrap_used, reason = "the default is in range")]
        let default = resolve_limit(None).unwrap();

        assert_eq!(default, 100);

        #[expect(clippy::unwrap_used, reason = "the edges are in range")]
        let edges = (
            resolve_limit(Some(1)).unwrap(),
            resolve_limit(Some(1000)).unwrap(),
        );

        assert_eq!(edges, (1, 1000));

        for outside in [0, -5, 1001] {
            #[expect(
                clippy::expect_used,
                reason = "the out-of-range value must be rejected"
            )]
            let rejected = resolve_limit(Some(outside))
                .expect_err("an out-of-range limit must be rejected with 400");

            assert!(
                matches!(
                    rejected,
                    crate::management_api::v0::Error::Validation { .. }
                ),
                "an out-of-range limit classifies as an invalid request"
            );
        }
    }

    /// Verifies the two rules the payload parsing cannot state (README §7.1): a repeated
    /// `group_by` dimension is a 400 naming the field, and a multi-value filter field over the
    /// shared cap is a 400 naming the exact field, while the cap itself stays accepted.
    ///
    /// # Panics
    ///
    /// Panics if a validation accepts or rejects outside its contract.
    #[test]
    fn enforces_the_grouping_and_filter_rules() {
        // The full closed set exactly once is legal; the empty set is legal.
        let every_dimension = resolve_group_by(vec![
            UsageGroupBy::Day,
            UsageGroupBy::Model,
            UsageGroupBy::Provider,
            UsageGroupBy::ApiKey,
            UsageGroupBy::EndpointType,
            UsageGroupBy::Outcome,
            UsageGroupBy::IsStream,
            UsageGroupBy::HttpStatus,
        ]);

        assert!(every_dimension.is_ok());
        assert!(resolve_group_by(vec![]).is_ok());

        // A repeated dimension is a 400 naming the field and the repeated dimension.
        let repeated = resolve_group_by(vec![UsageGroupBy::Day, UsageGroupBy::Day]);

        assert!(
            matches!(
                &repeated,
                Err(Error::Validation { message, .. })
                    if message == "`group_by` repeats `day`"
            ),
            "a repeated dimension must be rejected with 400 naming the field: {repeated:?}"
        );

        // At the cap the filter passes; one past it fails naming the field.
        let values = vec![0; FILTER_VALUES_CAP];
        let at_cap = UsageFilter {
            model_id: Some(list1(&values)),
            ..UsageFilter::default()
        };

        assert!(
            resolve_filter(Some(at_cap)).is_ok(),
            "a filter at the cap must be accepted"
        );

        let mut over = values;
        over.push(1);
        let over_cap = UsageFilter {
            model_id: Some(list1(&over)),
            ..UsageFilter::default()
        };

        let rejected = resolve_filter(Some(over_cap));

        assert!(
            matches!(
                &rejected,
                Err(Error::Validation { message, .. })
                    if message.contains("filter.model_id")
            ),
            "a filter over the cap must be rejected with 400 naming the field: {rejected:?}"
        );

        // An absent filter resolves to the empty filter, filtering nothing.
        assert!(
            resolve_filter(None)
                .is_ok_and(|filter| filter == crate::storage::api_logs::UsageFilter::default()),
            "an absent filter resolves to the empty filter"
        );
    }

    /// Pins the wire shapes at the JSON level (README §7.1, §7.0 rule 7): the row renders its
    /// fourteen published columns with the nullable mirrors as `null` — and positively carries
    /// no row id, no name, and no content column — while a bucket renders its composite key with
    /// exactly the requested dimensions beside plain numeric sums.
    ///
    /// # Panics
    ///
    /// Panics if a rendered body diverges from the documented shape.
    #[test]
    fn renders_the_documented_wire_shapes() {
        let log = crate::storage::api_logs::ApiLog {
            started_at: 1_700_000_000_000_000,
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            endpoint_type: ApiLogEndpointType::OpenaiResponses,
            is_stream: true,
            http_status: None,
            outcome: ApiLogOutcome::ClientAborted,
            duration: 12,
            input_tokens: Some(11),
            output_tokens: None,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        };

        #[expect(clippy::expect_used, reason = "the row body always serializes")]
        let rendered =
            serde_json::to_value(ApiLogResponse::from_log(&log)).expect("the row body serializes");

        assert_eq!(
            rendered,
            serde_json::json!({
                "started_at": 1_700_000_000_000_000_i64,
                "api_key_id": 3,
                "provider_id": 5,
                "model_id": 7,
                "endpoint_type": "openai_responses",
                "is_stream": true,
                "http_status": null,
                "outcome": "client_aborted",
                "duration": 12,
                "input_tokens": 11,
                "output_tokens": null,
                "cached_tokens": null,
                "cache_write_tokens": null,
                "reasoning_tokens": null,
            })
        );

        // The row carries the log's own facts alone: no row id, no name, no content column.
        #[expect(clippy::expect_used, reason = "the row renders as an object")]
        let rendered = rendered.as_object().expect("the row renders as an object");
        assert!(!rendered.contains_key("id"));
        assert!(!rendered.contains_key("name"));
        assert!(!rendered.contains_key("provider"));
        assert!(!rendered.contains_key("prompt"));
        assert!(!rendered.contains_key("completion"));
        assert!(!rendered.contains_key("reasoning_effort"));

        // A composite key renders exactly the requested dimensions, in request order.
        let aggregate = crate::storage::api_logs::UsageAggregate {
            key: crate::storage::api_logs::UsageBucketKey {
                day: Some("2026-01-02".to_string()),
                model_id: Some(42),
                ..crate::storage::api_logs::UsageBucketKey::default()
            },
            requests: 4,
            input_tokens: 10,
            output_tokens: 20,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 5,
        };

        #[expect(clippy::expect_used, reason = "the bucket body always serializes")]
        let rendered = serde_json::to_value(UsageBucket::from_aggregate(
            &aggregate,
            &[UsageGroupBy::Day, UsageGroupBy::Model],
        ))
        .expect("the bucket body serializes");

        assert_eq!(
            rendered,
            serde_json::json!({
                "key": {"day": "2026-01-02", "model_id": 42},
                "requests": 4,
                "input_tokens": 10,
                "output_tokens": 20,
                "cached_tokens": 0,
                "cache_write_tokens": 0,
                "reasoning_tokens": 5,
            })
        );

        // The `NULL` status bucket renders `null`, and the total bucket's key is empty.
        let aggregate = crate::storage::api_logs::UsageAggregate {
            key: crate::storage::api_logs::UsageBucketKey {
                outcome: Some(ApiLogOutcome::ClientAborted),
                http_status: None,
                ..crate::storage::api_logs::UsageBucketKey::default()
            },
            requests: 1,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        };

        #[expect(clippy::expect_used, reason = "the bucket body always serializes")]
        let rendered = serde_json::to_value(UsageBucket::from_aggregate(
            &aggregate,
            &[UsageGroupBy::Outcome, UsageGroupBy::HttpStatus],
        ))
        .expect("the bucket body serializes");

        assert_eq!(
            rendered.get("key"),
            Some(&serde_json::json!({"outcome": "client_aborted", "http_status": null}))
        );

        let aggregate = crate::storage::api_logs::UsageAggregate {
            key: crate::storage::api_logs::UsageBucketKey::default(),
            requests: 7,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        };

        #[expect(clippy::expect_used, reason = "the bucket body always serializes")]
        let rendered = serde_json::to_value(UsageBucket::from_aggregate(&aggregate, &[]))
            .expect("the bucket body serializes");

        assert_eq!(rendered.get("key"), Some(&serde_json::json!({})));
        assert_eq!(rendered.get("requests"), Some(&serde_json::json!(7)));
    }
}
