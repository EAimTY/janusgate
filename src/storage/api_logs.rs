//! Request accounting: the `api_logs` shard store — its write path and read paths (README §3
//! *ApiLog Shard Store*).
//!
//! One row is one dispatched inference request — the instant the upstream HTTP request was issued
//! (`send()`). Everything before that instant (authentication, routing, token refresh, upstream
//! payload rendering) is never recorded, and no content — prompt, completion, headers, upstream
//! error bodies — ever reaches the table. The rows live outside the main database: one SQLite
//! (Turso) file per UTC calendar day of `started_at`, created lazily on first write, written
//! through a hot-shard connection cache, and expired by whole-file deletion.
//!
//! The draft lifecycle ([`ApiLogDraft`]) and the management read surface live beside the shard
//! store in the same module as the table statements. Recording is best-effort by settled
//! boundary, not by accident: a non-streaming client's disconnect *after* the response has been
//! handed to hyper is unobservable — that row records `completed` with the status it rendered —
//! and a connection that cancels the handler while it still awaits the upstream response never
//! reaches a finalization point, so it writes no row at all. Those two classes of missed
//! recording are the documented edge of the accounting semantics (README §3 *ApiLog*).
//!
//! Storage failures classify through the shared storage error domain ([`Error`]): a constraint
//! backstop classifies as a conflict on the management taxonomy with no new mapping, and a
//! recorded row's closed-vocabulary columns parse back as drift rather than as a client fault.
//! Every recording failure is logged and swallowed — it never enters any HTTP error domain.

use super::Error;
use crate::{
    database::{Database, Row, Value, params, schema::Registry},
    ir::IrUsage,
    unix_time,
};
use log::{error, warn};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, hash_map::Entry},
    num::NonZero,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use vec1::Vec1;

/// Microseconds in one UTC day: the width of one shard.
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// How many shards the hot cache holds: today + yesterday + margin for stragglers finalized
/// after midnight. A performance hint, never a correctness bound — a miss beyond it lazily opens
/// the existing file.
const HOT_SHARD_CAPACITY: usize = 3;

/// Wait for a free shard pool connection, matching the main database's connection configuration.
const SHARD_POOL_WAIT: Duration = Duration::from_secs(5);

/// Column order of api log selects, consumed positionally by [`api_log_from_row`].
const API_LOG_COLUMNS: &str = "started_at, api_key_id, provider_id, model_id, \
     endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
     output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens";

/// The fixed shard connection pool size for writes; a performance choice, not configuration.
///
/// # Panics
///
/// Never: the literal 2 is nonzero, and the `expect` states that invariant.
fn shard_write_pool_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 2 is nonzero")]
    let connections = NonZero::new(2).expect("2 is nonzero");

    connections
}

/// The fixed shard connection pool size for reads: one read query runs per shard at a time.
///
/// # Panics
///
/// Never: the literal 1 is nonzero, and the `expect` states that invariant.
fn shard_read_pool_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 1 is nonzero")]
    let connections = NonZero::new(1).expect("1 is nonzero");

    connections
}

/// One UTC calendar day: the shard key, derived purely from a row's `started_at`. There is no
/// manifest or registry anywhere — the shard directory's listing is the only truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ShardDay(i64);

impl ShardDay {
    /// Derives the shard of a `started_at` stamp: the stamp's UTC calendar day.
    pub(crate) fn of_started_at(started_at: i64) -> Self {
        Self(started_at.div_euclid(MICROS_PER_DAY))
    }

    /// The first microsecond after the day. Wide in `i128` because the day after the largest
    /// representable stamp's day may end past `i64::MAX`.
    fn end_micros(self) -> i128 {
        i128::from(self.0 + 1) * i128::from(MICROS_PER_DAY)
    }

    /// Renders the shard file's stem: `YYYY-MM-DD`, zero-padded, sortable.
    fn file_stem(self) -> String {
        let (year, month, day) = self.civil();

        format!("{year:04}-{month:02}-{day:02}")
    }

