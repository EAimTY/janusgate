//! The api-log store tests: day-file partitioning by the row's own dispatch stamp, the
//! newest-first management list with its timestamp cursor and filters, the retention sweep's
//! strictly-older deletion with its unparseable-name and just-created exceptions, the aggregation
//! merge across files, the recording-switch draft semantics, and the typed drift answer of a
//! day file this build cannot interpret.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or list is that test's failure report"
)]

use super::{
    ApiLog, ApiLogDraft, ApiLogStore, UsageFilter, UsageSelection, finalize_api_log, list_api_logs,
    query_usage,
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome, UsageGroupBy},
    database::{Database, schema::Registry},
    ir::IrUsage,
    storage::Error as StorageError,
};
use std::{error::Error as StdError, num::NonZero, path::PathBuf, time::Duration};

/// Microseconds in one UTC day: the api-log partition width.
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// The Julian Day Number of the Unix epoch, 1970-01-01.
const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;

/// The api-log day of a microsecond stamp.
fn day_of(stamp: i64) -> i64 {
    stamp.div_euclid(MICROS_PER_DAY)
}

/// The `YYYY-MM-DD` file stem of the day containing `stamp`, derived exactly like the store's
/// own mapping.
fn file_stem(stamp: i64) -> String {
    let julian_day = i32::try_from(day_of(stamp) + i64::from(UNIX_EPOCH_JULIAN_DAY))
        .expect("fixture stamps are inside the supported calendar");

    let date = time::Date::from_julian_day(julian_day).expect("fixture stamps are valid dates");

    let (year, month, day) = date.to_calendar_date();

    format!("{year:04}-{:02}-{day:02}", u8::from(month))
}

/// The microsecond stamp of the day whose index is `day_index`, at `intra_day_micros` into the
/// day.
fn stamp(day_index: i64, intra_day_micros: i64) -> i64 {
    day_index * MICROS_PER_DAY + intra_day_micros
}

/// A fresh store directory beside its best-effort cleanup path.
struct StoreDirectory(PathBuf);

impl Drop for StoreDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds a fresh store over a fresh temporary directory with the given whole-day retention.
fn temp_store(label: &str, retention_days: u32) -> (ApiLogStore, StoreDirectory) {
    let directory = StoreDirectory(std::env::temp_dir().join(format!(
        "janusgate-api-logs-test-{}-{label}",
        rand::random::<u64>()
    )));

    let retention = NonZero::new(retention_days).expect("the fixture retention is nonzero");

    (ApiLogStore::new(directory.0.clone(), retention), directory)
}

/// Builds one accounting row with fixture defaults over `started_at` and the overridable
/// attribution and outcome facts.
fn row(
    started_at: i64,
    api_key_id: i64,
    provider_id: i64,
    outcome: ApiLogOutcome,
    http_status: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
) -> ApiLog {
    ApiLog {
        model_route_id: 4,
        upstream_http_status: http_status,
        started_at,
        api_key_id,
        provider_id,
        model_id: 3,
        endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
        is_stream: false,
        http_status,
        outcome,
        duration: 10,
        input_tokens,
        output_tokens,
        cached_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
    }
}

/// Parses a usage filter fixture from its wire JSON — the filter's window bounds are private to
/// the read engine, so the fixture travels through the wire deserialization.
fn filter(json: serde_json::Value) -> UsageFilter {
    serde_json::from_value(json).expect("fixture usage filter parses")
}

/// The api-log file names the store directory currently holds — every `.db` entry, sorted. An
/// open database keeps its engine sidecar files beside it, which are bookkeeping, not partitions.
fn directory_files(directory: &StoreDirectory) -> Vec<String> {
    let mut names = std::fs::read_dir(&directory.0)
        .expect("the fixture directory is listable")
        .map(|entry| {
            entry
                .expect("the listing entry delivers")
                .file_name()
                .into_string()
                .expect("UTF-8 name")
        })
        .filter(|name| name.ends_with(".db"))
        .collect::<Vec<_>>();

    names.sort();

    names
}

