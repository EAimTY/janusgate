//! One coherent route snapshot and a bounded priority scan for a fresh ordinary operation.

use super::{AttemptOutcome, Context, Error, ModelRouting, execute_attempt};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    affinity::{Accepted, Decision, Evidence, Namespace, Ownership, WaitOutcome},
    database::Database,
    format::ParsedRequest,
    storage::{
        self,
        api_logs::{ApiLogDraft, ApiLogGuard},
        quota_blocks::QuotaBlock,
        route_snapshot::{MemberSnapshot, RouteSnapshot, TargetSnapshot},
    },
};
use std::{collections::HashSet, num::NonZeroU64};

#[cfg(test)]
mod tests;

/// A request-local scan; every account comes from one released database snapshot.
struct Selection {
    /// Complete coherent route snapshot, retaining missing/disabled pool targets for bindings.
    snapshot: RouteSnapshot,
    /// Position in the authored total order; never wraps or reloads between account attempts.
    next_member: usize,
    /// Request-local tried set; no recursion or priority wraparound is permitted.
    tried_providers: HashSet<i64>,
    /// Whether a candidate was blocked or positively rejected for quota.
    has_quota_restrictions: bool,
    /// Complete reset bounds for quota-limited candidates; evaluated again at final failure.
    candidate_resets: Vec<CandidateReset>,
}

impl Selection {
    /// Loads route gates and complete membership before any network operation.
    ///
    /// # Errors
    ///
    /// Returns storage or model-not-found failures; empty pools remain available for binding
    /// lookup.
    async fn load(database: &Database, public_id: &str, api_key_id: i64) -> Result<Self, Error> {
        let not_found = || Error::ModelNotFound {
            model: public_id.into(),
        };
        let snapshot =
            storage::route_snapshot::find_admitted_route(database, public_id, api_key_id)
                .await?
                .ok_or_else(not_found)?;
        if !snapshot.route.is_enabled {
            return Err(not_found());
        }
        match &snapshot.target {
            TargetSnapshot::Direct { provider, model } => {
                if !model.is_effectively_enabled(provider.is_enabled) {
                    return Err(not_found());
                }
            }
            TargetSnapshot::Pool { is_enabled, .. } => {
                if !is_enabled {
                    return Err(not_found());
                }
            }
        }
        Ok(Self {
            snapshot,
            next_member: 0,
            tried_providers: HashSet::new(),
            has_quota_restrictions: false,
            candidate_resets: Vec::new(),
        })
    }

