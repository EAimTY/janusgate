//! Request accounting: the `api_logs` store — its write path and read paths (README §3
//! *ApiLog Store*).
//!
//! One row is one dispatched physical-account attempt — the instant the upstream HTTP request was
//! issued (`send()`). Everything before that instant (authentication, routing, token refresh,
//! upstream payload rendering) is never recorded, and no content — prompt, completion, headers,
//! upstream error bodies — ever reaches the table. The rows live outside the main database: one
//! SQLite (Turso) file per UTC calendar day of `started_at`, created lazily on first write, written
//! through a hot-file connection cache, and expired by whole-file deletion.
//!
//! The module splits by direction: [`store`] is the api-log store itself — the day mapping,
//! the hot connection cache, file creation, the retention sweep, and the INSERT write path — and
//! [`usage`] is the management read engine — the shared filter, the per-file queries, and the
//! aggregation merge. This module carries the write contract the pipeline sees: the draft
//! lifecycle ([`ApiLogDraft`]) with its predispatch construction, the cancellation-safe draft
//! owner ([`ApiLogGuard`]) that carries one attempt to its single finalization or transfer, and
//! finalization, either by an existing response owner ([`finalize_api_log`]) or a
//! cancellation-independent writer ([`spawn_api_log_finalization`]).
//!
//! Recording survives a departing client by settled boundary, not by accident: both response
//! finalization tasks own their request's draft, so a client disconnecting mid-generation records
//! `client_aborted` rather than losing the row. The one accepted edge is the non-streaming
//! disconnect *after* the rendered body has been handed to hyper — unobservable, and that row
//! records `completed` with the status it rendered (README §3 *ApiLog*).
//!
//! Storage failures classify through the shared storage error domain
//! ([`Error`](super::Error)): a constraint backstop classifies as a conflict on the management
//! taxonomy with no new mapping, and a recorded row's closed-vocabulary columns parse back as
//! drift rather than as a client fault. Every recording failure is logged and swallowed — it
//! never enters any HTTP error domain. The accounting vocabularies the rows and reads speak —
//! [`ApiLogEndpointType`](crate::accounting::ApiLogEndpointType),
//! [`ApiLogOutcome`](crate::accounting::ApiLogOutcome), and
//! [`UsageGroupBy`](crate::accounting::UsageGroupBy) — live in [`crate::accounting`].

mod store;
#[cfg(test)]
mod test_support;
mod usage;

pub(crate) use self::{
    store::ApiLogStore,
    usage::{
        ApiLog, UsageAggregate, UsageBucketKey, UsageFilter, UsageSelection, list_api_logs,
        query_usage,
    },
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    ir::IrUsage,
    unix_time,
};
use log::error;

#[cfg(test)]
mod attempt_tests;

/// Data-plane progress for one account attempt, independent of log recording.
#[derive(Debug, Clone, Default)]
pub(crate) enum AttemptProgress {
    /// No data-plane stage has been sent on this account.
    #[default]
    Predispatch,
    /// One or more stages have been sent; successful headers permanently commit the account.
    Dispatched {
        /// Positive number of data-plane dispatches in this attempt.
        count: std::num::NonZeroU32,
        /// Whether any stage received accepted headers.
        has_accepted: bool,
    },
}

/// One accounting row about to be inserted: every column but the server-assigned `id`.
#[derive(Debug, Clone)]
struct ApiLogWrite {
    /// Stable requested route identity, independent of the current physical target.
    model_route_id: i64,
    /// First data-plane header status; null means no response headers were received.
    upstream_http_status: Option<i64>,
    /// Dispatch-start instant in UTC microseconds; stamped the moment `send()` is called.
    started_at: i64,

    /// Row id of the authenticated gateway API key.
    api_key_id: i64,

    /// Row id of the routed provider.
    provider_id: i64,

    /// Row id of the routed model.
    model_id: i64,

    /// Outward completion endpoint the request arrived on.
    endpoint_type: ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    is_stream: bool,

    /// HTTP status delivered to the client; `NULL` exactly when no response was ever produced
    /// (README §3 *ApiLog*).
    http_status: Option<i64>,

    /// Recorded outcome of the request.
    outcome: ApiLogOutcome,

    /// Milliseconds between the dispatch stamp and the accounting instant.
    duration: i64,

    /// Tokens consumed by the input; `NULL` when the upstream reported no usage.
    input_tokens: Option<i64>,

    /// Tokens produced by the output, `reasoning_tokens` included; `NULL` without usage.
    output_tokens: Option<i64>,

    /// Input tokens served from the upstream prompt cache; `NULL` without that breakdown.
    cached_tokens: Option<i64>,