/// Recording partitions rows by the UTC day of their own dispatch stamp into `YYYY-MM-DD.db`
/// files, and the management list merges the files newest first behind its timestamp cursor and
/// filters.
#[tokio::test]
async fn recording_partitions_by_day_and_list_merges_newest_first() -> Result<(), Box<dyn StdError>>
{
    let (store, directory) = temp_store("partitioning", 30);

    let day_one = day_of(1_800_000_000_000_000);
    let day_two = day_one + 1;

    store
        .record(row(
            stamp(day_two, 10),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            Some(10),
            Some(5),
        ))
        .await?;

    store
        .record(row(
            stamp(day_one, 20),
            2,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            None,
            None,
        ))
        .await?;

    store
        .record(row(
            stamp(day_two, 20),
            2,
            3,
            ApiLogOutcome::UpstreamError,
            Some(502),
            Some(7),
            Some(2),
        ))
        .await?;

    assert_eq!(
        directory_files(&directory),
        [
            format!("{}.db", file_stem(stamp(day_one, 0))),
            format!("{}.db", file_stem(stamp(day_two, 0)))
        ],
        "one file per UTC day of the dispatch stamps"
    );

    let listed = list_api_logs(&store, 10, None, &UsageFilter::default()).await?;

    let stamps = listed
        .iter()
        .map(|entry| entry.started_at)
        .collect::<Vec<_>>();

    assert_eq!(
        stamps,
        [stamp(day_two, 20), stamp(day_two, 10), stamp(day_one, 20)],
        "the merged page orders newest first across files"
    );

    // The timestamp cursor is exclusive and drives the second page.
    let second_page = list_api_logs(
        &store,
        10,
        Some(stamp(day_two, 20)),
        &UsageFilter::default(),
    )
    .await?;

    let cursor_stamps = second_page
        .iter()
        .map(|entry| entry.started_at)
        .collect::<Vec<_>>();

    assert_eq!(
        cursor_stamps,
        [stamp(day_two, 10), stamp(day_one, 20)],
        "the cursor excludes the boundary stamp and older pages follow"
    );

    // The attribution and outcome filters narrow the rows.
    let by_key = list_api_logs(
        &store,
        10,
        None,
        &filter(serde_json::json!({"api_key_id": [2]})),
    )
    .await?;

    assert_eq!(by_key.len(), 2);

    let by_outcome = list_api_logs(
        &store,
        10,
        None,
        &filter(serde_json::json!({"outcome": ["upstream_error"]})),
    )
    .await?;

    assert_eq!(by_outcome.len(), 1);

    assert_eq!(
        by_outcome.first().map(|entry| entry.outcome),
        Some(ApiLogOutcome::UpstreamError),
    );

    // A null outward status matches no status list, by the engine's own NULL semantics.
    store
        .record(row(
            stamp(day_two, 30),
            1,
            2,
            ApiLogOutcome::ClientAborted,
            None,
            None,
            None,
        ))
        .await?;

    let by_status = list_api_logs(
        &store,
        10,
        None,
        &filter(serde_json::json!({"http_status": [200, 502]})),
    )
    .await?;

    assert_eq!(
        by_status.len(),
        3,
        "the null-status row matches no status list"
    );

    let unknown_key = list_api_logs(
        &store,
        10,
        None,
        &filter(serde_json::json!({"api_key_id": [999]})),
    )
    .await?;

    assert!(unknown_key.is_empty(), "a filter locates, never addresses");

    Ok(())
}

