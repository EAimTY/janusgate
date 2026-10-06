//! Bounded digest indexes and cancellation-safe ownership of fresh account establishment.

use super::{Anchor, Digest, Evidence, HistoryPrefix, Namespace, Projection, RetryFingerprint};
use std::{
    collections::{BTreeSet, HashMap},
    num::NonZeroU64,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

/// One shared runtime cache. All lookup/index mutations hold one short synchronous mutex.
#[derive(Default)]
pub(crate) struct Cache {
    /// Digest-only entries, ordered access indexes, and live namespace generations.
    state: Mutex<State>,
}

/// Account-selection evidence; a fresh lease alone permits preacceptance quota failover.
pub(crate) enum Decision {
    /// Established history, retry, or active-artifact binding.
    Bound(BoundLease),
    /// Fresh leader or independent uncached selection.
    Fresh(FreshLease),
    /// A different request owns this full-history establishment reservation.
    Wait(Waiter),
    /// Active authoritative/artifact evidence disagrees with a single live cached provider.
    Conflict,
}

/// A follower's account decision, never the leader's completion content.
pub(crate) enum WaitOutcome {
    /// Accepted account; the follower executes its own operation.
    Bound(BoundLease),
    /// The leader released or its namespace was deleted; reload route/cache evidence.
    Reevaluate,
}

/// A fresh operation that owns only its own pending reservation, when one was admitted.
pub(crate) struct FreshLease {
    /// Cancellation-safe memory lease, with no database/network cleanup.
    lease: Lease,
}

/// An established account carried independently of cache residency through result rendering.
pub(crate) struct BoundLease {
    /// Lifetime-fenced output registration and harmless final drop.
    lease: Lease,
    /// Real selected physical account, immutable after acceptance/binding.
    provider_id: i64,
}

/// A follower owns its namespace lifetime and its wait, never the leader's reservation.
pub(crate) struct Waiter {
    /// Independent request token; dropping it cannot release the leader.
    lease: Lease,
    /// Account-only establishment notification.
    receiver: watch::Receiver<Signal>,
}

/// Account-only single-flight notifications.
#[derive(Clone, Copy)]
enum Signal {
    /// The leader has not accepted any data-plane operation.
    Pending,
    /// First accepted headers committed this physical account.
    Committed(i64),
    /// Failure, cancellation, or namespace deletion requires re-evaluation.
    Released,
}

/// Common request ownership, retaining no prompts, outputs, or client session identifiers.
struct Lease {
    /// Runtime whose entries this request can mutate.
    cache: Arc<Cache>,
    /// Deletion fence shared by every live request/entry of this namespace generation.
    lifetime: Arc<NamespaceLifetime>,
    /// Non-recycled request ownership token.
    token: NonZeroU64,
    /// Only a fresh cached leader carries its own pending key.
    pending_key: Option<RetryKey>,
    /// Whether this operation can register eligible output under normal cache admission.
    can_register: bool,
}

/// Ephemeral namespace generation; it dies with its entries and in-flight handles.
struct NamespaceLifetime {
    /// API-key/pool domain, excluding route/model/streaming metadata.
    namespace: Namespace,
    /// Deletion permanently invalidates this generation's late callbacks.
    is_valid: AtomicBool,
}

/// History projection strength or a latest-artifact identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum AnchorKind {
    /// Canonical complete history prefix.
    History(Projection),
    /// Complete replayable artifact.
    Artifact,
}

/// One counted anchor-index identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct AnchorKey {
    /// Pool-wide namespace.
    namespace: Namespace,
    /// History identity strength or artifact family.
    kind: AnchorKind,
    /// Process-keyed digest, never logged or persisted.
    digest: Digest,
}

impl AnchorKey {
    /// Resolves one eligible output anchor into its lookup key.
    fn new(namespace: Namespace, anchor: Anchor) -> Self {
        match anchor {
            Anchor::History(prefix) => Self {
                namespace,
                kind: AnchorKind::History(prefix.projection),
                digest: prefix.digest,
            },
            Anchor::Artifact(digest) => Self {
                namespace,
                kind: AnchorKind::Artifact,
                digest,
            },
        }
    }
}

/// One counted exact-request retry identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RetryKey {
    /// Pool-wide namespace.
    namespace: Namespace,
    /// Complete exact projection when available, otherwise relational.
    projection: Projection,
    /// Complete active-history digest.
    digest: Digest,
}

