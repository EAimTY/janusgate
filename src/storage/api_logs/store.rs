//! The api-log shard store (README §3 *ApiLog Shard Store*): the day-partitioned shard files
//! under its directory, the hot-shard write cache over them, the retention sweep, and the INSERT
//! write path. The management read engine over the same shards lives in [`super::usage`].

use super::ApiLogWrite;
use crate::{
    database::{Database, params, schema::Registry},
    storage::Error,
    unix_time,
};
use log::warn;
use std::{num::NonZero, path::PathBuf, sync::Arc, time::Duration};

/// Microseconds in one UTC day: the width of one shard.
pub(super) const MICROS_PER_DAY: i64 = 86_400_000_000;

/// How many shards the hot cache holds: today + yesterday + margin for stragglers finalized
/// after midnight. A performance hint, never a correctness bound — a miss beyond it lazily opens
/// the existing file.
const HOT_SHARD_CAPACITY: usize = 3;

/// Wait for a free shard pool connection, matching the main database's connection configuration.
pub(super) const SHARD_POOL_WAIT: Duration = Duration::from_secs(5);

/// The fixed shard connection pool size for writes; a performance choice, not configuration.
///
/// # Panics
///
/// Never: the literal 2 is nonzero, and the `expect` states that invariant.
pub(super) fn shard_write_pool_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 2 is nonzero")]
    let connections = NonZero::new(2).expect("2 is nonzero");

    connections
}

/// One UTC calendar day: the shard key, derived purely from a row's `started_at`. There is no
/// manifest or registry anywhere — the shard directory's listing is the only truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ShardDay(i64);

impl ShardDay {
    /// Derives the shard of a `started_at` stamp: the stamp's UTC calendar day.
    pub(super) fn of_started_at(started_at: i64) -> Self {
        Self(started_at.div_euclid(MICROS_PER_DAY))
    }

    /// The first microsecond after the day. Wide in `i128` because the day after the largest
    /// representable stamp's day may end past `i64::MAX`.
    fn end_micros(self) -> i128 {
        i128::from(self.0 + 1) * i128::from(MICROS_PER_DAY)
    }

    /// Renders the shard file's stem: `YYYY-MM-DD`, zero-padded, sortable.
    pub(super) fn file_stem(self) -> String {
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
    pub(super) fn shard_path(&self, day: ShardDay) -> PathBuf {
        self.inner.directory.join(format!("{}.db", day.file_stem()))
    }

    /// Inserts one accounting row into its day's shard, deriving the shard from the row's own
    /// `started_at` stamp — never from the finalization clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the shard cannot be opened or the insert fails. The caller logs
    /// and swallows every failure: recording never enters any HTTP error domain.
    pub(super) async fn record(&self, write: ApiLogWrite) -> Result<(), Error> {
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

    /// Every shard day the directory currently holds, ascending. An absent directory reads as
    /// empty; an unreadable one fails the read explicitly.
    pub(super) fn retained_days(&self) -> Result<Vec<ShardDay>, Error> {
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
    use super::{
        super::{ApiLogDraft, finalize_api_log, test_support},
        *,
    };
    use crate::{
        accounting::{ApiLogEndpointType, ApiLogOutcome},
        database::Value,
        storage::{
            self,
            api_keys::{self, NewApiKey},
            models::ModelCreate,
        },
    };
    use serde::Serialize;
    use std::{error::Error as StdError, num::NonZero, path::PathBuf};

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

    /// File guard cleaning up the temporary main test database on drop.
    struct TempDbGuard(PathBuf);

    impl Drop for TempDbGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
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
    fn full_write(started_at: i64) -> ApiLogWrite {
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

    /// The first microsecond of a shard day: the inverse of [`ShardDay::of_started_at`], for
    /// placing fixture stamps inside a chosen day.
    fn day_start(day: ShardDay) -> i64 {
        // `of_started_at` divides by `MICROS_PER_DAY`, so the product can never overflow.
        day.0 * MICROS_PER_DAY
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

        let full_shard =
            test_support::open_shard(&store, ShardDay::of_started_at(full_stamp)).await;
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

        let today_shard =
            test_support::open_shard(&store, ShardDay::of_started_at(sparse_stamp)).await;
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
        let (store, _guard) = test_support::create_test_store(90);
        let database = test_support::open_shard(&store, ShardDay::of_started_at(1)).await;

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
        let (store, _store_guard) = test_support::create_test_store(90);

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

        let rows =
            read_all(&test_support::open_shard(&store, ShardDay::of_started_at(1_000)).await)
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
        let (store, guard) = test_support::create_test_store(90);
        let now = unix_time::unix_now_micros();

        let old_day = ShardDay::of_started_at(now - 91 * MICROS_PER_DAY);
        let fresh_day = ShardDay::of_started_at(now);

        // Creating the expired shard sweeps nothing else and never deletes itself.
        store
            .record(test_support::write(
                day_start(old_day) + 100,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;
        assert!(store.shard_path(old_day).exists());

        // Unparseable filenames a sweep must leave alone.
        std::fs::write(guard.0.join("garbage.db"), b"junk")?;
        std::fs::write(guard.0.join("2026-02-30.db"), b"junk")?;

        // Creating the fresh shard sweeps the expired one — now strictly past its retention —
        // leaving the just-created file and the unparseable names alone.
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
            !store.shard_path(old_day).exists(),
            "the expired shard must be deleted when a new shard file is created"
        );
        assert!(store.shard_path(fresh_day).exists());
        assert!(guard.0.join("garbage.db").exists());
        assert!(guard.0.join("2026-02-30.db").exists());

        // A late finalization whose shard was already retention-deleted recreates the file with
        // that one row — deterministic attribution, accepted.
        store
            .record(test_support::write(
                day_start(old_day) + 200,
                1,
                1,
                1,
                [None; 5],
            ))
            .await?;

        let rows = read_all(&test_support::open_shard(&store, old_day).await).await?;
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
        test_support::open_shard(&store, planted_day).await;
        assert!(store.shard_path(planted_day).exists());

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
            store.shard_path(planted_day).exists(),
            "a write into an existing shard must not run the sweep"
        );

        Ok(())
    }
}