    /// Authored namespace identity; direct targets cannot acquire pool affinity.
    fn pool_id(&self) -> Option<i64> {
        match self.snapshot.target {
            TargetSnapshot::Pool { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Acquires request affinity and reauthorizes the route whenever a reservation is released.
    /// Authenticated native requirements are resolved against each refreshed pool snapshot;
    /// quota and structural availability remain the selected account's next step.
    ///
    /// # Errors
    ///
    /// Returns storage, admission, native incompatibility, or conflicting affinity failures.
    ///
    /// # Panics
    ///
    /// Panics if a coherent pooled snapshot violates its authored target invariant.
    async fn acquire_affinity(
        &mut self,
        context: &Context,
        api_key_id: i64,
        mut evidence: Evidence,
        preparation: Option<&crate::compaction_model::boundary::BoundaryPreparation>,
    ) -> Result<Ownership, Error> {
        loop {
            let Some(pool_id) = self.pool_id() else {
                return Ok(Ownership::Direct);
            };
            evidence.proven_provider = preparation
                .map(|preparation| self.compaction_provider(preparation))
                .transpose()?
                .flatten();
            let namespace = Namespace {
                api_key_id,
                pool_id,
            };
            let decision = context.affinity_cache.acquire(namespace, &evidence);
            // Acquisition precedes this committed presence read: a deletion before acquisition
            // is caught here, while a later deletion invalidates the already-created generation.
            let presence =
                storage::routing::namespace_presence(&context.database, api_key_id, pool_id)
                    .await?;
            if !presence.has_api_key {
                context.affinity_cache.remove_api_key(api_key_id);
            }
            if !presence.has_pool {
                context.affinity_cache.remove_pool(pool_id);
            }
            return match decision {
                Decision::Fresh(lease) => Ok(Ownership::Fresh(lease)),
                Decision::Bound(lease) => Ok(Ownership::Bound(lease)),
                Decision::Conflict => Err(Error::RoutingStateConflict {
                    model: self.snapshot.route.public_id.clone(),
                }),
                Decision::Wait(waiter) => match waiter.wait().await {
                    WaitOutcome::Bound(lease) => Ok(Ownership::Bound(lease)),
                    WaitOutcome::Reevaluate => {
                        *self = Self::load(
                            &context.database,
                            &self.snapshot.route.public_id,
                            api_key_id,
                        )
                        .await?;
                        continue;
                    }
                },
            };
        }
    }

    /// Constructs one structurally eligible physical target without retaining duplicate state.
    fn pooled_target(&self, member: &MemberSnapshot) -> Option<ModelRouting> {
        member
            .model
            .as_ref()
            .filter(|model| model.is_effectively_enabled(member.provider.is_enabled))
            .map(|model| ModelRouting {
                model_route_id: self.snapshot.route.id,
                public_id: self.snapshot.route.public_id.clone(),
                provider: member.provider.clone(),
                model: model.clone(),
            })
    }

    /// Advances once through the coherent physical snapshot, cloning only a selected account.
    fn next_structural(&mut self) -> Option<ModelRouting> {
        match &self.snapshot.target {
            TargetSnapshot::Direct { provider, model } if self.next_member == 0 => {
                self.next_member = 1;
                Some(ModelRouting {
                    model_route_id: self.snapshot.route.id,
                    public_id: self.snapshot.route.public_id.clone(),
                    provider: provider.as_ref().clone(),
                    model: model.as_ref().clone(),
                })
            }
            TargetSnapshot::Pool { members, .. } => {
                let (index, target) = members.iter().enumerate().skip(self.next_member).find_map(
                    |(index, member)| self.pooled_target(member).map(|target| (index, target)),
                )?;
                self.next_member = index + 1;
                Some(target)
            }
            _ => None,
        }
    }

    /// Takes the first structural target for the existing same-account compaction coordinator.
    ///
    /// # Errors
    ///
    /// Returns pool-unavailable for an enabled pool without any structural target.
    fn first_structural(&mut self) -> Result<ModelRouting, Error> {
        self.next_structural()
            .ok_or_else(|| Error::PoolUnavailable {
                model: self.snapshot.route.public_id.clone(),
            })
    }

    /// Resolves an authenticated native producer within the authored pool before upstream I/O.
    /// A superseding portable checkpoint releases the requirement; direct routes retain their
    /// existing target-dependent compatibility and explicitly authored export behavior.
    ///
    /// # Errors
    ///
    /// Returns inaccessible-state for foreign producer/model/format. Structural eligibility is
    /// checked only after affinity acquisition, so unavailable members cannot hide conflicts.
    ///
    /// # Panics
    ///
    /// Panics if a pooled snapshot has a direct authored target, prevented by snapshot loading.
    fn compaction_provider(
        &self,
        preparation: &crate::compaction_model::boundary::BoundaryPreparation,
    ) -> Result<Option<i64>, Error> {
        let Some(requirement) = preparation.native_requirement() else {
            return Ok(None);
        };
        let TargetSnapshot::Pool { members, .. } = &self.snapshot.target else {
            return Ok(None);
        };
        let upstream_id = match &self.snapshot.route.target {
            crate::routing::ModelRouteTarget::Pool { upstream_id, .. } => upstream_id,
            #[expect(
                clippy::unreachable,
                reason = "snapshot target and authored route are coherent"
            )]
            crate::routing::ModelRouteTarget::Direct { .. } => {
                unreachable!("pooled snapshot has pooled route")
            }
        };
        let incompatible = || {
            Error::from(
                crate::compaction_model::boundary::BoundaryError::inaccessible_native_state(
                    "the wrapped native state belongs to another provider, model, or format",
                ),
            )
        };
        if requirement.upstream_model_id != *upstream_id {
            return Err(incompatible());
        }
        let member = members
            .iter()
            .find(|member| {
                requirement.producer_instance_id
                    == crate::compaction_model::envelope::NativeWindowPayload::producer_instance(
                        member.provider.id,
                    )
            })
            .ok_or_else(incompatible)?;
        if requirement.native_format != member.provider.definition.payload.format {
            return Err(incompatible());
        }
        Ok(Some(member.provider.id))
    }

    /// Resolves a cached account against the full authored pool, never selecting an alternative.
    ///
    /// # Errors
    ///
    /// Returns bound-account-unavailable when membership or structural eligibility is absent.
    ///
    /// # Panics
    ///
    /// Panics if a direct route reaches pool binding, prevented by namespace acquisition.
    fn required_target(&self, provider_id: i64) -> Result<ModelRouting, Error> {
        match &self.snapshot.target {
            TargetSnapshot::Pool { members, .. } => members
                .iter()
                .find(|member| member.provider.id == provider_id)
                .and_then(|member| self.pooled_target(member))
                .ok_or_else(|| Error::BoundAccountUnavailable {
                    model: self.snapshot.route.public_id.clone(),
                }),
            #[expect(
                clippy::unreachable,
                reason = "direct routes cannot acquire an affinity namespace"
            )]
            TargetSnapshot::Direct { .. } => unreachable!("only pools resolve cached accounts"),
        }
    }

    /// Checks the required account's optional quota refresh and every applicable stored
    /// restriction.
    ///
    /// # Errors
    ///
    /// Returns storage or explicit bound-account availability failures, never another account.
    async fn bound(&mut self, context: &Context, provider_id: i64) -> Result<ModelRouting, Error> {
        let target = self.required_target(provider_id)?;
        self.bound_target(context, target).await
    }

    /// Checks only the already resolved required account's quota state.
    ///
    /// # Errors
    ///
    /// Returns storage, unavailable, or quota-exhausted failures without another account.
    async fn bound_target(
        &mut self,
        context: &Context,
        target: ModelRouting,
    ) -> Result<ModelRouting, Error> {
        let Some(state) = context
            .quota_coordinator
            .check(
                &context.database,
                &context.control_plane,
                target.provider.id,
                &target.model.upstream_id,
            )
            .await?
        else {
            return Err(Error::BoundAccountUnavailable {
                model: self.snapshot.route.public_id.clone(),
            });
        };
        if self.observe_blocks(&state.blocks, &target.model.upstream_id) {
            return Err(self.bound_quota_failure());
        }
        Ok(target)
    }

    /// Formats the required account's own complete future resets, without another account's data.
    fn bound_quota_failure(&self) -> Error {
        Error::BoundAccountQuotaExhausted {
            model: self.snapshot.route.public_id.clone(),
            retry_after: pool_retry_after(
                &self.candidate_resets,
                crate::unix_time::unix_now_micros(),
            ),
        }
    }

    /// Retains only applicable restrictions and complete future candidate-reset evidence.
    fn observe_blocks(&mut self, blocks: &[QuotaBlock], upstream_id: &str) -> bool {
        let applicable: Vec<_> = blocks
            .iter()
            .filter(|block| block.applies_to.is_applicable_to(upstream_id))
            .collect();
        if applicable.is_empty() {
            return false;
        }
        self.has_quota_restrictions = true;
        let now = crate::unix_time::unix_now_micros();
        // Every applicable restriction must reset before this candidate can be rechecked. An
        // unknown candidate does not suppress another candidate's complete known reset.
        let reset = applicable.iter().try_fold(
            CandidateReset {
                earliest: i64::MAX,
                latest: now,
            },
            |bounds, block| {
                block
                    .blocked_until
                    .filter(|reset| *reset > now)
                    .map(|reset| CandidateReset {
                        earliest: bounds.earliest.min(reset),
                        latest: bounds.latest.max(reset),
                    })
            },
        );
        if let Some(reset) = reset {
            self.candidate_resets.push(reset);
        }
        true
    }

    /// Checks optional quota freshness and authoritative blocks before each physical attempt.
    ///
    /// # Errors
    ///
    /// Returns storage failures or a synthesized exhaustion/unavailable selection error.
    async fn next(&mut self, context: &Context) -> Result<ModelRouting, Error> {
        while let Some(target) = self.next_structural() {
            if !self.tried_providers.insert(target.provider.id) {
                continue;
            }
            if self.pool_id().is_none() {
                return Ok(target);
            }
            let Some(state) = context
                .quota_coordinator
                .check(
                    &context.database,
                    &context.control_plane,
                    target.provider.id,
                    &target.model.upstream_id,
                )
                .await?
            else {
                continue;
            };
            if !self.observe_blocks(&state.blocks, &target.model.upstream_id) {
                return Ok(target);
            }
        }
        if self.has_quota_restrictions {
            Err(Error::PoolQuotaExhausted {
                model: self.snapshot.route.public_id.clone(),
                retry_after: pool_retry_after(
                    &self.candidate_resets,
                    crate::unix_time::unix_now_micros(),
                ),
            })
        } else {
            Err(Error::PoolUnavailable {
                model: self.snapshot.route.public_id.clone(),
            })
        }
    }

    /// Reloads the just-rejected account's persisted quota evidence and observes it as fresh
    /// candidate evidence. The rejection's classification has already persisted its
    /// generation-fenced storage, so this reload reads reset hints without writing a second
    /// revision or issuing another classification. Returns whether stored state exists.
    ///
    /// # Errors
    ///
    /// Returns storage failures.
    async fn absorb_rejection_evidence(
        &mut self,
        database: &Database,
        provider_id: i64,
        upstream_id: &str,
    ) -> Result<bool, crate::storage::Error> {
        let Some(state) = storage::quota_blocks::load_state(database, provider_id).await? else {
            return Ok(false);
        };
        self.observe_blocks(&state.blocks, upstream_id);
        Ok(true)
    }
}

