//! Content-free pool affinity: canonical fingerprint identities and bounded runtime hints.

use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

mod cache;
pub(crate) mod fingerprint;
pub(crate) use cache::{BoundLease, Cache, Decision, FreshLease, WaitOutcome};
pub(crate) use fingerprint::initialize_key;

/// An opaque process-keyed fingerprint; never a client identifier or a disk value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Digest([u8; 32]);

/// Pool-wide account affinity, independent of public route, physical model, or result mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Namespace {
    /// Authenticated gateway key row identity.
    pub(crate) api_key_id: i64,
    /// Authored pool row identity.
    pub(crate) pool_id: i64,
}

/// Fingerprint identity strength; exact wire identities win equal-length matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Projection {
    /// Calls/results use their canonical relationship ordinals.
    Relational,
    /// Actual wire call IDs and corresponding references participate.
    Exact,
}

/// One completed canonical-unit prefix supplied by a client-codec projector.
#[derive(Clone, Copy)]
pub(crate) struct HistoryPrefix {
    /// Exact or relational identity semantics.
    projection: Projection,
    /// Completed prefix chain digest.
    digest: Digest,
    /// Positive number of completed canonical units, used for longest-prefix ranking.
    units: NonZeroU64,
}

/// Complete request identity, exact when available and otherwise relational.
#[derive(Clone, Copy)]
pub(crate) struct RetryFingerprint {
    /// Identity semantics of this complete projection.
    projection: Projection,
    /// Whole accepted active-history digest; absent for an unresolved suffix.
    pub(crate) digest: Digest,
}

/// Active evidence after target-independent state authentication and boundary planning.
#[derive(Default)]
pub(crate) struct Evidence {
    /// Canonical active-history prefixes; superseded history is absent.
    history: Vec<HistoryPrefix>,
    /// Complete request retry/concurrency identity, when fingerprintable.
    pub(crate) retry: Option<RetryFingerprint>,
    /// Latest usable authenticated carrier identity or canonical raw-artifact digest.
    pub(crate) artifact: Option<Digest>,
    /// Authoritative active native producer, whose compatibility is checked by routing.
    pub(crate) proven_provider: Option<i64>,
}

/// An actual completed outward history or replayable artifact eligible for registration.
#[derive(Clone, Copy)]
pub(crate) enum Anchor {
    /// A completed assistant/artifact history prefix, never a first user message alone.
    History(HistoryPrefix),
    /// A complete replayable artifact identity, possibly published before later generation fails.
    Artifact(Digest),
}

/// Request-owned account commitment, independent of cache residency.
pub(crate) enum Ownership {
    /// A direct route bypasses affinity.
    Direct,
    /// Only a fresh operation can move accounts before acceptance.
    Fresh(FreshLease),
    /// An accepted or previously established account cannot change.
    Bound(BoundLease),
}

impl Ownership {
    /// Whether this operation still permits positively classified quota failover.
    pub(crate) fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh(_))
    }

    /// Commits first successful data-plane headers before any body parsing or follower wake.
    pub(crate) fn accept(&mut self, provider_id: i64) {
        *self = match std::mem::replace(self, Self::Direct) {
            Self::Fresh(lease) => Self::Bound(lease.commit(provider_id)),
            established => established,
        };
    }
}

/// Accepted account and digest-only continuation owned by the actual outward render task.
pub(crate) struct Accepted {
    /// Account lease surviving cache eviction and fenced by namespace deletion.
    binding: BoundLease,
    /// Client-visible inbound tail, never expanded model-visible compaction content.
    projector: fingerprint::Projector,
    /// Independently complete artifact digests awaiting their actual outward artifact boundary.
    artifacts: Vec<Anchor>,
    /// Whether the projector already contains the single outward checkpoint prefix.
    is_checkpoint: bool,
}

impl Accepted {
    /// Transfers accepted account ownership from selection to result rendering.
    pub(crate) fn new(binding: BoundLease, projector: fingerprint::Projector) -> Self {
        Self {
            binding,
            projector,
            artifacts: Vec::new(),
            is_checkpoint: false,
        }
    }

    /// Carries a completed single-block prefix's artifact and window digests to actual delivery.
    pub(crate) fn completed_prefix(mut self, anchors: Vec<Anchor>) -> Self {
        self.artifacts = anchors;
        self
    }

