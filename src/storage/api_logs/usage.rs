//! The management read engine over the api-log shards: `usage:list` and `usage:query` (README
//! §7.1).
//!
//! Both reads run per shard — each retained shard answers its own page or aggregation under the
//! shared [`UsageFilter`] — and merge in Rust: pages sort newest first on `started_at` with each
//! shard's per-shard rowid tie-break preserved, and buckets merge by key, then order by the key
//! expressions in request order under SQLite's ordering, re-implemented by
//! [`compare_bucket_keys`]. Day grouping is shard-aligned, so day buckets never straddle shards
//! and the merge is exact. The page read walks the shards newest day first and stops once a
//! finished shard has filled the page — every row of a newer shard outranks every row of an
//! older one, so the aggregation scan of every remaining shard is never paid for a page that is
//! already complete; `usage:query` keeps scanning all candidate shards, since an aggregation
//! needs everything. The reads are never gated by the recording switch: an absent store
//! answers empty (README §3 *ApiLog*).

use super::store::{ApiLogStore, SHARD_POOL_WAIT, ShardDay};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome, UsageGroupBy},
    database::{Database, Row, Value, schema::Registry},
    storage::Error,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, hash_map::Entry};
use vec1::Vec1;

/// Column order of api log selects, consumed positionally by [`api_log_from_row`].
const API_LOG_COLUMNS: &str = "started_at, api_key_id, provider_id, model_id, \
     endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
     output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens";

/// The fixed shard connection pool size for reads: one read query runs per shard at a time.
///
/// # Panics
///
/// Never: the literal 1 is nonzero, and the `expect` states that invariant.
fn shard_read_pool_connections() -> std::num::NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 1 is nonzero")]
    let connections = std::num::NonZero::new(1).expect("1 is nonzero");

    connections
}

impl ApiLogStore {
    /// Opens an existing shard file for reading. A file that vanished between the directory
    /// listing and this open reads as the empty partition it became.
    async fn open_shard_for_read(&self, day: ShardDay) -> Result<Option<Database>, Error> {
        let path = self.shard_path(day);

        if !path.exists() {
            return Ok(None);
        }

        Database::open(
            &path,
            Registry::ApiLogShard,
            shard_read_pool_connections(),
            SHARD_POOL_WAIT,
        )
        .await
        .map(Some)
        .map_err(Error::from)
    }

    /// The retained shard days a read with `filter` must consider, ascending: the directory
    /// listing narrowed by the filter's time window — the window's `from` is inclusive and its
    /// `to` exclusive, so the last candidate day is the day of `to - 1`.
    fn candidate_days(&self, filter: &UsageFilter) -> Result<Vec<ShardDay>, Error> {
        let lower = filter.from.map(ShardDay::of_started_at);
        let upper = filter
            .to
            .map(|to| ShardDay::of_started_at(to.saturating_sub(1)));

        Ok(self
            .retained_days()?
            .into_iter()
            .filter(|day| {
                lower.is_none_or(|lower| *day >= lower) && upper.is_none_or(|upper| *day <= upper)
            })
            .collect())
    }

