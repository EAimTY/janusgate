//! Management routes reading the request-accounting rows (`usage:list`, `usage:query`).
//!
//! Both reads answer `POST` with typed bodies (README §7.0 rule 6), serve rows and aggregates
//! that carry only the log's own facts — never a name, never content (rule 7) — and are never
//! gated by the `api_logs.is_enabled` write switch: existing rows stay readable (README §3
//! *ApiLog*).

use super::{Error, route};
use crate::{
    api_logs::{self, ApiLog, UsageAggregate, UsageGroupBy, list_api_logs, query_usage},
    context::Context,
    ir::InvalidRequest,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};

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

/// One accounting row in its management wire shape (README §7.1): the table row as-is — all
/// fifteen columns, `id` included because it *is* the pagination cursor. `http_status` and the
/// five token columns mirror their nullable storage columns (`T | null`, rule 2), and no field
/// carries a name or content: labels are resolved through the entity list routes (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ApiLogResponse {
    /// Row identifier; the strictly-older pagination cursor of `usage:list`.
    pub id: i64,

    /// Dispatch-start instant in UTC microseconds.
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
            id: log.id,
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

/// Strictly parsed `usage:list` request (README §7.1): both fields are absent-keeps (rule 2).
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

    /// Exclusive upper cursor: only strictly older rows — `id < before_id` — are listed. Absent
    /// starts at the newest row; the next page's cursor is the last returned row's `id`, which
    /// the client derives and the response never echoes (rule 7).
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub before_id: Option<i64>,
}

/// The `usage:list` success body (README §7.1): rows ordered by `id` descending.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListUsageResponse {
    /// One page of accounting rows, newest first.
    pub api_logs: Vec<ApiLogResponse>,
}

/// Strictly parsed `usage:query` request (README §7.1): `group_by` is a typed enum — the type is
/// the validation — and every other field is absent-keeps (rule 2).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct QueryUsageRequest {
    /// Bucketing dimension.
    pub group_by: UsageGroupBy,

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

    /// Model row id filter; absent lists every model, an unknown id yields no rows (a filter
    /// locates, it never 404s).
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub model_id: Option<i64>,

    /// Provider row id filter; absent lists every provider.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_id: Option<i64>,

    /// API key row id filter; absent lists every key.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub api_key_id: Option<i64>,
}

/// One `usage:query` bucket (README §7.1): the log's own numeric facts under its grouping key —
/// `requests` counts rows and every token is a `NULL`-excluding sum rendered as a number. No name
/// rides along (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct UsageBucket {
    /// Bucket key: the UTC day (`YYYY-MM-DD`) or the grouped row id, per `group_by`.
    pub key: super::UsageBucketKey,

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
    /// Mirrors one aggregation row.
    fn from_aggregate(aggregate: &UsageAggregate) -> Self {
        Self {
            key: aggregate.key.clone(),
            requests: aggregate.requests,
            input_tokens: aggregate.input_tokens,
            output_tokens: aggregate.output_tokens,
            cached_tokens: aggregate.cached_tokens,
            cache_write_tokens: aggregate.cache_write_tokens,
            reasoning_tokens: aggregate.reasoning_tokens,
        }
    }
}

/// The `usage:query` success body (README §7.1): buckets ordered by key ascending.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct QueryUsageResponse {
    /// One bucket per distinct grouping key inside the window and filters.
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

        Some(_) => Err(Error::Validation(InvalidRequest::field(
            "limit",
            format!("`limit` must be between 1 and {MAX_LIMIT}"),
        ))),
    }
}

/// Handles `POST /usage:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ListUsageRequest>, Error>,
) -> Result<Json<ListUsageResponse>, Error> {
    let limit = resolve_limit(request.limit)?;

    let logs = list_api_logs(&context.database, limit, request.before_id).await?;

    Ok(Json(ListUsageResponse {
        api_logs: logs.iter().map(ApiLogResponse::from_log).collect(),
    }))
}

