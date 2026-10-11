//! Tests for the bounded affinity cache: decision precedence across history, retry, artifact,
//! and proven-producer evidence; the follower wait; the live idle and retry windows; namespace
//! invalidation; and bounded capacity (`docs/design/routing.md` *Conversation affinity*).

#![expect(
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::await_holding_lock,
    reason = "a test fails by panicking; classification helpers panic on the wrong decision and \
              a # Panics section on every test would restate that, and the async tests hold the \
              fixture configuration lock across their waits so the shared installed \
              configuration outlives every await"
)]

use super::{Cache, Decision, FreshLease, WaitOutcome, Waiter};
use crate::{
    affinity::{Anchor, Digest, Evidence, HistoryPrefix, Namespace, Projection, RetryFingerprint},
    test_support::{
        await_reload, configuration_body, configuration_directory, configuration_file_path,
        configuration_lock, install_configuration,
    },
};
use std::{
    num::NonZeroU64,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

/// A fixed process-local digest indexed by a test ordinal: the index fills the leading bytes
/// little-endian, so cache keys built from distinct ordinals never collide, whatever entry
/// count the live configuration bounds a test by.
fn digest(index: u64) -> Digest {
    let mut bytes = [0; 32];

    for (slot, byte) in bytes.iter_mut().zip(index.to_le_bytes()) {
        *slot = byte;
    }

    Digest(bytes)
}

/// A history prefix of `units` canonical units over one fixed indexed digest.
fn prefix(projection: Projection, index: u64, units: u64) -> HistoryPrefix {
    #[expect(clippy::expect_used, reason = "the fixture unit count is positive")]
    let units = NonZeroU64::new(units).expect("fixture prefixes are nonempty");

    HistoryPrefix {
        projection,
        digest: digest(index),
        units,
    }
}

/// Evidence carrying only canonical history prefixes.
fn history_evidence(prefixes: Vec<HistoryPrefix>) -> Evidence {
    Evidence {
        history: prefixes,
        ..Evidence::default()
    }
}

/// Evidence carrying only one complete exact-projection retry fingerprint.
fn retry_evidence(index: u64) -> Evidence {
    Evidence {
        retry: Some(RetryFingerprint {
            projection: Projection::Exact,
            digest: digest(index),
        }),
        ..Evidence::default()
    }
}

/// Adds one complete retry fingerprint to otherwise unchanged evidence.
fn with_retry(mut evidence: Evidence, index: u64) -> Evidence {
    evidence.retry = Some(RetryFingerprint {
        projection: Projection::Exact,
        digest: digest(index),
    });

    evidence
}

/// A namespace of the fixed test pool under the given API key.
fn namespace(api_key_id: i64) -> Namespace {
    Namespace {
        api_key_id,
        pool_id: 70,
    }
}

/// A lookup instant `seconds` ahead of the process clock.
fn at(seconds: u64) -> Instant {
    Instant::now() + Duration::from_secs(seconds)
}

/// The fresh lease of a fresh decision; a test expecting a fresh decision fails otherwise.
fn fresh(decision: Decision) -> FreshLease {
    match decision {
        Decision::Fresh(lease) => lease,
        _ => panic!("the test expected a fresh decision"),
    }
}

/// The follower of a wait decision; a test expecting a wait fails otherwise.
fn waiter(decision: Decision) -> Waiter {
    match decision {
        Decision::Wait(waiter) => waiter,
        _ => panic!("the test expected a wait decision"),
    }
}

/// The provider of a bound decision, and nothing for any other decision.
fn bound_provider(decision: &Decision) -> Option<i64> {
    match decision {
        Decision::Bound(lease) => Some(lease.provider_id()),
        _ => None,
    }
}

/// The provider of a bound wait outcome; a test expecting a binding fails otherwise.
fn bound_outcome(outcome: WaitOutcome) -> i64 {
    match outcome {
        WaitOutcome::Bound(lease) => lease.provider_id(),
        WaitOutcome::Reevaluate => panic!("the test expected a bound wait outcome"),
    }
}

/// Renders the shared test configuration body with one added pooling capacity: a mirror of the
/// canonical installer body so the reload applies the same database, management token, sealing
/// keys, and log setup while only the bounded-capacity value under test changes.
fn pooled_configuration_body(directory: &Path, capacity: usize) -> String {
    let database = directory
        .join("db.sqlite")
        .to_string_lossy()
        .replace('\\', "/");
    let logs = directory
        .join("api-logs")
        .to_string_lossy()
        .replace('\\', "/");

    format!(
        r#"{{
    "listen": "127.0.0.1:0",
    "database": {{"path": "{database}"}},
    "management_api": {{"token": "{}"}},
    "api": {{"cors": {{"allowed_origin": "*"}}, "logs": {{"is_enabled": true, "path": "{logs}"}}}},
    "compaction": {{"keys": [
        {{"id": "test-primary", "key": "{}"}},
        {{"id": "test-spare", "key": "{}"}}
    ]}},
    "pooling": {{"sticky_maximum_entries": {capacity}}}
}}"#,
        crate::test_support::MANAGEMENT_TOKEN,
        crate::test_support::SEALING_KEY_PRIMARY,
        crate::test_support::SEALING_KEY_SECONDARY,
    )
}

