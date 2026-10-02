//! Remote compaction: the gateway's fulfillment of client-directed context
//! compaction requests.
//!
//! A client using the Responses or Messages protocol can request compaction
//! even when its selected upstream cannot perform native compaction. The
//! gateway fulfills that request using ordinary inference on the same
//! provider/model and returns a replayable gateway artifact in the client's
//! compaction protocol, so the client continues using its existing
//! remote-compaction state machine.
//!
//! The feature is client-directed: the gateway never adds compaction to an
//! ordinary request unless the client supplied an explicit compact
//! operation/trigger or explicitly enabled threshold compaction. Decoding an
//! existing artifact is not a new compaction and never triggers a summary
//! call.
//!
//! The module decomposes into:
//!
//! - [`mechanism`]: the closed vocabulary of the five compaction mechanisms and their support
//!   states.
//! - [`support_facts`]: the per-model mechanism-to-support fact map and its three-layer resolution.
//! - [`identity`]: stable item identities used to detect overlapping history across a compaction
//!   boundary.
//! - [`envelope`]: the versioned gateway envelope holding a gateway summary or a wrapped native
//!   window.
//! - [`sealing`]: authenticated encryption of envelopes into the namespaced gateway carrier format.
//! - [`error`]: the typed compaction failures and their stable message codes.
//! - [`accounting`]: the per-request ledger of dispatched calls.
//! - [`threshold`]: the shared context estimator and the documented threshold defaults.
//! - [`history`]: boundary preparation, window restoration, checkpoint planning, and sealing.
//! - [`summary`]: the default summarization prompt, the summarizer planner, and summary validity.
//! - [`native`]: the native compaction operation dispatches.
//! - [`executor`]: strategy selection and multi-call coordination for compaction operations.

pub(crate) mod accounting;
pub(crate) mod envelope;
pub(crate) mod error;
pub(crate) mod executor;
pub(crate) mod history;
pub(crate) mod identity;
pub(crate) mod mechanism;
pub(crate) mod native;
pub(crate) mod sealing;
pub(crate) mod summary;
pub(crate) mod support_facts;
pub(crate) mod threshold;

#[cfg(test)]
mod tests;