/// The retention sweep runs only when a new day file is created, deletes exactly the files whose
/// day-end is strictly older than now minus the retention, leaves unparseable filenames alone,
/// and never deletes the just-created file even when the late row's day is long expired.
#[tokio::test]
async fn retention_sweeps_strictly_expired_files_only() -> Result<(), Box<dyn StdError>> {
    let (store, directory) = temp_store("retention", 1);

    let now = crate::unix_time::unix_now_micros();

    let today = day_of(now);
    let yesterday = today - 1;
    let two_days_ago = today - 2;
    let three_days_ago = today - 3;

    // Pre-create the older day files directly: the store's own recording would sweep them at
    // creation, and the sweep under test must see them pre-existing.
    for day in [yesterday, two_days_ago, three_days_ago] {
        let path = directory.0.join(format!("{}.db", file_stem(stamp(day, 0))));

        Database::open(
            &path,
            Registry::ApiLogs,
            pool_connections(),
            Duration::from_secs(5),
        )
        .await?;
    }

    std::fs::write(directory.0.join("notes.txt"), b"not an api log")?;

    std::fs::write(directory.0.join("2020-13-45.db"), b"impossible date")?;

    // Recording today's first row creates today's file and triggers the sweep.
    store
        .record(row(
            stamp(today, 100),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            None,
            None,
        ))
        .await?;

    assert_eq!(
        directory_files(&directory),
        [
            "2020-13-45.db".to_string(),
            format!("{}.db", file_stem(stamp(yesterday, 0))),
            format!("{}.db", file_stem(stamp(today, 0))),
        ],
        "the sweep deleted the strictly expired files and left the rest alone"
    );

    assert!(
        directory.0.join("notes.txt").exists(),
        "the unparseable non-database file is left alone"
    );

    // Recording into an existing file never sweeps: the yesterday file stays.
    store
        .record(row(
            stamp(yesterday, 100),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            None,
            None,
        ))
        .await?;

    assert_eq!(
        directory_files(&directory).len(),
        3,
        "recording into an existing file does not sweep"
    );

    // A late finalization recreates an expired partition, and the sweep never deletes the
    // just-created file.
    let (late_store, late_directory) = temp_store("late-finalization", 1);

    late_store
        .record(row(
            stamp(today - 5, 100),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            None,
            None,
        ))
        .await?;

    let late_files = directory_files(&late_directory);

    assert_eq!(
        late_files,
        [format!("{}.db", file_stem(stamp(today - 5, 0)))],
        "the just-created expired file survives its own creation sweep"
    );

    let listed = list_api_logs(&late_store, 10, None, &UsageFilter::default()).await?;

    assert_eq!(listed.len(), 1, "the late row is readable");

    Ok(())
}

