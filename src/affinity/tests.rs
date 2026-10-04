//! Affinity evidence precedence, reservation ownership, invalidation, and bounded cache behavior.

use super::*;
use crate::test_support;
use std::{
    error::Error as StdError,
    sync::Arc,
    time::{Duration, Instant},
};

/// Fixed test namespace; client routes and physical models are intentionally not cache keys.
fn namespace() -> Namespace {
    Namespace {
        api_key_id: 1,
        pool_id: 2,
    }
}

/// Opaque already-projected identities; tests exercise first-party cache rules, not cryptography.
fn digest(value: u8) -> Digest {
    Digest([value; 32])
}

/// One exact or relational canonical historical prefix.
///
/// # Panics
///
/// Panics if a fixture supplies a zero unit count.
fn prefix(value: u8, units: u64, projection: Projection) -> HistoryPrefix {
    #[expect(
        clippy::expect_used,
        reason = "fixture prefixes have positive unit counts"
    )]
    HistoryPrefix {
        projection,
        digest: digest(value),
        units: NonZeroU64::new(units).expect("units"),
    }
}

/// A complete exact fresh request, without established historical evidence.
fn request(value: u8) -> Evidence {
    Evidence {
        retry: Some(RetryFingerprint {
            projection: Projection::Exact,
            digest: digest(value),
        }),
        ..Evidence::default()
    }
}

/// Takes a fresh leader and makes its unexpected decision the test failure report.
///
/// # Panics
///
/// Panics if the fixture did not produce a fresh leader.
fn fresh(decision: Decision) -> FreshLease {
    match decision {
        Decision::Fresh(lease) => lease,
        #[expect(
            clippy::panic,
            reason = "an unexpected decision is the test failure report"
        )]
        _ => panic!("a fresh selection was required"),
    }
}

/// Reads only a bound account; a miss releases any newly created reservation normally.
fn provider(decision: Decision) -> Option<i64> {
    match decision {
        Decision::Bound(lease) => Some(lease.provider_id()),
        _ => None,
    }
}

/// Committed management deletions fence existing handles even when their HTTP caller cancels;
/// a foreign-key conflict leaves both the pool and its affinity evidence live.
///
/// # Errors
///
/// Returns fixture, HTTP, database, or timeout errors.
///
/// # Panics
///
/// Panics if cancellation loses a mutation or a failed deletion invalidates live evidence.
#[expect(
    clippy::await_holding_lock,
    reason = "authentication and live cache configuration exclude reload fixtures"
)]
#[tokio::test]
async fn management_deletion_owns_cache_invalidation_across_caller_cancellation()
-> Result<(), Box<dyn StdError>> {
    use axum::{Router, body::Body, http::Request};
    use std::{future::Future as _, task::Poll};
    use tower::ServiceExt as _;

    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let (database, _database_guard) = test_support::create_test_database().await;
    let key = crate::storage::api_keys::insert_api_key(
        &database,
        &crate::storage::api_keys::ApiKeyFields {
            name: "client",
            key: "client-secret",
            is_enabled: true,
        },
    )
    .await?;
    let pool_id =
        crate::storage::routing::create_pool(&database, "pool", "pool-pool-", true, &[]).await?;
    let route_id = crate::storage::routing::create_route(
        &database,
        "model",
        true,
        &crate::routing::ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "M".into(),
        },
    )
    .await?;
    let namespace = Namespace {
        api_key_id: key.id,
        pool_id,
    };
    let cache = Arc::new(Cache::default());
    let selected = fresh(cache.acquire(namespace, &request(1))).commit(10);
    let context = crate::context::Context {
        database: database.clone(),
        api_log_store: None,
        data_plane: reqwest::Client::new(),
        control_plane: reqwest::Client::new(),
        quota_coordinator: Arc::new(crate::provider::quota::coordinator::Coordinator::default()),
        affinity_cache: cache.clone(),
    };
    let router = Router::new()
        .nest("/management-api", crate::management_api::router())
        .with_state(context);
    let deletion_request = |path: &str, body: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", &crate::CONFIG.management_api.token)
            .body(Body::from(body.to_string()))
    };
    let response = router
        .clone()
        .oneshot(deletion_request(
            "/management-api/v0/provider-pools:delete",
            serde_json::json!({"provider_pool_id":pool_id}),
        )?)
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(selected.register(&[Anchor::Artifact(digest(9))]), 1);
    crate::storage::routing::delete_route(&database, route_id).await?;

    // The current-thread executor cannot run the detached owner before this first poll returns.
    // Drop the HTTP future at that handoff, then yield only while checking committed storage.
    for (path, body, is_pool) in [
        (
            "/management-api/v0/provider-pools:delete",
            serde_json::json!({"provider_pool_id":pool_id}),
            true,
        ),
        (
            "/management-api/v0/api-keys:delete",
            serde_json::json!({"api_key_id":key.id}),
            false,
        ),
    ] {
        let selected = if is_pool {
            fresh(cache.acquire(namespace, &request(2))).commit(10)
        } else {
            let other_pool =
                crate::storage::routing::create_pool(&database, "other", "pool-other-", true, &[])
                    .await?;
            fresh(cache.acquire(
                Namespace {
                    pool_id: other_pool,
                    ..namespace
                },
                &request(3),
            ))
            .commit(20)
        };
        let mut pending = Box::pin(router.clone().oneshot(deletion_request(path, body)?));
        let first_poll =
            std::future::poll_fn(|context| Poll::Ready(pending.as_mut().poll(context))).await;
        assert!(
            first_poll.is_pending(),
            "deletion reaches its mutation owner"
        );
        drop(pending);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let is_deleted = if is_pool {
                    !crate::storage::routing::list_pools(&database)
                        .await?
                        .iter()
                        .any(|pool| pool.id == pool_id)
                } else {
                    crate::storage::api_keys::find_api_key_by_id(&database, key.id)
                        .await?
                        .is_none()
                };
                if is_deleted {
                    return Ok::<_, crate::storage::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        assert_eq!(selected.register(&[Anchor::Artifact(digest(8))]), 0);
    }
    assert_eq!(selected.register(&[Anchor::Artifact(digest(7))]), 0);
    Ok(())
}

