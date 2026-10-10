//! The management read engine over the api-log files: `usage:list` and `usage:query` (README
//! §7.1).
//!
//! Both reads run per file — each retained file answers its own page or aggregation under the
//! shared [`UsageFilter`] — and merge in Rust: pages sort newest first on `started_at` with each
//! file's own rowid tie-break preserved, and buckets merge by key, then order by the
//! key expressions in request order under SQLite's ordering, re-implemented by
//! [`compare_bucket_keys`]. The day dimension derives no SQL key column: one file is one UTC
//! day, so the merge stamps the bucket day from each row's own file — the same [`ApiLogDay`]
//! mapping the write path used to place the row — and day buckets never straddle files, so
//! the merge is exact. The page read walks the files newest day first and stops once a
//! finished file has filled the page — every row of a newer file outranks every row of an
//! older one, so the aggregation scan of every remaining file is never paid for a page that is
//! already complete; `usage:query` keeps scanning all candidate files, since an aggregation
//! needs everything. The reads are never gated by the recording switch: an absent store
//! answers empty (README §3 *ApiLog*).

use super::{
    ApiLog,
    store::{ApiLogDay, ApiLogStore},
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome, UsageGroupBy},
    database::{Row, Value},
    storage::{Error, numbered_placeholders},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, hash_map::Entry};
use vec1::Vec1;

/// Column order of api log selects, consumed positionally by [`api_log_from_row`].
const API_LOG_COLUMNS: &str = "started_at, api_key_id, provider_id, model_id, \
     endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
     output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens, \
     model_route_id, upstream_http_status";

impl ApiLogStore {
    /// The retained days a read must consider, ascending: the directory listing narrowed by
    /// the read's time bounds — the window's `from` is inclusive and its `to` exclusive, so the
    /// last candidate day is the day of `to - 1`, and `list`'s pagination cursor is a time
    /// bound of the same kind (`started_at < before`), so the tighter of the two upper bounds
    /// applies. A day strictly above the cursor's own day holds only rows at or after the
    /// cursor, so a deep cursor never re-opens the newer day files it cannot read from. The
    /// listing runs on the blocking thread pool (README §3 *ApiLog Store*).
    async fn candidate_days(
        &self,
        filter: &UsageFilter,
        before: Option<i64>,
    ) -> Result<Vec<ApiLogDay>, Error> {
        let lower = filter.from.map(ApiLogDay::of_started_at);
        let upper = filter
            .to
            .map(|to| ApiLogDay::of_started_at(to.saturating_sub(1)))
            .into_iter()
            .chain(before.map(ApiLogDay::of_started_at))
            .min();

        Ok(self
            .retained_days()
            .await?
            .into_iter()
            .filter(|day| {
                lower.is_none_or(|lower| *day >= lower) && upper.is_none_or(|upper| *day <= upper)
            })
            .collect())
    }

