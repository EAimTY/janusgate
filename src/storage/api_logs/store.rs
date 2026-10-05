//! The api-log store (README §3 *ApiLog Store*): the day-partitioned api-log files under its
//! directory, the hot-file write cache over them, the retention sweep, and the INSERT write
//! path. The management read engine over the same files lives in [`super::usage`].

use super::ApiLogWrite;
use crate::{
    database::{Database, params, schema::Registry},
    storage::Error,
    unix_time,
};
use log::warn;
use std::{future::Future, num::NonZero, path::PathBuf, sync::Arc, time::Duration};

/// Microseconds in one UTC day: the width of one api-log file's partition.
pub(super) const MICROS_PER_DAY: i64 = 86_400_000_000;

/// How many api-log files the hot cache holds: today + yesterday + margin for stragglers
/// finalized after midnight. A performance hint, never a correctness bound — a miss beyond it
/// lazily opens the existing file.
const HOT_FILE_CAPACITY: usize = 3;

/// Wait for a free api-log file pool connection, matching the main database's connection
/// configuration.
pub(super) const FILE_POOL_WAIT: Duration = Duration::from_secs(5);

/// The fixed api-log file connection pool size for writes; a performance choice, not
/// configuration.
///
/// # Panics
///
/// Never: the literal 2 is nonzero, and the `expect` states that invariant.
pub(super) fn write_pool_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 2 is nonzero")]
    let connections = NonZero::new(2).expect("2 is nonzero");

    connections
}

/// The fixed api-log file connection pool size for reads: one read query runs per file at a
/// time; a performance choice, not configuration.
///
/// # Panics
///
/// Never: the literal 1 is nonzero, and the `expect` states that invariant.
fn read_pool_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 1 is nonzero")]
    let connections = NonZero::new(1).expect("1 is nonzero");

    connections
}

/// One UTC calendar day: the key of the day's api-log file, derived purely from a row's
/// `started_at`. There is no manifest or registry anywhere — the store directory's listing is
/// the only truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ApiLogDay(i64);

impl ApiLogDay {
    /// Derives the day of a `started_at` stamp: the stamp's UTC calendar day.
    pub(super) fn of_started_at(started_at: i64) -> Self {
        Self(started_at.div_euclid(MICROS_PER_DAY))
    }

    /// The first microsecond after the day. Wide in `i128` because the day after the largest
    /// representable stamp's day may end past `i64::MAX`.
    fn end_micros(self) -> i128 {
        i128::from(self.0 + 1) * i128::from(MICROS_PER_DAY)
    }

    /// Renders the api-log file's stem: `YYYY-MM-DD`, zero-padded, sortable.
    pub(super) fn file_stem(self) -> String {
        let (year, month, day) = self.civil();

        format!("{year:04}-{month:02}-{day:02}")
    }

    /// Parses an api-log file name back into its day: exactly `YYYY-MM-DD.db` naming a real
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

    /// The day number of a proleptic Gregorian calendar date — the inverse of [`ApiLogDay::civil`].
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

/// The api-log store (README §3 *ApiLog Store*): the day-partitioned api-log files under its
/// directory, the hot-file write cache over them, and the retention policy. Cloned handles
/// share one directory, one retention, and one cache. The read-side methods live in
/// [`super::usage`].
#[derive(Clone, Debug)]
pub(crate) struct ApiLogStore {
    /// State shared by every clone of the store.
    inner: Arc<StoreInner>,
}

/// The shared state of an [`ApiLogStore`].
#[derive(Debug)]
struct StoreInner {
    /// Directory holding the api-log files; consumed once at startup from the configuration.
    directory: PathBuf,

    /// Retention in whole days: the sweep deletes api-log files whose day-end is strictly older
    /// than now minus this.
    retention_days: NonZero<u32>,

