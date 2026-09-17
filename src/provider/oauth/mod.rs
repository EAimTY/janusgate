//! OAuth authorization flows, token exchange, refresh leasing, and Google Code Assist project
//! resolution for OAuth-backed providers.
//!
//! The submodule layout mirrors the concerns: [`flow`] is the OAuth protocol (authorization begin,
//! code exchange, refresh grant), [`project`] is the Google Code Assist project onboarding,
//! [`refresh`] is the lease-guarded refresh loop, [`code_assist`] is the shared Antigravity Code
//! Assist transport, and [`token`] holds the JWT claim and token-lifetime helpers shared by the
//! flow and refresh paths.

use super::{http_body, http_body::ExchangeFailure, storage::Error as StorageError};

mod code_assist;
mod flow;
mod project;
mod refresh;
mod token;

pub(in crate::provider) use code_assist::post_code_assist;
pub(crate) use flow::{OAuthFlow, begin_flow, exchange_code};
pub(crate) use refresh::ensure_fresh_token;

/// OAuth token acquisition failures: authorization code exchange, refresh grants, Google Code
/// Assist project resolution, and the lease-guarded refresh loop over them.
///
/// Every upstream-facing variant renders the complete human message both HTTP surfaces display; the
/// surfaces only decide the status and the machine-readable code. Upstream rejections on these
/// paths always pin 502, so the foreign status rides in the message and the variant payload.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The upstream endpoint answered a non-2xx status. These paths never forward foreign statuses,
    /// so the status rides in the message.
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

    /// The addressed provider row vanished mid-refresh.
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

/// Converts the shared upstream exchange failure into the generic upstream-facing variants: the
/// rejection detail is the body excerpt under the one truncation policy
/// ([`http_body::truncate_excerpt`]). The token-grant paths classify the untruncated body
/// themselves and do not go through this conversion.
impl From<ExchangeFailure> for Error {
    fn from(failure: ExchangeFailure) -> Self {
        match failure {
            ExchangeFailure::Rejected { status, body } => Self::Rejected {
                status,
                detail: http_body::truncate_excerpt(&body),
            },

            ExchangeFailure::Unavailable { detail } => Self::Unavailable { detail },
        }
    }
}