    /// Lists accounting rows newest first: at most `limit` rows inside `filter` whose
    /// `started_at` is strictly below `before`, the timestamp cursor derived by the client from
    /// the previous page's last row. Each retained file answers its own newest-first page
    /// under the same filter and cursor, and the pages merge on `started_at`.
    ///
    /// The files are walked newest day first, and the walk stops once a finished file has
    /// filled the page: every row of a newer file outranks every row of an older one — a
    /// row's own stamp selects its file — so the newer files' answers already contain the
    /// whole page and the remaining, strictly older files can contribute nothing to it.
    /// The cursor bounds the walk itself before any file opens: a day strictly above the
    /// cursor's own day holds only rows at or after the cursor, so the deep cursors normal
    /// pagination produces never re-open the newer files they cannot read from. Within the
    /// page, equal stamps never span files, so the stable merge preserves each
    /// file's own `id DESC` tie-break and the result is one global
    /// `ORDER BY started_at DESC, id DESC`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a file cannot be opened, a query fails, or a row is malformed —
    /// a corrupt file fails the request explicitly.
    async fn list(
        &self,
        limit: i64,
        before: Option<i64>,
        filter: &UsageFilter,
    ) -> Result<Vec<ApiLog>, Error> {
        // The wire bounds `limit` at 1..=1000; the conversion never truncates in practice.
        let page_limit = usize::try_from(limit).unwrap_or(usize::MAX);

        let mut page = Vec::new();

        for day in self.candidate_days(filter, before).await?.into_iter().rev() {
            let Some(database) = self.open_day_file_for_read(day).await? else {
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

            // A file's answer is only superseded by newer files, all of which finished before
            // it: once the page is full, every still-unread file holds strictly older rows
            // alone. The last file's answer can overflow the page, so the truncation below
            // still runs.
            if page.len() >= page_limit {
                break;
            }
        }

        page.sort_by_key(|log| std::cmp::Reverse(log.started_at));
        page.truncate(page_limit);

        Ok(page)
    }

    /// Aggregates accounting rows into `selection`'s buckets: each retained file groups and
    /// sums its own rows under [`key_expression`]'s SQL key columns, and the per-file buckets
    /// merge in Rust — counts and sums combine, buckets merge by key, and the merged buckets
    /// order by the key expressions in request order under SQLite's ordering, re-implemented
    /// by [`compare_bucket_keys`]. Day grouping is file-aligned — the merge stamps each day
    /// bucket with its file's own day, and one file is one day — so day buckets never
    /// straddle files and the merge is exact.
    ///
    /// An empty grouping aggregates one total bucket: over an empty store, exactly one zeroed
    /// bucket, never an empty list.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a file cannot be opened, a query fails, or a row is malformed.
    async fn query(&self, selection: &UsageSelection) -> Result<Vec<UsageAggregate>, Error> {
        let key_expressions: Vec<&str> = selection
            .group_by
            .iter()
            .filter_map(|dimension| key_expression(*dimension))
            .collect();
        let key_list = key_expressions.join(", ");

        let aggregates = "COUNT(*), \
             COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), \
             COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(cache_write_tokens), 0), \
             COALESCE(SUM(reasoning_tokens), 0)";

        // No SQL-backed dimensions — an empty selection, or the day dimension alone, whose key
        // the merge stamps from each file — no key columns and no grouping: every file answers
        // the single row an aggregate without `GROUP BY` produces.
        let (select_keys, grouping) = if key_expressions.is_empty() {
            (String::new(), String::new())
        } else {
            (format!("{key_list}, "), format!(" GROUP BY {key_list}"))
        };

        let mut merged: HashMap<UsageBucketKey, UsageAggregate> = HashMap::new();

        if selection.group_by.is_empty() {
            // The total bucket exists even when no file answers: one zeroed bucket, never an
            // empty list.
            merged.insert(UsageBucketKey::default(), UsageAggregate::default());
        }

        for day in self.candidate_days(&selection.filter, None).await? {
            let Some(database) = self.open_day_file_for_read(day).await? else {
                continue;
            };

            let (conditions, values) = filter_conditions(&selection.filter);

            let sql = format!(
                "SELECT {select_keys}{aggregates} FROM api_logs{}{grouping}",
                where_clause(&conditions),
            );

            let rows = database.query(sql, values).await?;

            for row in &rows {
                let Some(aggregate) = usage_aggregate_from_row(row, &selection.group_by, day)?
                else {
                    continue;
                };

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

/// Lists accounting rows through the api-log store. See [`ApiLogStore::list`]
/// for the merge semantics (README §7.1).
///
/// # Errors
///
/// Returns [`Error`] when a file cannot be opened, a query fails, or a row is malformed.
pub(crate) async fn list_api_logs(
    store: &ApiLogStore,
    limit: i64,
    before: Option<i64>,
    filter: &UsageFilter,
) -> Result<Vec<ApiLog>, Error> {
    store.list(limit, before, filter).await
}

/// Aggregates accounting rows through the api-log store. See [`ApiLogStore::query`] for the
/// merge semantics.
///
/// # Errors
///
/// Returns [`Error`] when a file cannot be opened, a query fails, or a row is malformed.
pub(crate) async fn query_usage(
    store: &ApiLogStore,
    selection: &UsageSelection,
) -> Result<Vec<UsageAggregate>, Error> {
    store.query(selection).await
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
pub(crate) struct UsageFilter {
    /// Inclusive lower bound of `started_at`, in UTC microseconds; absent sets no bound.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    from: Option<i64>,

    /// Exclusive upper bound of `started_at`, in UTC microseconds; absent sets no bound.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    to: Option<i64>,

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

    /// Stable public route identities a row may carry; absent lists every route.
    #[serde(
        default,
        deserialize_with = "crate::serde_helpers::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<number>", optional)
    )]
    pub(crate) model_route_id: Option<Vec1<i64>>,

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
    is_stream: Option<bool>,

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
        push_in_condition(&mut conditions, &mut values, "api_key_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(ids) = &filter.provider_id {
        push_in_condition(&mut conditions, &mut values, "provider_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(ids) = &filter.model_id {
        push_in_condition(&mut conditions, &mut values, "model_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(ids) = &filter.model_route_id {
        push_in_condition(&mut conditions, &mut values, "model_route_id", ids, |id| {
            Value::Integer(*id)
        });
    }

    if let Some(types) = &filter.endpoint_type {
        push_in_condition(
            &mut conditions,
            &mut values,
            "endpoint_type",
            types,
            |kind| Value::Text(kind.as_str().to_string()),
        );
    }

    if let Some(outcomes) = &filter.outcome {
        push_in_condition(
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
        push_in_condition(
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

/// Appends one `column IN (…)` condition over `list`: every element maps through `to_value`
/// into its bound value, and the placeholders number in order off `values`'s current length,
/// so the clause binds exactly the values it pushed. The list is non-empty by its `Vec1` type
/// — the wire parser rejects an empty array — so the `IN` never degenerates.
fn push_in_condition<T>(
    conditions: &mut Vec<String>,
    values: &mut Vec<Value>,
    column: &str,
    list: &Vec1<T>,
    to_value: impl Fn(&T) -> Value,
) {
    let placeholders = numbered_placeholders(list.len(), values.len() + 1);

    values.extend(list.iter().map(to_value));
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
        started_at: row.get(0)?,
        api_key_id: row.get(1)?,
        provider_id: row.get(2)?,
        model_id: row.get(3)?,
        endpoint_type: text_enum_column(
            "endpoint_type",
            row.get_value(4)?,
            ApiLogEndpointType::parse,
        )?,
        is_stream: row.get::<i64>(5)? != 0,
        http_status: row.get(6)?,
        outcome: text_enum_column("outcome", row.get_value(7)?, ApiLogOutcome::parse)?,
        duration: row.get(8)?,
        input_tokens: row.get(9)?,
        output_tokens: row.get(10)?,
        cached_tokens: row.get(11)?,
        cache_write_tokens: row.get(12)?,
        reasoning_tokens: row.get(13)?,
        model_route_id: row.get(14)?,
        upstream_http_status: row.get(15)?,
    })
}

/// Parses one aggregation row: one SQL key column per SQL-backed requested dimension, in
/// request order, then the count and the five coalesced sums. The day dimension carries no key
/// column (see [`key_expression`]): the key is stamped with the row's own file's day before
/// the columns walk.
///
/// Answers [`None`] for the one row a day-only selection must never see: a file whose
/// selection matched no rows still answers one zero-count row when the query groups by
/// nothing, and a day-only selection is exactly such a query — one file is one day, so SQL
/// groups by nothing and the file's empty partition must stay bucketless, as a `GROUP BY`-ed
/// day query would answer. A group in any other selection forms only around rows, so its
/// `requests` is never zero, and the empty selection's total bucket needs its zero row (README
/// §7.1).
fn usage_aggregate_from_row(
    row: &Row,
    group_by: &[UsageGroupBy],
    day: ApiLogDay,
) -> Result<Option<UsageAggregate>, Error> {
    let key_columns = group_by
        .iter()
        .filter_map(|dimension| key_expression(*dimension))
        .count();
    let requests: i64 = row.get(key_columns)?;

    if key_columns == 0 && group_by.contains(&UsageGroupBy::Day) && requests == 0 {
        return Ok(None);
    }

    let mut key = UsageBucketKey::default();

    // One file is one UTC day (README §3 *ApiLog Store*): every row this file answers
    // carries the file's own day, stamped here so the bucket day is the floor-day of every
    // stamp by construction — the same [`ApiLogDay`] mapping the write path used — never a
    // SQL-side re-derivation of it.
    if group_by.contains(&UsageGroupBy::Day) {
        key.day = Some(day.file_stem());
    }

    let mut column = 0;

    for dimension in group_by {
        // The SQL-backed dimensions sit at consecutive columns in request order, so each reads
        // the next column and the columnless day dimension reads none.
        if key_expression(*dimension).is_some() {
            read_key_part(&mut key, *dimension, row, column)?;
            column += 1;
        }
    }

    Ok(Some(UsageAggregate {
        key,
        requests,
        input_tokens: row.get(key_columns + 1)?,
        output_tokens: row.get(key_columns + 2)?,
        cached_tokens: row.get(key_columns + 3)?,
        cache_write_tokens: row.get(key_columns + 4)?,
        reasoning_tokens: row.get(key_columns + 5)?,
    }))
}

/// Reads one closed-vocabulary text enum column, failing with internal drift on anything outside
/// the enum's vocabulary — see the `parse` each call site passes for the accepted spellings. The
/// parse-failure message spells the kind as the column name with its underscores as spaces.
fn text_enum_column<T>(
    column: &'static str,
    value: Value,
    parse: fn(&str) -> Option<T>,
) -> Result<T, Error> {
    match value {
        Value::Text(text) => parse(&text).ok_or_else(|| Error::Drift {
            message: format!("unknown api log {} `{text}`", column.replace('_', " ")),
        }),

        _ => Err(Error::Drift {
            message: format!("{column} column must be a text value"),
        }),
    }
}

/// The bucket key of one aggregation row (README §7.1): a fixed declaration-ordered object
/// carrying exactly the requested dimensions — `None` for every dimension the aggregation
/// did not request — while the request order drives only the bucket ordering, never the
/// wire layout of the key. `http_status` is `None` both for an unrequested dimension and for
/// the bucket the `NULL` rows form; the wire layer decides the field's presence from the
/// requested dimensions and renders that bucket as `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct UsageBucketKey {
    /// Grouped public route identity.
    pub(crate) model_route_id: Option<i64>,
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
    /// Folds another file's identical-key bucket into this one: counts and sums combine. Sums
    /// saturate because a merge of real token counts never approaches `i64::MAX` and a merge
    /// must not fail where no single file's query would.
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

/// One dimension's bucket-key value: the single ordered view the dimension descriptor speaks.
/// The merge ordering compares two keys dimension by dimension through [`key_part`], and only
/// the view's inner order is ever exercised — both compared parts always answer the same
/// requested dimension, so the derived cross-variant order (declaration order) carries no
/// contract. Each variant's inner order is its stored column's SQLite ordering (README §7.1):
/// text by its bytes, numbers ascending, booleans as their stored `0`/`1`, and every absence
/// — carried by the [`Option`] wrapper [`key_part`] returns, never inside a variant — first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KeyPart<'a> {
    /// The `day` text, byte-ordered like every SQLite text column.
    Text(&'a str),

    /// A numeric column: a row id, or the `http_status` number.
    Id(i64),

    /// A closed-vocabulary column's stored spelling, byte-ordered — the enums order by their
    /// recorded vocabularies exactly as the SQL `GROUP BY` ordered them.
    EnumStr(&'static str),

    /// The stream flag, ordered as its stored `0`/`1` — `false` before `true`.
    Flag(bool),
}

/// Reads the key's part for `dimension`: [`None`] when the dimension was not requested — every
/// key field of an unrequested dimension defaults to [`None`] — and the dimension's [`KeyPart`]
/// view otherwise. The view folds the one nullable bucket value into the same wrapper: the
/// `http_status` `NULL` rows' bucket reads [`None`] exactly as `NULL` sorts before every
/// answered status, so `Option<KeyPart>` orders as SQLite orders the column.
fn key_part(key: &UsageBucketKey, dimension: UsageGroupBy) -> Option<KeyPart<'_>> {
    match dimension {
        UsageGroupBy::Day => key.day.as_deref().map(KeyPart::Text),
        UsageGroupBy::Model => key.model_id.map(KeyPart::Id),
        UsageGroupBy::ModelRoute => key.model_route_id.map(KeyPart::Id),
        UsageGroupBy::Provider => key.provider_id.map(KeyPart::Id),
        UsageGroupBy::ApiKey => key.api_key_id.map(KeyPart::Id),
        UsageGroupBy::EndpointType => key
            .endpoint_type
            .map(|kind| KeyPart::EnumStr(kind.as_str())),
        UsageGroupBy::Outcome => key
            .outcome
            .map(|outcome| KeyPart::EnumStr(outcome.as_str())),
        UsageGroupBy::IsStream => key.is_stream.map(KeyPart::Flag),
        UsageGroupBy::HttpStatus => key.http_status.map(KeyPart::Id),
    }
}

/// The SQL key column a dimension groups, selects, and orders by — [`None`] when it needs no
/// SQL column at all. Only the day dimension needs none: one file is one UTC day (README §3
/// *ApiLog Store*), so [`usage_aggregate_from_row`] stamps the bucket day from each row's
/// own file — the very [`ApiLogDay`] mapping the write path used to place the row — and the key
/// is the floor-day of every stamp by construction, signed stamps included. Deriving the day
/// in SQL instead would re-implement that mapping with SQLite's `/`, which truncates toward
/// zero and so disagrees with the file's floor division for a pre-epoch stamp — a divergence
/// from the file-alignment invariant, unreachable through the dispatch clock but never the
/// mapping's to assume.
fn key_expression(dimension: UsageGroupBy) -> Option<&'static str> {
    match dimension {
        UsageGroupBy::Day => None,
        UsageGroupBy::Model => Some("model_id"),
        UsageGroupBy::ModelRoute => Some("model_route_id"),
        UsageGroupBy::Provider => Some("provider_id"),
        UsageGroupBy::ApiKey => Some("api_key_id"),
        UsageGroupBy::EndpointType => Some("endpoint_type"),
        UsageGroupBy::Outcome => Some("outcome"),
        UsageGroupBy::IsStream => Some("is_stream"),
        UsageGroupBy::HttpStatus => Some("http_status"),
    }
}

/// Reads the one SQL key column at `column` — the per-dimension key columns sit in request
/// order — into its key field. Every column carries exactly its stored type's values (README §3
/// *ApiLog*): the ids, the flag, and the enum spellings are `NOT NULL`, and only `http_status`
/// may read `NULL`, landing as the [`None`] that [`key_part`] orders first. The day dimension
/// carries no column — [`usage_aggregate_from_row`] stamps its key from the file.
fn read_key_part(
    key: &mut UsageBucketKey,
    dimension: UsageGroupBy,
    row: &Row,
    column: usize,
) -> Result<(), Error> {
    match dimension {
        UsageGroupBy::Day => {}
        UsageGroupBy::Model => key.model_id = Some(row.get(column)?),
        UsageGroupBy::ModelRoute => key.model_route_id = Some(row.get(column)?),
        UsageGroupBy::Provider => key.provider_id = Some(row.get(column)?),
        UsageGroupBy::ApiKey => key.api_key_id = Some(row.get(column)?),
        UsageGroupBy::EndpointType => {
            key.endpoint_type = Some(text_enum_column(
                "endpoint_type",
                row.get_value(column)?,
                ApiLogEndpointType::parse,
            )?);
        }
        UsageGroupBy::Outcome => {
            key.outcome = Some(text_enum_column(
                "outcome",
                row.get_value(column)?,
                ApiLogOutcome::parse,
            )?);
        }
        UsageGroupBy::IsStream => key.is_stream = Some(row.get::<i64>(column)? != 0),
        UsageGroupBy::HttpStatus => key.http_status = row.get(column)?,
    }

    Ok(())
}

/// Orders two bucket keys the way SQLite orders the key expressions (README §7.1): per
/// requested dimension in request order, the first non-equal dimension deciding. Each
/// dimension's ordering is [`KeyPart`]'s derived order through [`key_part`] — `NULL` first,
/// text by its bytes (the enums by their stored spellings), numbers ascending — so the
/// per-dimension comparison exists exactly once.
fn compare_bucket_keys(
    first: &UsageBucketKey,
    second: &UsageBucketKey,
    group_by: &[UsageGroupBy],
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    for dimension in group_by {
        let ordering = key_part(first, *dimension).cmp(&key_part(second, *dimension));

        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    Ordering::Equal
}