/// Request-owned compaction account selection retained across its same-account stages.
pub(crate) struct CompactionSelection {
    /// Remaining candidates from the original coherent snapshot; direct routes have no scan.
    selection: Option<Selection>,
    /// Exact physical target selected from one coherent route snapshot.
    pub(crate) routing: ModelRouting,
    /// Cache reservation or established binding, never an account alternative list.
    pub(crate) ownership: Ownership,
    /// Content-free visible history for later actual artifact/output registration.
    projector: Option<crate::affinity::fingerprint::Projector>,
}

impl CompactionSelection {
    /// Constructs an explicit direct target without affinity or alternative accounts.
    fn direct(routing: ModelRouting) -> Self {
        Self {
            selection: None,
            routing,
            ownership: Ownership::Direct,
            projector: None,
        }
    }

    /// Handles one already-persisted quota rejection without re-resolving the public route.
    /// A bound operation fails explicitly; a fresh first-stage rejection finalizes its own
    /// accounting before advancing, while later stages retain the original mirrored failure.
    ///
    /// # Errors
    ///
    /// Returns storage, exhausted-pool, or bound-account failures.
    pub(crate) async fn advance_after_quota_rejection(
        &mut self,
        context: &Context,
        draft: &mut ApiLogDraft,
    ) -> Result<bool, Error> {
        let Some(selection) = self.selection.as_mut() else {
            return Ok(false);
        };
        if !self.ownership.is_fresh() && !matches!(self.ownership, Ownership::Bound(_)) {
            return Ok(false);
        }
        if self.ownership.is_fresh() && !draft.can_abandon_quota_rejection() {
            return Ok(false);
        }
        if matches!(self.ownership, Ownership::Bound(_)) {
            selection.candidate_resets.clear();
            selection
                .absorb_rejection_evidence(
                    &context.database,
                    self.routing.provider.id,
                    &self.routing.model.upstream_id,
                )
                .await?;
            return Err(selection.bound_quota_failure());
        }

        // The detached write owns the abandoned facts before any cancellable await. Re-arming the
        // coordinator's draft as a fresh predispatch one prevents departure or failed next
        // selection from recording the abandoned attempt twice.
        let re_armed = ApiLogDraft::predispatch(
            draft.api_key_id,
            draft.provider_id,
            draft.model_id,
            draft.model_route_id,
            draft.endpoint_type,
            draft.is_stream,
        );
        let abandoned = std::mem::replace(draft, re_armed);
        ApiLogGuard::new(context.api_log_store.clone(), abandoned)
            .finalize(ApiLogOutcome::UpstreamError, None)
            .await;
        selection.has_quota_restrictions = true;
        selection
            .absorb_rejection_evidence(
                &context.database,
                self.routing.provider.id,
                &self.routing.model.upstream_id,
            )
            .await?;
        self.routing = selection.next(context).await?;
        draft.provider_id = self.routing.provider.id;
        draft.model_id = self.routing.model.id;
        draft.model_route_id = self.routing.model_route_id;
        Ok(true)
    }

