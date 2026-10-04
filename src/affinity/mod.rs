//! Content-free pool affinity: canonical fingerprint identities and bounded runtime hints.

use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

mod cache;
pub(crate) mod fingerprint;
pub(crate) use cache::{BoundLease, Cache, Decision, FreshLease, WaitOutcome};
pub(crate) use fingerprint::initialize_key;

/// An opaque process-keyed fingerprint; never a client identifier or a disk value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Digest(pub(in crate::affinity) [u8; 32]);

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
pub(crate) enum Projection {
    /// Calls/results use their canonical relationship ordinals.
    Relational,
    /// Actual wire call IDs and corresponding references participate.
    Exact,
}

/// One completed canonical-unit prefix supplied by a client-codec projector.
#[derive(Clone, Copy)]
pub(crate) struct HistoryPrefix {
    /// Exact or relational identity semantics.
    pub(crate) projection: Projection,
    /// Completed prefix chain digest.
    pub(crate) digest: Digest,
    /// Positive number of completed canonical units, used for longest-prefix ranking.
    pub(crate) units: NonZeroU64,
}

/// Complete request identity, exact when available and otherwise relational.
#[derive(Clone, Copy)]
pub(crate) struct RetryFingerprint {
    /// Identity semantics of this complete projection.
    pub(crate) projection: Projection,
    /// Whole accepted active-history digest; absent for an unresolved suffix.
    pub(crate) digest: Digest,
}

/// Active evidence after target-independent state authentication and boundary planning.
#[derive(Default)]
pub(crate) struct Evidence {
    /// Canonical active-history prefixes; superseded history is absent.
    pub(crate) history: Vec<HistoryPrefix>,
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

    /// Enables actual answer projection, omitting an already prepared outward checkpoint.
    pub(crate) fn begin_projection(&self, renderer: &mut dyn crate::format::ClientSseRenderer) {
        if self.is_checkpoint {
            renderer.begin_checkpoint_projection(self.continuation());
        } else {
            renderer.begin_projection(self.continuation());
        }
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

    /// Registers a newly completed actual native artifact at its own outward frame boundary.
    pub(crate) fn register_released_artifact(&self, anchors: &[Anchor]) {
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

    /// Registers only a completed actual JSON rendering that extends the accepted history.
    pub(crate) fn complete_json<F: crate::format::ClientFormat>(
        &mut self,
        response: &F::JsonResponse,
    ) {
        self.complete_artifacts();
        self.binding
            .register(&F::completed_json_artifacts(response));
        let before = self.projector.evidence().retry.map(|retry| retry.digest);
        let is_replayable = if self.is_checkpoint {
            F::project_json_checkpoint(response, &mut self.projector)
        } else {
            F::project_json_response(response, &mut self.projector)
        };
        if is_replayable && before != self.projector.evidence().retry.map(|retry| retry.digest) {
            self.binding.register(&self.projector.completed_anchors());
        }
    }
}
