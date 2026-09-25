//! Shared fixtures for the shard-store and read-engine tests: the temporary shard-directory
//! guard, the store constructor, the raw shard opener, and the write fixture both test modules
//! place rows with.

use super::{
    ApiLogWrite,
    store::{ApiLogStore, SHARD_POOL_WAIT, ShardDay, shard_write_pool_connections},
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    database::{Database, schema::Registry},
};
use std::{num::NonZero, path::PathBuf};

/// Directory guard cleaning up the temporary shard tree on drop.
pub(super) struct TempStoreGuard(pub(super) PathBuf);

impl Drop for TempStoreGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates a shard store over a fresh temporary directory with `retention_days`.
///
/// # Panics
///
/// Panics on a zero `retention_days`, which no test passes.
pub(super) fn create_test_store(retention_days: u32) -> (ApiLogStore, TempStoreGuard) {
    let directory =
        std::env::temp_dir().join(format!("janusgate-apilogs-test-{}", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
    let retention = NonZero::new(retention_days).unwrap();

    (
        ApiLogStore::new(directory.clone(), retention),
        TempStoreGuard(directory),
    )
}

/// Opens one shard of `store` for raw statement probes, creating the file when absent.
///
/// # Panics
///
/// Panics if opening the shard fails.
pub(super) async fn open_shard(store: &ApiLogStore, day: ShardDay) -> Database {
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

/// One read-fixture write: fixed identity and terminal facts, caller-chosen dispatch time and
/// token columns (`[input, output, cached, cache_write, reasoning]`).
pub(super) fn write(
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