    /// Lists accounting rows newest first: at most `limit` rows inside `filter` whose
    /// `started_at` is strictly below `before`, the timestamp cursor derived by the client from
    /// the previous page's last row. Each retained shard answers its own newest-first page
    /// under the same filter and cursor, and the pages merge on `started_at`.
    ///
    /// The shards are walked newest day first, and the walk stops once a finished shard has
    /// filled the page: every row of a newer shard outranks every row of an older one — a row's
    /// own stamp selects its shard — so the newer shards' answers already contain the whole page
    /// and the remaining, strictly older shards can contribute nothing to it. Within the page,
    /// equal stamps never span shards, so the stable merge preserves each shard's own `id DESC`
    /// tie-break and the result is one global `ORDER BY started_at DESC, id DESC`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a shard cannot be opened, a query fails, or a row is malformed —
    /// a corrupt shard fails the request explicitly.
    async fn list(
        &self,
        limit: i64,
        before: Option<i64>,
        filter: &UsageFilter,
    ) -> Result<Vec<ApiLog>, Error> {
        // The wire bounds `limit` at 1..=1000; the conversion never truncates in practice.
        let page_limit = usize::try_from(limit).unwrap_or(usize::MAX);

        let mut page = Vec::new();

        for day in self.candidate_days(filter)?.into_iter().rev() {
            let Some(database) = self.open_shard_for_read(day).await? else {
                continue;
            };

            let (mut conditions, mut values) = filter_conditions(filter);

            // The cursor rides behind the filter fields: both compose with AND.
            if let Some(before) = before {
                push_bound(&mut conditions, &mut values, "started_at <", before);
            }

            values.push(Value::Integer(limit));

            let sql = format!(
                "SELECT {API_LOG_COLUMNS} FROM api_logs{} \
                 ORDER BY started_at DESC, id DESC LIMIT ?{}",
                where_clause(&conditions),
                values.len(),
            );

            let rows = database.query(sql, values).await?;

            page.extend(
                rows.iter()
                    .map(api_log_from_row)
                    .collect::<Result<Vec<_>, _>>()?,
            );

            // A shard's answer is only superseded by newer shards, all of which finished before
            // it: once the page is full, every still-unread shard holds strictly older rows
            // alone. The last shard's answer can overflow the page, so the truncation below
            // still runs.
            if page.len() >= page_limit {
                break;
            }
        }

        page.sort_by_key(|log| std::cmp::Reverse(log.started_at));
        page.truncate(page_limit);

        Ok(page)
    }

    /// Aggregates accounting rows into `selection`'s buckets: each retained shard groups and
    /// sums its own rows, and the per-shard buckets merge in Rust — counts and sums combine,
    /// buckets merge by key, and the merged buckets order by the key expressions in request
    /// order under SQLite's ordering, re-implemented by [`compare_bucket_keys`]. Day grouping
    /// is shard-aligned, so day buckets never straddle shards and the merge is exact.
    ///
    /// An empty grouping aggregates one total bucket: over an empty store, exactly one zeroed
    /// bucket, never an empty list.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a shard cannot be opened, a query fails, or a row is malformed.
    async fn query(&self, selection: &UsageSelection) -> Result<Vec<UsageAggregate>, Error> {
        let key_expressions: Vec<&str> = selection
            .group_by
            .iter()
            .map(|dimension| key_expression(*dimension))
            .collect();
        let key_list = key_expressions.join(", ");

        let aggregates = "COUNT(*), \
             COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), \
             COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(cache_write_tokens), 0), \
             COALESCE(SUM(reasoning_tokens), 0)";

        // No dimensions, no key columns and no grouping: every shard answers the single total
        // bucket an aggregate without `GROUP BY` produces.
        let (select_keys, grouping) = if key_expressions.is_empty() {
            (String::new(), String::new())
        } else {
            (format!("{key_list}, "), format!(" GROUP BY {key_list}"))
        };

        let mut merged: HashMap<UsageBucketKey, UsageAggregate> = HashMap::new();

        if selection.group_by.is_empty() {
            // The total bucket exists even when no shard answers: one zeroed bucket, never an
            // empty list.
            merged.insert(UsageBucketKey::default(), UsageAggregate::default());
        }

        for day in self.candidate_days(&selection.filter)? {
            let Some(database) = self.open_shard_for_read(day).await? else {
                continue;
            };

            let (conditions, values) = filter_conditions(&selection.filter);

            let sql = format!(
                "SELECT {select_keys}{aggregates} FROM api_logs{}{grouping}",
                where_clause(&conditions),
            );

            let rows = database.query(sql, values).await?;

            for row in &rows {
                let aggregate = usage_aggregate_from_row(row, &selection.group_by)?;

                match merged.entry(aggregate.key.clone()) {
                    Entry::Occupied(mut existing) => existing.get_mut().absorb(&aggregate),
                    Entry::Vacant(slot) => {
                        slot.insert(aggregate);
                    }
                }
            }
        }

        let mut buckets: Vec<UsageAggregate> = merged.into_values().collect();
        buckets.sort_by(|first, second| {
            compare_bucket_keys(&first.key, &second.key, &selection.group_by)
        });

        Ok(buckets)
    }
}