    /// Replaces covered input with one actual checkpoint and transfers its completed anchors.
    pub(crate) fn completed_checkpoint(
        mut self,
        projector: fingerprint::Projector,
        anchors: Vec<Anchor>,
    ) -> Self {
        self.projector = projector;
        self.artifacts = anchors;
        self.is_checkpoint = true;
        self
    }

    /// Whether the accepted history's outward tail is one already prepared checkpoint, which
    /// the delivery side omits from projection.
    pub(crate) fn is_checkpoint(&self) -> bool {
        self.is_checkpoint
    }

    /// The accepted client-visible projection's digest-only state, for reads and folds by the
    /// delivery side's format glue.
    pub(crate) fn projector(&self) -> &fingerprint::Projector {
        &self.projector
    }

    /// The accepted projection's working state, folded by the delivery side's format glue.
    pub(crate) fn projector_mut(&mut self) -> &mut fingerprint::Projector {
        &mut self.projector
    }

    /// Returns the codec's actual completed-prefix boundary while prepared hints await release.
    pub(crate) fn prepared_prefix_boundary(&self, prefix_boundary: Option<usize>) -> Option<usize> {
        if self.artifacts.is_empty() {
            None
        } else {
            prefix_boundary
        }
    }

    /// Registers completed artifact metadata at its own successful outward artifact boundary.
    pub(crate) fn complete_artifacts(&mut self) {
        self.binding.register(&std::mem::take(&mut self.artifacts));
    }

    /// Registers completed artifact anchors at their own successful outward boundary.
    pub(crate) fn register_artifacts(&self, anchors: &[Anchor]) {
        self.binding.register(anchors);
    }

    /// Supplies digest-only accepted history to a codec's incremental outward projection.
    pub(crate) fn continuation(&self) -> fingerprint::Projector {
        self.projector.clone()
    }

    /// Registers a complete emitted stream that extends the accepted client-visible history.
    pub(crate) fn complete_projection(&mut self, projector: fingerprint::Projector) {
        let before = self.projector.evidence().retry.map(|retry| retry.digest);
        self.projector = projector;
        if before != self.projector.evidence().retry.map(|retry| retry.digest) {
            self.binding.register(&self.projector.completed_anchors());
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for request-owned account commitment and the accepted-render continuation handles:
    //! a fresh operation accepts exactly once, established and direct ownership ignore
    //! acceptance, and the accepted history's boundary decisions
    //! (`docs/design/routing.md` *Conversation affinity*).

    #![expect(
        clippy::missing_panics_doc,
        clippy::panic,
        reason = "a test fails by panicking; classification matches panic on the wrong decision \
                  and a # Panics section on every test would restate that"
    )]

    use super::{
        Accepted, Anchor, Cache, Decision, Digest, Evidence, FreshLease, Namespace, Ownership,
        Projection, RetryFingerprint,
        fingerprint::{Projector, Role},
    };
    use crate::test_support::{configuration_lock, install_configuration};
    use std::sync::Arc;

    /// A fixed process-local digest standing in for one projected conversation identity.
    fn digest() -> Digest {
        Digest([9; 32])
    }

    /// A complete exact-projection retry fingerprint over one fixed digest.
    fn retry_evidence() -> Evidence {
        Evidence {
            retry: Some(RetryFingerprint {
                projection: Projection::Exact,
                digest: digest(),
            }),
            ..Evidence::default()
        }
    }

    /// A namespace of the fixed test pool under the given API key.
    fn namespace(api_key_id: i64) -> Namespace {
        Namespace {
            api_key_id,
            pool_id: 70,
        }
    }

    /// The fresh lease of a fresh decision; a test expecting a fresh decision fails otherwise.
    fn fresh(decision: Decision) -> FreshLease {
        match decision {
            Decision::Fresh(lease) => lease,
            _ => panic!("the test expected a fresh decision"),
        }
    }

    /// The provider of a bound decision, and nothing for any other decision.
    fn bound_provider(decision: &Decision) -> Option<i64> {
        match decision {
            Decision::Bound(lease) => Some(lease.provider_id()),
            _ => None,
        }
    }