    /// Input tokens written to the upstream prompt cache; `NULL` without that breakdown.
    cache_write_tokens: Option<i64>,

    /// Output tokens spent on reasoning; `NULL` without that breakdown.
    reasoning_tokens: Option<i64>,
}

/// One dispatched request's accounting draft: every fact known before dispatch, carried through
/// [`crate::gateway::execute`] and closed exactly once by its current attempt or response-task
/// owner.
///
/// The module's single invariant: **`started_at` is stamped ⇔ the request was dispatched with
/// accounting enabled**. Who stamps, who finalizes, and whether a row is written are all decided
/// by that one variable — [`ApiLogDraft::stamp`] reads the live presence of the `api_logs`
/// configuration section, and [`finalize_api_log`] writes if and only if a stamp is present.
#[derive(Debug, Clone)]
pub(crate) struct ApiLogDraft {
    /// Stable requested public route identity.
    pub(crate) model_route_id: i64,
    /// First data-plane header status, preserved through same-account stage continuation.
    pub(crate) upstream_http_status: Option<i64>,
    /// Typed attempt commitment; recording disabled does not disable this state.
    pub(crate) progress: AttemptProgress,
    /// Dispatch-start instant in UTC microseconds; `None` until [`ApiLogDraft::stamp`] runs at the
    /// `send()` call, and permanently `None` when accounting is disabled.
    pub(crate) started_at: Option<i64>,

    /// Row id of the authenticated gateway API key.
    pub(crate) api_key_id: i64,

    /// Row id of the routed provider.
    pub(crate) provider_id: i64,

    /// Row id of the routed model.
    pub(crate) model_id: i64,

    /// Outward completion endpoint the request arrived on.
    pub(crate) endpoint_type: ApiLogEndpointType,

    /// Whether the client requested a streamed response.
    pub(crate) is_stream: bool,

    /// The billed usage stamped for the request, when some participating call's
    /// accounting reaches the row through no single upstream observation — a
    /// compaction request's ledger totals over its compaction calls. The
    /// finalization adds these totals to its own usage parameter, which covers
    /// the boundary's last call alone (README §3 *ApiLog*).
    pub(crate) billed_usage: Option<IrUsage>,
}

impl ApiLogDraft {
    /// Builds the draft from every fact known before dispatch: the authenticated key, the routed
    /// physical target, and the surface the request arrived on. Every dispatch-derived fact —
    /// the stamp, the progress, the first headers, and the billed usage — starts empty for
    /// [`ApiLogDraft::stamp`], the response observation, and the compaction ledger to fill.
    pub(crate) fn predispatch(
        api_key_id: i64,
        provider_id: i64,
        model_id: i64,
        model_route_id: i64,
        endpoint_type: ApiLogEndpointType,
        is_stream: bool,
    ) -> Self {
        Self {
            model_route_id,
            provider_id,
            model_id,
            api_key_id,
            endpoint_type,
            is_stream,
            upstream_http_status: None,
            progress: AttemptProgress::default(),
            started_at: None,
            billed_usage: None,
        }
    }

    /// Advances data-plane progress immediately before `send()`. The first stage fixes duration
    /// origin and the live recording switch: `started_at` is stamped only when `api_logs` is
    /// present then. Later stages preserve that origin and switch, including across reloads.
    /// Progress remains active when recording is disabled.
    pub(crate) fn stamp(&mut self) {
        let is_first_dispatch = matches!(self.progress, AttemptProgress::Predispatch);
        self.progress = match self.progress {
            AttemptProgress::Predispatch => AttemptProgress::Dispatched {
                count: std::num::NonZeroU32::MIN,
                has_accepted: false,
            },
            AttemptProgress::Dispatched {
                count,
                has_accepted,
            } => AttemptProgress::Dispatched {
                count: count.saturating_add(1),
                has_accepted,
            },
        };
        if is_first_dispatch && crate::CONFIG.api_logs.is_some() {
            self.started_at = Some(unix_time::unix_now_micros());
        }
    }

    /// Records the first data-plane headers and commits on any successful response.
    ///
    /// # Panics
    ///
    /// Panics if a response is observed before its data-plane dispatch was stamped.
    pub(crate) fn observe_response(&mut self, status: reqwest::StatusCode) {
        match &mut self.progress {
            AttemptProgress::Dispatched { has_accepted, .. } => {
                *has_accepted |= status.is_success();
            }
            #[expect(clippy::unreachable, reason = "headers require an already sent stage")]
            AttemptProgress::Predispatch => {
                unreachable!("response headers require a dispatched account attempt");
            }
        }
        self.upstream_http_status
            .get_or_insert(i64::from(status.as_u16()));
    }