    /// Parses a shard file name back into its day: exactly `YYYY-MM-DD.db` naming a real
    /// calendar day. Anything else — including an impossible date such as `2026-02-30` — is
    /// unparseable, and the caller leaves such files alone.
    fn parse_file_stem_of_file_name(file_name: &str) -> Option<Self> {
        let stem = file_name.strip_suffix(".db")?;
        let (year, rest) = stem.split_once('-')?;
        let (month, day) = rest.split_once('-')?;

        let is_digits =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());

        if !is_digits(year)
            || year.len() > 6
            || !is_digits(month)
            || month.len() != 2
            || !is_digits(day)
            || day.len() != 2
        {
            return None;
        }

        let year: i64 = year.parse().ok()?;
        let month: u32 = month.parse().ok()?;
        let day: u32 = day.parse().ok()?;

        if !(1..=12).contains(&month) || !(1..=Self::days_in_month(year, month)).contains(&day) {
            return None;
        }

        Some(Self(Self::days_from_civil(year, month, day)))
    }

    /// The day's `(year, month, day)` in the proleptic Gregorian calendar.
    fn civil(self) -> (i64, u32, u32) {
        // Howard Hinnant's civil_from_days. The era division truncates toward zero like the
        // original C++, hence the conditional adjustment for negative values; every quantity
        // below it is non-negative.
        let z = self.0 + 719_468;
        let era = if z >= 0 {
            z / 146_097
        } else {
            (z - 146_096) / 146_097
        };
        let day_of_era = z - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_pointer = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_pointer + 2) / 5 + 1;
        let month = if month_pointer < 10 {
            month_pointer + 3
        } else {
            month_pointer - 9
        };

        (
            year + i64::from(month <= 2),
            u32::try_from(month).unwrap_or(1),
            u32::try_from(day).unwrap_or(1),
        )
    }

    /// The day number of a proleptic Gregorian calendar date — the inverse of [`ShardDay::civil`].
    fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
        // Howard Hinnant's days_from_civil, era division truncating toward zero as above.
        let y = year - i64::from(month <= 2);
        let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
        let year_of_era = y - era * 400;
        let month_pointer = i64::from(month) + if month > 2 { -3 } else { 9 };
        let day_of_year = (153 * month_pointer + 2) / 5 + i64::from(day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

        era * 146_097 + day_of_era - 719_468
    }

    /// The number of days in the month, leap years included; an out-of-range month has none,
    /// which the parser's day-range check rejects.
    fn days_in_month(year: i64, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if Self::is_leap_year(year) => 29,
            2 => 28,
            _ => 0,
        }
    }

    /// The Gregorian leap-year rule.
    fn is_leap_year(year: i64) -> bool {
        (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
    }
}

/// The api-log shard store (README §3 *ApiLog Shard Store*): the day-partitioned shard files
/// under its directory, the hot-shard write cache over them, and the retention policy. Cloned
/// handles share one directory, one retention, and one cache.
#[derive(Clone, Debug)]
pub(crate) struct ApiLogStore {
    /// State shared by every clone of the store.
    inner: Arc<StoreInner>,
}

/// The shared state of an [`ApiLogStore`].
#[derive(Debug)]
struct StoreInner {
    /// Directory holding the shard files; consumed once at startup from the configuration.
    directory: PathBuf,

    /// Retention in whole days: the sweep deletes shard files whose day-end is strictly older
    /// than now minus this.
    retention_days: NonZero<u32>,

    /// The hot-shard cache, most recently written shard first and at most
    /// [`HOT_SHARD_CAPACITY`] entries. The lock is asynchronous and covers the whole
    /// get-or-open decision, so two racing writers for one shard can never both apply the
    /// shard DDL.
    hot: tokio::sync::Mutex<Vec<(ShardDay, Database)>>,
}

