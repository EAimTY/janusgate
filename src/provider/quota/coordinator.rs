//! Cancellation-resistant account quota refresh and live monotonic cadence.

use super::{Error, Observation, SafeTimestamp};
use crate::{
    database::Database,
    provider::Error as ProviderError,
    storage::{
        self,
        quota_blocks::{AccountQuotaState, QuotaBlock, QuotaFence},
    },
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::sync::watch;

/// Closed content-free failure categories shown by availability administration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RefreshFailure {
    /// Credential acquisition failed before the quota read.
    CredentialRenewal,
    /// The read exceeded the control-plane timeout.
    Timeout,
    /// The read failed without a usable upstream response.
    Transport,
    /// The upstream answered with non-success headers.
    UpstreamRejected,
    /// JSON decoding or atomic authored extraction failed.
    MalformedObservation,
}

/// Current-generation refresh metadata, containing no observation body or foreign values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LastRefresh {
    /// This process has no completed current-generation check.
    NotObserved,
    /// A full reading completed; a lost revision fence still records its cadence.
    Succeeded {
        /// UTC microseconds at query initiation.
        attempted_at: SafeTimestamp,
        /// UTC microseconds at successful JSON decoding.
        observed_at: SafeTimestamp,
    },
    /// A read failed and left established restrictions intact.
    Failed {
        /// UTC microseconds at query initiation.
        attempted_at: SafeTimestamp,
        /// Closed safe failure category.
        reason: RefreshFailure,
    },
    /// The current exact definition has no quota capability.
    Unsupported {
        /// UTC microseconds at the attempted capability check.
        attempted_at: SafeTimestamp,
    },
    /// An explicit administrator reset installed an unblocked grace period.
    Reset {
        /// UTC microseconds at reset completion.
        attempted_at: SafeTimestamp,
    },
}

/// A known applicable reset converted once into a monotonic deadline.
struct ResetDeadline {
    /// Exact restriction identity, permitting preservation on revision-only changes.
    key: Vec<String>,
    /// The absolute stored reset from which this deadline was derived.
    blocked_until: i64,
    /// Exact stored routing applicability.
    applies_to: super::QuotaApplicability,
    /// Monotonic recheck deadline; crossing it does not prove quota replenishment.
    deadline: Instant,
}

/// One generation's completed check and live cadence inputs.
struct MemoryRecord {
    /// Current authoritative interpretation and evidence order.
    fence: QuotaFence,
    /// Content-free completed check category.
    last_refresh: LastRefresh,
    /// Latest successful decoding time within this generation, retained through failed reads.
    last_observed_at: Option<SafeTimestamp>,
    /// Monotonic completion time, also the reset grace origin.
    completed_at: Option<Instant>,
    /// Current restrictions with known resets, interpreted at the latest evidence update.
    resets: Vec<ResetDeadline>,
}

impl MemoryRecord {
    /// Creates a generation with no completed automatic attempt.
    fn unobserved(fence: QuotaFence) -> Self {
        Self {
            fence,
            last_refresh: LastRefresh::NotObserved,
            last_observed_at: None,
            completed_at: None,
            resets: Vec::new(),
        }
    }

    /// Replaces reset evidence while retaining a due deadline for the same unchanged block.
    fn update_resets(&mut self, blocks: &[QuotaBlock], now: Instant, utc_now: i64) {
        let old = std::mem::take(&mut self.resets);
        self.resets = blocks
            .iter()
            .filter_map(|block| {
                let reset = block.blocked_until?;
                let deadline = old
                    .iter()
                    .find(|prior| prior.key == block.key && prior.blocked_until == reset)
                    .map(|prior| prior.deadline)
                    .or_else(|| {
                        let remaining = reset.checked_sub(utc_now)?;
                        let remaining = u64::try_from(remaining).ok().filter(|value| *value > 0)?;
                        now.checked_add(Duration::from_micros(remaining))
                    })?;
                Some(ResetDeadline {
                    key: block.key.clone(),
                    blocked_until: reset,
                    applies_to: block.applies_to.clone(),
                    deadline,
                })
            })
            .collect();
    }