    /// Whether a positively classified rejection came from this attempt's first unaccepted stage.
    pub(crate) fn can_abandon_quota_rejection(&self) -> bool {
        matches!(self.progress, AttemptProgress::Dispatched { count, has_accepted: false }
            if count.get() == 1)
    }

    /// Stamps the billed totals the row must record: the accounting the
    /// compaction executor computed over the request's compaction calls,
    /// which no single upstream usage observation carries. The finalization
    /// adds these totals to its own usage observation. Absence preserves unknown token facts,
    /// while an explicit zero measurement remains a reported total.
    pub(crate) fn stamp_billed_usage(&mut self, usage: Option<IrUsage>) {
        self.billed_usage = usage;
    }
}

/// Exactly one owner of an attempt's draft until finalization or transfer to a response task,
/// shared by every pipeline boundary — the account coordinator of an ordinary selection and the
/// compaction coordinators alike. Dropping an unfinished owner closes dispatched work as a client
/// cancellation.
///
/// The ownership policy:
///
/// - [`ApiLogGuard::draft`] and [`ApiLogGuard::draft_mut`] lend the untransferred draft into the
///   data-plane stages and routing decisions that stamp and observe it; the guard itself records
///   nothing.
/// - [`ApiLogGuard::take`] hands the draft to a response owner exactly once.
/// - [`ApiLogGuard::finalize`] closes a failed attempt with the boundary's chosen outward facts
///   through the detached writer, which keeps ownership even if cancellation interrupts its await.
/// - Dropping a guard that still holds a stamped draft records the row as a client cancellation
///   with no outward status; an unstamped draft — or one already taken or finalized — records
///   nothing, because [`spawn_api_log_finalization`] returns before spawning without the dispatch
///   stamp.
pub(crate) struct ApiLogGuard {
    /// The current attempt's draft; absent once taken or explicitly finalized.
    draft: Option<ApiLogDraft>,
    /// Store retained independently of the request handler's lifetime.
    store: Option<ApiLogStore>,
}

impl ApiLogGuard {
    /// Starts owning `draft`, retaining `store` independently of the request handler's lifetime —
    /// a departing client drops the handler, never the finalization.
    pub(crate) fn new(store: Option<ApiLogStore>, draft: ApiLogDraft) -> Self {
        Self {
            draft: Some(draft),
            store,
        }
    }

    /// Borrows the untransferred draft for routing decisions.
    ///
    /// # Panics
    ///
    /// Panics if the draft was already taken, prevented by exactly-once call sites.
    pub(crate) fn draft(&self) -> &ApiLogDraft {
        #[expect(clippy::expect_used, reason = "an active owner has its draft")]
        self.draft.as_ref().expect("an active owner has its draft")
    }

    /// Lends the draft into one attempt's data-plane stages, which stamp and observe it.
    ///
    /// # Panics
    ///
    /// Panics if the draft was already taken, prevented by exactly-once call sites.
    pub(crate) fn draft_mut(&mut self) -> &mut ApiLogDraft {
        #[expect(clippy::expect_used, reason = "an active owner has its draft")]
        self.draft.as_mut().expect("an active owner has its draft")
    }

    /// Hands the draft to a response owner, disabling cancellation finalization here.
    ///
    /// # Panics
    ///
    /// Panics if the draft was already handed away, prevented by exactly-once call sites.
    pub(crate) fn take(&mut self) -> ApiLogDraft {
        #[expect(
            clippy::expect_used,
            reason = "a response owner takes the draft exactly once"
        )]
        self.draft.take().expect("an active owner has its draft")
    }

    /// Closes a failed attempt with the boundary's chosen outward facts. The detached writer keeps
    /// ownership even if cancellation interrupts this await, and a draft a response owner already
    /// took finalizes nothing here.
    pub(crate) async fn finalize(mut self, outcome: ApiLogOutcome, http_status: Option<i64>) {
        if let Some(draft) = self.draft.take()
            && let Some(task) =
                spawn_api_log_finalization(self.store.clone(), draft, outcome, http_status, None)
        {
            let _ = task.await;
        }
    }
}

impl Drop for ApiLogGuard {
    /// Records a still-held stamped draft as a client cancellation with no outward status.
    fn drop(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        spawn_api_log_finalization(
            self.store.clone(),
            draft,
            ApiLogOutcome::ClientAborted,
            None,
            None,
        );
    }
}

