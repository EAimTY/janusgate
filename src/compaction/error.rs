//! Typed remote-compaction errors (`docs/design/compaction.md`): one variant per failure
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
//! matching variant, and the three stable codes shared with the boundary leaf are
//! declared once as [`BoundaryError`]'s code constants and consumed by this
//! module's rendering, so both failure paths spell each shared code identically.
//! Refusal and documented native no-summary outcomes are not
//! errors at all — they are protocol outcomes the executor models separately.

use crate::{compaction_model::boundary::BoundaryError, gateway::Error as GatewayError};
use thiserror::Error;

/// A remote-compaction failure.
#[derive(Debug, Error)]
pub(super) enum CompactionError {
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
    #[error("{}: {message}", BoundaryError::INACCESSIBLE_STATE_CODE)]
    InaccessibleNativeState {
        /// Human-readable failure detail.
        message: String,
    },

    /// A gateway carrier failed verification: unknown key, malformed carrier,
    /// failed authentication, or a mismatched visible-content binding. A
    /// recognizable but damaged marker is an explicit client-state error, never
    /// reclassified as native state and never summarized.
    #[error("{}: {message}", BoundaryError::INVALID_GATEWAY_STATE_CODE)]
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
    #[error("{}: {message}", BoundaryError::CONFIGURATION_MISSING_CODE)]
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
    pub(super) fn unusable_history(message: impl Into<String>) -> Self {
        Self::UnusableHistory {
            message: message.into(),
        }
    }

    /// Builds the inaccessible-native-state failure.
    pub(super) fn inaccessible_native_state(message: impl Into<String>) -> Self {
        Self::InaccessibleNativeState {
            message: message.into(),
        }
    }

    /// Builds the invalid-gateway-state failure.
    pub(super) fn invalid_gateway_state(message: impl Into<String>) -> Self {
        Self::InvalidGatewayState {
            message: message.into(),
        }
    }

    /// Builds the unusable-summary failure.
    pub(super) fn unusable_summary(reason: impl Into<String>) -> Self {
        Self::UnusableSummary {
            reason: reason.into(),
        }
    }

    /// Builds the budget-exceeded failure.
    pub(super) fn budget_exceeded(message: impl Into<String>) -> Self {
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::CompactionError;
    use crate::{
        compaction_model::{boundary::BoundaryError, sealing::UnsealError},
        gateway::Error as GatewayError,
    };
    use reqwest::StatusCode;
    use std::error::Error as StdError;

    /// The text before the first colon of a failure's rendering: its stable code.
    fn stable_code(error: &CompactionError) -> String {
        error
            .to_string()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Every client-state failure renders its documented stable code — the shared boundary codes
    /// spelled through the boundary vocabulary's constants — and converts into an invalid-request
    /// gateway error carrying that code, the 400 taxonomy the failures table pins.
    #[test]
    fn client_state_failures_render_their_stable_codes_as_invalid_requests()
    -> Result<(), Box<dyn StdError>> {
        let failures = [
            (
                CompactionError::unusable_history("an orphan tool result"),
                "compaction_unusable_history",
            ),
            (
                CompactionError::inaccessible_native_state("another producer's state"),
                BoundaryError::INACCESSIBLE_STATE_CODE,
            ),
            (
                CompactionError::invalid_gateway_state("a tampered carrier"),
                BoundaryError::INVALID_GATEWAY_STATE_CODE,
            ),
            (
                CompactionError::budget_exceeded("no older prefix remains"),
                "compaction_budget_exceeded",
            ),
            (
                CompactionError::UnsupportedMeasurement,
                "compaction_unsupported_measurement",
            ),
            (
                CompactionError::UnknownCapacity {
                    model: String::from("public-model"),
                },
                "compaction_unknown_capacity",
            ),
        ];

        for (failure, code) in failures {
            assert_eq!(stable_code(&failure), code, "{failure}");

            assert!(
                failure.to_string().contains(code),
                "the stable code rides the rendered message"
            );

            match GatewayError::from(failure) {
                GatewayError::InvalidRequest(_) => {}

                other => {
                    return Err(format!(
                        "a client-state failure is an invalid request, got {other:?}"
                    )
                    .into());
                }
            }
        }

        let capacity = CompactionError::UnknownCapacity {
            model: String::from("public-model"),
        };

        assert!(
            capacity.to_string().contains("public-model"),
            "the unknown-capacity failure names the public model"
        );

        Ok(())
    }

    /// The non-client failures map onto their own taxonomy entries: an unusable summary is an
    /// upstream-unavailable failure (the dispatch happened, the result is unusable), the missing
    /// sealing configuration is the server's internal gap, and a dispatched upstream failure
    /// passes through unchanged with its own semantics.
    #[test]
    fn non_client_failures_keep_their_own_taxonomy_entries() -> Result<(), Box<dyn StdError>> {
        let summary = CompactionError::unusable_summary("the summary was truncated");

        assert_eq!(stable_code(&summary), "compaction_summary_unusable");

        match GatewayError::from(summary) {
            GatewayError::Unavailable { detail } => {
                assert!(detail.contains("compaction_summary_unusable"));
            }

            other => {
                return Err(format!("an unusable summary is unavailable, got {other:?}").into());
            }
        }

        match GatewayError::from(CompactionError::ConfigurationMissing {
            message: String::from("no sealing keys"),
        }) {
            GatewayError::Internal { message } => {
                assert_eq!(message, "no sealing keys");
            }

            other => {
                return Err(format!("a configuration gap is internal, got {other:?}").into());
            }
        }

        let upstream = GatewayError::Upstream {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail: String::from("overloaded"),
        };

        assert!(matches!(
            GatewayError::from(CompactionError::Upstream(upstream)),
            GatewayError::Upstream { .. }
        ));

        Ok(())
    }

    /// The unseal vocabulary maps losslessly onto the boundary semantics: a carrier naming a key
    /// the configuration no longer knows, a malformed carrier, and failed authentication are all
    /// client-state invalid-gateway-state failures — removing a key invalidates its artifacts —
    /// while the missing sealing configuration stays the server's own gap.
    #[test]
    fn unseal_failures_map_onto_the_boundary_semantics() -> Result<(), Box<dyn StdError>> {
        for unseal in [
            UnsealError::UnknownKey,
            UnsealError::Malformed,
            UnsealError::AuthenticationFailed,
        ] {
            let mapped = CompactionError::from(unseal);

            assert_eq!(
                stable_code(&mapped),
                BoundaryError::INVALID_GATEWAY_STATE_CODE,
                "carrier state the client presented is always invalid gateway state"
            );
        }

        let missing = CompactionError::from(UnsealError::ConfigurationMissing);

        assert_eq!(
            stable_code(&missing),
            BoundaryError::CONFIGURATION_MISSING_CODE,
            "the absent sealing configuration stays the configuration-missing failure"
        );

        Ok(())
    }
}