    /// Recomputes the deadline from the current interval and applicable known resets.
    fn is_due(&self, now: Instant, interval: Duration, upstream_id: Option<&str>) -> bool {
        let Some(completed) = self.completed_at else {
            return true;
        };
        let deadline = self
            .resets
            .iter()
            .filter(|reset| {
                upstream_id.is_none_or(|model| reset.applies_to.is_applicable_to(model))
            })
            .map(|reset| reset.deadline)
            .fold(completed + interval, Instant::min);
        now >= deadline
    }
}

/// Shared outcome retained only for callers of an active read.
type FlightResult = Result<Observation, Error>;

/// One per-account occupied query slot, even when its interpretation generation becomes old.
struct Flight {
    /// Interpretation generation initiating this slot.
    generation: i64,
    /// Completion channel; a dropped caller never owns or cancels the query task.
    receiver: watch::Receiver<Option<Arc<FlightResult>>>,
}

/// One account's optional memory record and exclusive network slot.
#[derive(Default)]
struct Slot {
    /// Completed current-generation cadence/outcome.
    record: Option<MemoryRecord>,
    /// Exactly one query, including an old-generation query still completing.
    flight: Option<Flight>,
}

impl Slot {
    /// Installs grace only for the reset's own generation and before a newer completed read.
    /// Later derived evidence retains its blocks while sharing the reset's original cadence.
    fn install_reset(
        &mut self,
        reset_fence: QuotaFence,
        state: &AccountQuotaState,
        completed_at: Instant,
        attempted_at: SafeTimestamp,
    ) {
        let now = Instant::now();
        let utc_now = crate::unix_time::unix_now_micros();
        if !self.synchronize(state, now, utc_now)
            || state.fence.generation != reset_fence.generation
            || self
                .record
                .as_ref()
                .is_some_and(|record| record.completed_at.is_some())
        {
            return;
        }
        let mut record = MemoryRecord::unobserved(state.fence);
        record.last_refresh = LastRefresh::Reset { attempted_at };
        record.completed_at = Some(completed_at);
        record.update_resets(&state.blocks, now, utc_now);
        self.record = Some(record);
    }

    /// Updates committed evidence without reviving stale snapshots or resetting cadence.
    fn synchronize(&mut self, state: &AccountQuotaState, now: Instant, utc_now: i64) -> bool {
        if self.record.as_ref().is_some_and(|record| {
            record.fence.generation > state.fence.generation
                || (record.fence.generation == state.fence.generation
                    && record.fence.revision > state.fence.revision)
        }) {
            return false;
        }
        if self
            .record
            .as_ref()
            .is_none_or(|record| record.fence.generation != state.fence.generation)
        {
            self.record = Some(MemoryRecord::unobserved(state.fence));
        }
        if let Some(record) = self.record.as_mut()
            && (record.fence.revision != state.fence.revision || record.completed_at.is_none())
        {
            record.update_resets(&state.blocks, now, utc_now);
            record.fence = state.fence;
        }
        true
    }
}

/// Shared account quota coordinator, cloned through the router context as one `Arc`.
#[derive(Default)]
pub(crate) struct Coordinator {
    /// Short synchronous metadata critical sections; no database or network await holds it.
    slots: Mutex<HashMap<i64, Slot>>,
}

/// A selection action made while holding only the short memory metadata lock.
enum Action {
    /// An automatic check is still fresh, including explicit reset grace.
    Cached,
    /// Wait for a slot; only a current-generation slot satisfies this caller's check.
    Wait {
        /// Cloned completion subscription, independent of every waiting caller's cancellation.
        receiver: watch::Receiver<Option<Arc<FlightResult>>>,
        /// Whether this read's generation still satisfies the caller's committed snapshot.
        is_current: bool,
    },
    /// Start the query after releasing the metadata lock.
    Start {
        /// Completion publisher owned by the detached task.
        sender: watch::Sender<Option<Arc<FlightResult>>>,
        /// Completion subscription for this initiating caller.
        receiver: watch::Receiver<Option<Arc<FlightResult>>>,
    },
    /// The just-loaded database snapshot lost to a newer committed memory record.
    Reload,
}