    /// The hot-file cache, most recently written day first and at most
    /// [`HOT_FILE_CAPACITY`] entries. The lock is asynchronous and covers the whole
    /// get-or-open decision, so two racing writers for one day can never both apply the
    /// day's DDL.
    hot: tokio::sync::Mutex<Vec<(ApiLogDay, Database)>>,
}

impl ApiLogStore {
    /// Builds the store over `directory` with `retention_days`. Nothing is touched on the
    /// filesystem: the directory is created lazily when the first api-log file is created.
    pub(crate) fn new(directory: PathBuf, retention_days: NonZero<u32>) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                directory,
                retention_days,
                hot: tokio::sync::Mutex::new(Vec::new()),
            }),
        }
    }

    /// The path of one day's api-log file.
    pub(super) fn day_file_path(&self, day: ApiLogDay) -> PathBuf {
        self.inner.directory.join(format!("{}.db", day.file_stem()))
    }

    /// Inserts one accounting row into its day's file, deriving the day from the row's own
    /// `started_at` stamp — never from the finalization clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the day's file cannot be opened or the insert fails — a drifted
    /// file, one whose stored schema version or application-id stamp the api_logs registry
    /// cannot interpret, classifies as a recording failure like any other open failure. The
    /// caller logs and swallows every failure: recording never enters any HTTP error domain.
    pub(super) async fn record(&self, write: ApiLogWrite) -> Result<(), Error> {
        let database = self
            .database_for_write(ApiLogDay::of_started_at(write.started_at))
            .await?;

        insert_api_log(&database, write).await
    }

    /// Returns a pooled connection to `day`'s file, consulting the hot cache and, on a miss,
    /// lazily opening the file — creating it, and with it running the retention sweep, only
    /// when no file is present yet.
    ///
    /// The asynchronous lock is held across the whole decision, so concurrent writers
    /// serialize on cache misses only; the returned handle is pooled and used outside the
    /// lock. Every blocking filesystem step of a miss — the existence probe and the file
    /// open inside it, and the retention sweep — runs on the blocking thread pool while the
    /// lock stays held, so no syscall of this decision ever blocks a worker thread and the
    /// get-or-open serialization is unchanged.
    async fn database_for_write(&self, day: ApiLogDay) -> Result<Database, Error> {
        let mut hot = self.inner.hot.lock().await;

        if let Some(position) = hot.iter().position(|(cached, _)| *cached == day) {
            // Restamp the entry as the most recently written day.
            let (_, database) = hot.remove(position);
            hot.insert(0, (day, database.clone()));

            return Ok(database);
        }

        let (database, is_new) = self.open_day_file_for_write(day).await?;

        if is_new {
            // Creating a new file is the sweep's only trigger; opening an existing file for
            // a late write never sweeps. The sweep's directory listing and unlinking are
            // blocking syscalls, so they run on the blocking thread pool while this lock
            // stays held, keeping the eviction-before-unlink ordering atomic with respect to
            // concurrent writers. The guarded cache is handed over owned (`std::mem::take`)
            // and restored after: while taken, the lock hides the temporarily empty vector
            // from every other writer.
            let swept = {
                let cache = std::mem::take(&mut *hot);
                let directory = self.inner.directory.clone();
                let retention_days = self.inner.retention_days;

                tokio::task::spawn_blocking(move || {
                    Self::sweep(directory, retention_days, cache, day)
                })
                .await
            };

            match swept {
                Ok(swept) => *hot = swept,

                // A panic in the sweep is a bug: resume it on this thread, where it fails
                // fast instead of being masked as tolerated maintenance damage.
                Err(join_error) if join_error.is_panic() => {
                    std::panic::resume_unwind(join_error.into_panic());
                }

                // The task was cancelled before it ran — only possible while the runtime is
                // shutting down — and the taken cache died with it. An empty cache is
                // correct: it is a performance hint, and misses lazily reopen their files.
                Err(join_error) => {
                    warn!("the api log retention sweep task never ran: {join_error}");
                }
            }
        }

        hot.insert(0, (day, database.clone()));
        hot.truncate(HOT_FILE_CAPACITY);

        Ok(database)
    }

    /// Opens one day's api-log file for the write path, creating the file and its parent
    /// directory when absent and answering whether the file is newly created — the retention
    /// sweep's only trigger (README §3 *ApiLog Store*). The open runs off the worker threads;
    /// a drifted file, one the api_logs registry cannot interpret, answers [`Error::Drift`],
    /// which the write path swallows as a best-effort recording failure like any other open
    /// failure (README §8).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the open fails; a file this build cannot interpret under the
    /// api_logs registry answers [`Error::Drift`].
    async fn open_day_file_for_write(&self, day: ApiLogDay) -> Result<(Database, bool), Error> {
        let path = self.day_file_path(day);

        Self::open_day_file_off_thread(async move {
            // The existence probe sits directly before the open — both polled on the blocking
            // thread, the open creating the file and its parent directory — so a file that
            // appears or vanishes between the probe and the open keeps exactly the documented
            // edge behavior of the probe's answer.
            let is_new = !path.exists();

            let database = Database::open(
                path,
                Registry::ApiLogs,
                write_pool_connections(),
                FILE_POOL_WAIT,
            )
            .await?;

            Ok((database, is_new))
        })
        .await
    }

    /// Opens one day's api-log file for the read paths, refusing to create: the open is
    /// [`Database::open_existing`], so a file absent at the open — one the retention sweep
    /// deleted between the directory listing and this open — reads as the empty partition it
    /// became (README §3 *ApiLog Store*), and a read never recreates an api-log file or its
    /// directory. The open runs off the worker threads like the write path's; a drifted file
    /// answers [`Error::Drift`] — the explicit failure this read fails its request with,
    /// never a process-killing panic. Reads hold no store lock: only the writers serialize.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the open fails; a file this build cannot interpret under the
    /// api_logs registry answers [`Error::Drift`].
    pub(super) async fn open_day_file_for_read(
        &self,
        day: ApiLogDay,
    ) -> Result<Option<Database>, Error> {
        let path = self.day_file_path(day);

        Self::open_day_file_off_thread(async move {
            Database::open_existing(
                path,
                Registry::ApiLogs,
                read_pool_connections(),
                FILE_POOL_WAIT,
            )
            .await
        })
        .await
    }

    /// Runs one api-log file open off the worker threads: the caller's open future is polled
    /// inside a blocking task via [`tokio::runtime::Handle::block_on`], so every filesystem
    /// effect of the open — the engine open, its schema bootstrap, and an existence probe
    /// composed ahead of either — happens off the async executor, in the future's own order.
    /// The open's failures are typed: a drifted file, one the api_logs registry cannot
    /// interpret, converts to [`Error::Drift`] — the write path swallows it as a recording
    /// failure (README §8) and a read fails its request with it, a corrupt file never killing
    /// the process (README §3 *ApiLog Store*).
    ///
    /// # Panics
    ///
    /// A panic inside the open task resumes on the caller's thread: a panic there is a bug —
    /// file drift is a typed error, never a panic — and fails fast instead of being masked as
    /// tolerated damage, the same policy the retention sweep and the retained-days walk hold.
    async fn open_day_file_off_thread<T, F>(open: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, crate::database::Error>> + Send + 'static,
    {
        let runtime = tokio::runtime::Handle::current();

        match tokio::task::spawn_blocking(move || runtime.block_on(open)).await {
            Ok(outcome) => outcome.map_err(Error::from),

            // A panic in the open is a bug: resume it on this thread, where it fails fast
            // instead of being masked as tolerated maintenance damage.
            Err(join_error) if join_error.is_panic() => {
                std::panic::resume_unwind(join_error.into_panic());
            }

            #[expect(
                clippy::unreachable,
                reason = "the open task is never aborted, and a panic inside it is resumed in \
                          the arm above; only runtime shutdown can cancel the task"
            )]
            Err(join_error) => {
                unreachable!("the api-log open blocking task cannot be cancelled: {join_error}")
            }
        }
    }

    /// Deletes the expired api-log files: every file whose day-end is strictly older than now
    /// minus the retention, except the file just created. A cached file is evicted before it is
    /// unlinked, so the pool never keeps writing into a deleted file. Unparseable filenames are
    /// left alone, and every failure here is logged and swallowed — retention is a
    /// maintenance sweep, never an error surface.
    ///
    /// Runs on the blocking thread pool, called with the store lock held by the caller: the
    /// guarded cache is handed over owned and returned, so the eviction-before-unlink ordering
    /// stays exclusive to the caller's lock.
    fn sweep(
        directory: PathBuf,
        retention_days: NonZero<u32>,
        mut cache: Vec<(ApiLogDay, Database)>,
        just_created: ApiLogDay,
    ) -> Vec<(ApiLogDay, Database)> {
        let cutoff = i128::from(unix_time::unix_now_micros())
            - i128::from(retention_days.get()) * i128::from(MICROS_PER_DAY);

        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,

            Err(failure) => {
                warn!(
                    "failed to list the api log file directory {}: {failure}",
                    directory.display()
                );

                return cache;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,

                Err(failure) => {
                    warn!("failed to read an api log file directory entry: {failure}");

                    continue;
                }
            };

            let Some(day) = entry
                .file_name()
                .to_str()
                .and_then(ApiLogDay::parse_file_stem_of_file_name)
            else {
                continue;
            };

            if day == just_created || day.end_micros() >= cutoff {
                continue;
            }

            cache.retain(|(cached, _)| *cached != day);

            if let Err(failure) = std::fs::remove_file(entry.path()) {
                warn!(
                    "failed to delete the expired api log file {}: {failure}",
                    entry.path().display()
                );

                continue;
            }

            // A stale `-wal`/`-shm` sibling — from a crash mid-write, say — must not outlive
            // its api-log file.
            for suffix in ["-wal", "-shm"] {
                let mut sibling = entry.file_name().to_os_string();
                sibling.push(suffix);
                let sibling = directory.join(sibling);
                let _ = std::fs::remove_file(sibling);
            }
        }

        cache
    }

    /// Every day the directory currently holds a file for, ascending. An absent directory reads
    /// as empty; an unreadable one fails the read explicitly, as does an entry the listing fails
    /// to deliver mid-iteration — a day must never read as absent. The walk runs on the
    /// blocking thread pool, like the sweep: the directory listing is a syscall that must never
    /// block a worker thread.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the directory exists but cannot be listed, and when a directory
    /// entry fails to be delivered mid-iteration.
    pub(super) async fn retained_days(&self) -> Result<Vec<ApiLogDay>, Error> {
        let directory = self.inner.directory.clone();

        match tokio::task::spawn_blocking(move || Self::retained_days_blocking(directory)).await {
            Ok(days) => days,

            // A panic in the walk is a bug: resume it on this thread, where it fails fast
            // instead of being masked as tolerated maintenance damage.
            Err(join_error) if join_error.is_panic() => {
                std::panic::resume_unwind(join_error.into_panic())
            }

            #[expect(
                clippy::unreachable,
                reason = "the retained-days walk task is never aborted, and a panic inside it \
                          is resumed in the arm above; only runtime shutdown can cancel the task"
            )]
            Err(join_error) => {
                unreachable!("the retained-days walk task cannot be cancelled: {join_error}")
            }
        }
    }

    /// The blocking body of [`ApiLogStore::retained_days`]: lists the directory and parses
    /// every api-log filename, ascending. Unparseable file names — non-UTF8 ones included — stay
    /// skipped (README §3 *ApiLog Store*), while an entry the iteration fails to deliver
    /// fails the listing: see [`Self::day_of_listing_entry`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the directory exists but cannot be listed, and when a directory
    /// entry fails to be delivered mid-iteration.
    fn retained_days_blocking(directory: PathBuf) -> Result<Vec<ApiLogDay>, Error> {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,

            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }

            Err(failure) => return Err(crate::database::Error::Io(failure).into()),
        };

        let mut days = Vec::new();

        for entry in entries {
            if let Some(day) = Self::day_of_listing_entry(entry)? {
                days.push(day);
            }
        }

        days.sort();

        Ok(days)
    }

    /// Classifies one entry of the store directory's listing: [`Some`] with the entry's
    /// [`ApiLogDay`] when its file name parses as an api-log file's, [`None`] for the names
    /// that are not api-log files' — unparseable names and non-UTF8 names alike, which the
    /// listing leaves alone (README §3 *ApiLog Store*). An entry the iteration itself failed
    /// to deliver — a transient failure on network storage, a permission blip — is an
    /// [`Error`]: the entry's day must not read as absent, so the failure propagates into the
    /// read that consumes the listing, the same explicit failure a corrupt file fails its read
    /// with.
    fn day_of_listing_entry(
        entry: std::io::Result<std::fs::DirEntry>,
    ) -> Result<Option<ApiLogDay>, Error> {
        let entry = entry.map_err(crate::database::Error::Io)?;

        let Some(name) = entry.file_name().into_string().ok() else {
            return Ok(None);
        };

        Ok(ApiLogDay::parse_file_stem_of_file_name(&name))
    }
}