impl ApiLogStore {
    /// Builds the store over `directory` with `retention_days`. Nothing is touched on the
    /// filesystem: the directory is created lazily when the first shard file is created.
    pub(crate) fn new(directory: PathBuf, retention_days: NonZero<u32>) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                directory,
                retention_days,
                hot: tokio::sync::Mutex::new(Vec::new()),
            }),
        }
    }

    /// The path of one day's shard file.
    fn shard_path(&self, day: ShardDay) -> PathBuf {
        self.inner.directory.join(format!("{}.db", day.file_stem()))
    }

    /// Inserts one accounting row into its day's shard, deriving the shard from the row's own
    /// `started_at` stamp — never from the finalization clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the shard cannot be opened or the insert fails. The caller logs
    /// and swallows every failure: recording never enters any HTTP error domain.
    async fn record(&self, write: ApiLogWrite) -> Result<(), Error> {
        let database = self
            .shard_for_write(ShardDay::of_started_at(write.started_at))
            .await?;

        insert_api_log(&database, write).await
    }

    /// Returns a pooled shard connection for `day`, consulting the hot cache and, on a miss,
    /// lazily opening the shard file — creating it, and with it running the retention sweep,
    /// only when no file is present yet.
    ///
    /// The asynchronous lock is held across the whole decision, so concurrent writers
    /// serialize on cache misses only; the returned handle is pooled and used outside the lock.
    async fn shard_for_write(&self, day: ShardDay) -> Result<Database, Error> {
        let mut hot = self.inner.hot.lock().await;

        if let Some(position) = hot.iter().position(|(cached, _)| *cached == day) {
            // Restamp the entry as the most recently written shard.
            let (_, database) = hot.remove(position);
            hot.insert(0, (day, database.clone()));

            return Ok(database);
        }

        let path = self.shard_path(day);
        let is_new = !path.exists();
        let database = Database::open(
            &path,
            Registry::ApiLogShard,
            shard_write_pool_connections(),
            SHARD_POOL_WAIT,
        )
        .await?;

        if is_new {
            // Creating a new file is the sweep's only trigger; opening an existing shard for a
            // late write never sweeps.
            self.sweep(&mut hot, day);
        }

        hot.insert(0, (day, database.clone()));
        hot.truncate(HOT_SHARD_CAPACITY);

        Ok(database)
    }

    /// Deletes the expired shard files: every shard whose day-end is strictly older than now
    /// minus the retention, except the file just created. A cached shard is evicted before its
    /// file is unlinked, so the pool never keeps writing into a deleted file. Unparseable
    /// filenames are left alone, and every failure here is logged and swallowed — retention is
    /// a maintenance sweep, never an error surface.
    ///
    /// Called with the hot cache locked, immediately after a new shard file was created.
    fn sweep(&self, hot: &mut Vec<(ShardDay, Database)>, just_created: ShardDay) {
        let cutoff = i128::from(unix_time::unix_now_micros())
            - i128::from(self.inner.retention_days.get()) * i128::from(MICROS_PER_DAY);

        let entries = match std::fs::read_dir(&self.inner.directory) {
            Ok(entries) => entries,

            Err(failure) => {
                warn!(
                    "failed to list the api log shard directory {}: {failure}",
                    self.inner.directory.display()
                );

                return;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,

                Err(failure) => {
                    warn!("failed to read an api log shard directory entry: {failure}");

                    continue;
                }
            };

            let Some(day) = entry
                .file_name()
                .to_str()
                .and_then(ShardDay::parse_file_stem_of_file_name)
            else {
                continue;
            };

            if day == just_created || day.end_micros() >= cutoff {
                continue;
            }

            hot.retain(|(cached, _)| *cached != day);

            if let Err(failure) = std::fs::remove_file(entry.path()) {
                warn!(
                    "failed to delete the expired api log shard file {}: {failure}",
                    entry.path().display()
                );

                continue;
            }

            // A stale `-wal`/`-shm` sibling — from a crash mid-write, say — must not outlive
            // its shard file.
            for suffix in ["-wal", "-shm"] {
                let mut sibling = entry.file_name().to_os_string();
                sibling.push(suffix);
                let sibling = self.inner.directory.join(sibling);
                let _ = std::fs::remove_file(sibling);
            }
        }
    }

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

    /// Every shard day the directory currently holds, ascending. An absent directory reads as
    /// empty; an unreadable one fails the read explicitly.
    fn retained_days(&self) -> Result<Vec<ShardDay>, Error> {
        let entries = match std::fs::read_dir(&self.inner.directory) {
            Ok(entries) => entries,

            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }

            Err(failure) => return Err(crate::database::Error::Io(failure).into()),
        };

        let mut days = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| ShardDay::parse_file_stem_of_file_name(&name))
            .collect::<Vec<_>>();

        days.sort();

        Ok(days)
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
    /// Equal stamps never span shards — a row's own stamp selects its shard — so the stable
    /// merge preserves each shard's own `id DESC` tie-break and the result is one global
    /// `ORDER BY started_at DESC, id DESC`.
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
        let mut page = Vec::new();

        for day in self.candidate_days(filter)? {
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
        }

        page.sort_by_key(|log| std::cmp::Reverse(log.started_at));
        page.truncate(
            // The wire bounds `limit` at 1000; the conversion never truncates in practice.
            usize::try_from(limit).unwrap_or(usize::MAX),
        );

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
            .map(|dimension| dimension.expression())
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