impl Coordinator {
    /// Takes the metadata lock; poisoning indicates a fatal first-party invariant failure.
    ///
    /// # Panics
    ///
    /// Panics if another task panicked while mutating shared quota metadata.
    fn slots(&self) -> MutexGuard<'_, HashMap<i64, Slot>> {
        #[expect(
            clippy::expect_used,
            reason = "poisoned quota metadata is a fatal invariant failure"
        )]
        let slots = self.slots.lock().expect("quota metadata is not poisoned");
        slots
    }

    /// Reads cached safe metadata without performing a quota query.
    #[cfg(test)]
    fn last_refresh(&self, provider_id: i64) -> LastRefresh {
        self.slots()
            .get(&provider_id)
            .and_then(|slot| slot.record.as_ref())
            .map_or(LastRefresh::NotObserved, |record| {
                record.last_refresh.clone()
            })
    }

    /// Loads committed restrictions and refresh metadata without upstream I/O.
    ///
    /// # Errors
    ///
    /// Returns database failures; absent accounts return `None`.
    pub(crate) async fn availability(
        &self,
        database: &Database,
        provider_id: i64,
    ) -> Result<Option<(Vec<QuotaBlock>, LastRefresh)>, storage::Error> {
        loop {
            let Some(state) = storage::quota_blocks::load_state(database, provider_id).await?
            else {
                self.forget(provider_id);
                return Ok(None);
            };
            let mut slots = self.slots();
            let slot = slots.entry(provider_id).or_default();
            if !slot.synchronize(&state, Instant::now(), crate::unix_time::unix_now_micros()) {
                continue;
            }
            let refresh = slot
                .record
                .as_ref()
                .map_or(LastRefresh::NotObserved, |record| {
                    record.last_refresh.clone()
                });
            return Ok(Some((state.blocks, refresh)));
        }
    }

    /// Drops deleted-account metadata, retaining only an occupied old query slot until completion.
    pub(crate) fn forget(&self, provider_id: i64) {
        let mut slots = self.slots();
        if slots
            .get(&provider_id)
            .is_some_and(|slot| slot.flight.is_some())
        {
            if let Some(slot) = slots.get_mut(&provider_id) {
                slot.record = None;
            }
        } else {
            slots.remove(&provider_id);
        }
    }

    /// Checks one account/model, throttling optional failures and reloading durable evidence.
    ///
    /// # Errors
    ///
    /// Returns database failures; a deleted account returns `None`.
    pub(crate) async fn check(
        self: &Arc<Self>,
        database: &Database,
        client: &Client,
        provider_id: i64,
        upstream_id: &str,
    ) -> Result<Option<AccountQuotaState>, storage::Error> {
        let _ = self
            .run(database, client, provider_id, Some(upstream_id), false)
            .await;
        storage::quota_blocks::load_state(database, provider_id).await
    }

    /// Forces a read, joining a current-generation occupied slot and bypassing completed cadence.
    ///
    /// # Errors
    ///
    /// Returns normal typed quota or credential-renewal failures.
    ///
    /// # Panics
    ///
    /// Panics if a forced check incorrectly returns a completed-cache hit.
    pub(crate) async fn refresh(
        self: &Arc<Self>,
        database: &Database,
        client: &Client,
        provider_id: i64,
    ) -> Result<Observation, Error> {
        #[expect(
            clippy::expect_used,
            reason = "forced refresh never returns a completed-cache hit"
        )]
        let reading = self
            .run(database, client, provider_id, None, true)
            .await?
            .expect("forced refresh has a query outcome");
        reading
    }

    /// Clears durable evidence and installs grace without allowing an old callback to replace it.
    ///
    /// # Errors
    ///
    /// Returns database failures or not-found for an absent account.
    pub(crate) async fn reset(
        self: &Arc<Self>,
        database: &Database,
        provider_id: i64,
    ) -> Result<(), storage::Error> {
        let reset_fence = storage::quota_blocks::reset(database, provider_id).await?;
        let completed_at = Instant::now();
        let attempted_at = timestamp();
        if let Some(state) = storage::quota_blocks::load_state(database, provider_id).await? {
            let mut slots = self.slots();
            let slot = slots.entry(provider_id).or_default();
            slot.install_reset(reset_fence, &state, completed_at, attempted_at);
        }
        Ok(())
    }

    /// Decides whether to query using committed evidence and current configuration, without I/O.
    fn action(
        &self,
        provider_id: i64,
        state: &AccountQuotaState,
        upstream_id: Option<&str>,
        is_forced: bool,
    ) -> Action {
        let mut slots = self.slots();
        let slot = slots.entry(provider_id).or_default();
        let now = Instant::now();
        if !slot.synchronize(state, now, crate::unix_time::unix_now_micros()) {
            return Action::Reload;
        }
        let interval = Duration::from_secs(u64::from(
            crate::CONFIG.pooling.quota_refresh_interval_seconds.get(),
        ));
        if !is_forced
            && slot
                .record
                .as_ref()
                .is_some_and(|record| !record.is_due(now, interval, upstream_id))
        {
            return Action::Cached;
        }
        if let Some(flight) = &slot.flight {
            return Action::Wait {
                receiver: flight.receiver.clone(),
                is_current: flight.generation == state.fence.generation,
            };
        }
        let (sender, receiver) = watch::channel(None);
        slot.flight = Some(Flight {
            generation: state.fence.generation,
            receiver: receiver.clone(),
        });
        Action::Start { sender, receiver }
    }

    /// Runs or joins at most one current-generation check; old occupied slots are waited first.
    async fn run(
        self: &Arc<Self>,
        database: &Database,
        client: &Client,
        provider_id: i64,
        upstream_id: Option<&str>,
        is_forced: bool,
    ) -> Result<Option<FlightResult>, Error> {
        loop {
            let state = storage::quota_blocks::load_state(database, provider_id)
                .await
                .map_err(ProviderError::from)?
                .ok_or_else(|| ProviderError::NotFound {
                    message: storage::provider_not_found_message(provider_id),
                })?;
            match self.action(provider_id, &state, upstream_id, is_forced) {
                Action::Cached => return Ok(None),
                Action::Reload => continue,
                Action::Wait {
                    receiver,
                    is_current,
                } => {
                    let result = completed(receiver).await;
                    if is_current {
                        return Ok(Some(result));
                    }
                }
                Action::Start { sender, receiver } => {
                    let coordinator = Arc::clone(self);
                    let database = database.clone();
                    let client = client.clone();
                    tokio::spawn(async move {
                        coordinator
                            .execute(
                                database,
                                client,
                                provider_id,
                                state.fence.generation,
                                sender,
                            )
                            .await;
                    });
                    return Ok(Some(completed(receiver).await));
                }
            }
        }
    }

    /// Owns a detached read, fence reconciliation, and conditional memory publication.
    async fn execute(
        self: Arc<Self>,
        database: Database,
        client: Client,
        provider_id: i64,
        generation: i64,
        sender: watch::Sender<Option<Arc<FlightResult>>>,
    ) {
        let attempted_at = timestamp();
        let reading = async {
            let provider = storage::providers::find_provider_by_id(&database, provider_id)
                .await
                .map_err(ProviderError::from)?
                .ok_or_else(|| ProviderError::NotFound {
                    message: storage::provider_not_found_message(provider_id),
                })?;
            let reading = super::query_fenced(&database, &client, &provider).await?;
            storage::quota_blocks::reconcile(
                &database,
                provider_id,
                reading.fence,
                &reading.observation,
            )
            .await
            .map_err(ProviderError::from)?;
            Ok(reading.observation)
        }
        .await;
        let state = storage::quota_blocks::load_state(&database, provider_id).await;
        let result = match state {
            Err(error) => Err(Error::Provider(error.into())),
            Ok(state) => {
                self.publish(
                    provider_id,
                    generation,
                    attempted_at,
                    &reading,
                    state.as_ref(),
                );
                reading
            }
        };
        if let Some(slot) = self.slots().get_mut(&provider_id) {
            slot.flight = None;
        }
        sender.send_replace(Some(Arc::new(result)));
    }

    /// Publishes only into the initiating generation, reloading after a lost revision fence.
    fn publish(
        &self,
        provider_id: i64,
        generation: i64,
        attempted_at: SafeTimestamp,
        reading: &FlightResult,
        state: Option<&AccountQuotaState>,
    ) {
        let mut slots = self.slots();
        let Some(state) = state else {
            slots.remove(&provider_id);
            return;
        };
        let slot = slots.entry(provider_id).or_default();
        let now = Instant::now();
        let utc_now = crate::unix_time::unix_now_micros();
        if !slot.synchronize(state, now, utc_now) {
            return;
        }
        if let Some(record) = slot
            .record
            .as_mut()
            .filter(|record| record.fence.generation == generation)
        {
            record.last_refresh = outcome(attempted_at, reading);
            if let Ok(observation) = reading {
                record.last_observed_at = Some(observation.observed_at);
            }
            record.completed_at = Some(now);
            // A completed attempt ignores resets already past at completion. Otherwise a
            // retained exhausted reading with a past reset would poll on every request.
            record.resets.clear();
            record.update_resets(&state.blocks, now, utc_now);
        }
    }
}