    /// Registers an actually rendered completed gateway checkpoint using its sealer-proven
    /// identity. The compact-only Responses codec carries these items verbatim in JSON/SSE.
    pub(crate) fn register_gateway_window(
        &self,
        artifact_id: &str,
        items: &[crate::format::compaction_wire::Checkpoint],
    ) {
        if let (Ownership::Bound(binding), Some(projector)) = (&self.ownership, &self.projector) {
            binding.register(&[crate::affinity::Anchor::Artifact(
                projector.gateway_artifact(artifact_id),
            )]);
            binding.register(&crate::format::openai::response::gateway_window_anchors(
                items,
            ));
        }
    }

    /// Registers the actual completed standalone artifact and returned canonical window on the
    /// already accepted account.
    /// Direct and deliberately uncached operations cannot create a side store.
    pub(crate) fn register_compacted(
        &self,
        body: &crate::format::openai::response::CompactedResponseBody,
    ) {
        if let (Ownership::Bound(binding), Some(projector)) = (&self.ownership, &self.projector) {
            if let Some(anchor) = body.artifact_anchor(projector) {
                binding.register(&[anchor]);
            }
            binding.register(&body.window_anchors());
        }
    }

    /// Transfers the accepted account and visible history to the actual outward rendering task.
    pub(crate) fn accepted(&mut self) -> Option<Accepted> {
        if !matches!(self.ownership, Ownership::Bound(_)) {
            return None;
        }
        let ownership = std::mem::replace(&mut self.ownership, Ownership::Direct);
        match (ownership, self.projector.take()) {
            (Ownership::Bound(binding), Some(projector)) => Some(Accepted::new(binding, projector)),
            _ => None,
        }
    }
}