/// Starts a cancellation-independent writer only when dispatch enabled recording for this draft.
/// The dispatch stamp, not the current configuration or store presence, decides whether to run:
/// a reload cannot retract an existing row, and a stamped draft without a startup store must still
/// log its missing-store diagnostic. Dropping the returned handle never cancels the writer.
///
/// # Panics
///
/// Panics if recording is enabled and this call runs outside the Tokio runtime.
pub(crate) fn spawn_api_log_finalization(
    store: Option<ApiLogStore>,
    draft: ApiLogDraft,
    outcome: ApiLogOutcome,
    http_status: Option<i64>,
    usage: Option<IrUsage>,
) -> Option<tokio::task::JoinHandle<()>> {
    draft.started_at?;
    Some(tokio::spawn(async move {
        finalize_api_log(store.as_ref(), &draft, outcome, http_status, usage).await;
    }))
}

/// Closes one dispatched request's accounting: builds the terminal write from `draft` plus the
/// finalization facts and inserts it into the draft's own day's file. Every call site runs at
/// the attempt's current exclusive finalization owner (README §3 *ApiLog*), so each
/// dispatched request is recorded exactly once.
///
/// This function never reports failure outward — an accounting INSERT failure must never enter
/// any HTTP error domain (README §8): it is logged at error level with the key, provider, and
/// model row ids for attribution and swallowed, leaving the served response and every later
/// database operation untouched. An unstamped draft — the request was never dispatched with
/// accounting enabled — performs no INSERT at all.
///
/// `duration` is the whole span from the dispatch stamp to this finalization instant, in
/// whole milliseconds: anything before dispatch (token refresh, payload rendering) is excluded by
/// construction. `usage` is the terminal usage report when one exists; without it the five token
/// columns stay `NULL`.
pub(crate) async fn finalize_api_log(
    store: Option<&ApiLogStore>,
    draft: &ApiLogDraft,
    outcome: ApiLogOutcome,
    http_status: Option<i64>,
    usage: Option<IrUsage>,
) {
    let Some(started_at) = draft.started_at else {
        return;
    };

    // A wall clock stepped backwards must not produce a negative duration.
    let duration = unix_time::unix_now_micros().saturating_sub(started_at) / 1000;

    // The row's totals are the sum of every accounted source: the compaction
    // executor's ledger totals over the compaction calls, plus the boundary's
    // own usage observation over the last call — a compact-and-continue
    // request's continuation — when it reported one. Either source alone
    // stands in as the total; `None` usage yields no fabricated counts.
    let usage = match (draft.billed_usage, usage) {
        (Some(billed), Some(observed)) => Some(billed.saturating_sum(observed)),

        (billed, observed) => billed.or(observed),
    };

    let (input_tokens, output_tokens, cached_tokens, cache_write_tokens, reasoning_tokens) =
        token_columns(usage);

    let write = ApiLogWrite {
        model_route_id: draft.model_route_id,
        upstream_http_status: draft.upstream_http_status,
        started_at,
        api_key_id: draft.api_key_id,
        provider_id: draft.provider_id,
        model_id: draft.model_id,
        endpoint_type: draft.endpoint_type,
        is_stream: draft.is_stream,
        http_status,
        outcome,
        duration,
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_write_tokens,
        reasoning_tokens,
    };

    let Some(store) = store else {
        // Reachable only when the `api_logs` section was added by a reload after a startup
        // without it: the stamp read the live switch, but the api-log store is constructed at
        // startup alone (README §8). The row is dropped with a logged attribution.
        error!(
            "api log recording is configured but the api-log store was not constructed at \
             startup; dropping the row for API key {} (provider {}, model {})",
            draft.api_key_id, draft.provider_id, draft.model_id,
        );

        return;
    };

    if let Err(failure) = store.record(write).await {
        error!(
            "failed to record api log for API key {} (provider {}, model {}): {failure}",
            draft.api_key_id, draft.provider_id, draft.model_id,
        );
    }
}

/// The five token columns of one accounting row, in column order; all `NULL` when the upstream
/// reported no usage.
type TokenColumns = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// Maps a terminal usage report onto the five token columns: no usage yields five `NULL`s, and a
/// breakdown the upstream did not report stays `NULL` beside its reported totals. Upstream token
/// counts are third-party integers — one beyond `i64` saturates instead of failing the
/// accounting.
fn token_columns(usage: Option<IrUsage>) -> TokenColumns {
    let Some(usage) = usage else {
        return (None, None, None, None, None);
    };

    let stored = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);

    (
        Some(stored(usage.input_tokens)),
        Some(stored(usage.output_tokens)),
        usage.cached_tokens.map(stored),
        usage.cache_write_tokens.map(stored),
        usage.reasoning_tokens.map(stored),
    )
}