impl RetryKey {
    /// Adds the authenticated pool namespace to a complete request fingerprint.
    fn new(namespace: Namespace, fingerprint: RetryFingerprint) -> Self {
        Self {
            namespace,
            projection: fingerprint.projection,
            digest: fingerprint.digest,
        }
    }
}

/// Live historical/artifact account evidence, without an unbounded producer set.
#[derive(Clone, Copy)]
enum AnchorValue {
    /// Every live registration agrees on one real account.
    One(i64),
    /// Conflicting live registrations; it remains ambiguous until idle expiry or eviction.
    Ambiguous,
}

/// Idle age and deterministic LRU ordering for one unpinned entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Access {
    /// Monotonic last-use timestamp.
    last_used: Instant,
    /// Unique sequence disambiguating equal timestamps without retaining an access journal.
    sequence: NonZeroU64,
}

/// One historical/artifact hint and its bounded bookkeeping.
struct AnchorEntry {
    /// Account evidence, never completed content.
    value: AnchorValue,
    /// Idle expiration and ordered access position.
    access: Access,
    /// Keeps its namespace generation alive while this entry exists.
    lifetime: Arc<NamespaceLifetime>,
}

/// One complete-request retry entry.
struct RetryEntry {
    /// Pending ownership or accepted-account hint.
    value: RetryValue,
    /// Keeps the namespace alive while pending or cached.
    lifetime: Arc<NamespaceLifetime>,
}

/// Retry states; pending entries are absent from the evictable/expiring access index.
enum RetryValue {
    /// Only this reservation may replace its provisional account or release its slot.
    Pending {
        /// Non-recycled leader identity.
        token: NonZeroU64,
        /// Followers receive only acceptance or re-evaluation.
        sender: watch::Sender<Signal>,
    },
    /// A short-lived accepted account, no longer pinned.
    Committed {
        /// Physical account fixed by accepted headers.
        provider_id: i64,
        /// Retry-specific idle expiration and LRU position.
        access: Access,
    },
}

/// Both logical indexes and their bounded ordered-access bookkeeping under one mutex.
struct State {
    /// Historical projection aliases, artifact hints, and ambiguity markers count individually.
    anchors: HashMap<AnchorKey, AnchorEntry>,
    /// Pending and committed complete-request hints count individually.
    retries: HashMap<RetryKey, RetryEntry>,
    /// Ordered historical access supports expiry without scanning the whole cache per request.
    anchor_access: BTreeSet<(Access, AnchorKey)>,
    /// Ordered committed-retry access; pending entries cannot expire or be evicted.
    retry_access: BTreeSet<(Access, RetryKey)>,
    /// Weak lifetime registry, bounded by current entries and in-flight handles, not tombstones.
    namespaces: HashMap<Namespace, Weak<NamespaceLifetime>>,
    /// Shared non-recycled sequence for request tokens and compact access ordering.
    next_sequence: NonZeroU64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            anchors: HashMap::new(),
            retries: HashMap::new(),
            anchor_access: BTreeSet::new(),
            retry_access: BTreeSet::new(),
            namespaces: HashMap::new(),
            next_sequence: NonZeroU64::MIN,
        }
    }
}