/// Handles `POST /usage:query`.
pub(super) async fn query(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<QueryUsageRequest>, Error>,
) -> Result<Json<QueryUsageResponse>, Error> {
    let aggregates = query_usage(
        &context.database,
        &api_logs::UsageSelection {
            group_by: request.group_by,
            from: request.from,
            to: request.to,
            model_id: request.model_id,
            provider_id: request.provider_id,
            api_key_id: request.api_key_id,
        },
    )
    .await?;

    Ok(Json(QueryUsageResponse {
        buckets: aggregates.iter().map(UsageBucket::from_aggregate).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{ApiLogResponse, ListUsageRequest, QueryUsageRequest, UsageBucket, resolve_limit};
    use crate::api_logs::{ApiLogEndpointType, ApiLogOutcome, UsageBucketKey};

    /// Verifies the two request payloads strictly (README §7.0 rules 2, §7.1): `group_by` parses
    /// only its four closed values, every field is either required (`group_by`) or absent-keeps —
    /// present-but-`null` rejected — and an unknown field never parses.
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
            serde_json::from_str(r#"{"limit":50,"before_id":7}"#).unwrap();

        assert_eq!(listed.limit, Some(50));
        assert_eq!(listed.before_id, Some(7));

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let defaulted: ListUsageRequest = serde_json::from_str(r#"{}"#).unwrap();

        assert_eq!(defaulted.limit, None);
        assert_eq!(defaulted.before_id, None);

        // Absent-keeps: an explicit null is rejected, an unknown field is rejected.
        assert!(serde_json::from_str::<ListUsageRequest>(r#"{"limit":null}"#).is_err());
        assert!(
            serde_json::from_str::<ListUsageRequest>(r#"{"oldest":true}"#).is_err(),
            "the request payload is strictly parsed"
        );

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let queried: QueryUsageRequest =
            serde_json::from_str(r#"{"group_by":"api_key","model_id":3}"#).unwrap();

        assert_eq!(queried.group_by, crate::api_logs::UsageGroupBy::ApiKey);
        assert_eq!(queried.model_id, Some(3));
        assert_eq!(queried.from, None);
        assert_eq!(queried.to, None);
        assert_eq!(queried.provider_id, None);
        assert_eq!(queried.api_key_id, None);

        // The grouping type is the validation: any string outside the closed set is rejected,
        // and so is an unknown field.
        assert!(serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":"hour"}"#).is_err());
        assert!(serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":"day","up":1}"#).is_err());
        assert!(
            serde_json::from_str::<QueryUsageRequest>(r#"{"group_by":"day","model_id":null}"#)
                .is_err()
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
                matches!(rejected, crate::management_api::v0::Error::Validation(_)),
                "an out-of-range limit classifies as an invalid request"
            );
        }
    }

    /// Pins the wire shapes at the JSON level (README §7.1, §7.0 rule 7): the row renders its
    /// `id` and fifteen columns with the nullable mirrors as `null` — and positively carries no
    /// name and no content column — while a bucket renders its key beside plain numeric sums.
    ///
    /// # Panics
    ///
    /// Panics if a rendered body diverges from the documented shape.
    #[test]
    fn renders_the_documented_wire_shapes() {
        let log = crate::api_logs::ApiLog {
            id: 9,
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
                "id": 9,
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

        // The row carries the log's own facts alone: no name, no label, no content column.
        #[expect(clippy::expect_used, reason = "the row renders as an object")]
        let rendered = rendered.as_object().expect("the row renders as an object");
        assert!(!rendered.contains_key("name"));
        assert!(!rendered.contains_key("provider"));
        assert!(!rendered.contains_key("prompt"));
        assert!(!rendered.contains_key("completion"));
        assert!(!rendered.contains_key("reasoning_effort"));

        let bucket = super::UsageBucket::from_aggregate(&crate::api_logs::UsageAggregate {
            key: UsageBucketKey::Day("2026-01-02".to_string()),
            requests: 4,
            input_tokens: 10,
            output_tokens: 20,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 5,
        });

        #[expect(clippy::expect_used, reason = "the bucket body always serializes")]
        let rendered = serde_json::to_value(bucket).expect("the bucket body serializes");

        assert_eq!(
            rendered,
            serde_json::json!({
                "key": "2026-01-02",
                "requests": 4,
                "input_tokens": 10,
                "output_tokens": 20,
                "cached_tokens": 0,
                "cache_write_tokens": 0,
                "reasoning_tokens": 5,
            })
        );

        // An id grouping renders the bare numeric key.
        let bucket = UsageBucket::from_aggregate(&crate::api_logs::UsageAggregate {
            key: UsageBucketKey::Id(42),
            requests: 1,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        });

        #[expect(clippy::expect_used, reason = "the bucket body always serializes")]
        let rendered = serde_json::to_value(bucket).expect("the bucket body serializes");

        assert_eq!(
            rendered,
            serde_json::json!({
                "key": 42,
                "requests": 1,
                "input_tokens": 0,
                "output_tokens": 0,
                "cached_tokens": 0,
                "cache_write_tokens": 0,
                "reasoning_tokens": 0,
            })
        );
    }
}