/// Registers one history anchor for `provider_id` by committing a fresh leader and registering
/// the prefix. The leader must carry a complete retry fingerprint — only a cached fresh
/// selection may register output — and `retry_index` must be unused in the namespace, so each
/// leader owns its own reservation.
fn register_anchor(
    cache: &Arc<Cache>,
    namespace: Namespace,
    retry_index: u64,
    provider_id: i64,
    anchor: Anchor,
    now: Instant,
) {
    fresh(cache.acquire_at(namespace, &retry_evidence(retry_index), now))
        .commit_at(provider_id, now)
        .register_at(&[anchor], now);
}

/// A fresh request reserves its retry identity, concurrent-looking followers wait for the
/// leader's account decision only, and both the follower and later lookups bind to the account
/// the leader committed at first accepted headers.
#[tokio::test]
async fn fresh_reservation_binds_followers_on_commit() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);
    let evidence = retry_evidence(1);

    let leader = fresh(cache.acquire_at(namespace, &evidence, at(0)));
    let waiter = waiter(cache.acquire_at(namespace, &evidence, at(0)));

    leader.commit_at(5, at(0));

    assert_eq!(bound_outcome(waiter.wait().await), 5);
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &evidence, at(1))),
        Some(5)
    );
}

/// A follower whose leader released its reservation without accepting re-evaluates instead of
/// binding, and the released slot admits a new reservation.
#[tokio::test]
async fn released_leader_reevaluates_the_follower() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);
    let evidence = retry_evidence(1);

    let leader = fresh(cache.acquire_at(namespace, &evidence, at(0)));
    let waiter = waiter(cache.acquire_at(namespace, &evidence, at(0)));

    drop(leader);

    assert!(matches!(waiter.wait().await, WaitOutcome::Reevaluate));
    assert!(matches!(
        cache.acquire_at(namespace, &evidence, at(1)),
        Decision::Fresh(_)
    ));
}

/// Deleting a pool invalidates its namespace generation: a waiting follower re-evaluates even
/// before the leader resolves, and the leader's late commit is fenced out of the cache.
#[tokio::test]
async fn deleted_namespace_reevaluates_the_follower() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);
    let evidence = retry_evidence(1);

    let leader = fresh(cache.acquire_at(namespace, &evidence, at(0)));
    let waiter = waiter(cache.acquire_at(namespace, &evidence, at(0)));

    cache.remove_pool(namespace.pool_id);

    assert!(matches!(waiter.wait().await, WaitOutcome::Reevaluate));

    let bound = leader.commit_at(5, at(1));

    assert_eq!(bound.provider_id(), 5);
    assert_eq!(bound.register_at(&[Anchor::Artifact(digest(9))], at(1)), 0);
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &evidence, at(2))),
        None
    );
}