impl State {
    /// Allocates an ownership/access token that never wraps or recycles.
    ///
    /// # Panics
    ///
    /// Panics on process-local sequence exhaustion, an intentional fail-fast invariant.
    fn next_token(&mut self) -> NonZeroU64 {
        let token = self.next_sequence;
        #[expect(
            clippy::expect_used,
            reason = "ownership tokens must never wrap or recycle"
        )]
        let next = token
            .checked_add(1)
            .expect("affinity process-local sequence exhausted");
        self.next_sequence = next;
        token
    }

    /// Makes a new ordered access position, without accumulating stale order records.
    fn access(&mut self, now: Instant) -> Access {
        Access {
            last_used: now,
            sequence: self.next_token(),
        }
    }

    /// Counts each identity once; ordered access is bounded bookkeeping for the same entries.
    fn entry_count(&self) -> usize {
        self.anchors.len() + self.retries.len()
    }

    /// Finds or creates the generation shared by entries and requests in this namespace.
    fn lifetime(&mut self, namespace: Namespace) -> Arc<NamespaceLifetime> {
        if let Some(lifetime) = self.namespaces.get(&namespace).and_then(Weak::upgrade) {
            return lifetime;
        }
        let lifetime = Arc::new(NamespaceLifetime {
            namespace,
            is_valid: AtomicBool::new(true),
        });
        self.namespaces.insert(namespace, Arc::downgrade(&lifetime));
        lifetime
    }

    /// Retires an otherwise dead weak registry record, protecting a newer generation at that key.
    fn retire_lifetime(&mut self, lifetime: &Arc<NamespaceLifetime>) {
        if Arc::strong_count(lifetime) == 1
            && self
                .namespaces
                .get(&lifetime.namespace)
                .is_some_and(|registered| registered.ptr_eq(&Arc::downgrade(lifetime)))
        {
            self.namespaces.remove(&lifetime.namespace);
        }
    }

    /// Builds one independent request handle while the namespace registry is locked.
    fn lease(
        &mut self,
        cache: &Arc<Cache>,
        namespace: Namespace,
        pending_key: Option<RetryKey>,
        can_register: bool,
    ) -> Lease {
        Lease {
            cache: Arc::clone(cache),
            lifetime: self.lifetime(namespace),
            token: self.next_token(),
            pending_key,
            can_register,
        }
    }

    /// Removes one anchor and its single access-index record.
    fn remove_anchor(&mut self, key: AnchorKey) {
        if let Some(entry) = self.anchors.remove(&key) {
            self.anchor_access.remove(&(entry.access, key));
            self.retire_lifetime(&entry.lifetime);
        }
    }

    /// Removes one retry, waking pending followers without awaiting or retaining a tombstone.
    fn remove_retry(&mut self, key: RetryKey) {
        if let Some(entry) = self.retries.remove(&key) {
            match entry.value {
                RetryValue::Pending { sender, .. } => {
                    sender.send_replace(Signal::Released);
                }
                RetryValue::Committed { access, .. } => {
                    self.retry_access.remove(&(access, key));
                }
            }
            self.retire_lifetime(&entry.lifetime);
        }
    }

    /// Evicts the globally oldest unpinned identity across the two bounded indexes.
    fn evict_oldest(&mut self) -> bool {
        match (
            self.anchor_access.first().copied(),
            self.retry_access.first().copied(),
        ) {
            (Some((anchor_access, anchor)), Some((retry_access, retry))) => {
                if anchor_access <= retry_access {
                    self.remove_anchor(anchor);
                } else {
                    self.remove_retry(retry);
                }
                true
            }
            (Some((_, anchor)), None) => {
                self.remove_anchor(anchor);
                true
            }
            (None, Some((_, retry))) => {
                self.remove_retry(retry);
                true
            }
            (None, None) => false,
        }
    }

    /// Applies live idle lifetimes and capacity, temporarily retaining only unavoidable pins.
    fn prune(&mut self, now: Instant) {
        let config = &crate::CONFIG.pooling;
        let history_lifetime =
            Duration::from_secs(u64::from(config.sticky_idle_timeout_seconds.get()));
        let retry_lifetime =
            Duration::from_secs(u64::from(config.sticky_retry_timeout_seconds.get()));
        while let Some((access, key)) = self.anchor_access.first().copied() {
            if now.saturating_duration_since(access.last_used) < history_lifetime {
                break;
            }
            self.remove_anchor(key);
        }
        while let Some((access, key)) = self.retry_access.first().copied() {
            if now.saturating_duration_since(access.last_used) < retry_lifetime {
                break;
            }
            self.remove_retry(key);
        }
        while self.entry_count() > config.sticky_maximum_entries.get() {
            if !self.evict_oldest() {
                break;
            }
        }
    }

    /// Makes room for one new identity; all-pinned capacity admits no cached reservation.
    fn admit(&mut self, now: Instant) -> bool {
        self.prune(now);
        while self.entry_count() >= crate::CONFIG.pooling.sticky_maximum_entries.get() {
            if !self.evict_oldest() {
                return false;
            }
        }
        true
    }

    /// Whether an established handle can attempt later normal output admission.
    fn can_register(&self) -> bool {
        self.entry_count() < crate::CONFIG.pooling.sticky_maximum_entries.get()
            || !self.anchor_access.is_empty()
            || !self.retry_access.is_empty()
    }

    /// Touches a known live anchor exactly once, replacing its prior access-index record.
    ///
    /// # Panics
    ///
    /// Panics if the caller's locked lookup was not present, an internal invariant.
    fn touch_anchor(&mut self, key: AnchorKey, now: Instant) -> AnchorValue {
        let access = self.access(now);
        #[expect(
            clippy::expect_used,
            reason = "the preceding lookup holds the same state lock"
        )]
        let entry = self
            .anchors
            .get_mut(&key)
            .expect("the live anchor lookup exists");
        self.anchor_access.remove(&(entry.access, key));
        entry.access = access;
        self.anchor_access.insert((access, key));
        entry.value
    }

    /// Touches a consulted committed retry; pending work is pinned without an idle clock.
    fn touch_retry(&mut self, key: RetryKey, now: Instant) {
        let access = self.access(now);
        if let Some(RetryEntry {
            value: RetryValue::Committed {
                access: previous, ..
            },
            ..
        }) = self.retries.get_mut(&key)
        {
            self.retry_access.remove(&(*previous, key));
            *previous = access;
            self.retry_access.insert((access, key));
        }
    }

    /// Chooses the longest live prefix, then exact strength; touches only the winning evidence.
    fn history(
        &mut self,
        namespace: Namespace,
        prefixes: &[HistoryPrefix],
        now: Instant,
    ) -> Option<AnchorValue> {
        let mut winner = None;
        for prefix in prefixes {
            let key = AnchorKey::new(namespace, Anchor::History(*prefix));
            if self.anchors.contains_key(&key) {
                let rank = (prefix.units, prefix.projection);
                if winner.is_none_or(|(current, _)| rank > current) {
                    winner = Some((rank, key));
                }
            }
        }
        winner.map(|(_, key)| self.touch_anchor(key, now))
    }

    /// Reads a complete-request accepted account, without allowing a pending provisional choice.
    fn retry_provider(&self, key: Option<RetryKey>) -> Option<i64> {
        match &self.retries.get(&key?)?.value {
            RetryValue::Committed { provider_id, .. } => Some(*provider_id),
            RetryValue::Pending { .. } => None,
        }
    }

    /// Invalidates deleted namespace generations and removes only their indexed identities.
    fn invalidate(&mut self, matches: impl Fn(Namespace) -> bool) {
        self.namespaces.retain(|namespace, registered| {
            if !matches(*namespace) {
                return true;
            }
            if let Some(lifetime) = registered.upgrade() {
                lifetime.is_valid.store(false, Ordering::Release);
            }
            false
        });
        let anchors: Vec<_> = self
            .anchors
            .keys()
            .filter(|key| matches(key.namespace))
            .copied()
            .collect();
        let retries: Vec<_> = self
            .retries
            .keys()
            .filter(|key| matches(key.namespace))
            .copied()
            .collect();
        for key in anchors {
            self.remove_anchor(key);
        }
        for key in retries {
            self.remove_retry(key);
        }
    }
}