/// The outward completion endpoint a request arrived on, as recorded in `api_logs.endpoint_type`.
///
/// The serde and TypeScript names are the storage names — the `CHECK` vocabulary of the column
/// and [`ApiLogEndpointType::as_str`] are one vocabulary (README §3 *ApiLog*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// One accounting row about to be inserted: every column but the server-assigned `id`.
#[derive(Debug, Clone)]
struct ApiLogWrite {
    /// Dispatch-start instant in UTC microseconds; stamped the moment `send()` is called.
    started_at: i64,

    /// Row id of the authenticated gateway API key.
    api_key_id: i64,

    /// Row id of the routed provider.
    provider_id: i64,

    /// Row id of the routed model.
    model_id: i64,

    /// Outward completion endpoint the request arrived on.
    endpoint_type: ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    is_stream: bool,

    /// HTTP status delivered to the client; `NULL` exactly when no response was ever produced
    /// (README §3 *ApiLog*).
    http_status: Option<i64>,

    /// Recorded outcome of the request.
    outcome: ApiLogOutcome,

    /// Milliseconds between the dispatch stamp and the accounting instant.
    duration: i64,

    /// Tokens consumed by the input; `NULL` when the upstream reported no usage.
    input_tokens: Option<i64>,

    /// Tokens produced by the output, `reasoning_tokens` included; `NULL` without usage.
    output_tokens: Option<i64>,

    /// Input tokens served from the upstream prompt cache; `NULL` without that breakdown.
    cached_tokens: Option<i64>,

    /// Input tokens written to the upstream prompt cache; `NULL` without that breakdown.
    cache_write_tokens: Option<i64>,

    /// Output tokens spent on reasoning; `NULL` without that breakdown.
    reasoning_tokens: Option<i64>,
}

/// One dispatched request's accounting draft: every fact known before dispatch, carried through
/// [`crate::gateway::execute`] and closed by exactly one of the three finalization points.
///
/// The module's single invariant: **`started_at` is stamped ⇔ the request was dispatched with
/// accounting enabled**. Who stamps, who finalizes, and whether a row is written are all decided
/// by that one variable — [`ApiLogDraft::stamp`] reads the live presence of the `api_logs`
/// configuration section, and [`finalize_api_log`] writes if and only if a stamp is present.
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
    /// `started_at` from the database clock when the `api_logs` configuration section is present,
    /// read live from the global configuration so a reload applies to the very next dispatch.
    /// With the section absent the draft stays unstamped and no row is ever written for this
    /// request — this one call is the whole switch.
    pub(crate) fn stamp(&mut self) {
        if crate::CONFIG.api_logs.is_some() {
            self.started_at = Some(unix_time::unix_now_micros());
        }
    }
}