/// A committed retry hint binds for the live retry window and expires exactly at its age: the
/// boundary itself is expired, and the window is read from the live pooling configuration.
#[test]
fn committed_retry_binds_then_expires_at_the_live_window() {
    let _guard = configuration_lock();

    install_configuration();

    let window = u64::from(crate::CONFIG.pooling.sticky_retry_timeout_seconds.get());
    let cache = Arc::new(Cache::default());
    let evidence = retry_evidence(1);

    for api_key_id in [1, 2, 3] {
        fresh(cache.acquire_at(namespace(api_key_id), &evidence, at(0)))
            .commit_at(api_key_id, at(0));
    }

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace(1), &evidence, at(window - 1))),
        Some(1)
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace(2), &evidence, at(window))),
        None
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace(3), &evidence, at(window + 1))),
        None
    );
}

/// A registered history anchor binds for the live idle window and expires exactly at its age,
/// read from the live pooling configuration.
#[test]
fn history_binding_expires_at_the_live_idle_window() {
    let _guard = configuration_lock();

    install_configuration();

    let window = u64::from(crate::CONFIG.pooling.sticky_idle_timeout_seconds.get());
    let anchor = Anchor::History(prefix(Projection::Exact, 1, 2));
    let evidence = history_evidence(vec![prefix(Projection::Exact, 1, 2)]);
    let cache = Arc::new(Cache::default());

    for api_key_id in [1, 2, 3] {
        register_anchor(&cache, namespace(api_key_id), 10, api_key_id, anchor, at(0));
    }

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace(1), &evidence, at(window - 1))),
        Some(1)
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace(2), &evidence, at(window))),
        None
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace(3), &evidence, at(window + 1))),
        None
    );
}

/// The longest known history prefix wins regardless of strength, and exact identities outrank
/// relational ones at equal length.
#[test]
fn longest_prefix_wins_and_exact_outranks_equal_length() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);

    register_anchor(
        &cache,
        namespace,
        20,
        5,
        Anchor::History(prefix(Projection::Relational, 1, 2)),
        at(0),
    );
    register_anchor(
        &cache,
        namespace,
        21,
        6,
        Anchor::History(prefix(Projection::Exact, 2, 2)),
        at(0),
    );

    let equal_length = history_evidence(vec![
        prefix(Projection::Exact, 2, 2),
        prefix(Projection::Relational, 1, 2),
    ]);

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &equal_length, at(0))),
        Some(6)
    );

    register_anchor(
        &cache,
        namespace,
        22,
        7,
        Anchor::History(prefix(Projection::Relational, 3, 3)),
        at(0),
    );

    let longer = history_evidence(vec![
        prefix(Projection::Exact, 2, 2),
        prefix(Projection::Relational, 1, 2),
        prefix(Projection::Relational, 3, 3),
    ]);

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &longer, at(0))),
        Some(7)
    );
}

/// A proven native producer or live artifact binds the request even without history, and each
/// evidence kind that disagrees with the proven producer conflicts before availability checks.
#[test]
fn proven_producer_and_artifact_evidence_bind_and_conflict() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);

    register_anchor(&cache, namespace, 30, 5, Anchor::Artifact(digest(9)), at(0));
    register_anchor(
        &cache,
        namespace,
        31,
        5,
        Anchor::History(prefix(Projection::Exact, 1, 2)),
        at(0),
    );

    let artifact_only = Evidence {
        artifact: Some(digest(9)),
        ..Evidence::default()
    };

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &artifact_only, at(0))),
        Some(5)
    );

    let proven_only = Evidence {
        proven_provider: Some(6),
        ..Evidence::default()
    };

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &proven_only, at(0))),
        Some(6)
    );

    let artifact_conflict = Evidence {
        artifact: Some(digest(9)),
        proven_provider: Some(6),
        ..Evidence::default()
    };

    assert!(matches!(
        cache.acquire_at(namespace, &artifact_conflict, at(0)),
        Decision::Conflict
    ));

    let history_conflict = Evidence {
        history: vec![prefix(Projection::Exact, 1, 2)],
        proven_provider: Some(6),
        ..Evidence::default()
    };

    assert!(matches!(
        cache.acquire_at(namespace, &history_conflict, at(0)),
        Decision::Conflict
    ));

    fresh(cache.acquire_at(namespace, &retry_evidence(2), at(0))).commit_at(5, at(0));

    let retry_conflict = Evidence {
        proven_provider: Some(6),
        ..retry_evidence(2)
    };

    assert!(matches!(
        cache.acquire_at(namespace, &retry_conflict, at(0)),
        Decision::Conflict
    ));
}