impl Cache {
    /// Locks only memory metadata; a poisoned invariant is fatal rather than silently repaired.
    ///
    /// # Panics
    ///
    /// Panics if an earlier operation poisoned the cache mutex.
    fn lock(&self) -> MutexGuard<'_, State> {
        #[expect(
            clippy::expect_used,
            reason = "a poisoned cache is a fatal invariant failure"
        )]
        self.state
            .lock()
            .expect("affinity cache mutex was poisoned")
    }

    /// Resolves active evidence or reserves fresh establishment, without network/database I/O.
    pub(crate) fn acquire(self: &Arc<Self>, namespace: Namespace, evidence: &Evidence) -> Decision {
        self.acquire_at(namespace, evidence, Instant::now())
    }

    /// Test-only synchronization at actual follower admission, independent of wall-clock sleeps.
    #[cfg(test)]
    pub(crate) fn has_waiting_request(&self, namespace: Namespace) -> bool {
        self.lock().retries.iter().any(|(key, entry)| {
            key.namespace == namespace
                && matches!(&entry.value,
                RetryValue::Pending { sender, .. } if sender.receiver_count() > 0)
        })
    }

    /// Performs one monotonic lookup; the explicit instant supports cohesive lifecycle tests.
    fn acquire_at(
        self: &Arc<Self>,
        namespace: Namespace,
        evidence: &Evidence,
        now: Instant,
    ) -> Decision {
        let mut state = self.lock();
        state.prune(now);

        // One registration posture for the whole lookup, computed under this
        // same lock: the touches below never change entry counts, and every
        // Bound path here carries evidence — a touched anchor or a committed
        // retry with its access record — that pins a non-empty access index,
        // so the computed value is `true` at each of them.
        let can_register = state.can_register();

        let artifact = evidence.artifact.and_then(|digest| {
            let key = AnchorKey::new(namespace, Anchor::Artifact(digest));
            state
                .anchors
                .contains_key(&key)
                .then(|| state.touch_anchor(key, now))
        });
        let history = state.history(namespace, &evidence.history, now);
        let retry_key = evidence.retry.map(|retry| RetryKey::new(namespace, retry));
        let retry_provider = state.retry_provider(retry_key);
        let artifact_provider = match artifact {
            Some(AnchorValue::One(provider)) => Some(provider),
            _ => None,
        };
        let required = match (evidence.proven_provider, artifact_provider) {
            (Some(proven), Some(cached)) if proven != cached => return Decision::Conflict,
            (proven, cached) => proven.or(cached),
        };
        if let Some(provider_id) = required {
            if history.is_some_and(
                |value| matches!(value, AnchorValue::One(other) if other != provider_id),
            ) || retry_provider.is_some_and(|other| other != provider_id)
            {
                if let Some(key) = retry_key {
                    state.touch_retry(key, now);
                }
                return Decision::Conflict;
            }
            if let Some(key) = retry_key {
                state.touch_retry(key, now);
            }
            return Decision::Bound(BoundLease {
                lease: state.lease(self, namespace, None, can_register),
                provider_id,
            });
        }
        if let Some(AnchorValue::One(provider_id)) = history {
            return Decision::Bound(BoundLease {
                lease: state.lease(self, namespace, None, can_register),
                provider_id,
            });
        }
        let has_ambiguity = matches!(history, Some(AnchorValue::Ambiguous));
        if !has_ambiguity && let Some(provider_id) = retry_provider {
            if let Some(key) = retry_key {
                state.touch_retry(key, now);
            }
            return Decision::Bound(BoundLease {
                lease: state.lease(self, namespace, None, can_register),
                provider_id,
            });
        }
        if let Some(key) = retry_key {
            if let Some(RetryEntry {
                value: RetryValue::Pending { sender, .. },
                ..
            }) = state.retries.get(&key)
            {
                let receiver = sender.subscribe();
                return Decision::Wait(Waiter {
                    lease: state.lease(self, namespace, None, true),
                    receiver,
                });
            }
            // Winning historical ambiguity bypasses, and replaces, a weaker committed retry.
            if has_ambiguity {
                state.remove_retry(key);
            }
            if state.admit(now) {
                let lease = state.lease(self, namespace, Some(key), true);
                let (sender, _) = watch::channel(Signal::Pending);
                state.retries.insert(
                    key,
                    RetryEntry {
                        lifetime: Arc::clone(&lease.lifetime),
                        value: RetryValue::Pending {
                            token: lease.token,
                            sender,
                        },
                    },
                );
                return Decision::Fresh(FreshLease { lease });
            }
        }
        Decision::Fresh(FreshLease {
            lease: state.lease(self, namespace, None, false),
        })
    }

    /// Drops every deleted pool namespace and invalidates its outstanding cache-write handles.
    pub(crate) fn remove_pool(&self, pool_id: i64) {
        self.lock()
            .invalidate(|namespace| namespace.pool_id == pool_id);
    }

    /// Drops every deleted API-key namespace without retaining permanent deletion tombstones.
    pub(crate) fn remove_api_key(&self, api_key_id: i64) {
        self.lock()
            .invalidate(|namespace| namespace.api_key_id == api_key_id);
    }
}