/// Closes one dispatched request's accounting: builds the terminal write from `draft` plus the
/// finalization facts and inserts it into the draft's own day's shard. Every call site runs at
/// exactly one of the three mutually exclusive finalization points (README §3 *ApiLog*), so each
/// dispatched request is recorded exactly once.
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
    store: Option<&ApiLogStore>,
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

    let Some(store) = store else {
        // Reachable only when the `api_logs` section was added by a reload after a startup
        // without it: the stamp read the live switch, but the shard store is constructed at
        // startup alone (README §8). The row is dropped with a logged attribution.
        error!(
            "api log recording is configured but the shard store was not constructed at startup; \
             dropping the row for API key {} (provider {}, model {})",
            draft.api_key_id, draft.provider_id, draft.model_id,
        );

        return;
    };

    if let Err(failure) = store.record(write).await {
        error!(
            "failed to record api log for API key {} (provider {}, model {}): {failure}",
            draft.api_key_id, draft.provider_id, draft.model_id,
        );
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

/// Inserts one accounting row into the shard database behind `database` — a single synchronous
/// statement on a pooled connection.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails — including a schema `CHECK` violation, which
/// classifies as [`Error::Constraint`].
async fn insert_api_log(database: &Database, write: ApiLogWrite) -> Result<(), Error> {
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
    pub(crate) use super::*;
    use crate::{
        database::{Database, Value},
        storage::{
            self,
            api_keys::{self, NewApiKey},
            models::ModelCreate,
        },
    };
    use std::{error::Error as StdError, path::PathBuf};

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
            reason = "the schema pins every row at fourteen columns"
        )]
        &row[index]
    }

    /// Directory guard cleaning up the temporary shard tree on drop.
    struct TempStoreGuard(PathBuf);

    impl Drop for TempStoreGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// File guard cleaning up the temporary main test database on drop.
    struct TempDbGuard(PathBuf);

    impl Drop for TempDbGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Creates a shard store over a fresh temporary directory with `retention_days`.
    ///
    /// # Panics
    ///
    /// Panics on a zero `retention_days`, which no test passes.
    fn create_test_store(retention_days: u32) -> (ApiLogStore, TempStoreGuard) {
        let directory =
            std::env::temp_dir().join(format!("janusgate-apilogs-test-{}", rand::random::<u64>()));

        #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
        let retention = NonZero::new(retention_days).unwrap();

        (
            ApiLogStore::new(directory.clone(), retention),
            TempStoreGuard(directory),
        )
    }

    /// Creates an initialized temporary main SQLite test database.
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
        let database = Database::open(&db_path, Registry::Main, max_connections, SHARD_POOL_WAIT)
            .await
            .unwrap();

        (database, TempDbGuard(db_path))
    }

    /// Opens one shard of `store` for raw statement probes, creating the file when absent.
    ///
    /// # Panics
    ///
    /// Panics if opening the shard fails.
    async fn open_shard(store: &ApiLogStore, day: ShardDay) -> Database {
        #[expect(clippy::unwrap_used, reason = "a test shard file always opens")]
        Database::open(
            store.shard_path(day),
            Registry::ApiLogShard,
            shard_write_pool_connections(),
            SHARD_POOL_WAIT,
        )
        .await
        .unwrap()
    }

    /// Reads every column of every stored row of one shard, in rowid order, as raw values the
    /// test asserts positionally.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    async fn read_all(database: &Database) -> Result<Vec<Vec<Value>>, Error> {
        let rows = database
            .query(
                "SELECT started_at, api_key_id, provider_id, model_id, endpoint_type, \
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

    /// The complete row used by the round-trip test: every nullable column populated and the
    /// non-default endpoint, outcome, and stream flag set. The dispatch stamp is caller-chosen,
    /// so a fixture can stay inside the retention window the sweep enforces on every new shard
    /// file.
    pub(crate) fn full_write(started_at: i64) -> ApiLogWrite {
        ApiLogWrite {
            started_at,
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

    /// One read-fixture write: fixed identity and terminal facts, caller-chosen dispatch time and
    /// token columns (`[input, output, cached, cache_write, reasoning]`).
    pub(crate) fn write(
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

    /// The first microsecond of a shard day: the inverse of [`ShardDay::of_started_at`], for
    /// placing fixture stamps inside a chosen day.
    fn day_start(day: ShardDay) -> i64 {
        // `of_started_at` divides by `MICROS_PER_DAY`, so the product can never overflow.
        day.0 * MICROS_PER_DAY
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

    /// Verifies the shard day mapping end to end: a stamp maps to its UTC calendar day, the day
    /// boundaries round-trip, the file stem renders `YYYY-MM-DD` and parses back — leap years
    /// included — and every malformed file name is unparseable, so a sweep leaves such files
    /// alone.
    ///
    /// # Panics
    ///
    /// Panics if a mapping or parse diverges from the documented behavior.
    #[test]
    fn shard_days_map_to_file_names_and_back() {
        // Day boundaries: the day of a stamp is the stamp's own calendar day, and the first
        // microsecond of the next day already belongs to the next shard.
        let day = ShardDay::of_started_at(1_700_000_000_000_000);
        let start = day_start(day);

        assert_eq!(ShardDay::of_started_at(start), day);
        assert_eq!(
            ShardDay::of_started_at(start + MICROS_PER_DAY - 1),
            day,
            "the day's last microsecond stays in the day",
        );
        assert_ne!(
            ShardDay::of_started_at(start + MICROS_PER_DAY),
            day,
            "the next day's first microsecond moves on",
        );

        // The civil conversion renders sortable ISO days, before the epoch included.
        assert_eq!(ShardDay::of_started_at(0).file_stem(), "1970-01-01");
        assert_eq!(day.file_stem(), "2023-11-14");
        assert_eq!(
            ShardDay::of_started_at(-1).file_stem(),
            "1969-12-31",
            "the mapping is UTC-calendar exact across the epoch",
        );

        // File names parse back into the day they render, leap days included.
        for (name, stamp) in [
            ("1970-01-01.db", 0),
            ("2023-11-14.db", 1_700_000_000_000_000),
            ("1969-12-31.db", -1),
        ] {
            #[expect(clippy::unwrap_used, reason = "the name is constructed to parse")]
            let parsed = ShardDay::parse_file_stem_of_file_name(name).unwrap();

            assert_eq!(parsed, ShardDay::of_started_at(stamp), "{name}");
        }

        #[expect(clippy::unwrap_used, reason = "2024 is a leap year")]
        let leap_day = ShardDay::parse_file_stem_of_file_name("2024-02-29.db").unwrap();
        assert_eq!(leap_day.file_stem(), "2024-02-29");

        // Every malformed name is unparseable: wrong shape, impossible dates, non-leap February
        // 29ths, and foreign extensions.
        for name in [
            "not-a-shard.db",
            "20260101.db",
            "2026-1-01.db",
            "2026-01-1.db",
            "2026-13-01.db",
            "2026-00-01.db",
            "2026-01-00.db",
            "2026-01-32.db",
            "2026-02-30.db",
            "2023-02-29.db",
            "2026-01-01.txt",
            "2026-01-01.db.bak",
            "",
        ] {
            assert_eq!(
                ShardDay::parse_file_stem_of_file_name(name),
                None,
                "{name} must not parse",
            );
        }
    }

    /// Verifies the write path column by column: a fully populated row round-trips exactly as
    /// written — the closed-vocabulary columns stored through `as_str`, the stream flag as
    /// `0`/`1` — a row whose seven nullable columns are all `NULL` reads back as `NULL` rather
    /// than a default, and each row lands in its own stamp's day shard. The `as_str` spellings
    /// are asserted equal to the serde vocabulary, because the storage string and the published
    /// wire string are one vocabulary.
    ///
    /// # Errors
    ///
    /// Returns an error if a store operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a stored column diverges from the written fixture.
    #[tokio::test]
    async fn records_rows_reading_back_every_column() -> Result<(), Box<dyn StdError>> {
        let (store, _guard) = create_test_store(90);

        // Yesterday and now: two recent, distinct days, safely inside the retention window.
        let full_stamp = unix_time::unix_now_micros() - MICROS_PER_DAY;
        store.record(full_write(full_stamp)).await?;

        let sparse_stamp = unix_time::unix_now_micros();
        let sparse = ApiLogWrite {
            started_at: sparse_stamp,
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

        store.record(sparse).await?;

        let full_shard = open_shard(&store, ShardDay::of_started_at(full_stamp)).await;
        let rows = read_all(&full_shard).await?;
        assert_eq!(rows.len(), 1);

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        let full = rows.first().unwrap();
        assert_eq!(column(full, 0), &Value::Integer(full_stamp));
        assert_eq!(column(full, 1), &Value::Integer(3));
        assert_eq!(column(full, 2), &Value::Integer(5));
        assert_eq!(column(full, 3), &Value::Integer(7));
        assert_eq!(
            column(full, 4),
            &Value::Text("openai_responses".to_string())
        );
        assert_eq!(column(full, 5), &Value::Integer(1));
        assert_eq!(column(full, 6), &Value::Integer(200));
        assert_eq!(column(full, 7), &Value::Text("client_aborted".to_string()));
        assert_eq!(column(full, 8), &Value::Integer(1234));
        assert_eq!(column(full, 9), &Value::Integer(11));
        assert_eq!(column(full, 10), &Value::Integer(22));
        assert_eq!(column(full, 11), &Value::Integer(3));
        assert_eq!(column(full, 12), &Value::Integer(4));
        assert_eq!(column(full, 13), &Value::Integer(5));

        let today_shard = open_shard(&store, ShardDay::of_started_at(sparse_stamp)).await;
        let rows = read_all(&today_shard).await?;
        assert_eq!(rows.len(), 1);

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        let sparse = rows.first().unwrap();
        assert_eq!(column(sparse, 0), &Value::Integer(sparse_stamp));
        assert_eq!(
            column(sparse, 4),
            &Value::Text("openai_chat_completions".to_string())
        );
        assert_eq!(column(sparse, 5), &Value::Integer(0));
        assert_eq!(column(sparse, 6), &Value::Null);
        assert_eq!(column(sparse, 7), &Value::Text("internal".to_string()));

        for index in 9..14 {
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

    /// Verifies that a schema `CHECK` rejection surfacing through a shard's write path
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
        let (store, _guard) = create_test_store(90);
        let database = open_shard(&store, ShardDay::of_started_at(1)).await;

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
    /// delete paths, the provider's cascade included — leaves the log row byte-for-byte intact in
    /// its shard, still attributing by the never-recycled ids it stores.
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
        let (store, _store_guard) = create_test_store(90);

        let api_key = api_keys::insert_api_key(
            &database,
            &NewApiKey {
                name: "service",
                key: "sk-live",
                is_enabled: true,
            },
        )
        .await?;

        let definition = crate::test_support::insert_static_key_definition(
            &database,
            "static-key",
            crate::format::UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        let provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "acme",
            "acme-",
            Some("http://upstream.example"),
            crate::provider::StoredCredentials::ApiKey {
                api_key: "secret".to_string(),
            },
        )
        .await;

        let model = storage::models::insert_model_checked(
            &database,
            ModelCreate {
                provider_id: provider.id,
                upstream_id: "gpt-4",
                id_override: None,
                is_enabled: true,
            },
        )
        .await?;

        finalize_api_log(
            Some(&store),
            &ApiLogDraft {
                started_at: Some(1_000),
                api_key_id: api_key.id,
                provider_id: provider.id,
                model_id: model.model.id,
                endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
                is_stream: false,
            },
            ApiLogOutcome::Completed,
            Some(200),
            Some(crate::ir::IrUsage {
                input_tokens: 1,
                output_tokens: 2,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        )
        .await;

        assert!(api_keys::delete_api_key(&database, api_key.id).await?);
        storage::models::delete_model_checked(&database, model.model.id).await?;
        assert!(storage::providers::delete_provider(&database, provider.id).await?);

        let rows = read_all(&open_shard(&store, ShardDay::of_started_at(1_000)).await).await?;

        assert_eq!(rows.len(), 1);

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        let row = rows.first().unwrap();
        assert_eq!(column(row, 0), &Value::Integer(1_000));
        assert_eq!(column(row, 1), &Value::Integer(api_key.id));
        assert_eq!(column(row, 2), &Value::Integer(provider.id));
        assert_eq!(column(row, 3), &Value::Integer(model.model.id));
        assert_eq!(column(row, 6), &Value::Integer(200));
        assert_eq!(column(row, 9), &Value::Integer(1));
        assert_eq!(column(row, 10), &Value::Integer(2));

        Ok(())
    }

    /// Verifies the best-effort accounting boundary (README §8): finalizing against a store whose
    /// shard directory cannot be created — the configured path sits beneath a regular file —
    /// neither panics nor surfaces anywhere, and the main database keeps serving reads and writes
    /// of other tables afterwards. The unstamped-draft path — the switch-off shape — performs no
    /// INSERT at all, with or without a store.
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

        // The shard directory path is doomed: its parent is a regular file, so no shard file can
        // ever be created beneath it.
        let blocker =
            std::env::temp_dir().join(format!("janusgate-apilogs-block-{}", rand::random::<u64>()));

        std::fs::write(&blocker, b"not a directory")?;

        #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
        let retention = NonZero::new(90).unwrap();
        let store = ApiLogStore::new(blocker.join("shards"), retention);

        let draft = ApiLogDraft {
            started_at: Some(1_000),
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            endpoint_type: ApiLogEndpointType::OpenaiResponses,
            is_stream: false,
        };

        // The failure is swallowed: this call returns no Result, and nothing below panics.
        finalize_api_log(
            Some(&store),
            &draft,
            ApiLogOutcome::Completed,
            Some(200),
            Some(crate::ir::IrUsage::default()),
        )
        .await;

        // An unstamped draft never reaches the store at all, store present or not.
        let unstamped = ApiLogDraft {
            started_at: None,
            ..draft.clone()
        };

        finalize_api_log(
            Some(&store),
            &unstamped,
            ApiLogOutcome::Completed,
            Some(200),
            None,
        )
        .await;

        finalize_api_log(None, &unstamped, ApiLogOutcome::Completed, Some(200), None).await;

        std::fs::remove_file(&blocker)?;

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

    /// Verifies the retention sweep (README §3 *ApiLog Shard Store*): creating a shard file
    /// deletes the expired shards — day-end strictly older than now minus the retention — while
    /// never deleting the file just created, leaving unparseable filenames alone; a cached shard
    /// is evicted before its unlink, so a late finalization whose shard was already deleted
    /// recreates the file with its one row; and writing into an existing shard never sweeps.
    ///
    /// # Errors
    ///
    /// Returns an error if a store operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a shard file survives or vanishes contrary to the documented sweep behavior.
    #[tokio::test]
    async fn retention_sweeps_expired_shards_only_when_a_file_is_created()
    -> Result<(), Box<dyn StdError>> {
        let (store, guard) = create_test_store(90);
        let now = unix_time::unix_now_micros();

        let old_day = ShardDay::of_started_at(now - 91 * MICROS_PER_DAY);
        let fresh_day = ShardDay::of_started_at(now);

        // Creating the expired shard sweeps nothing else and never deletes itself.
        store
            .record(write(day_start(old_day) + 100, 1, 1, 1, [None; 5]))
            .await?;
        assert!(store.shard_path(old_day).exists());

        // Unparseable filenames a sweep must leave alone.
        std::fs::write(guard.0.join("garbage.db"), b"junk")?;
        std::fs::write(guard.0.join("2026-02-30.db"), b"junk")?;

        // Creating the fresh shard sweeps the expired one — now strictly past its retention —
        // leaving the just-created file and the unparseable names alone.
        store
            .record(write(day_start(fresh_day) + 100, 1, 1, 1, [None; 5]))
            .await?;
        assert!(
            !store.shard_path(old_day).exists(),
            "the expired shard must be deleted when a new shard file is created"
        );
        assert!(store.shard_path(fresh_day).exists());
        assert!(guard.0.join("garbage.db").exists());
        assert!(guard.0.join("2026-02-30.db").exists());

        // A late finalization whose shard was already retention-deleted recreates the file with
        // that one row — deterministic attribution, accepted.
        store
            .record(write(day_start(old_day) + 200, 1, 1, 1, [None; 5]))
            .await?;

        let rows = read_all(&open_shard(&store, old_day).await).await?;
        assert_eq!(
            rows.len(),
            1,
            "the recreated shard carries exactly the late row"
        );

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        let row = rows.first().unwrap();
        assert_eq!(column(row, 0), &Value::Integer(day_start(old_day) + 200));

        // Writing into an existing, cached shard never sweeps: an expired shard planted now
        // survives the next write, which hits the hot cache.
        let planted_day = ShardDay::of_started_at(now - 92 * MICROS_PER_DAY);
        open_shard(&store, planted_day).await;
        assert!(store.shard_path(planted_day).exists());

        store
            .record(write(day_start(fresh_day) + 200, 1, 1, 1, [None; 5]))
            .await?;
        assert!(
            store.shard_path(planted_day).exists(),
            "a write into an existing shard must not run the sweep"
        );

        Ok(())
    }
}