/// Acquires active compaction evidence before required-account quota checks or fresh selection.
///
/// # Errors
///
/// Returns invalid/incompatible state, storage, affinity conflict, or availability failures.
///
/// # Panics
///
/// Panics if coherent route snapshots violate their authored target invariant.
pub(super) async fn select_compaction(
    context: &Context,
    parsed: &ParsedRequest,
    preparation: &crate::compaction_model::boundary::BoundaryPreparation,
    api_key_id: i64,
) -> Result<CompactionSelection, Error> {
    let mut selection = Selection::load(&context.database, &parsed.ir.model, api_key_id).await?;
    if selection.pool_id().is_none() {
        return Ok(CompactionSelection::direct(selection.first_structural()?));
    }
    let projection = preparation.routing_preparation(parsed)?;
    let mut evidence = projection.projector.evidence();
    evidence.artifact = projection.artifact;
    let ownership = selection
        .acquire_affinity(context, api_key_id, evidence, Some(preparation))
        .await?;
    if matches!(ownership, Ownership::Direct) {
        return Ok(CompactionSelection::direct(selection.first_structural()?));
    }
    let routing = match &ownership {
        Ownership::Bound(lease) => selection.bound(context, lease.provider_id()).await?,
        Ownership::Fresh(_) | Ownership::Direct => selection.next(context).await?,
    };
    Ok(CompactionSelection {
        selection: Some(selection),
        routing,
        ownership,
        projector: Some(projection.projector),
    })
}

/// A final single-account result and its real target; rejected results still own their drafts.
pub(crate) struct SelectedAttempt {
    /// Actual physical target used by the final attempt.
    pub(crate) routing: ModelRouting,
    /// Unfinalized final result; the outward boundary takes ownership next.
    pub(crate) outcome: AttemptOutcome,
    /// Accepted client continuation transferred to the result task independently of IR events.
    pub(crate) affinity: Option<Accepted>,
}

/// Builds the final result for a dispatch whose post-rejection evidence reload failed storage:
/// the failure ends account selection immediately, and the attempt's unfinalized draft travels
/// to the outward boundary that owns error accounting.
fn dispatched_storage_failure(
    routing: ModelRouting,
    attempt: ApiLogGuard,
    error: crate::storage::Error,
) -> SelectedAttempt {
    SelectedAttempt {
        routing,
        affinity: None,
        outcome: AttemptOutcome::DispatchedFailure {
            error: Error::Storage(error),
            attempt,
        },
    }
}