    /// A fresh operation accepts its committed account exactly once: acceptance turns fresh
    /// ownership bound, further acceptance is a no-op, and established or direct ownership never
    /// changes.
    #[test]
    fn fresh_ownership_accepts_once_and_established_ownership_ignores_acceptance() {
        let _guard = configuration_lock();

        install_configuration();

        let cache = Arc::new(Cache::default());
        let namespace = namespace(1);
        let evidence = retry_evidence();

        let mut ownership = match cache.acquire(namespace, &evidence) {
            Decision::Fresh(lease) => Ownership::Fresh(lease),
            _ => panic!("the test expected a fresh decision"),
        };

        assert!(ownership.is_fresh());
        ownership.accept(5);
        assert!(!ownership.is_fresh());

        let Ownership::Bound(bound) = &ownership else {
            panic!("acceptance binds fresh ownership");
        };

        assert_eq!(bound.provider_id(), 5);

        ownership.accept(6);

        let Ownership::Bound(bound) = &ownership else {
            panic!("established ownership stays bound");
        };

        assert_eq!(bound.provider_id(), 5);

        let mut direct = Ownership::Direct;

        assert!(!direct.is_fresh());
        direct.accept(1);

        assert!(matches!(direct, Ownership::Direct));
    }

    /// Acceptance commits the retry hint: a later lookup for the same request binds to the
    /// accepted account, so followers of a fresh leader share its establishment decision.
    #[test]
    fn acceptance_commits_the_retry_hint_for_later_lookups() {
        let _guard = configuration_lock();

        install_configuration();

        let cache = Arc::new(Cache::default());
        let namespace = namespace(1);
        let evidence = retry_evidence();

        let mut ownership = match cache.acquire(namespace, &evidence) {
            Decision::Fresh(lease) => Ownership::Fresh(lease),
            _ => panic!("the test expected a fresh decision"),
        };

        ownership.accept(5);

        assert_eq!(
            bound_provider(&cache.acquire(namespace, &evidence)),
            Some(5)
        );
    }

    /// The accepted history's prepared-prefix boundary is the codec's own boundary only once
    /// completed artifact anchors await release; without artifacts there is no boundary to
    /// report.
    #[test]
    fn accepted_reports_its_prepared_prefix_boundary_only_with_artifacts() {
        let _guard = configuration_lock();

        install_configuration();

        let cache = Arc::new(Cache::default());
        let lease = fresh(cache.acquire(namespace(1), &retry_evidence()));
        let accepted = Accepted::new(lease.commit(5), Projector::new());

        assert_eq!(accepted.prepared_prefix_boundary(Some(3)), None);
        assert_eq!(accepted.prepared_prefix_boundary(None), None);

        let anchored = accepted.completed_prefix(vec![Anchor::Artifact(digest())]);

        assert_eq!(anchored.prepared_prefix_boundary(Some(3)), Some(3));
        assert_eq!(anchored.prepared_prefix_boundary(None), None);
    }

    /// Completing the accepted projection registers its completed output anchors only when the
    /// projection's retry digest moved: re-completing the already-accepted history registers
    /// nothing, while a completed extension registers its new tail and binds later lookups to
    /// the accepted account.
    #[test]
    fn completed_projections_register_anchors_only_when_the_retry_digest_moves() {
        let _guard = configuration_lock();

        install_configuration();

        let cache = Arc::new(Cache::default());
        let namespace = namespace(1);

        let mut accepted_history = Projector::new();
        accepted_history.text(Role::User, b"prompt");
        accepted_history.text(Role::Assistant, b"answer");

        let lease = fresh(cache.acquire(namespace, &retry_evidence()));
        let mut accepted = Accepted::new(lease.commit(5), accepted_history.clone());

        // Re-completing the accepted history keeps the retry digest, so even a two-turn prefix
        // carrying completed assistant output stays unregistered and unbound.
        accepted.complete_projection(accepted_history.clone());

        assert_eq!(
            bound_provider(&cache.acquire(namespace, &accepted_history.evidence())),
            None
        );

        let mut extended = accepted_history.clone();
        extended.text(Role::User, b"follow-up");
        extended.text(Role::Assistant, b"reply");
        let extended_evidence = extended.evidence();

        accepted.complete_projection(extended);

        assert_eq!(
            bound_provider(&cache.acquire(namespace, &extended_evidence)),
            Some(5)
        );
    }
}
