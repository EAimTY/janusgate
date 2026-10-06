//! Typed remote-compaction errors (README §6.8): one variant per failure
//! decision, each carrying its stable message code into the outward error
//! envelopes.
//!
//! The mapping onto the shared gateway error taxonomy is total and single-sourced
//! in [`From<CompactionError> for crate::gateway::Error`]: client-state failures
//! render as invalid requests (400), a summary the upstream produced but the
//! gateway cannot accept renders as an upstream-unavailable failure (502), a
//! missing sealing configuration renders as an internal failure (500), and a
//! dispatched native operation's arbitrary failure passes through unchanged with
//! its existing upstream semantics. Boundary failures compose losslessly: the
//! executor elevates a [`BoundaryError`] raised before account selection onto the
//! matching variant. Refusal and documented native no-summary outcomes are not
//! errors at all — they are protocol outcomes the executor models separately.

use crate::{compaction_model::boundary::BoundaryError, gateway::Error as GatewayError};
use thiserror::Error;

/// A remote-compaction failure.
#[derive(Debug, Error)]
pub(crate) enum CompactionError {
    /// The request's history is structurally invalid — an orphan tool result or
    /// an interaction unit the gateway cannot indivisibly retain. The history is
    /// rejected without discarding content or fabricating state.
    #[error("compaction_unusable_history: {message}")]
    UnusableHistory {
        /// Human-readable failure detail.
        message: String,
    },

    /// Native state the selected target cannot access: a wrapped window whose
    /// producing format differs from the target, opaque state a restoration
    /// would silently lose, or a native result that failed its completion
    /// validation. Inaccessible state is never silently discarded.
    #[error("compaction_inaccessible_state: {message}")]
    InaccessibleNativeState {
        /// Human-readable failure detail.
        message: String,
    },

    /// A gateway carrier failed verification: unknown key, malformed carrier,
    /// failed authentication, or a mismatched visible-content binding. A
    /// recognizable but damaged marker is an explicit client-state error, never
    /// reclassified as native state and never summarized.
    #[error("compaction_invalid_gateway_state: {message}")]
    InvalidGatewayState {
        /// Human-readable failure detail.
        message: String,
    },

    /// The summary inference completed but its result is unusable — a refusal,
    /// truncation, an empty text, or a tool call. No successful artifact is
    /// fabricated from it, and no second attempt is made.
    #[error("compaction_summary_unusable: {reason}")]
    UnusableSummary {
        /// Why the summary cannot stand in for the covered history.
        reason: String,
    },

    /// The context cannot be measured for a threshold decision: it carries
    /// image content or opaque native state, and no native counter backs the
    /// measurement. The threshold was not evaluated, so the request fails
    /// rather than pretending it was.
    #[error(
        "compaction_unsupported_measurement: the request context carries content \
         that cannot be counted for the threshold decision"
    )]
    UnsupportedMeasurement,

    /// The gateway's declared-capacity default threshold needs the model's
    /// effective maximum input tokens, and no such fact is declared. A missing
    /// capacity is rejected rather than inventing a context window.
    #[error(
        "compaction_unknown_capacity: the model `{model}` declares no effective \
         maximum input tokens, which the gateway default threshold requires"
    )]
    UnknownCapacity {
        /// The public model the threshold was resolved for.
        model: String,
    },

    /// Mandatory state prevents any useful compaction: the retention budget
    /// leaves no older prefix to summarize. A clear failure is returned instead
    /// of a successful unchanged checkpoint.
    #[error("compaction_budget_exceeded: {message}")]
    BudgetExceeded {
        /// Human-readable failure detail.
        message: String,
    },

    /// No sealing keys are configured, so no gateway artifact can be produced
    /// or consumed. A server configuration gap, not a client fault.
    #[error("compaction_configuration_missing: {message}")]
    ConfigurationMissing {
        /// Human-readable failure detail.
        message: String,
    },

    /// A dispatched native operation failed with its existing upstream
    /// semantics — authentication, overload, network failure, or an arbitrary
    /// rejection the definition's classifier did not positively match. Such
    /// failures propagate unchanged; there is no semantic fallback.
    #[error(transparent)]
    Upstream(#[from] GatewayError),
}

impl CompactionError {
    /// Builds the invalid-history failure.
    pub(crate) fn unusable_history(message: impl Into<String>) -> Self {
        Self::UnusableHistory {
            message: message.into(),
        }
    }

    /// Builds the inaccessible-native-state failure.
    pub(crate) fn inaccessible_native_state(message: impl Into<String>) -> Self {
        Self::InaccessibleNativeState {
            message: message.into(),
        }
    }

    /// Builds the invalid-gateway-state failure.
    pub(crate) fn invalid_gateway_state(message: impl Into<String>) -> Self {
        Self::InvalidGatewayState {
            message: message.into(),
        }
    }

    /// Builds the unusable-summary failure.
    pub(crate) fn unusable_summary(reason: impl Into<String>) -> Self {
        Self::UnusableSummary {
            reason: reason.into(),
        }
    }

    /// Builds the budget-exceeded failure.
    pub(crate) fn budget_exceeded(message: impl Into<String>) -> Self {
        Self::BudgetExceeded {
            message: message.into(),
        }
    }
}

impl From<crate::compaction_model::sealing::UnsealError> for CompactionError {
    fn from(error: crate::compaction_model::sealing::UnsealError) -> Self {
        // The unseal failure semantics are single-sourced in the boundary
        // failure vocabulary.
        Self::from(BoundaryError::from(error))
    }
}

impl From<BoundaryError> for CompactionError {
    fn from(error: BoundaryError) -> Self {
        match error {
            BoundaryError::ConfigurationMissing { message } => {
                Self::ConfigurationMissing { message }
            }
            BoundaryError::InvalidGatewayState { message } => Self::InvalidGatewayState { message },
            BoundaryError::InaccessibleNativeState { message } => {
                Self::InaccessibleNativeState { message }
            }
        }
    }
}

impl From<CompactionError> for GatewayError {
    fn from(error: CompactionError) -> Self {
        match error {
            // Client-state failures render as invalid requests, their stable
            // code riding the rendered message.
            CompactionError::UnusableHistory { .. }
            | CompactionError::InaccessibleNativeState { .. }
            | CompactionError::InvalidGatewayState { .. }
            | CompactionError::BudgetExceeded { .. }
            | CompactionError::UnsupportedMeasurement
            | CompactionError::UnknownCapacity { .. } => {
                Self::InvalidRequest(crate::ir::InvalidRequest::request(error.to_string()))
            }

            // A summary the upstream produced but the gateway cannot accept is
            // an upstream-side failure: the compaction work was dispatched and
            // its result is unusable (502).
            CompactionError::UnusableSummary { .. } => Self::Unavailable {
                detail: error.to_string(),
            },

            // Configuration is the server's own gap; its detail reaches the
            // log record only, beside the stable code the error carries.
            CompactionError::ConfigurationMissing { message } => Self::Internal { message },

            CompactionError::Upstream(error) => error,
        }
    }
}