/// An established history binding outranks a weaker committed retry hint for the same request.
#[test]
fn established_history_outranks_a_committed_retry() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);

    register_anchor(
        &cache,
        namespace,
        40,
        5,
        Anchor::History(prefix(Projection::Exact, 1, 2)),
        at(0),
    );

    fresh(cache.acquire_at(namespace, &retry_evidence(2), at(0))).commit_at(6, at(0));

    let both = with_retry(history_evidence(vec![prefix(Projection::Exact, 1, 2)]), 2);

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &both, at(0))),
        Some(5)
    );
}

/// Ambiguous historical evidence binds nothing, suppresses a weaker committed retry hint by
/// re-establishing afresh, and leaves the ambiguous entry unresolved.
#[test]
fn ambiguous_history_binds_nothing_and_suppresses_the_weaker_retry() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);
    let anchor = Anchor::History(prefix(Projection::Exact, 1, 1));
    let history = history_evidence(vec![prefix(Projection::Exact, 1, 1)]);
    let retry = retry_evidence(2);

    register_anchor(&cache, namespace, 50, 5, anchor, at(0));
    register_anchor(&cache, namespace, 51, 6, anchor, at(0));

    fresh(cache.acquire_at(namespace, &retry, at(0))).commit_at(5, at(0));

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry, at(0))),
        Some(5)
    );
    assert!(matches!(
        cache.acquire_at(namespace, &history, at(0)),
        Decision::Fresh(_)
    ));

    let both = with_retry(history_evidence(vec![prefix(Projection::Exact, 1, 1)]), 2);

    assert!(matches!(
        cache.acquire_at(namespace, &both, at(0)),
        Decision::Fresh(_)
    ));
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry, at(0))),
        None
    );
}

/// Re-registering one anchor from one account merges, while a disagreeing account marks the
/// anchor ambiguous until expiry or eviction.
#[test]
fn registration_merges_same_provider_evidence_and_marks_disagreement() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);
    let anchor = Anchor::History(prefix(Projection::Exact, 1, 1));
    let evidence = history_evidence(vec![prefix(Projection::Exact, 1, 1)]);

    register_anchor(&cache, namespace, 60, 5, anchor, at(0));
    register_anchor(&cache, namespace, 61, 5, anchor, at(0));

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &evidence, at(0))),
        Some(5)
    );

    register_anchor(&cache, namespace, 62, 6, anchor, at(0));

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &evidence, at(0))),
        None
    );
}

/// Deleting a pool or API key drops exactly its namespaces' entries and fences the deleted
/// generations' outstanding write handles, while every other namespace survives untouched.
#[test]
fn namespace_deletion_drops_entries_and_fences_outstanding_handles() {
    let _guard = configuration_lock();

    install_configuration();

    let cache = Arc::new(Cache::default());
    let kept = namespace(1);
    let doomed = Namespace {
        api_key_id: 2,
        pool_id: 71,
    };
    let other_pool = Namespace {
        api_key_id: 3,
        pool_id: 72,
    };
    let evidence = retry_evidence(1);

    let doomed_leader = fresh(cache.acquire_at(doomed, &evidence, at(0)));
    let kept_leader = fresh(cache.acquire_at(kept, &evidence, at(0)));
    let other_leader = fresh(cache.acquire_at(other_pool, &evidence, at(0)));

    other_leader.commit_at(7, at(0));

    cache.remove_pool(doomed.pool_id);

    let doomed_bound = doomed_leader.commit_at(5, at(1));

    assert_eq!(
        bound_provider(&cache.acquire_at(doomed, &evidence, at(1))),
        None
    );
    assert_eq!(
        doomed_bound.register_at(&[Anchor::Artifact(digest(9))], at(1)),
        0
    );

    let kept_bound = kept_leader.commit_at(6, at(1));

    assert_eq!(
        bound_provider(&cache.acquire_at(other_pool, &evidence, at(1))),
        Some(7)
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(kept, &evidence, at(1))),
        Some(6)
    );
    assert_eq!(
        kept_bound.register_at(&[Anchor::Artifact(digest(8))], at(1)),
        1
    );

    cache.remove_api_key(kept.api_key_id);

    assert_eq!(
        bound_provider(&cache.acquire_at(kept, &evidence, at(2))),
        None
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(other_pool, &evidence, at(2))),
        Some(7)
    );
}

