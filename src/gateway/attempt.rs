//! Single-account attempt ownership before the outward response task takes over.

use super::{Context, Error, IrEventStream, IrRequest, ModelRouting, execute};
use crate::{
    accounting::ApiLogOutcome,
    storage::api_logs::{ApiLogDraft, ApiLogStore, AttemptProgress, finalize_api_log},
};

/// Exactly one owner of an attempt's draft until finalization or response-task transfer.
/// Dropping an unfinished owner closes dispatched work as a client cancellation.
pub(crate) struct AccountAttempt {
    /// Present until an explicit consuming transfer; never copied for two finalizers.
    draft: Option<ApiLogDraft>,
    /// Store retained independently of the request handler's lifetime.
    store: Option<ApiLogStore>,
}

impl AccountAttempt {
    /// Starts an unfinalized attempt, including its predispatch cancellation window.
    fn new(context: &Context, draft: ApiLogDraft) -> Self {
        Self {
            draft: Some(draft),
            store: context.api_log_store.clone(),
        }
    }

    /// Borrows the untransferred facts for routing decisions.
    ///
    /// # Panics
    ///
    /// Panics if an already consumed owner is accessed, prevented by consuming transfers.
    pub(crate) fn draft(&self) -> &ApiLogDraft {
        #[expect(clippy::expect_used, reason = "transfers consume the attempt owner")]
        self.draft.as_ref().expect("an active owner has its draft")
    }

    /// Borrows the draft for one account's data-plane stages.
    ///
    /// # Panics
    ///
    /// Panics if an already consumed owner is accessed, prevented by consuming transfers.
    fn draft_mut(&mut self) -> &mut ApiLogDraft {
        #[expect(clippy::expect_used, reason = "transfers consume the attempt owner")]
        self.draft.as_mut().expect("an active owner has its draft")
    }

    /// Hands the draft to the existing response task, disabling cancellation finalization here.
    ///
    /// # Panics
    ///
    /// Panics if the draft was already handed away, prevented by consuming this owner.
    pub(crate) fn transfer(mut self) -> ApiLogDraft {
        #[expect(
            clippy::expect_used,
            reason = "a consuming transfer occurs exactly once"
        )]
        self.draft
            .take()
            .expect("the owner transfers its draft once")
    }

    /// Closes an abandoned or final failed attempt with the coordinator's chosen outward facts.
    /// The detached writer keeps ownership even if cancellation interrupts this await.
    pub(crate) async fn finalize(self, outcome: ApiLogOutcome, http_status: Option<i64>) {
        let store = self.store.clone();
        let draft = self.transfer();
        let task = tokio::spawn(async move {
            finalize_api_log(store.as_ref(), &draft, outcome, http_status, None).await;
        });
        let _ = task.await;
    }
}

impl Drop for AccountAttempt {
    fn drop(&mut self) {
        let Some(draft) = self.draft.take().filter(|draft| draft.started_at.is_some()) else {
            return;
        };
        let store = self.store.clone();
        tokio::spawn(async move {
            finalize_api_log(
                store.as_ref(),
                &draft,
                ApiLogOutcome::ClientAborted,
                None,
                None,
            )
            .await;
        });
    }
}

/// Typed result of exactly one account dispatch. No variant has finalized its draft.
pub(crate) enum AttemptOutcome {
    /// Preparation or required credential renewal failed before any data-plane stage.
    PredispatchFailure {
        /// Original failure, never authorization to select another account.
        error: Error,
        /// Unstamped owner; no accounting row exists.
        attempt: AccountAttempt,
    },
    /// Rejected headers with bounded, positively classified quota evidence when available.
    Rejected {
        /// Typed quota rejection or ordinary mirrored rejection.
        error: Error,
        /// Unfinalized owner carrying first headers and dispatch commitment.
        attempt: AccountAttempt,
    },
    /// A dispatched transport or storage failure with an ambiguous execution outcome.
    DispatchedFailure {
        /// Failure that ends account selection immediately.
        error: Error,
        /// Unfinalized owner; no invented successful usage.
        attempt: AccountAttempt,
    },
    /// Accepted headers commit the account before parsing or exposing events.
    Accepted {
        /// Parsed events, whose later errors never re-enter selection.
        events: IrEventStream,
        /// Owner to transfer exactly once to the result task.
        attempt: AccountAttempt,
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
    let mut attempt = AccountAttempt::new(context, draft);
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
