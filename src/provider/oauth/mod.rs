//! OAuth authorization flows: flow begin and completion, the token exchange with terminal-code
//! classification, the lease-guarded renewal, the background device poller, and the shared
//! exchange-to-provider-write completion pipeline.
//!
//! The submodule layout mirrors the concerns: [`flow`] is the grant mechanics (authorize-URL
//! building, device authorization, token exchange, expiry resolution), [`completion`] is the
//! pipeline both completion surfaces share (the exchange folded through provisioning into the
//! stored credentials, and the provider write), [`provisioning`] is the ordered
//! control-request sequence a completed exchange — and a reprovisioning renewal — folds into
//! the working state, [`refresh`] is the lease-guarded renewal loop over the definition's
//! refresh mode, and [`poller`] is the background scheduler resuming every unexpired pending
//! device flow from its stored schedule.
//!
//! Every provider-specific fact lives in the definition the flow references; the engines carry
//! the grant mechanics alone and never special-case a provider.

use crate::storage::Error as StorageError;

mod completion;
mod flow;
mod poller;
mod provisioning;
mod refresh;

pub(crate) use completion::{complete_exchange, completed_outcome, write_completion_provider};
pub(crate) use flow::{
    AUTHORIZATION_CODE_TTL, DEFAULT_DEVICE_INTERVAL_SECONDS, build_authorize_url,
    execute_device_authorization, execute_exchange, initialize_slots, random_url_safe,
    seconds_from_now,
};
#[cfg(test)]
pub(crate) use flow::{DeviceAuthorization, ExchangeOutcome};
pub(crate) use poller::spawn_device_poller;
#[cfg(test)]
pub(crate) use provisioning::{ProvisioningEnv, WorkingState, run_steps};
pub(crate) use refresh::ensure_fresh_token;

/// OAuth token acquisition failures: the device authorization, token exchange, refresh grants,
/// reprovisioning, and the lease-guarded renewal loop over them.
///
/// Every upstream-facing variant renders the complete human message both HTTP surfaces display;
/// the surfaces only decide the status and the machine-readable code. Upstream rejections on
/// these paths always pin 502, so the foreign status rides in the message and the variant
/// payload.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum Error {
    /// The upstream endpoint answered a non-2xx status. These paths never forward foreign
    /// statuses, so the status rides in the message.
    #[error("Upstream request failed with status {status}: {detail}")]
    Rejected {
        /// Upstream status code, embedded in the message.
        status: u16,

        /// Truncated upstream body excerpt.
        detail: String,
    },

    /// The upstream endpoint failed without a usable HTTP response: a transport failure, a
    /// malformed body, or a missing required field.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// The upstream endpoint was cut off by the HTTP client's timeout budget and produced no
    /// usable response. Rendered exactly like every other transport failure; carried typed so
    /// the device poller's backoff can match the fact instead of parsing the message.
    #[error("Upstream request failed: {detail}")]
    Timeout {
        /// Human-readable failure detail.
        detail: String,
    },

    /// The addressed provider row vanished mid-refresh. Every management consumer classifies
    /// this as a 404 `not_found` — the reauthorize surface's vanished-provider precedent, which
    /// `models:discover` now shares because its refresh runs inside this engine; the dispatch
    /// surface renders it as an internal failure.
    #[error("{message}")]
    NotFound {
        /// Human-readable failure message.
        message: String,
    },

    /// Token acquisition failed on JanusGate's side: a lease transaction could not be driven, or
    /// storage reported drift.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message.
        message: String,
    },
}

impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound { message } => Self::NotFound { message },

            // Constraint and busy contention cannot arise from the refresh path's unconditional
            // writes; the surfaces render them as gateway-internal failures.
            other => Self::Internal {
                message: other.to_string(),
            },
        }
    }
}

impl From<crate::database::Error> for Error {
    fn from(error: crate::database::Error) -> Self {
        Self::Internal {
            message: error.to_string(),
        }
    }
}

impl From<crate::provider::control::ControlFailure> for Error {
    fn from(failure: crate::provider::control::ControlFailure) -> Self {
        match failure {
            crate::provider::control::ControlFailure::Rejected { status, body } => Self::Rejected {
                status,
                detail: crate::http_body::truncate_excerpt(&body),
            },

            crate::provider::control::ControlFailure::Unavailable { detail } => {
                Self::Unavailable { detail }
            }

            crate::provider::control::ControlFailure::Timeout { detail } => {
                Self::Timeout { detail }
            }
        }
    }
}