/// Bounded admission evicts the globally oldest entry across both indexes: a full cache admits
/// new identities by evicting oldest first, evicted identities stop binding, touched survivors
/// keep binding, and a new anchor displaces the oldest retry hint.
#[test]
fn capacity_admission_evicts_the_globally_oldest_entry() {
    let _guard = configuration_lock();

    install_configuration();

    let capacity = crate::CONFIG.pooling.sticky_maximum_entries.get();
    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);

    for index in 0..capacity as u64 {
        fresh(cache.acquire_at(namespace, &retry_evidence(index), at(0)))
            .commit_at(index as i64 + 1, at(0));
    }

    fresh(cache.acquire_at(namespace, &retry_evidence(capacity as u64), at(0)))
        .commit_at(capacity as i64 + 1, at(0));

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(capacity as u64 - 1), at(1))),
        Some(capacity as i64)
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(0), at(1))),
        None
    );

    register_anchor(
        &cache,
        namespace,
        capacity as u64 + 5,
        1000,
        Anchor::History(prefix(Projection::Exact, 200, 1)),
        at(1),
    );

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(1), at(1))),
        None
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(
            namespace,
            &history_evidence(vec![prefix(Projection::Exact, 200, 1)]),
            at(1)
        )),
        Some(1000)
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(capacity as u64 - 1), at(1))),
        Some(capacity as i64)
    );
}

/// The bounded capacity is live configuration: under a reloaded capacity of four, pressure
/// evicts oldest entries, an all-pinned cache admits fresh requests without a reservation whose
/// later commit or registration touches the cache, and releasing the pins restores admission.
#[tokio::test]
async fn live_capacity_configuration_bounds_reservations_and_eviction()
-> Result<(), Box<dyn std::error::Error>> {
    let guard = configuration_lock();

    install_configuration();

    let original_capacity = crate::CONFIG.pooling.sticky_maximum_entries.get();
    let directory = configuration_directory();

    std::fs::write(
        configuration_file_path(),
        pooled_configuration_body(&directory, 4),
    )?;

    await_reload(|| crate::CONFIG.pooling.sticky_maximum_entries.get() == 4).await;

    let cache = Arc::new(Cache::default());
    let namespace = namespace(1);

    for index in 0..4 {
        fresh(cache.acquire_at(namespace, &retry_evidence(index), at(0)))
            .commit_at(index as i64 + 1, at(0));
    }

    fresh(cache.acquire_at(namespace, &retry_evidence(4), at(0))).commit_at(5, at(0));

    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(1), at(1))),
        Some(2)
    );
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(0), at(1))),
        None
    );

    let pinned = Vec::from_iter(
        (10..14).map(|index| fresh(cache.acquire_at(namespace, &retry_evidence(index), at(1)))),
    );

    let uncached = fresh(cache.acquire_at(namespace, &retry_evidence(20), at(1)));
    let bound = uncached.commit_at(9, at(1));

    assert_eq!(bound.provider_id(), 9);
    assert_eq!(bound.register_at(&[Anchor::Artifact(digest(50))], at(1)), 0);
    assert_eq!(
        bound_provider(&cache.acquire_at(namespace, &retry_evidence(20), at(2))),
        None
    );

    drop(pinned);

    assert!(matches!(
        cache.acquire_at(namespace, &retry_evidence(20), at(2)),
        Decision::Fresh(_)
    ));

    std::fs::write(
        configuration_file_path(),
        configuration_body(
            &directory.join("db.sqlite"),
            Some(&directory.join("api-logs")),
        ),
    )?;

    await_reload(move || crate::CONFIG.pooling.sticky_maximum_entries.get() == original_capacity)
        .await;

    drop(guard);

    Ok(())
}
