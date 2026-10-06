//! Shared fixtures for the api-log store and read-engine tests: the store constructor, the raw
//! day-file opener, and the row fixture both test modules place rows with.

use super::{
    ApiLog,
    store::{ApiLogDay, ApiLogStore, FILE_POOL_WAIT, write_pool_connections},
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    database::{Database, schema::Registry},
    test_support::TempDirGuard,
};
use std::num::NonZero;

/// Creates an api-log store over a fresh temporary directory with `retention_days`.
///
/// # Panics
///
/// Panics on a zero `retention_days`, which no test passes.
pub(super) fn create_test_store(retention_days: u32) -> (ApiLogStore, TempDirGuard) {
    let directory =
        std::env::temp_dir().join(format!("janusgate-apilogs-test-{}", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
    let retention = NonZero::new(retention_days).unwrap();

    (
        ApiLogStore::new(directory.clone(), retention),
        TempDirGuard(directory),
    )
}

/// Opens one day's api-log file of `store` for raw statement probes, creating the file when
/// absent.
///
/// # Panics
///
/// Panics if opening the file fails.
pub(super) async fn open_day_file(store: &ApiLogStore, day: ApiLogDay) -> Database {
    #[expect(clippy::unwrap_used, reason = "a test api-log file always opens")]
    Database::open(
        store.day_file_path(day),
        Registry::ApiLogs,
        write_pool_connections(),
        FILE_POOL_WAIT,
    )
    .await
    .unwrap()
}

/// One read-fixture row: fixed identity and terminal facts, caller-chosen dispatch time and
/// token columns (`[input, output, cached, cache_write, reasoning]`).
pub(super) fn api_log(
    started_at: i64,
    api_key_id: i64,
    provider_id: i64,
    model_id: i64,
    tokens: [Option<i64>; 5],
) -> ApiLog {
    let [
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_write_tokens,
        reasoning_tokens,
    ] = tokens;

    ApiLog {
        started_at,
        api_key_id,
        provider_id,
        model_id,
        model_route_id: model_id,
        upstream_http_status: Some(200),
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