impl FreshLease {
    /// Commits the first accepted account before exposing events or waking followers.
    pub(crate) fn commit(self, provider_id: i64) -> BoundLease {
        self.commit_at(provider_id, Instant::now())
    }

    /// Commits only this lease's reservation; deletion/newer ownership makes a late write a no-op.
    fn commit_at(mut self, provider_id: i64, now: Instant) -> BoundLease {
        {
            let mut state = self.lease.cache.lock();
            state.prune(now);
            if self.lease.lifetime.is_valid.load(Ordering::Acquire)
                && let Some(key) = self.lease.pending_key.take()
                && let Some(RetryEntry {
                    value: RetryValue::Pending { token, .. },
                    ..
                }) = state.retries.get(&key)
                && *token == self.lease.token
            {
                let access = state.access(now);
                if let Some(entry) = state.retries.get_mut(&key) {
                    let previous = std::mem::replace(
                        &mut entry.value,
                        RetryValue::Committed {
                            provider_id,
                            access,
                        },
                    );
                    if let RetryValue::Pending { sender, .. } = previous {
                        sender.send_replace(Signal::Committed(provider_id));
                    }
                    state.retry_access.insert((access, key));
                }
            }
            // Shrinking capacity may evict this now-unpinned hint. The selected account survives
            // in the handle and in follower notifications independently of cache residency.
            state.prune(now);
        }
        self.lease.pending_key = None;
        BoundLease {
            lease: self.lease,
            provider_id,
        }
    }