/// Resolves ordinary affinity and executes on its required account or bounded fresh priority scan.
///
/// # Errors
///
/// Returns route/selection failures, which have no artificial final accounting row.
pub(crate) async fn execute_selection(
    context: &Context,
    parsed: ParsedRequest,
    api_key_id: i64,
    endpoint_type: ApiLogEndpointType,
) -> Result<SelectedAttempt, Error> {
    let ir = parsed.ir;
    let is_client_streaming = parsed.is_streaming;
    let mut selection = Selection::load(&context.database, &ir.model, api_key_id).await?;
    let projector = selection
        .pool_id()
        .map(|_| parsed.affinity.project(&ir.messages));
    let mut ownership = match projector.as_ref() {
        Some(projector) => {
            selection
                .acquire_affinity(context, api_key_id, projector.evidence(), None)
                .await?
        }
        None => Ownership::Direct,
    };
    loop {
        let routing = match &ownership {
            Ownership::Bound(lease) => selection.bound(context, lease.provider_id()).await?,
            Ownership::Direct | Ownership::Fresh(_) => selection.next(context).await?,
        };
        let draft = ApiLogDraft::predispatch(
            api_key_id,
            routing.provider.id,
            routing.model.id,
            routing.model_route_id,
            endpoint_type,
            is_client_streaming,
        );
        let outcome = execute_attempt(
            context,
            routing.clone(),
            ir.clone(),
            is_client_streaming,
            draft,
            Some(&mut ownership),
        )
        .await;
        match outcome {
            AttemptOutcome::Rejected {
                error: Error::QuotaRejected { status, .. },
                attempt,
            } if ownership.is_fresh() && attempt.draft().can_abandon_quota_rejection() => {
                // The rejection itself is positive quota evidence even when the reload below
                // finds no stored state.
                selection.has_quota_restrictions = true;
                if let Err(error) = selection
                    .absorb_rejection_evidence(
                        &context.database,
                        routing.provider.id,
                        &routing.model.upstream_id,
                    )
                    .await
                {
                    return Ok(dispatched_storage_failure(routing, attempt, error));
                }
                log::debug!(
                    "abandoning quota-rejected account {} model {} route {} status {}",
                    routing.provider.id,
                    routing.model.id,
                    routing.model_route_id,
                    status.as_u16()
                );
                attempt.finalize(ApiLogOutcome::UpstreamError, None).await;
            }
            AttemptOutcome::Rejected {
                error: Error::QuotaRejected { .. },
                attempt,
            } if matches!(ownership, Ownership::Bound(_)) => {
                selection.candidate_resets.clear();
                if let Err(error) = selection
                    .absorb_rejection_evidence(
                        &context.database,
                        routing.provider.id,
                        &routing.model.upstream_id,
                    )
                    .await
                {
                    return Ok(dispatched_storage_failure(routing, attempt, error));
                }
                return Ok(SelectedAttempt {
                    routing,
                    affinity: None,
                    outcome: AttemptOutcome::Rejected {
                        error: selection.bound_quota_failure(),
                        attempt,
                    },
                });
            }
            AttemptOutcome::Accepted { events, attempt } => {
                let affinity = match (ownership, projector) {
                    (Ownership::Bound(binding), Some(projector)) => {
                        Some(Accepted::new(binding, projector))
                    }
                    _ => None,
                };
                return Ok(SelectedAttempt {
                    routing,
                    outcome: AttemptOutcome::Accepted { events, attempt },
                    affinity,
                });
            }
            outcome => {
                return Ok(SelectedAttempt {
                    routing,
                    outcome,
                    affinity: None,
                });
            }
        }
    }
}

/// Bounds over every applicable block on one candidate with complete known future resets.
struct CandidateReset {
    /// Earliest constituent reset; if due, the candidate's information is no longer complete.
    earliest: i64,
    /// Latest constituent reset; every independent restriction must be rechecked.
    latest: i64,
}

/// Chooses the earliest candidate whose complete reset evidence remains future at response time.
///
/// # Panics
///
/// Panics if an i64 timestamp difference in seconds exceeds u64, which arithmetic prevents.
fn pool_retry_after(candidates: &[CandidateReset], now: i64) -> Option<NonZeroU64> {
    let reset = candidates
        .iter()
        .filter(|bounds| bounds.earliest > now)
        .map(|bounds| bounds.latest)
        .min()?;
    let delta = i128::from(reset) - i128::from(now);
    #[expect(
        clippy::expect_used,
        reason = "an i64 timestamp difference in seconds fits u64"
    )]
    let seconds =
        u64::try_from((delta + 999_999) / 1_000_000).expect("positive reset seconds fit u64");
    NonZeroU64::new(seconds)
}