/// Historical evidence prefers length then exact identity, retains branches, and merges
/// conflicting live accounts into a persistent ambiguity instead of reviving weaker hints.
///
/// # Panics
///
/// Panics if ranking, ambiguity, or historical/retry precedence changes.
#[test]
fn historical_strength_and_ambiguity_do_not_revive_weaker_or_retry_hints() {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let cache = Arc::new(Cache::default());
    let first = fresh(cache.acquire(namespace(), &request(1))).commit(10);
    let second = fresh(cache.acquire(namespace(), &request(2))).commit(20);
    let exact = prefix(11, 2, Projection::Exact);
    let relational = prefix(12, 2, Projection::Relational);
    let longest = prefix(13, 3, Projection::Relational);
    let earlier = prefix(14, 1, Projection::Exact);
    first.register(&[Anchor::History(exact), Anchor::History(earlier)]);
    second.register(&[Anchor::History(relational), Anchor::History(longest)]);
    let query = Evidence {
        history: vec![relational, exact],
        ..request(2)
    };
    assert_eq!(provider(cache.acquire(namespace(), &query)), Some(10));
    let ambiguous_relational = prefix(15, 2, Projection::Relational);
    first.register(&[Anchor::History(ambiguous_relational)]);
    second.register(&[Anchor::History(ambiguous_relational)]);
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![ambiguous_relational, exact],
                ..request(2)
            }
        )),
        Some(10),
        "an unambiguous exact anchor outranks relational ambiguity at the same depth"
    );
    let query = Evidence {
        history: vec![exact, longest],
        ..Evidence::default()
    };
    assert_eq!(provider(cache.acquire(namespace(), &query)), Some(20));
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![earlier],
                ..Evidence::default()
            }
        )),
        Some(10),
        "earlier branches remain anchored"
    );
    second.register(&[Anchor::History(exact)]);
    // Identical-provider registration cannot erase an ambiguity created by another producer.
    first.register(&[Anchor::History(exact)]);
    let query = Evidence {
        history: vec![earlier, relational, exact],
        ..request(2)
    };
    let replacement = fresh(cache.acquire(namespace(), &query));
    let waiting = cache.acquire(namespace(), &query);
    assert!(
        matches!(waiting, Decision::Wait(_)),
        "ambiguity may join a fresh pending decision"
    );
    drop(waiting);
    drop(replacement);
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![relational],
                ..Evidence::default()
            }
        )),
        Some(20)
    );
}