/// Lists accounting rows through the shard store; `None` — the gateway started without an
/// `api_logs` section — is the empty store, answering an empty page. See [`ApiLogStore::list`]
/// for the merge semantics (README §7.1).
///
/// # Errors
///
/// Returns [`Error`] when a shard cannot be opened, a query fails, or a row is malformed.
pub(crate) async fn list_api_logs(
    store: Option<&ApiLogStore>,
    limit: i64,
    before: Option<i64>,
    filter: &UsageFilter,
) -> Result<Vec<ApiLog>, Error> {
    match store {
        Some(store) => store.list(limit, before, filter).await,
        None => Ok(Vec::new()),
    }
}

/// Aggregates accounting rows through the shard store; `None` — the gateway started without an
/// `api_logs` section — is the empty store, answering one zeroed total bucket for an empty
/// grouping and no buckets otherwise. See [`ApiLogStore::query`] for the merge semantics.
///
/// # Errors
///
/// Returns [`Error`] when a shard cannot be opened, a query fails, or a row is malformed.
pub(crate) async fn query_usage(
    store: Option<&ApiLogStore>,
    selection: &UsageSelection,
) -> Result<Vec<UsageAggregate>, Error> {
    match store {
        Some(store) => store.query(selection).await,

        None if selection.group_by.is_empty() => Ok(vec![UsageAggregate::default()]),

        None => Ok(Vec::new()),
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
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub(crate) from: Option<i64>,

    /// Exclusive upper bound of `started_at`, in UTC microseconds; absent sets no bound.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub(crate) to: Option<i64>,

    /// API key row ids a row may carry; absent lists every key. Non-emptiness is a server-side
    /// guarantee the plain array binding does not restate.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub(crate) api_key_id: Option<Vec1<i64>>,

    /// Provider row ids a row may carry; absent lists every provider.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub(crate) provider_id: Option<Vec1<i64>>,

    /// Model row ids a row may carry; absent lists every model.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub(crate) model_id: Option<Vec1<i64>>,

    /// Outward completion endpoints a row may carry; absent lists every endpoint.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<ApiLogEndpointType>", optional)
    )]
    pub(crate) endpoint_type: Option<Vec1<ApiLogEndpointType>>,

    /// Recorded outcomes a row may carry; absent lists every outcome.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<ApiLogOutcome>", optional)
    )]
    pub(crate) outcome: Option<Vec1<ApiLogOutcome>>,

    /// Stream flag a row must carry; absent lists both.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub(crate) is_stream: Option<bool>,

    /// HTTP statuses a row may carry; absent lists every status. A `NULL` status — a client
    /// disconnected before the first frame — matches no list, and there is no direct way to
    /// select those rows; `outcome` reaches them indirectly (README §7.1).
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub(crate) http_status: Option<Vec1<i64>>,
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
        push_in_condition_mapped(&mut conditions, &mut values, "api_key_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(ids) = &filter.provider_id {
        push_in_condition_mapped(&mut conditions, &mut values, "provider_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(ids) = &filter.model_id {
        push_in_condition_mapped(&mut conditions, &mut values, "model_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(types) = &filter.endpoint_type {
        push_in_condition_mapped(
            &mut conditions,
            &mut values,
            "endpoint_type",
            types,
            |kind| Value::Text(kind.as_str().to_string()),
        );
    }

    if let Some(outcomes) = &filter.outcome {
        push_in_condition_mapped(
            &mut conditions,
            &mut values,
            "outcome",
            outcomes,
            |outcome| Value::Text(outcome.as_str().to_string()),
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
        push_in_condition_mapped(
            &mut conditions,
            &mut values,
            "http_status",
            statuses,
            |status| Value::Integer(*status),
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

/// Appends one `column IN (…)` condition over a mapped list: every element maps through
/// `to_value` into its bound value, and the placeholders number in order, so the clause binds
/// exactly the values it pushed. The list is non-empty by its `Vec1` type — the wire parser
/// rejects an empty array — so the `IN` never degenerates.
fn push_in_condition_mapped<T>(
    conditions: &mut Vec<String>,
    values: &mut Vec<Value>,
    column: &str,
    list: &Vec1<T>,
    to_value: impl Fn(&T) -> Value,
) {
    push_in_condition(
        conditions,
        values,
        column,
        &list.iter().map(to_value).collect::<Vec<_>>(),
    );
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
        started_at: row.get(0)?,
        api_key_id: row.get(1)?,
        provider_id: row.get(2)?,
        model_id: row.get(3)?,
        endpoint_type: endpoint_type_from_value(row.get_value(4)?)?,
        is_stream: row.get::<i64>(5)? != 0,
        http_status: row.get(6)?,
        outcome: outcome_from_value(row.get_value(7)?)?,
        duration: row.get(8)?,
        input_tokens: row.get(9)?,
        output_tokens: row.get(10)?,
        cached_tokens: row.get(11)?,
        cache_write_tokens: row.get(12)?,
        reasoning_tokens: row.get(13)?,
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

/// One stored accounting row: the fourteen published columns exactly as stored — the row the
/// management read surface mirrors (README §3 *ApiLog*). The rowid stays internal: it never
/// appears here, serving only as the per-shard tie-break of the list ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiLog {
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

/// The bucket key of one aggregation row (README §7.1): one value per requested dimension —
/// in request order on the wire — and `None` for every dimension the aggregation did not
/// request. `http_status` is `None` both for an unrequested dimension and for the bucket the
/// `NULL` rows form; the wire layer decides the field's presence from the requested
/// dimensions and renders that bucket as `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
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

/// One aggregation bucket of `usage:query` (README §7.1): the bucket key, the row count, and the
/// token sums — `NULL` token columns never participate in a sum, so a bucket whose rows all
/// reported no usage sums to `0`, and every sum renders as a plain number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

impl UsageAggregate {
    /// Folds another shard's identical-key bucket into this one: counts and sums combine. Sums
    /// saturate because a merge of real token counts never approaches `i64::MAX` and a merge
    /// must not fail where no single shard's query would.
    fn absorb(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(other.cached_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
    }
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

/// The SQL key expression the dimension groups, selects, and orders by (README §7.1): the
/// stored column itself, except the day dimension, whose expression derives the UTC day of
/// the dispatch stamp.
fn key_expression(dimension: UsageGroupBy) -> &'static str {
    match dimension {
        UsageGroupBy::Day => "date(started_at / 1000000, 'unixepoch')",
        UsageGroupBy::Model => "model_id",
        UsageGroupBy::Provider => "provider_id",
        UsageGroupBy::ApiKey => "api_key_id",
        UsageGroupBy::EndpointType => "endpoint_type",
        UsageGroupBy::Outcome => "outcome",
        UsageGroupBy::IsStream => "is_stream",
        UsageGroupBy::HttpStatus => "http_status",
    }
}

/// Orders two bucket keys the way SQLite orders the key expressions: per requested dimension in
/// request order, `NULL` first, text by its bytes — the stored spellings — and numbers
/// ascending. The first non-equal dimension decides; `Option`'s own ordering is SQLite's,
/// placing `None` before every value.
fn compare_bucket_keys(
    first: &UsageBucketKey,
    second: &UsageBucketKey,
    group_by: &[UsageGroupBy],
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    for dimension in group_by {
        let ordering = match dimension {
            UsageGroupBy::Day => first.day.cmp(&second.day),
            UsageGroupBy::Model => first.model_id.cmp(&second.model_id),
            UsageGroupBy::Provider => first.provider_id.cmp(&second.provider_id),
            UsageGroupBy::ApiKey => first.api_key_id.cmp(&second.api_key_id),
            UsageGroupBy::EndpointType => first
                .endpoint_type
                .map(ApiLogEndpointType::as_str)
                .cmp(&second.endpoint_type.map(ApiLogEndpointType::as_str)),
            UsageGroupBy::Outcome => first
                .outcome
                .map(ApiLogOutcome::as_str)
                .cmp(&second.outcome.map(ApiLogOutcome::as_str)),
            UsageGroupBy::IsStream => first.is_stream.cmp(&second.is_stream),
            UsageGroupBy::HttpStatus => first.http_status.cmp(&second.http_status),
        };

        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            ApiLogWrite,
            test_support::{create_test_store, open_shard, write},
        },
        *,
    };
    use crate::{accounting::ApiLogEndpointType, database::params, unix_time};
    use std::error::Error as StdError;

    /// Builds one non-empty filter list from a fixture slice.
    ///
    /// # Panics
    ///
    /// Panics on an empty slice, which no test fixture constructs.
    fn list1<T: Clone>(values: &[T]) -> Vec1<T> {
        #[expect(clippy::unwrap_used, reason = "test fixtures are non-empty")]
        Vec1::try_from_vec(values.to_vec()).unwrap()
    }

    /// Verifies `usage:list` over the shards (README §7.1): an empty store answers an empty
    /// page; rows order newest first across shards — equal stamps never span shards, and each
    /// shard's own rowid tie-break survives the merge — the limit bounds each page, `before` is
    /// strictly older by `started_at` — the cursor row itself never reappears — the rows sharing
    /// a boundary microsecond may be skipped, and the filter composes with the cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if a store operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a page diverges from the documented order or cursor semantics.
    #[tokio::test]
    async fn lists_newest_first_with_strictly_older_timestamp_cursors()
    -> Result<(), Box<dyn StdError>> {
        let (store, _guard) = create_test_store(90);
        let no_filter = UsageFilter::default();

        // An empty store is an empty page.
        assert!(
            list_api_logs(Some(&store), 100, None, &no_filter)
                .await?
                .is_empty()
        );
        // And so is an absent store.
        assert!(list_api_logs(None, 100, None, &no_filter).await?.is_empty());

        const DAY_MICROS: i64 = 86_400_000_000;
        // Three recent days, safely inside the retention window the sweep enforces on every new
        // shard file.
        let third_day = unix_time::unix_now_micros();
        let second_day = third_day - DAY_MICROS;
        let first_day = third_day - 2 * DAY_MICROS;

        // Five rows over three shards; the two first-day rows share one stamp and are told
        // apart only by their rowids, the second-inserted one carrying the higher rowid.
        store
            .record(write(first_day + 1_000, 1, 1, 1, [None; 5]))
            .await?;
        store
            .record(write(first_day + 1_000, 1, 1, 3, [None; 5]))
            .await?;
        store
            .record(write(second_day + 1_000, 1, 1, 1, [None; 5]))
            .await?;
        store
            .record(write(third_day + 2_000, 1, 1, 1, [None; 5]))
            .await?;
        store
            .record(write(third_day + 3_000, 1, 1, 2, [None; 5]))
            .await?;

        let stamps = |logs: &[ApiLog]| logs.iter().map(|log| log.started_at).collect::<Vec<i64>>();
        let models = |logs: &[ApiLog]| logs.iter().map(|log| log.model_id).collect::<Vec<i64>>();

        // The default page merges every shard newest first; the equal-stamp pair orders by the
        // per-shard rowid tie-break, higher rowid (inserted later) first.
        let page = list_api_logs(Some(&store), 100, None, &no_filter).await?;
        assert_eq!(
            stamps(&page),
            [
                third_day + 3_000,
                third_day + 2_000,
                second_day + 1_000,
                first_day + 1_000,
                first_day + 1_000,
            ]
        );
        assert_eq!(models(&page), [2, 1, 1, 3, 1]);

        // The limit bounds the page, order unchanged.
        let page = list_api_logs(Some(&store), 2, None, &no_filter).await?;
        assert_eq!(stamps(&page), [third_day + 3_000, third_day + 2_000]);

        // `before` is strict by stamp: the cursor row is excluded, only older rows follow.
        let page = list_api_logs(Some(&store), 100, Some(third_day + 2_000), &no_filter).await?;
        assert_eq!(
            stamps(&page),
            [second_day + 1_000, first_day + 1_000, first_day + 1_000]
        );

        // A limit smaller than the remainder keeps walking strictly older.
        let page = list_api_logs(Some(&store), 1, Some(second_day + 1_000), &no_filter).await?;
        assert_eq!(stamps(&page), [first_day + 1_000]);

        // A cursor at the oldest stamp is the empty final page: the equal-stamp rows share the
        // boundary microsecond and may be skipped — the documented edge.
        let page = list_api_logs(Some(&store), 100, Some(first_day + 1_000), &no_filter).await?;
        assert!(page.is_empty());

        // The filter narrows the stream to its own rows, newest first across shards.
        let model_one = UsageFilter {
            model_id: Some(list1(&[1])),
            ..UsageFilter::default()
        };
        let page = list_api_logs(Some(&store), 100, None, &model_one).await?;
        assert_eq!(models(&page), [1, 1, 1]);

        // A filtered page continues through the filter: cursor and filter compose.
        let page = list_api_logs(Some(&store), 100, Some(third_day + 2_000), &model_one).await?;
        assert_eq!(stamps(&page), [second_day + 1_000, first_day + 1_000]);

        let page = list_api_logs(Some(&store), 100, Some(second_day + 1_000), &model_one).await?;
        assert_eq!(stamps(&page), [first_day + 1_000]);

        // The time window bounds the shards considered: the window below the second day never
        // reaches the later shards.
        let windowed = UsageFilter {
            to: Some(second_day),
            ..UsageFilter::default()
        };
        let page = list_api_logs(Some(&store), 100, None, &windowed).await?;
        assert_eq!(stamps(&page), [first_day + 1_000, first_day + 1_000]);

        let windowed = UsageFilter {
            from: Some(second_day),
            ..UsageFilter::default()
        };
        let page = list_api_logs(Some(&store), 100, None, &windowed).await?;
        assert_eq!(
            stamps(&page),
            [third_day + 3_000, third_day + 2_000, second_day + 1_000]
        );

        Ok(())
    }

    /// Verifies `usage:query` end to end over a hand-checked fixture spanning two shards (README
    /// §7.1): every dimension buckets and orders by its own key, dimension sets compose the key
    /// and the ordering in request order, the empty set aggregates one total bucket — zero rows
    /// included — `NULL` statuses form their own bucket ahead of every answered one, sums exclude
    /// `NULL` tokens while still counting their row, the window is inclusive from and exclusive
    /// to, the filter locates, and the cross-shard merge reproduces exactly the buckets one
    /// unsharded table would answer.
    ///
    /// # Errors
    ///
    /// Returns an error if a store operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if any bucket, sum, window edge, or filter diverges from the documented semantics.
    #[tokio::test]
    async fn aggregates_by_dimension_sets_over_the_shared_filter() -> Result<(), Box<dyn StdError>>
    {
        let (store, _guard) = create_test_store(90);

        const DAY_MICROS: i64 = 86_400_000_000;
        // Two recent days — and so two shards — safely inside the retention window.
        let second_day = unix_time::unix_now_micros();
        let first_day = second_day - DAY_MICROS;
        let first_stem = ShardDay::of_started_at(first_day).file_stem();
        let second_stem = ShardDay::of_started_at(second_day).file_stem();

        // Four rows spanning both days — and so both shards — two models, two providers, two
        // keys, two endpoints, both stream flags, three outcomes, and one `NULL` status (recorded
        // no usage at all).
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

            store
                .record(ApiLogWrite {
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
                })
                .await?;
        }

        let selection =
            |group_by: Vec<UsageGroupBy>, filter: UsageFilter| UsageSelection { group_by, filter };

        let no_filter = UsageFilter::default();

        // Grouping by UTC day: ISO keys ascending across the two shards, NULL-excluding sums,
        // the usage-less row still counted.
        let buckets = query_usage(
            Some(&store),
            &selection(vec![UsageGroupBy::Day], no_filter.clone()),
        )
        .await?;
        assert_eq!(
            buckets,
            vec![
                UsageAggregate {
                    key: UsageBucketKey {
                        day: Some(first_stem.clone()),
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
                        day: Some(second_stem.clone()),
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

        // The empty dimension set aggregates one total bucket whose key is empty, merging the
        // two shards' totals.
        let buckets = query_usage(Some(&store), &selection(vec![], no_filter.clone())).await?;
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

        // An empty store answers the total bucket zeroed, never an empty list.
        let (empty_store, _empty_guard) = create_test_store(90);
        let buckets =
            query_usage(Some(&empty_store), &selection(vec![], no_filter.clone())).await?;
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

        // A total bucket over an empty selection is one zeroed bucket, never an empty list.
        let unknown = UsageFilter {
            model_id: Some(list1(&[999])),
            ..UsageFilter::default()
        };
        let buckets = query_usage(Some(&store), &selection(vec![], unknown.clone())).await?;
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
            Some(&store),
            &selection(vec![UsageGroupBy::Model], unknown.clone()),
        )
        .await?;
        assert!(buckets.is_empty());

        // A composite key carries exactly the requested dimensions, and the requested order
        // drives the ordering: model-major here.
        let buckets = query_usage(
            Some(&store),
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
                    day: Some(first_stem.clone()),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    model_id: Some(1),
                    day: Some(second_stem.clone()),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    model_id: Some(2),
                    day: Some(first_stem.clone()),
                    ..UsageBucketKey::default()
                },
            ]
        );

        // The reversed request orders day-major instead: the request order is the ordering.
        let buckets = query_usage(
            Some(&store),
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
                    day: Some(first_stem.clone()),
                    model_id: Some(1),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    day: Some(first_stem.clone()),
                    model_id: Some(2),
                    ..UsageBucketKey::default()
                },
                UsageBucketKey {
                    day: Some(second_stem.clone()),
                    model_id: Some(1),
                    ..UsageBucketKey::default()
                },
            ]
        );

        // The `NULL` status rows form their own bucket ahead of every answered one, and the
        // enum dimension orders by its stored spelling — SQLite's ordering, re-implemented by
        // the merge comparator over the merged buckets.
        let buckets = query_usage(
            Some(&store),
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
            Some(&store),
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
            Some(&store),
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
            Some(&store),
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
            Some(&store),
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
            Some(&store),
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
            Some(&store),
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
        let buckets = query_usage(Some(&store), &selection(vec![], filtered)).await?;

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
        let buckets = query_usage(Some(&store), &selection(vec![], filtered)).await?;

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
        let buckets = query_usage(Some(&store), &selection(vec![], filtered)).await?;

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
        let buckets = query_usage(Some(&store), &selection(vec![], filtered)).await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| bucket.requests)
                .collect::<Vec<_>>(),
            [0]
        );

        // The window is inclusive at `from` — the row stamped exactly at the bound is in — and
        // exclusive at `to`; the bounds also bound which shards are read.
        let windowed = UsageFilter {
            from: Some(second_day),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            Some(&store),
            &selection(vec![UsageGroupBy::Day], windowed.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.day.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(second_stem.clone()), 2)]
        );

        let windowed = UsageFilter {
            from: Some(first_day + 2),
            to: Some(second_day),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            Some(&store),
            &selection(vec![UsageGroupBy::Day], windowed.clone()),
        )
        .await?;

        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.key.day.clone(), bucket.requests))
                .collect::<Vec<_>>(),
            vec![(Some(first_stem.clone()), 1)]
        );

        // An empty window is an empty aggregation with a real grouping, not an error.
        let windowed = UsageFilter {
            from: Some(second_day),
            to: Some(second_day),
            ..UsageFilter::default()
        };
        let buckets = query_usage(
            Some(&store),
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
            Some(&store),
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

    /// Performance-and-correctness sanity over a ten-thousand-row fixture spread over two shards
    /// (README §3 *ApiLog* coverage): both read paths stay correct across the merge — newest-first
    /// pagination over the whole store and aggregations whose recomputed sums match the generating
    /// formula exactly — and each shard's aggregation window still reads through its own
    /// `idx_api_logs_started_at`, asserted through `EXPLAIN QUERY PLAN`. If Turso's plan output
    /// ever stops naming the index, this assertion fails loudly instead of silently weakening to a
    /// mere-existence check. No timing assertion exists: CI jitter makes wall-clock bounds
    /// meaningless, and the write side is a single INSERT by contract rather than by benchmark.
    ///
    /// # Errors
    ///
    /// Returns an error if a store operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a page, bucket, sum, or the query plan diverges from its contract.
    #[tokio::test]
    async fn serves_a_large_fixture_correctly_with_the_window_index()
    -> Result<(), Box<dyn StdError>> {
        let (store, _guard) = create_test_store(90);

        const ROWS: i64 = 10_000;
        const DAY_MICROS: i64 = 86_400_000_000;
        // Two recent days, safely inside the retention window the sweep enforces on every new
        // shard file.
        let second_day = unix_time::unix_now_micros();
        let first_day = second_day - DAY_MICROS;
        let first_stem = ShardDay::of_started_at(first_day).file_stem();
        let second_stem = ShardDay::of_started_at(second_day).file_stem();

        // Even-indexed rows stamp the first day, odd-indexed the second; each shard receives its
        // own rows in one statement. Rows alternate days, cycle three models, and report no usage
        // every fifth row.
        for (parity, day) in [(0, first_day), (1, second_day)] {
            let database = open_shard(&store, ShardDay::of_started_at(day)).await;
            let mut fixture = String::from(
                "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                 endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
                 output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens) VALUES ",
            );

            for index in (parity..ROWS).step_by(2) {
                let model_id = index % 3 + 1;

                let (input_tokens, output_tokens) = if index % 5 == 0 {
                    ("NULL".to_string(), "NULL".to_string())
                } else {
                    (format!("{}", 10 + index), format!("{}", 5 + index))
                };

                if index > parity {
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

            fixture.push(';');
            database.execute(&fixture, ()).await?;
        }

        // Both read paths agree with the generating formula across the shard merge.
        let no_filter = UsageFilter::default();
        let first_page = list_api_logs(Some(&store), 100, None, &no_filter).await?;
        let page_stamps = first_page
            .iter()
            .map(|log| log.started_at)
            .collect::<Vec<i64>>();

        // Every second-day stamp exceeds every first-day stamp, so the newest stamp is the last
        // row's and the whole first page is second-day rows, strictly decreasing.
        assert_eq!(page_stamps.first(), Some(&(second_day + 9_999)));
        assert_eq!(page_stamps.last(), Some(&(second_day + 9_801)));

        for (newer, older) in page_stamps.iter().zip(page_stamps.iter().skip(1)) {
            assert!(
                newer > older,
                "pages order newest first: {newer} !> {older}"
            );
        }

        // The next page continues where the cursor left off: index 9799 stamps the second day.
        let cursor = second_day + 9_801;
        let second_page = list_api_logs(Some(&store), 100, Some(cursor), &no_filter).await?;

        assert_eq!(
            second_page.first().map(|log| log.started_at),
            Some(second_day + 9_799),
        );

        // A cursor three microseconds into the first day leaves exactly its index-0 and
        // index-2 rows: every second-day stamp is a full day newer.
        let tail = list_api_logs(Some(&store), 100, Some(first_day + 3), &no_filter).await?;
        assert_eq!(
            tail.iter().map(|log| log.started_at).collect::<Vec<i64>>(),
            [first_day + 2, first_day]
        );

        let day_selection = UsageSelection {
            group_by: vec![UsageGroupBy::Day],
            filter: UsageFilter::default(),
        };

        let day_buckets = query_usage(Some(&store), &day_selection).await?;
        let day_requests = day_buckets
            .iter()
            .map(|bucket| (bucket.key.day.clone(), bucket.requests))
            .collect::<Vec<_>>();

        assert_eq!(
            day_requests,
            vec![
                (Some(first_stem.clone()), ROWS / 2),
                (Some(second_stem.clone()), ROWS / 2),
            ]
        );

        // Recompute the per-model counts and token sums from the generating formula and require
        // the merged aggregation to match digit for digit.
        let expected_requests =
            |model_id: i64| (0..ROWS).filter(|index| index % 3 + 1 == model_id).count() as i64;
        let expected_sum = |model_id: i64, base_offset: i64| -> i64 {
            (0..ROWS)
                .filter(|index| index % 3 + 1 == model_id && index % 5 != 0)
                .map(|index| base_offset + index)
                .sum()
        };

        let model_buckets = query_usage(
            Some(&store),
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

        // A one-sided window reads only the second day's shard.
        let window_buckets = query_usage(
            Some(&store),
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
            vec![(Some(second_stem.clone()), ROWS / 2)]
        );

        // The window predicate reads through the started_at index, inside a shard.
        let second_shard = open_shard(&store, ShardDay::of_started_at(second_day)).await;
        let plan = second_shard
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
        let filtered_plan = second_shard
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