    /// Test-only ownership identity; never a management or outward inspection surface.
    #[cfg(test)]
    fn token(&self) -> NonZeroU64 {
        self.lease.token
    }
}

impl BoundLease {
    /// The established physical account, unaffected by cache eviction or priority changes.
    pub(crate) fn provider_id(&self) -> i64 {
        self.provider_id
    }

    /// Registers only fully replayable outward anchors under ordinary bounded admission.
    /// Namespace deletion and deliberately uncached selection make registration a no-op.
    pub(crate) fn register(&self, anchors: &[Anchor]) -> usize {
        self.register_at(anchors, Instant::now())
    }

    /// Merges live same-provider evidence and preserves ambiguity until expiry/eviction.
    fn register_at(&self, anchors: &[Anchor], now: Instant) -> usize {
        if !self.lease.can_register {
            return 0;
        }
        let mut state = self.lease.cache.lock();
        state.prune(now);
        if !self.lease.lifetime.is_valid.load(Ordering::Acquire) {
            return 0;
        }
        let mut registered = 0;
        for anchor in anchors {
            let key = AnchorKey::new(self.lease.lifetime.namespace, *anchor);
            if let Some(entry) = state.anchors.get_mut(&key) {
                if matches!(entry.value, AnchorValue::One(other) if other != self.provider_id) {
                    entry.value = AnchorValue::Ambiguous;
                }
                state.touch_anchor(key, now);
                registered += 1;
            } else if state.admit(now) {
                let access = state.access(now);
                state.anchors.insert(
                    key,
                    AnchorEntry {
                        value: AnchorValue::One(self.provider_id),
                        access,
                        lifetime: Arc::clone(&self.lease.lifetime),
                    },
                );
                state.anchor_access.insert((access, key));
                registered += 1;
            }
        }
        registered
    }

    /// Test-only ownership identity, unavailable to runtime inspection APIs.
    #[cfg(test)]
    fn token(&self) -> NonZeroU64 {
        self.lease.token
    }
}

impl Waiter {
    /// Waits only for the provider decision; cancellation drops this wait without mutating a slot.
    pub(crate) async fn wait(mut self) -> WaitOutcome {
        loop {
            if !self.lease.lifetime.is_valid.load(Ordering::Acquire) {
                return WaitOutcome::Reevaluate;
            }
            let signal = *self.receiver.borrow_and_update();
            match signal {
                Signal::Committed(provider_id) => {
                    return WaitOutcome::Bound(BoundLease {
                        lease: self.lease,
                        provider_id,
                    });
                }
                Signal::Released => return WaitOutcome::Reevaluate,
                Signal::Pending => {}
            }
            if self.receiver.changed().await.is_err() {
                let signal = *self.receiver.borrow();
                if let Signal::Committed(provider_id) = signal
                    && self.lease.lifetime.is_valid.load(Ordering::Acquire)
                {
                    return WaitOutcome::Bound(BoundLease {
                        lease: self.lease,
                        provider_id,
                    });
                }
                return WaitOutcome::Reevaluate;
            }
        }
    }

    /// Test-only independent follower identity.
    #[cfg(test)]
    fn token(&self) -> NonZeroU64 {
        self.lease.token
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.cache.lock();
        if let Some(key) = self.pending_key.take()
            && state.retries.get(&key).is_some_and(|entry| {
                matches!(entry.value, RetryValue::Pending { token, .. } if token == self.token)
            }) {
            state.remove_retry(key);
        }
        state.prune(Instant::now());
        state.retire_lifetime(&self.lifetime);
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