/// Waits cancellation-safely for detached work; task failure is a fatal invariant violation.
///
/// # Panics
///
/// Panics if the owning refresh task disappears before publishing its result.
async fn completed(mut receiver: watch::Receiver<Option<Arc<FlightResult>>>) -> FlightResult {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return (*result).clone();
        }
        #[expect(
            clippy::expect_used,
            reason = "the detached task always publishes before dropping"
        )]
        receiver
            .changed()
            .await
            .expect("quota refresh task publishes a result");
    }
}

/// Stamps safe UTC microseconds under the gateway's clock contract.
///
/// # Panics
///
/// Panics if the clock is outside the documented management timestamp domain.
fn timestamp() -> SafeTimestamp {
    #[expect(
        clippy::expect_used,
        reason = "the gateway clock satisfies its timestamp contract"
    )]
    let timestamp = crate::unix_time::unix_now_micros()
        .try_into()
        .expect("safe UTC timestamp");
    timestamp
}

/// Maps complete outcomes to safe metadata without retaining observations or error text.
fn outcome(attempted_at: SafeTimestamp, result: &FlightResult) -> LastRefresh {
    let reason = match result {
        Ok(observation) => {
            return LastRefresh::Succeeded {
                attempted_at,
                observed_at: observation.observed_at,
            };
        }
        Err(Error::NotConfigured { .. }) => return LastRefresh::Unsupported { attempted_at },
        Err(Error::Renewal(_)) => RefreshFailure::CredentialRenewal,
        Err(Error::MalformedResponse | Error::Extraction(_)) => {
            RefreshFailure::MalformedObservation
        }
        Err(Error::Provider(ProviderError::Timeout { .. })) => RefreshFailure::Timeout,
        Err(Error::Provider(ProviderError::Rejected { .. })) => RefreshFailure::UpstreamRejected,
        Err(Error::Provider(_)) => RefreshFailure::Transport,
    };
    LastRefresh::Failed {
        attempted_at,
        reason,
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
