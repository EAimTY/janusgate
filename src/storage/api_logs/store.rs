//! The api-log store (`docs/design/storage.md` *ApiLog Store*): the day-partitioned api-log files
//! under its directory, the hot-file write cache over them, the retention sweep, and the INSERT
//! write path. The management read engine over the same files lives in [`super::usage`].

use super::ApiLog;
use crate::{
    database::{Database, params, schema::Registry},
    storage::Error,
    unix_time,
};
use log::warn;
use std::{future::Future, num::NonZero, path::PathBuf, sync::Arc, time::Duration};
use time::{Date, Month};

/// Microseconds in one UTC day: the width of one api-log file's partition.
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// The Julian Day Number of the Unix epoch, 1970-01-01: the fixed offset between [`Date`]'s
/// Julian-day mapping and Unix epoch days.
const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;

/// How many api-log files the hot cache holds: today + yesterday + margin for stragglers
/// finalized after midnight. A performance hint, never a correctness bound — a miss beyond it
/// lazily opens the existing file.
const HOT_FILE_CAPACITY: usize = 3;

/// Wait for a free api-log file pool connection, matching the main database's fixed pool wait.
const FILE_POOL_WAIT: Duration = Duration::from_secs(5);

/// The fixed api-log file connection pool size for writes; a performance choice, not
/// configuration.
///
/// # Panics
///
/// Never: the literal 2 is nonzero, and the `expect` states that invariant.
fn write_pool_connections() -> NonZero<usize> {
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
        let (year, month, day) = self.date().to_calendar_date();

        format!("{year:04}-{:02}-{day:02}", u8::from(month))
    }

    /// Parses an api-log file name back into its day: exactly `YYYY-MM-DD.db` naming a real
    /// calendar day. Anything else — a malformed stem or an impossible date such as
    /// `2026-02-30` — is unparseable, and the caller leaves such files alone.
    fn parse_file_stem_of_file_name(file_name: &str) -> Option<Self> {
        let stem = file_name.strip_suffix(".db")?;
        let (year, rest) = stem.split_once('-')?;
        let (month, day) = rest.split_once('-')?;

        let is_digits =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());

        // The stem's shape is parsed by hand — the calendar library would accept the unpadded
        // and over-long spellings the file-name contract rejects — while the calendar date
        // itself is validated and mapped by the library, so no impossible date parses.
        if !is_digits(year)
            || year.len() > 6
            || !is_digits(month)
            || month.len() != 2
            || !is_digits(day)
            || day.len() != 2
        {
            return None;
        }

        let date = Date::from_calendar_date(
            year.parse().ok()?,
            Month::try_from(month.parse::<u8>().ok()?).ok()?,
            day.parse().ok()?,
        )
        .ok()?;

        Some(Self::of_date(date))
    }

    /// The day's calendar date in the proleptic Gregorian calendar.
    ///
    /// # Panics
    ///
    /// Panics when the day does not name a supported calendar date. A day is derived only from
    /// the dispatch clock's microsecond stamps and from parsed file stems, both far inside the
    /// supported range, so an unrepresentable day is a bug rather than a tolerated input.
    fn date(self) -> Date {
        #[expect(
            clippy::expect_used,
            reason = "an api-log day derives only from the dispatch clock's stamps and from \
                      parsed file stems, both far inside the supported calendar"
        )]
        let date = self
            .0
            .checked_add(i64::from(UNIX_EPOCH_JULIAN_DAY))
            .and_then(|julian_day| i32::try_from(julian_day).ok())
            .and_then(|julian_day| Date::from_julian_day(julian_day).ok())
            .expect("an api-log day names a supported calendar date");

        date
    }

    /// The day of a calendar date: the inverse of [`ApiLogDay::date`]'s mapping.
    fn of_date(date: Date) -> Self {
        Self(i64::from(date.to_julian_day()) - i64::from(UNIX_EPOCH_JULIAN_DAY))
    }
}

/// The api-log store (`docs/design/storage.md` *ApiLog Store*): the day-partitioned api-log files
/// under its directory, the hot-file write cache over them, and the retention policy. Cloned
/// handles share one directory, one retention, and one cache. The read-side methods live in
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
    fn day_file_path(&self, day: ApiLogDay) -> PathBuf {
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
    pub(super) async fn record(&self, row: ApiLog) -> Result<(), Error> {
        let database = self
            .database_for_write(ApiLogDay::of_started_at(row.started_at))
            .await?;

        insert_api_log(&database, row).await
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
    /// sweep's only trigger (`docs/design/storage.md` *ApiLog Store*). The open runs off the worker
    /// threads; a drifted file, one the api_logs registry cannot interpret, answers
    /// [`Error::Drift`], which the write path swallows as a best-effort recording failure like
    /// any other open failure (`docs/reference/configuration.md`).
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
    /// became (`docs/design/storage.md` *ApiLog Store*), and a read never recreates an api-log file
    /// or its directory. The open runs off the worker threads like the write path's; a drifted
    /// file answers [`Error::Drift`] — the explicit failure this read fails its request with,
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
    /// failure (`docs/reference/configuration.md`) and a read fails its request with it, a corrupt
    /// file never killing the process (`docs/design/storage.md` *ApiLog Store*).
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

            // A stale `-wal`/`-tshm` sibling — from a crash mid-write, say — must not outlive
            // its api-log file. The engine's WAL has no `-shm` file; its cross-process
            // coordination sidecar is `-tshm`, created only under the experimental
            // multiprocess-WAL mode this gateway never enables.
            for suffix in ["-wal", "-tshm"] {
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
    /// skipped (`docs/design/storage.md` *ApiLog Store*), while an entry the iteration fails to
    /// deliver fails the listing: see [`Self::day_of_listing_entry`].
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
    /// listing leaves alone (`docs/design/storage.md` *ApiLog Store*). An entry the iteration
    /// itself failed to deliver — a transient failure on network storage, a permission blip —
    /// is an [`Error`]: the entry's day must not read as absent, so the failure propagates into the
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
async fn insert_api_log(database: &Database, row: ApiLog) -> Result<(), Error> {
    database
        .execute(
            "INSERT INTO api_logs (started_at, api_key_id, provider_id, model_id, \
             endpoint_type, is_stream, http_status, outcome, duration, input_tokens, \
             output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens, \
             model_route_id, upstream_http_status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                row.started_at,
                row.api_key_id,
                row.provider_id,
                row.model_id,
                row.endpoint_type.as_str(),
                i64::from(row.is_stream),
                row.http_status,
                row.outcome.as_str(),
                row.duration,
                row.input_tokens,
                row.output_tokens,
                row.cached_tokens,
                row.cache_write_tokens,
                row.reasoning_tokens,
                row.model_route_id,
                row.upstream_http_status,
            ],
        )
        .await?;

    Ok(())
}