/// Only a leader can establish or release a fresh reservation. Followers receive an account,
/// never the leader's output, and dropping a follower leaves the leader pinned.
///
/// # Errors
///
/// Returns a timeout if reservation notifications do not arrive.
///
/// # Panics
///
/// Panics if cancellation, commitment, or pending ownership diverges.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the fixture excludes global configuration reloads"
)]
async fn followers_commit_or_reevaluate_without_owning_the_leader_slot()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let cache = Arc::new(Cache::default());
    let input = request(1);
    let leader = fresh(cache.acquire(namespace(), &input));
    let leader_token = leader.token();
    let dropped = cache.acquire(namespace(), &input);
    assert!(matches!(dropped, Decision::Wait(_)));
    drop(dropped);
    #[expect(
        clippy::panic,
        reason = "an unexpected decision is the test failure report"
    )]
    let Decision::Wait(waiter) = cache.acquire(namespace(), &input) else {
        panic!("follower");
    };
    assert_ne!(waiter.token(), leader_token);
    let accepted = leader.commit(10);
    let outcome = tokio::time::timeout(Duration::from_secs(1), waiter.wait()).await?;
    #[expect(
        clippy::panic,
        reason = "an unexpected decision is the test failure report"
    )]
    let WaitOutcome::Bound(follower) = outcome else {
        panic!("accepted account");
    };
    assert_eq!(follower.provider_id(), 10);
    assert_ne!(follower.token(), accepted.token());
    drop(accepted);
    drop(follower);
    assert_eq!(provider(cache.acquire(namespace(), &input)), Some(10));
    let next = request(2);
    let canceled = fresh(cache.acquire(namespace(), &next));
    #[expect(
        clippy::panic,
        reason = "an unexpected decision is the test failure report"
    )]
    let Decision::Wait(waiter) = cache.acquire(namespace(), &next) else {
        panic!("follower");
    };
    drop(canceled);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), waiter.wait()).await?,
        WaitOutcome::Reevaluate
    ));
    let newer = fresh(cache.acquire(namespace(), &next)).commit(20);
    assert_eq!(provider(cache.acquire(namespace(), &next)), Some(20));
    drop(newer);
    Ok(())
}

/// Deletion invalidates outstanding writers, including a late old leader whose identical key
/// has acquired a newer lifetime. The old handle cannot delete or commit the newer reservation.
///
/// # Errors
///
/// Returns a timeout if deletion does not wake a waiting follower.
///
/// # Panics
///
/// Panics if namespace invalidation permits resurrection or crosses API-key/pool boundaries.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the fixture excludes global configuration reloads"
)]
async fn deleted_namespaces_fence_late_writers_without_permanent_tombstones()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let cache = Arc::new(Cache::default());
    let input = request(1);
    let old = fresh(cache.acquire(namespace(), &input));
    let old_cancelled = fresh(cache.acquire(namespace(), &request(8)));
    #[expect(
        clippy::panic,
        reason = "an unexpected decision is the test failure report"
    )]
    let Decision::Wait(waiter) = cache.acquire(namespace(), &input) else {
        panic!("follower");
    };
    cache.remove_pool(2);
    let newer = fresh(cache.acquire(namespace(), &input));
    let newer_cancelled = fresh(cache.acquire(namespace(), &request(8)));
    drop(old_cancelled);
    assert!(matches!(
        cache.acquire(namespace(), &request(8)),
        Decision::Wait(_)
    ));
    drop(newer_cancelled);
    let old = old.commit(10);
    assert_eq!(old.register(&[Anchor::Artifact(digest(9))]), 0);
    drop(old);
    assert!(matches!(
        cache.acquire(namespace(), &input),
        Decision::Wait(_)
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), waiter.wait()).await?,
        WaitOutcome::Reevaluate
    ));
    let selected = newer.commit(20);
    assert_eq!(selected.register(&[Anchor::Artifact(digest(9))]), 1);
    let other = Namespace {
        api_key_id: 3,
        pool_id: 2,
    };
    let independent = fresh(cache.acquire(other, &input)).commit(30);
    cache.remove_api_key(1);
    assert_eq!(
        selected.register(&[Anchor::History(prefix(3, 2, Projection::Exact))]),
        0
    );
    assert_eq!(provider(cache.acquire(other, &input)), Some(30));
    drop(selected);
    drop(independent);
    cache.remove_pool(2);
    assert_eq!(
        cache.lock().namespaces.len(),
        0,
        "no deleted-namespace tombstone map"
    );
    Ok(())
}

