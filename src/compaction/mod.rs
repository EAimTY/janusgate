//! The compaction executor: the gateway's fulfillment of client-directed
//! context compaction requests.
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
//! This is the one module allowed to depend on the layers above the
//! [`compaction_model`] vocabulary it consumes — `gateway`, `api`, `provider`,
//! and `storage` — and it is the executor: strategy selection and multi-call
//! coordination for compaction operations. The module decomposes into:
//!
//! - [`error`]: the typed compaction failures and their stable message codes.
//! - [`threshold`]: the shared context estimator and the documented threshold defaults.
//! - [`history`]: window restoration, checkpoint planning, and sealing.
//! - [`summary`]: the default summarization prompt, the summarizer planner, and summary validity.
//! - [`native`]: the native compaction operation dispatches.
//! - [`executor`]: strategy selection and multi-call coordination for compaction operations.

mod error;
pub(crate) mod executor;
mod history;
pub(crate) mod native;
mod summary;
mod threshold;
