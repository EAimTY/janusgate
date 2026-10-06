//! The provisioning descriptor: the optional ordered control-request sequence a provider
//! definition runs after a successful token exchange — and, for the reprovisioning refresh mode,
//! under the refresh lease (README §5.4).

use super::template::{
    ControlRequest, ExtractionShape, ResponseEncoding, ResponsePath, ResponseTarget,
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
/// One provisioning step (README §5.4): one control request, the extractions folding its
/// response into the flow's working state, an optional slot guard, and an optional long-running
/// operation poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ProvisioningStep {
    /// The control request executing the step.
    pub request: ControlRequest,

    /// The wire encoding the step response arrives in.
    pub response_encoding: ResponseEncoding,

    /// The extractions folding the response into the flow's working slots, tokens, or base URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<ProvisionExtraction>,

    /// The slot whose absence runs this step: when the named slot is present the step is skipped,
    /// making it the conditional arm of the sequence — Antigravity's onboarding runs only when
    /// `loadCodeAssist` placed no project slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_if: Option<String>,

    /// The optional long-running operation the step starts and the engine polls until done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<Poll>,
}

/// One provisioning extraction: the value at `path` is folded into `target` when present. A
/// missing required path fails the step; a missing optional path leaves its target unchanged; a
/// present value of the wrong declared shape always fails (README §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ProvisionExtraction {
    /// The closed path of the response value.
    pub path: ResponsePath,

    /// Where the extracted value is stored.
    pub target: ResponseTarget,

    /// The declared shape of the value.
    pub shape: ExtractionShape,

    /// Whether a missing path fails the step (`true`) or leaves the target unchanged (`false`).
    pub required: bool,
}

/// The long-running operation poll of a provisioning step (README §5.4): the engine repeats the
/// configured request — `extract` runs before the poll, so its request can reference an operation
/// name placed in a slot — until the done path turns true, within the wall-clock budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Poll {
    /// The control request repeating the poll.
    pub request: ControlRequest,

    /// The wire encoding the poll response arrives in.
    pub response_encoding: ResponseEncoding,

    /// The path whose boolean marks the operation done: a non-empty member chain, whose empty
    /// spelling the write boundary rejects — it would address the response root, whose truthy
    /// read is always false, and the poll would end only at budget exhaustion.
    pub done_path: ResponsePath,

    /// The path carrying the terminal failure payload of a completed operation; the empty
    /// chain is the configured no-error-inspection, and the poll never fails on a payload
    /// under it (README §5.4).
    pub error_path: ResponsePath,

    /// The extractions folding the completed operation's response into the working state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub result_extract: Vec<ProvisionExtraction>,

    /// The delay between poll attempts, in milliseconds.
    pub interval_ms: NonZeroU64,

    /// The total wall-clock budget of the poll, in milliseconds, checked before every attempt.
    pub budget_ms: NonZeroU64,
}