/// Verified/native and latest-artifact evidence constrain uncovered history and retry hints;
/// ambiguity supplies no contrary provider and cannot defeat an authoritative producer.
///
/// # Panics
///
/// Panics if artifact precedence silently rewrites conflicting evidence.
#[test]
fn artifact_and_verified_producer_conflicts_remain_explicit() {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let cache = Arc::new(Cache::default());
    let first = fresh(cache.acquire(namespace(), &request(1))).commit(10);
    let second = fresh(cache.acquire(namespace(), &request(2))).commit(20);
    let history = prefix(3, 2, Projection::Exact);
    first.register(&[Anchor::History(history)]);
    second.register(&[Anchor::Artifact(digest(4))]);
    assert!(matches!(
        cache.acquire(
            namespace(),
            &Evidence {
                history: vec![history],
                artifact: Some(digest(4)),
                ..Evidence::default()
            }
        ),
        Decision::Conflict
    ));
    assert!(matches!(
        cache.acquire(
            namespace(),
            &Evidence {
                artifact: Some(digest(4)),
                proven_provider: Some(10),
                ..Evidence::default()
            }
        ),
        Decision::Conflict
    ));
    first.register(&[Anchor::Artifact(digest(4))]);
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                artifact: Some(digest(4)),
                proven_provider: Some(10),
                ..Evidence::default()
            }
        )),
        Some(10)
    );
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![history],
                artifact: Some(digest(4)),
                ..Evidence::default()
            }
        )),
        Some(10)
    );
    assert!(matches!(
        cache.acquire(
            namespace(),
            &Evidence {
                proven_provider: Some(10),
                ..request(2)
            }
        ),
        Decision::Conflict
    ));
}

/// Retry TTL is short, history TTL is idle-based, and expired conflicting evidence merges as a
/// new single-provider anchor. Pending establishment remains pinned beyond either timeout.
///
/// # Panics
///
/// Panics if expiration clears pending work or retains an expired ambiguity.
#[test]
fn expiry_uses_entry_kind_and_keeps_pending_establishment_pinned() {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let cache = Arc::new(Cache::default());
    let start = Instant::now();
    let first = fresh(cache.acquire_at(namespace(), &request(1), start)).commit_at(10, start);
    let second = fresh(cache.acquire_at(namespace(), &request(2), start)).commit_at(20, start);
    let history = prefix(3, 2, Projection::Exact);
    first.register_at(&[Anchor::History(history)], start);
    second.register_at(&[Anchor::History(history)], start);
    let retry_lifetime = Duration::from_secs(u64::from(
        crate::CONFIG.pooling.sticky_retry_timeout_seconds.get(),
    ));
    let history_lifetime = Duration::from_secs(u64::from(
        crate::CONFIG.pooling.sticky_idle_timeout_seconds.get(),
    ));
    let later = start + retry_lifetime + Duration::from_secs(1);
    let expired_retry = fresh(cache.acquire_at(namespace(), &request(1), later));
    let query = Evidence {
        history: vec![history],
        ..Evidence::default()
    };
    assert_eq!(
        provider(cache.acquire_at(namespace(), &query, later)),
        None,
        "winning ambiguity is a miss and touching it renews its idle lifetime"
    );
    let after_idle = later + history_lifetime + Duration::from_secs(1);
    first.register_at(&[Anchor::History(history)], after_idle);
    assert_eq!(
        provider(cache.acquire_at(namespace(), &query, after_idle)),
        Some(10)
    );
    assert!(
        matches!(
            cache.acquire_at(namespace(), &request(1), after_idle),
            Decision::Wait(_)
        ),
        "pending establishment is not an artificial inference deadline"
    );
    drop(expired_retry);
}