/// The usage aggregation merges counts and sums across files per bucket key, orders buckets by
/// the requested dimensions with nulls first, stamps day buckets from their own file, and answers
/// exactly one total bucket over an empty store.
#[tokio::test]
async fn usage_query_groups_merges_and_orders_buckets() -> Result<(), Box<dyn StdError>> {
    let (store, _directory) = temp_store("aggregation", 30);

    let day_one = day_of(1_800_000_000_000_000);
    let day_two = day_one + 1;

    store
        .record(row(
            stamp(day_one, 10),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            Some(10),
            Some(5),
        ))
        .await?;

    store
        .record(row(
            stamp(day_one, 20),
            2,
            2,
            ApiLogOutcome::UpstreamError,
            Some(502),
            Some(7),
            Some(2),
        ))
        .await?;

    store
        .record(row(
            stamp(day_two, 10),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            Some(3),
            Some(4),
        ))
        .await?;

    store
        .record(row(
            stamp(day_two, 20),
            1,
            2,
            ApiLogOutcome::ClientAborted,
            None,
            None,
            None,
        ))
        .await?;

    let selection = UsageSelection {
        group_by: vec![UsageGroupBy::Provider, UsageGroupBy::Outcome],
        filter: UsageFilter::default(),
    };

    let buckets = query_usage(&store, &selection).await?;

    assert_eq!(
        buckets.len(),
        3,
        "one bucket per distinct (provider, outcome) group"
    );

    let rendered = buckets
        .iter()
        .map(|bucket| {
            (
                bucket.key.outcome,
                bucket.requests,
                bucket.input_tokens,
                bucket.output_tokens,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            (Some(ApiLogOutcome::ClientAborted), 1, 0, 0),
            (Some(ApiLogOutcome::Completed), 2, 13, 9),
            (Some(ApiLogOutcome::UpstreamError), 1, 7, 2),
        ],
        "buckets merge across files, null token columns never participate in sums, and the \
         enum spelling orders the outcome dimension"
    );

    // The day dimension stamps each bucket from its own file.
    let selection = UsageSelection {
        group_by: vec![UsageGroupBy::Day],
        filter: UsageFilter::default(),
    };

    let buckets = query_usage(&store, &selection).await?;

    let days = buckets
        .iter()
        .map(|bucket| (bucket.key.day.clone(), bucket.requests))
        .collect::<Vec<_>>();

    assert_eq!(
        days,
        [
            (Some(file_stem(stamp(day_one, 0))), 2),
            (Some(file_stem(stamp(day_two, 0))), 2),
        ],
        "day buckets never straddle files and sort by their day text"
    );

    // The null outward statuses form their own first-ordering bucket.
    let selection = UsageSelection {
        group_by: vec![UsageGroupBy::HttpStatus],
        filter: UsageFilter::default(),
    };

    let buckets = query_usage(&store, &selection).await?;

    let statuses = buckets
        .iter()
        .map(|bucket| (bucket.key.http_status, bucket.requests))
        .collect::<Vec<_>>();

    assert_eq!(
        statuses,
        [(None, 1), (Some(200), 2), (Some(502), 1)],
        "null statuses order first like the SQL column orders them"
    );

    // The filter narrows every file's query; a time window reaching only one file isolates it.
    let selection = UsageSelection {
        group_by: vec![UsageGroupBy::Outcome],
        filter: filter(serde_json::json!({
            "from": stamp(day_two, 0),
            "to": stamp(day_two + 1, 0),
        })),
    };

    let buckets = query_usage(&store, &selection).await?;

    assert_eq!(buckets.len(), 2, "only the second file's outcomes answer");

    // An empty grouping over an empty store answers exactly one zeroed bucket.
    let (empty_store, _empty_directory) = temp_store("empty-aggregation", 30);

    let selection = UsageSelection {
        group_by: Vec::new(),
        filter: UsageFilter::default(),
    };

    let buckets = query_usage(&empty_store, &selection).await?;

    assert_eq!(buckets.len(), 1);

    assert_eq!(buckets.first().map(|bucket| bucket.requests), Some(0));

    assert_eq!(buckets.first().map(|bucket| bucket.input_tokens), Some(0));

    Ok(())
}

/// The draft's dispatch stamp decides the recording: a stamped draft finalizes exactly one row
/// into its own day's file with the observed facts and summed usage, and an unstamped draft —
/// the request never dispatched with recording enabled — records nothing.
#[tokio::test]
async fn draft_finalization_records_stamped_drafts_only() -> Result<(), Box<dyn StdError>> {
    let _configuration = crate::test_support::install_configuration();

    let (store, directory) = temp_store("drafts", 30);

    let mut draft =
        ApiLogDraft::predispatch(1, 2, 3, 4, ApiLogEndpointType::OpenaiChatCompletions, true);

    draft.stamp();

    draft.observe_response(reqwest::StatusCode::OK);

    draft.stamp_billed_usage(Some(IrUsage {
        input_tokens: 100,
        output_tokens: 10,
        cached_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: Some(4),
    }));

    finalize_api_log(
        &store,
        &draft,
        ApiLogOutcome::Completed,
        Some(200),
        Some(IrUsage {
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: Some(2),
            cache_write_tokens: None,
            reasoning_tokens: None,
        }),
    )
    .await;

    let listed = list_api_logs(&store, 10, None, &UsageFilter::default()).await?;

    assert_eq!(listed.len(), 1, "exactly one row records");

    let recorded = listed.first().ok_or(StorageError::NotFound {
        message: "the stamped draft recorded its row".to_string(),
    })?;

    assert_eq!(recorded.api_key_id, 1);

    assert_eq!(recorded.provider_id, 2);

    assert_eq!(recorded.model_id, 3);

    assert_eq!(recorded.model_route_id, 4);

    assert_eq!(
        recorded.endpoint_type,
        ApiLogEndpointType::OpenaiChatCompletions
    );

    assert!(recorded.is_stream);

    assert_eq!(recorded.outcome, ApiLogOutcome::Completed);

    assert_eq!(recorded.http_status, Some(200));

    assert_eq!(recorded.upstream_http_status, Some(200));

    assert!(
        recorded.duration >= 0,
        "the duration is whole milliseconds from the dispatch stamp"
    );

    assert_eq!(
        (
            recorded.input_tokens,
            recorded.output_tokens,
            recorded.cached_tokens,
            recorded.reasoning_tokens
        ),
        (Some(110), Some(15), Some(2), Some(4)),
        "the billed totals and the boundary observation sum into the row"
    );

    assert_eq!(
        directory_files(&directory).len(),
        1,
        "the row landed in its own day's file"
    );

    // An unstamped draft records nothing: no row, no file.
    let unstamped =
        ApiLogDraft::predispatch(1, 2, 3, 4, ApiLogEndpointType::OpenaiChatCompletions, false);

    finalize_api_log(&store, &unstamped, ApiLogOutcome::ClientAborted, None, None).await;

    let listed = list_api_logs(&store, 10, None, &UsageFilter::default()).await?;

    assert_eq!(listed.len(), 1, "the unstamped draft recorded nothing");

    Ok(())
}

/// A day file this build cannot interpret — an unknown stored schema version — fails both the
/// read and the best-effort write of its partition with the typed drift error instead of killing
/// the process. The write only re-validates on a hot-cache miss, so the drifted file is first
/// evicted by newer days.
#[tokio::test]
async fn drifted_day_file_fails_its_read_and_write() -> Result<(), Box<dyn StdError>> {
    let (store, directory) = temp_store("drift", 30);

    let today = day_of(crate::unix_time::unix_now_micros());

    // Four recording days fill and rotate the three-entry hot cache, leaving the oldest day
    // evicted.
    for offset in [3, 2, 1, 0] {
        store
            .record(row(
                stamp(today - offset, 100),
                1,
                2,
                ApiLogOutcome::Completed,
                Some(200),
                None,
                None,
            ))
            .await?;
    }

    let evicted_day = today - 3;

    let day_file = directory
        .0
        .join(format!("{}.db", file_stem(stamp(evicted_day, 0))));

    {
        let database = Database::open(
            &day_file,
            Registry::ApiLogs,
            pool_connections(),
            Duration::from_secs(5),
        )
        .await?;

        database.execute("PRAGMA user_version = 999", ()).await?;

        drop(database);
    }

    let read = list_api_logs(&store, 10, None, &UsageFilter::default()).await;

    assert!(
        matches!(&read, Err(StorageError::Drift { .. })),
        "the corrupt partition fails the read explicitly, got {read:?}"
    );

    let write = store
        .record(row(
            stamp(evicted_day, 200),
            1,
            2,
            ApiLogOutcome::Completed,
            Some(200),
            None,
            None,
        ))
        .await;

    assert!(
        matches!(&write, Err(StorageError::Drift { .. })),
        "the corrupt partition fails the write as a recording failure, got {write:?}"
    );

    Ok(())
}

/// The pooled open arguments of a direct api-log file open.
fn pool_connections() -> std::num::NonZero<usize> {
    NonZero::new(2_usize).expect("the literal 2 is nonzero")
}

/// A missing store reads as empty: the read answers no rows and creates no directory.
#[tokio::test]
async fn missing_store_reads_as_empty() -> Result<(), Box<dyn StdError>> {
    let (store, directory) = temp_store("missing", 30);

    let listed = list_api_logs(&store, 10, None, &UsageFilter::default()).await?;

    assert!(listed.is_empty(), "an absent store lists empty");

    let selection = UsageSelection {
        group_by: Vec::new(),
        filter: UsageFilter::default(),
    };

    let buckets = query_usage(&store, &selection).await?;

    assert_eq!(
        buckets.len(),
        1,
        "the total bucket exists even without files"
    );

    // A day-only selection over the empty partition answers no bucket at all.
    let selection = UsageSelection {
        group_by: vec![UsageGroupBy::Day],
        filter: UsageFilter::default(),
    };

    let buckets = query_usage(&store, &selection).await?;

    assert!(
        buckets.is_empty(),
        "a day-only selection over nothing stays bucketless"
    );

    assert!(
        !directory.0.exists(),
        "a read never creates the store directory"
    );

    Ok(())
}
