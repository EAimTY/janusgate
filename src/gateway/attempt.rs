//! Single-account dispatch outcomes before the outward response task takes over.

use super::{Context, Error, IrEventStream, IrRequest, ModelRouting, execute};
use crate::storage::api_logs::{ApiLogDraft, ApiLogGuard, AttemptProgress};

/// Typed result of exactly one account dispatch. No variant has finalized its draft.
pub(crate) enum AttemptOutcome {
    /// Preparation or required credential renewal failed before any data-plane stage.
    PredispatchFailure {
        /// Original failure, never authorization to select another account.
        error: Error,
        /// Unstamped owner; no accounting row exists.
        attempt: ApiLogGuard,
    },
    /// Rejected headers with bounded, positively classified quota evidence when available.
    Rejected {
        /// Typed quota rejection or ordinary mirrored rejection.
        error: Error,
        /// Unfinalized owner carrying first headers and dispatch commitment.
        attempt: ApiLogGuard,
    },
    /// A dispatched transport or storage failure with an ambiguous execution outcome.
    DispatchedFailure {
        /// Failure that ends account selection immediately.
        error: Error,
        /// Unfinalized owner; no invented successful usage.
        attempt: ApiLogGuard,
    },
    /// Accepted headers commit the account before parsing or exposing events.
    Accepted {
        /// Parsed events, whose later errors never re-enter selection.
        events: IrEventStream,
        /// Owner to hand to the result task exactly once.
        attempt: ApiLogGuard,
    },
}

/// Executes one physical account without routing recursion, retry, or final-error finalization.
pub(crate) async fn execute_attempt(
    context: &Context,
    routing: ModelRouting,
    ir: IrRequest,
    is_client_streaming: bool,
    draft: ApiLogDraft,
    affinity: Option<&mut crate::affinity::Ownership>,
) -> AttemptOutcome {
    let mut attempt = ApiLogGuard::new(context.api_log_store.clone(), draft);
    match execute(
        context,
        routing,
        ir,
        is_client_streaming,
        attempt.draft_mut(),
        None,
        affinity,
    )
    .await
    {
        Ok(events) => AttemptOutcome::Accepted { events, attempt },
        Err(error) if matches!(attempt.draft().progress, AttemptProgress::Predispatch) => {
            AttemptOutcome::PredispatchFailure { error, attempt }
        }
        Err(error @ (Error::Upstream { .. } | Error::QuotaRejected { .. })) => {
            AttemptOutcome::Rejected { error, attempt }
        }
        Err(error) => AttemptOutcome::DispatchedFailure { error, attempt },
    }
}