/// Applies live positive cache limits through the same watcher used by production reloads.
///
/// # Errors
///
/// Returns configuration-file, serialization, or watcher-timeout failures.
async fn limits(capacity: usize, idle: u32, retry: u32) -> Result<(), Box<dyn StdError>> {
    let path = test_support::test_configuration_file_path();
    let mut body: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    #[expect(
        clippy::indexing_slicing,
        reason = "the fixture configuration is a JSON object"
    )]
    let pooling = &mut body["pooling"];
    *pooling = serde_json::json!({
        "quota_refresh_interval_seconds":crate::CONFIG.pooling.quota_refresh_interval_seconds.get(),
        "sticky_idle_timeout_seconds":idle, "sticky_retry_timeout_seconds":retry,
        "sticky_maximum_entries":capacity,
    });
    std::fs::write(path, serde_json::to_string(&body)?)?;
    tokio::time::timeout(Duration::from_secs(15), async {
        while crate::CONFIG.pooling.sticky_maximum_entries.get() != capacity
            || crate::CONFIG.pooling.sticky_idle_timeout_seconds.get() != idle
            || crate::CONFIG.pooling.sticky_retry_timeout_seconds.get() != retry
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

/// Both projection aliases count individually. All-pinned capacity produces an uncached handle
/// that cannot insert after capacity becomes free; shrinking leaves pinned entries until release.
/// Accepted handles retain their provider after their retry entry is evicted.
///
/// # Errors
///
/// Returns configuration mutation or watcher-timeout failures.
///
/// # Panics
///
/// Panics if capacity, live pruning, or uncached completion creates an unbounded side store.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "this fixture owns live cache-limit reloads"
)]
async fn capacity_and_live_limits_preserve_pins_and_uncached_ownership()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let original = (
        crate::CONFIG.pooling.sticky_maximum_entries.get(),
        crate::CONFIG.pooling.sticky_idle_timeout_seconds.get(),
        crate::CONFIG.pooling.sticky_retry_timeout_seconds.get(),
    );
    let mut projection = crate::affinity::fingerprint::Projector::new();
    projection.text(
        crate::affinity::fingerprint::Role::User,
        b"stable across reloads",
    );
    let original_digest = projection.evidence().retry.map(|retry| retry.digest);
    limits(2, 2, 2).await?;
    let cache = Arc::new(Cache::default());
    let first = fresh(cache.acquire(namespace(), &request(1)));
    let second = fresh(cache.acquire(namespace(), &request(2)));
    let uncached = fresh(cache.acquire(namespace(), &request(3)));
    assert_eq!(cache.lock().entry_count(), 2);
    let accepted = first.commit(10);
    let third = fresh(cache.acquire(namespace(), &request(4)));
    assert_eq!(
        accepted.provider_id(),
        10,
        "evicting its retry never changes an in-flight account"
    );
    limits(1, 1, 1).await?;
    let mut reloaded = crate::affinity::fingerprint::Projector::new();
    reloaded.text(
        crate::affinity::fingerprint::Role::User,
        b"stable across reloads",
    );
    assert!(reloaded.evidence().retry.map(|retry| retry.digest) == original_digest);
    let no_key = fresh(cache.acquire(namespace(), &Evidence::default()));
    assert_eq!(
        cache.lock().entry_count(),
        2,
        "live shrink cannot evict two pending leaders"
    );
    drop(no_key);
    drop(third);
    let selected = second.commit(20);
    assert_eq!(cache.lock().entry_count(), 1);
    let independent = uncached.commit(30);
    assert_eq!(
        independent.register(&[Anchor::Artifact(digest(8))]),
        0,
        "an uncached completion cannot force later insertion"
    );
    limits(2, 2, 2).await?;
    let exact = prefix(5, 2, Projection::Exact);
    let relational = prefix(6, 2, Projection::Relational);
    accepted.register(&[Anchor::History(exact), Anchor::History(relational)]);
    assert_eq!(
        cache.lock().entry_count(),
        2,
        "two aliases are two counted entries"
    );
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![exact],
                ..Evidence::default()
            }
        )),
        Some(10)
    );
    let pending = fresh(cache.acquire(namespace(), &request(7)));
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![exact],
                ..Evidence::default()
            }
        )),
        Some(10),
        "touching exact evicts the older relational alias first"
    );
    assert_eq!(
        provider(cache.acquire(
            namespace(),
            &Evidence {
                history: vec![relational],
                ..Evidence::default()
            }
        )),
        None
    );
    drop(pending);
    drop(selected);
    limits(original.0, original.1, original.2).await?;
    Ok(())
}
