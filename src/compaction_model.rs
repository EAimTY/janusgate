//! The compaction domain model: the pure vocabulary every layer shares, kept
//! strictly below the executor that coordinates actual compaction work.
//!
//! This module is a dependency leaf. It may depend only on crate primitives —
//! [`crate::ir`] (the IR's compaction facts included), the configuration schema
//! and global loader, and the pure fingerprint primitives of
//! [`crate::affinity`] — beside the format area's parsed-request vocabulary
//! ([`crate::format::UpstreamFormat`], [`crate::format::CompactionBoundary`],
//! [`crate::format::NativeArtifact`], [`crate::format::ParsedCompaction`], and
//! [`crate::format::ParsedRequest`]). It never depends on `gateway`, `api`,
//! `provider`, `storage`, or the `compaction` executor. Storage, provider
//! definitions, discovery, the outward API surfaces, the gateway selection
//! pipeline, the configuration loader, and the client codecs all import this
//! model, while it imports none of them back.
//!
//! The module decomposes into:
//!
//! - [`mechanism`]: the closed vocabulary of the five compaction mechanisms and their support
//!   states.
//! - [`support_facts`]: the per-model mechanism-to-support fact map and its three-layer resolution.
//! - [`identity`]: stable item identities used to detect overlapping history across a compaction
//!   boundary, and the digest forms every carrier comparison uses.
//! - [`envelope`]: the versioned gateway envelope holding a gateway summary or a wrapped native
//!   window.
//! - [`sealing`]: authenticated encryption of envelopes into the namespaced gateway carrier format.
//! - [`boundary`]: target-independent boundary authentication, routing projection, and the native
//!   producer requirement, with the boundary failure type.
//! - [`accounting`]: the per-request ledger of dispatched calls.
//!
//! The coordination that dispatches calls, selects strategies, and maps
//! failures onto the gateway error taxonomy lives in [`crate::compaction`],
//! the only module allowed to depend on the layers above this model.

pub(crate) mod accounting;
pub(crate) mod boundary;
pub(crate) mod envelope;
pub(crate) mod identity;
pub(crate) mod mechanism;
pub(crate) mod sealing;
pub(crate) mod support_facts;
