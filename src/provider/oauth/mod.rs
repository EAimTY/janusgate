//! OAuth authorization flows, token exchange, refresh leasing, and Antigravity project
//! resolution for OAuth-backed providers.
//!
//! The submodule layout mirrors the concerns: [`flow`] is the OAuth protocol (authorization begin,
//! code exchange, refresh grant), [`project`] is the Antigravity project onboarding,
//! [`refresh`] is the lease-guarded refresh loop, [`antigravity`] is the shared Antigravity
//! transport, and [`token`] holds the JWT claim and token-lifetime helpers shared by the
//! flow and refresh paths.
//!
//! # Reference implementations
//!
//! Both provider flows mirror their canonical clients wire-for-wire, pinned below; the gateway
//! speaks the same protocol so the same upstream accepts it. Deliberate divergences are
//! documented where they occur, and every mirrored fact was verified live against the real
//! endpoints.
//!
//! ## OpenAI Codex (`codex_oauth`)
//!
//! Mirrors `openai/codex` at rev `fcf05456bb27e6c3d5677550f54011db6a2a0817` (main, 2026-09-17):
//! `codex-rs/login/src/server.rs` (authorize URL, code exchange, token-endpoint error shapes),
//! `codex-rs/login/src/pkce.rs` (PKCE generation), `codex-rs/login/src/token_data.rs` (identity
//! claims), `codex-rs/login/src/auth/manager.rs` (refresh grant, terminal refresh codes).
//!
//! End to end: [`flow::begin_flow`] mints the PKCE pair and the authorize URL the administrator
//! opens; the provider redirects to the loopback callback with a single-use code, which the
//! administrator pastes back; [`flow::exchange_code`] redeems it for the token set and reads the
//! ChatGPT account id from the identity token; [`refresh::ensure_fresh_token`] renews the access
//! token under the database lease once it expires.
//!
//! ## Google Antigravity (`antigravity_oauth`)
//!
//! Mirrors `NoeFabris/opencode-antigravity-auth` at rev `16e0056` (2026-08-28), the most widely
//! ported open implementation of the IDE's flow: `src/constants.ts` (client, scopes, redirect,
//! endpoints, headers), `src/antigravity/oauth.ts` (authorize, exchange, project lookup),
//! `src/plugin/project.ts` (onboarding), `src/plugin/quota.ts` (`fetchAvailableModels`),
//! `src/plugin/request.ts` (inference envelope and headers). Google's own `google-antigravity/*`
//! repositories carry no OAuth login code (verified by inspection), so the port stands in as the
//! reference.
//!
//! End to end: [`flow::begin_flow`] mints the authorize URL (client-secret style, no PKCE); the
//! pasted code is redeemed by [`flow::exchange_code`], which resolves the Antigravity project id
//! via [`project::resolve_project_id`] (`loadCodeAssist`, then `onboardUser`); the credentials
//! then serve inference and [`refresh::ensure_fresh_token`] renews them under the database
//! lease.

use super::{http_body, http_body::ExchangeFailure};
use crate::storage::Error as StorageError;

mod antigravity;
mod flow;
mod project;
mod refresh;
mod token;

pub(in crate::provider) use antigravity::post_antigravity;
pub(crate) use flow::{OAuthFlow, begin_flow, exchange_code};
pub(crate) use refresh::ensure_fresh_token;

/// OAuth token acquisition failures: authorization code exchange, refresh grants, Antigravity
/// project resolution, and the lease-guarded refresh loop over them.
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