/// Inserts one accounting row into the api-log database behind `database` — a single synchronous
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
             output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens, \
             model_route_id, upstream_http_status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                write.model_route_id,
                write.upstream_http_status,
            ],
        )
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        super::{ApiLogDraft, finalize_api_log, test_support},
        *,
    };
    use crate::{
        accounting::{ApiLogEndpointType, ApiLogOutcome},
        database::Value,
        storage::{
            self,
            api_keys::{self, ApiKeyFields},
            models::ModelCreate,
        },
        test_support::create_test_database,
    };
    use serde::Serialize;
    use std::{error::Error as StdError, num::NonZero};

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

    /// Reads every column of every stored row of one api-log file, in rowid order, as raw
    /// values the test asserts positionally.
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
    /// so a fixture can stay inside the retention window the sweep enforces on every new api-log
    /// file.
    fn full_write(started_at: i64) -> ApiLogWrite {
        ApiLogWrite {
            started_at,
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            model_route_id: 7,
            upstream_http_status: Some(200),
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

    /// The first microsecond of a day: the inverse of [`ApiLogDay::of_started_at`], for
    /// placing fixture stamps inside a chosen day.
    fn day_start(day: ApiLogDay) -> i64 {
        // `of_started_at` divides by `MICROS_PER_DAY`, so the product can never overflow.
        day.0 * MICROS_PER_DAY
    }

    /// Verifies the day mapping end to end: a stamp maps to its UTC calendar day, the day
    /// boundaries round-trip, the file stem renders `YYYY-MM-DD` and parses back — leap years
    /// included — and every malformed file name is unparseable, so a sweep leaves such files
    /// alone.
    ///
    /// # Panics
    ///
    /// Panics if a mapping or parse diverges from the documented behavior.
    #[test]
    fn days_map_to_file_names_and_back() {
        // Day boundaries: the day of a stamp is the stamp's own calendar day, and the first
        // microsecond of the next day already belongs to the next day's file.
        let day = ApiLogDay::of_started_at(1_700_000_000_000_000);
        let start = day_start(day);

        assert_eq!(ApiLogDay::of_started_at(start), day);
        assert_eq!(
            ApiLogDay::of_started_at(start + MICROS_PER_DAY - 1),
            day,
            "the day's last microsecond stays in the day",
        );
        assert_ne!(
            ApiLogDay::of_started_at(start + MICROS_PER_DAY),
            day,
            "the next day's first microsecond moves on",
        );

        // The civil conversion renders sortable ISO days, before the epoch included.
        assert_eq!(ApiLogDay::of_started_at(0).file_stem(), "1970-01-01");
        assert_eq!(day.file_stem(), "2023-11-14");
        assert_eq!(
            ApiLogDay::of_started_at(-1).file_stem(),
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
            let parsed = ApiLogDay::parse_file_stem_of_file_name(name).unwrap();

            assert_eq!(parsed, ApiLogDay::of_started_at(stamp), "{name}");
        }

        #[expect(clippy::unwrap_used, reason = "2024 is a leap year")]
        let leap_day = ApiLogDay::parse_file_stem_of_file_name("2024-02-29.db").unwrap();
        assert_eq!(leap_day.file_stem(), "2024-02-29");

        // Every malformed name is unparseable: wrong shape, impossible dates, non-leap February
        // 29ths, and foreign extensions.
        for name in [
            "not-an-api-log.db",
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
                ApiLogDay::parse_file_stem_of_file_name(name),
                None,
                "{name} must not parse",
            );
        }
    }

    /// Verifies the write path column by column: a fully populated row round-trips exactly as
    /// written — the closed-vocabulary columns stored through `as_str`, the stream flag as
    /// `0`/`1` — a row whose seven nullable columns are all `NULL` reads back as `NULL` rather
    /// than a default, and each row lands in its own stamp's day file. The `as_str` spellings
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
        let (store, _guard) = test_support::create_test_store(90);

        // Yesterday and now: two recent, distinct days, safely inside the retention window.
        let full_stamp = unix_time::unix_now_micros() - MICROS_PER_DAY;
        store.record(full_write(full_stamp)).await?;

        let sparse_stamp = unix_time::unix_now_micros();
        let sparse = ApiLogWrite {
            started_at: sparse_stamp,
            api_key_id: 1,
            provider_id: 1,
            model_id: 1,
            model_route_id: 1,
            upstream_http_status: Some(200),
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

        let full_day_file =
            test_support::open_day_file(&store, ApiLogDay::of_started_at(full_stamp)).await;
        let rows = read_all(&full_day_file).await?;
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

        let today_day_file =
            test_support::open_day_file(&store, ApiLogDay::of_started_at(sparse_stamp)).await;
        let rows = read_all(&today_day_file).await?;
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

    /// Verifies that a schema `CHECK` rejection surfacing through a file's write path
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
        let (store, _guard) = test_support::create_test_store(90);
        let database = test_support::open_day_file(&store, ApiLogDay::of_started_at(1)).await;

        let base = "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
                    model_route_id, endpoint_type, is_stream, outcome, duration) \
                    VALUES (1, 1, 1, 1, 1, ";

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
    /// delete paths, the provider's cascade included — leaves the log row byte-for-byte intact
    /// in its day's file, still attributing by the never-recycled ids it stores.
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
        let (store, _store_guard) = test_support::create_test_store(90);

        let api_key = api_keys::insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service",
                key: "sk-live",
                is_enabled: true,
                access_control: &crate::test_support::allow_all_access(),
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
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support_override: None,
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
                model_route_id: model.model.id,
                upstream_http_status: None,
                progress: crate::storage::api_logs::AttemptProgress::default(),
                endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
                is_stream: false,
                billed_usage: None,
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

        let rows =
            read_all(&test_support::open_day_file(&store, ApiLogDay::of_started_at(1_000)).await)
                .await?;

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

    /// Verifies the best-effort accounting boundary (README §8): finalizing against a store
    /// whose file directory cannot be created — the configured path sits beneath a regular
    /// file — neither panics nor surfaces anywhere, and the main database keeps serving reads
    /// and writes of other tables afterwards. The unstamped-draft path — the switch-off shape —
    /// performs no INSERT at all, with or without a store.
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

        // The store's directory path is doomed: its parent is a regular file, so no api-log
        // file can ever be created beneath it.
        let blocker =
            std::env::temp_dir().join(format!("janusgate-apilogs-block-{}", rand::random::<u64>()));

        std::fs::write(&blocker, b"not a directory")?;

        #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
        let retention = NonZero::new(90).unwrap();
        let store = ApiLogStore::new(blocker.join("api-logs"), retention);

        let draft = ApiLogDraft {
            started_at: Some(1_000),
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            model_route_id: 7,
            upstream_http_status: None,
            progress: crate::storage::api_logs::AttemptProgress::default(),
            endpoint_type: ApiLogEndpointType::OpenaiResponses,
            is_stream: false,
            billed_usage: None,
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
            &ApiKeyFields {
                name: "still-works",
                key: "sk-after",
                is_enabled: true,
                access_control: &crate::test_support::allow_all_access(),
            },
        )
        .await?;

        assert_eq!(
            api_keys::find_api_key_by_id(&database, api_key.id).await?,
            Some(api_key)
        );

        Ok(())
    }

    /// Verifies the best-effort accounting boundary against a damaged api-log file (README §3
    /// *ApiLog Store*): a file whose open answers typed schema drift — here a file stamped
    /// for the main registry, which the api-log open's stamp verification rejects —
    /// classifies as a recording failure. `record` answers `Err`, the finalization swallows
    /// it without surfacing anywhere, and a write into a healthy file still succeeds
    /// afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error if a database operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if the damaged file's classification diverges from drift or a later operation
    /// fails.
    #[tokio::test]
    async fn swallows_a_drifted_day_file_open() -> Result<(), Box<dyn StdError>> {
        let (store, _guard) = test_support::create_test_store(90);
        let damaged_day = ApiLogDay::of_started_at(unix_time::unix_now_micros());

        // Plant a main-registry database at the day's file path and drop its pool: the
        // store's api_logs-registry open of this file then hits the stamp verification,
        // which answers typed schema drift (README §3 *Database Abstraction & Connection
        // Pooling*).
        drop(
            Database::open(
                store.day_file_path(damaged_day),
                Registry::Main,
                write_pool_connections(),
                FILE_POOL_WAIT,
            )
            .await?,
        );

        // The write path reports the drifted open as an ordinary recording failure.
        let outcome = store
            .record(test_support::write(
                day_start(damaged_day) + 100,
                1,
                1,
                1,
                [None; 5],
            ))
            .await;

        assert!(
            matches!(outcome, Err(Error::Drift { .. })),
            "a drifted api-log open must classify as a recording failure"
        );

        // The finalization boundary swallows it: this call returns no Result and must not panic.
        finalize_api_log(
            Some(&store),
            &ApiLogDraft {
                started_at: Some(day_start(damaged_day) + 100),
                api_key_id: 3,
                provider_id: 5,
                model_id: 7,
                model_route_id: 7,
                upstream_http_status: None,
                progress: crate::storage::api_logs::AttemptProgress::default(),
                endpoint_type: ApiLogEndpointType::OpenaiResponses,
                is_stream: false,
                billed_usage: None,
            },
            ApiLogOutcome::Completed,
            Some(200),
            Some(crate::ir::IrUsage::default()),
        )
        .await;

        // One damaged file must not poison the store: a healthy day's file still records.
        let healthy_day = ApiLogDay::of_started_at(unix_time::unix_now_micros() - MICROS_PER_DAY);
        store
            .record(test_support::write(
                day_start(healthy_day) + 100,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;

        let rows = read_all(&test_support::open_day_file(&store, healthy_day).await).await?;
        assert_eq!(rows.len(), 1, "the healthy file keeps recording");

        Ok(())
    }

    /// Verifies the retention sweep (README §3 *ApiLog Store*): creating an api-log file
    /// deletes the expired files — day-end strictly older than now minus the retention — while
    /// never deleting the file just created, leaving unparseable filenames alone; a cached
    /// file is evicted before its unlink, so a late finalization whose file was already
    /// deleted recreates the file with its one row; and writing into an existing file never
    /// sweeps.
    ///
    /// # Errors
    ///
    /// Returns an error if a store operation or assertion fails.
    ///
    /// # Panics
    ///
    /// Panics if a file survives or vanishes contrary to the documented sweep behavior.
    #[tokio::test]
    async fn retention_sweeps_expired_files_only_when_a_file_is_created()
    -> Result<(), Box<dyn StdError>> {
        let (store, guard) = test_support::create_test_store(90);
        let now = unix_time::unix_now_micros();

        let old_day = ApiLogDay::of_started_at(now - 91 * MICROS_PER_DAY);
        let fresh_day = ApiLogDay::of_started_at(now);

        // Creating the expired day's file sweeps nothing else and never deletes itself.
        store
            .record(test_support::write(
                day_start(old_day) + 100,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;
        assert!(store.day_file_path(old_day).exists());

        // Unparseable filenames a sweep must leave alone.
        std::fs::write(guard.0.join("garbage.db"), b"junk")?;
        std::fs::write(guard.0.join("2026-02-30.db"), b"junk")?;

        // Creating the fresh day's file sweeps the expired one — now strictly past its
        // retention — leaving the just-created file and the unparseable names alone.
        store
            .record(test_support::write(
                day_start(fresh_day) + 100,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;
        assert!(
            !store.day_file_path(old_day).exists(),
            "the expired api-log file must be deleted when a new file is created"
        );
        assert!(store.day_file_path(fresh_day).exists());
        assert!(guard.0.join("garbage.db").exists());
        assert!(guard.0.join("2026-02-30.db").exists());

        // A late finalization whose file was already retention-deleted recreates the file
        // with that one row — deterministic attribution, accepted.
        store
            .record(test_support::write(
                day_start(old_day) + 200,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;

        let rows = read_all(&test_support::open_day_file(&store, old_day).await).await?;
        assert_eq!(
            rows.len(),
            1,
            "the recreated file carries exactly the late row"
        );

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        let row = rows.first().unwrap();
        assert_eq!(column(row, 0), &Value::Integer(day_start(old_day) + 200));

        // Writing into an existing, cached file never sweeps: an expired day's file planted
        // now survives the next write, which hits the hot cache.
        let planted_day = ApiLogDay::of_started_at(now - 92 * MICROS_PER_DAY);
        test_support::open_day_file(&store, planted_day).await;
        assert!(store.day_file_path(planted_day).exists());

        store
            .record(test_support::write(
                day_start(fresh_day) + 200,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;
        assert!(
            store.day_file_path(planted_day).exists(),
            "a write into an existing file must not run the sweep"
        );

        Ok(())
    }

    /// Verifies the store directory listing's per-entry classification (README §3 *ApiLog
    /// Store*): an api-log-named entry yields its day, an unparseable name stays skipped, and
    /// an entry the iteration fails to deliver is an explicit database-I/O storage error — the
    /// same class a corrupt file fails its read with — instead of reading its day as absent.
    /// A mid-iteration delivery failure is unreachable through a real filesystem without races,
    /// so the test drives the per-entry classification seam directly; the listing over real
    /// directories stays covered by the usage-read suite. The non-UTF8 skip is not
    /// fixture-able portably — APFS makes the whole iteration fail on such a name while Linux
    /// delivers it — so it stays with the code path it shares with the unparseable name.
    ///
    /// # Errors
    ///
    /// Returns an error if the fixture directory cannot be prepared.
    ///
    /// # Panics
    ///
    /// Panics if a classified entry diverges from the documented semantics.
    #[test]
    fn classifies_listing_entries_and_fails_an_undelivered_entry() -> Result<(), Box<dyn StdError>>
    {
        let directory = std::env::temp_dir().join(format!(
            "janusgate-apilogs-listing-{}",
            rand::random::<u64>()
        ));
        let _guard = crate::test_support::TempDirGuard(directory.clone());

        std::fs::create_dir(&directory)?;
        std::fs::write(directory.join("2026-01-02.db"), b"junk")?;
        std::fs::write(directory.join("garbage.db"), b"junk")?;

        let mut classified = std::fs::read_dir(&directory)?
            .map(ApiLogStore::day_of_listing_entry)
            .collect::<Result<Vec<_>, _>>()?;

        classified.sort_by_key(|day| day.map(|day| day.0));

        #[expect(
            clippy::unwrap_used,
            reason = "the planted file name is constructed to parse"
        )]
        let expected = ApiLogDay::parse_file_stem_of_file_name("2026-01-02.db").unwrap();

        // The api-log-named entry yields its day; every other name stays skipped.
        assert_eq!(classified.last(), Some(&Some(expected)));

        assert!(
            classified.iter().rev().skip(1).all(|day| day.is_none()),
            "only the api-log-named entry may classify as a day",
        );

        // An entry the iteration fails to deliver fails the listing explicitly: the day it
        // carries must not read as absent.
        let undelivered = ApiLogStore::day_of_listing_entry(Err(std::io::Error::other(
            "transient directory iteration failure",
        )));

        assert!(
            matches!(
                undelivered,
                Err(Error::Database(crate::database::Error::Io(_)))
            ),
            "an undelivered entry must be an explicit storage error",
        );

        Ok(())
    }
}
