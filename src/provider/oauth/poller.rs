//! The background device poller: one task resumes every unexpired pending device flow from its
//! stored schedule at startup and keeps polling newly started flows (`docs/design/storage.md` and
//! `docs/reference/management-api.md`).
//!
//! Claiming a due flow is one conditional `UPDATE … RETURNING` — the conditional database lease —
//! and a claim older than the claim-staleness window (`CLAIM_STALE_AFTER`, strictly beyond the
//! control-request timeout that bounds every exchange) is recovered by the next claimant, so
//! competing pollers never double-exchange an upstream grant concurrently. The lease alone
//! cannot cover a poller that dies after its exchange completed, so the claimant persists the
//! exchange checkpoint the moment the exchange completes — before provisioning runs — and a
//! claim recovered with a checkpoint aboard resumes into provisioning and commit from it,
//! never re-presenting the consumed device code. Only the current claimant persists the result
//! or next schedule: every persist conditions on the claim stamp.

use super::{
    completion,
    exchange::{
        DEFAULT_DEVICE_INTERVAL_SECONDS, ExchangeFailure, ExchangeOutcome, ExchangeProduct,
        SLOW_DOWN_PENALTY_SECONDS, execute_classified_exchange,
    },
};
use crate::{
    database::Database,
    http_client,
    provider::{
        Error,
        definition::{
            credential::{Grant, OAuthRecipe},
            eval::ValueEnv,
        },
    },
    storage,
    storage::oauth_flows::{DevicePollOutcome, FlowPurpose, OngoingFlow},
    unix_time,
};
use log::{debug, info, warn};
use reqwest::Client;
use std::time::Duration;

/// How long a device-flow claim is trusted before another poller steals it: twice the
/// control-plane timeout that bounds every exchange attempt. The first timeout-width covers the
/// exchange's whole budget; the second is the strict slack the assert below demands, keeping a
/// claim unstealable until a full extra control-request timeout has elapsed past the latest
/// instant the exchange can still return — so the holder's claim-conditioned persist, the
/// completed exchange's checkpoint or the timed-out exchange's retry schedule, always lands on
/// a live claim instead of being silently discarded by a claimant that recovered the
/// just-became-stale row first and re-ran the already-consumed grant into a misleading
/// upstream rejection. The trade-off is one slower stale-claim recovery (a dead poller's flow
/// is recovered after 60 seconds rather than 30): that delay is the price the checkpoint's
/// guarantee costs. The same strict-slack demand the sibling refresh lease asserts
/// (`REFRESH_LEASE_STALE_AFTER` in `refresh.rs`) holds here.
const CLAIM_STALE_AFTER: Duration =
    Duration::from_secs(2 * http_client::CONTROL_PLANE_TOTAL_TIMEOUT.as_secs());

// Compile-time guarantee that the claim staleness strictly outlives the control-plane timeout,
// so a claim is never stealable while its holder's exchange budget is still live and a
// completed exchange's checkpoint persist always wins over a competing recovery — the same
// demand the refresh lease asserts in `refresh.rs`.
const _: () =
    assert!(http_client::CONTROL_PLANE_TOTAL_TIMEOUT.as_secs() < CLAIM_STALE_AFTER.as_secs());

/// The delay a connection-timeout backoff grows to before it stops doubling.
const BACKOFF_INTERVAL_CAP_SECONDS: i64 = 120;

/// How often the poller wakes to sweep expiries and claim due flows.
const POLL_TICK: Duration = Duration::from_millis(500);

/// Spawns the background device poller over the shared database and control-plane clients.
pub(crate) fn spawn_device_poller(database: Database, http: Client) {
    tokio::spawn(async move {
        loop {
            let now = unix_time::unix_now_micros();

            if let Err(error) = storage::oauth_flows::sweep_expired_flows(&database, now).await {
                warn!("device poller failed to sweep expired flows: {error}");
            }

            // Every due flow is claimed and driven before the next tick; a claim loop error ends
            // the batch and the next tick retries.
            loop {
                let flow = claim_next_due_flow(&database).await;

                let Ok(Some(flow)) = flow else {
                    if let Err(error) = flow {
                        warn!("device poller failed to claim a due flow: {error}");
                    }

                    break;
                };

                if let Err(error) = process(&database, &http, &flow).await {
                    warn!(
                        "device poller failed to process flow {} of definition `{}`: {error}",
                        flow.id, flow.definition_name
                    );
                }
            }

            tokio::time::sleep(POLL_TICK).await;
        }
    });
}

/// Claims the next due device flow: the claim stamp and its staleness threshold derive from one
/// fresh instant per claim, so a batch of slow flows cannot stretch the window in which a dead
/// poller's claim stays unstealable — a threshold frozen at the tick start would delay a stale
/// claim's recovery by the whole batch's runtime.
///
/// # Errors
///
/// Returns [`storage::Error`] when the claim statement fails.
async fn claim_next_due_flow(database: &Database) -> Result<Option<OngoingFlow>, storage::Error> {
    let now = unix_time::unix_now_micros();

    storage::oauth_flows::claim_due_device_flow(
        database,
        now,
        now - i64::try_from(CLAIM_STALE_AFTER.as_micros()).unwrap_or(i64::MAX),
    )
    .await
}

/// Drives one claimed flow's poll attempt. A flow whose row carries an exchange checkpoint — a
/// previous claimant's completed exchange — resumes straight into provisioning and commit from
/// it; otherwise the definition's exchange request executes, the outcome is classified, and the
/// next attempt is scheduled. A completed exchange is checkpointed before provisioning runs, so
/// a restart between the exchange and its commit resumes from the checkpoint instead of
/// re-presenting the consumed device code.
async fn process(database: &Database, http: &Client, flow: &OngoingFlow) -> Result<(), Error> {
    let Some(definition) = storage::provider_definitions::find_definition(
        database,
        &flow.definition_name,
        flow.definition_version,
    )
    .await?
    else {
        // The definition version the flow references is protected by ON DELETE RESTRICT, so a
        // missing one is drift; the flow fails with a gateway-owned error instead of polling a
        // recipe nobody can read.
        return persist_terminal(
            database,
            flow,
            flow.device_poll_started_at,
            "internal_error",
            "the flow's provider definition vanished",
        )
        .await;
    };

    let Some(recipe) = definition.oauth_recipe() else {
        return persist_terminal(
            database,
            flow,
            flow.device_poll_started_at,
            "internal_error",
            "the flow's definition carries no OAuth credential",
        )
        .await;
    };

    let OAuthRecipe {
        client_id,
        client_secret,
        grant,
        exchange,
        ..
    } = recipe;

    let Grant::Device { .. } = grant else {
        return persist_terminal(
            database,
            flow,
            flow.device_poll_started_at,
            "internal_error",
            "the flow's definition carries no device grant",
        )
        .await;
    };

    // A flow whose row carries an exchange checkpoint has already consumed its grant: a
    // previous claimant completed the exchange and died before its commit, and the stale-claim
    // recovery handed the flow here. Resuming from the checkpoint — never re-presenting the
    // device code — is what keeps a restart from turning the user-approved grant into a
    // misleading upstream rejection (`docs/design/storage.md` and
    // `docs/reference/management-api.md`). A checkpoint the row carries but no longer parses is
    // storage drift: it fails the flow terminally — clearing itself with the terminal persist —
    // instead of looping the reclaim.
    if let Some(checkpoint) = &flow.device_exchange_checkpoint {
        let Ok(outcome) = serde_json::from_str::<ExchangeOutcome>(checkpoint) else {
            return persist_terminal(
                database,
                flow,
                flow.device_poll_started_at,
                "internal_error",
                "the flow's stored exchange checkpoint is malformed",
            )
            .await;
        };

        return commit_success(database, http, flow, &definition, outcome).await;
    }

    let env = ValueEnv::for_device_poll(
        &flow.slots,
        flow.base_url.as_deref(),
        client_id.as_str(),
        client_secret.as_deref(),
        flow.device_code.as_deref(),
    );

    debug!(
        "device flow {} poll attempt due at {:?} against definition `{}`",
        flow.id, flow.device_next_poll_at, definition.name
    );

    let product = match execute_classified_exchange(http, exchange, &env).await {
        Ok(product) => product,

        // A fold failure after the upstream's decoded 2xx response is a deterministic
        // response-shape violation a retry cannot fix, and the single-use device code is
        // consumed upstream whether or not the gateway could use the response — so the flow
        // records the gateway-owned terminal pair instead of scheduling a retry that could
        // only re-present the consumed code and lose the approved grant to a misleading
        // upstream rejection (`docs/design/storage.md` and `docs/reference/management-api.md`).
        Err(ExchangeFailure::Consumed(error)) => {
            return persist_terminal(
                database,
                flow,
                flow.device_poll_started_at,
                "exchange_failed",
                &format!("the exchange succeeded but its token response was unusable: {error}"),
            )
            .await;
        }

        // A transport timeout backs off the next attempt — the stored interval is persisted
        // unchanged, so the transient doubling never compounds across claims (RFC 8628 §3.5
        // reserves the permanent raise for `slow_down`); any other transport or response
        // failure schedules the plain interval retry, so a transient outage never kills the
        // flow.
        Err(ExchangeFailure::Retryable(error)) => {
            debug!("device flow {} exchange attempt failed: {error}", flow.id);

            return schedule_retry(database, flow, is_timeout(&error)).await;
        }
    };

    let interval = flow
        .device_poll_interval_seconds
        .unwrap_or(DEFAULT_DEVICE_INTERVAL_SECONDS);

    match product {
        ExchangeProduct::Pending => schedule_retry_at(database, flow, interval, interval).await,

        ExchangeProduct::SlowDown => {
            // `slow_down` permanently raises this flow's interval (RFC 8628 §3.5); an interval
            // huge enough to overflow the adjustment is upstream-reported drift, mapped to the
            // upstream-unavailable failure like every overflowing duration.
            let raised = interval
                .checked_add(SLOW_DOWN_PENALTY_SECONDS)
                .ok_or_else(|| interval_overflow(interval))?;

            schedule_retry_at(database, flow, raised, raised).await
        }

        ExchangeProduct::Other(code) => {
            // A failure code outside the configured terminal codes is not a terminal outcome
            // (`docs/design/providers.md`): only the configured codes — plus the engine-owned
            // polling codes — end a flow. The retry rides the stored interval until the
            // flow's own expiry bounds it, so a transient upstream rejection never
            // kills the flow mid-window.
            debug!(
                "device flow {} received the non-terminal error `{code}`; retrying at the stored \
                 interval",
                flow.id
            );

            schedule_retry_at(database, flow, interval, interval).await
        }

        ExchangeProduct::Terminal(code) => {
            persist_terminal(
                database,
                flow,
                flow.device_poll_started_at,
                "upstream_rejected",
                &format!(
                    "the token endpoint rejected the device code with `{code}`: start a fresh flow \
                 and authorize it promptly"
                ),
            )
            .await
        }

        ExchangeProduct::Completed(outcome) => {
            let is_checkpointed = checkpoint_completed_exchange(database, flow, &outcome).await?;

            if is_checkpointed {
                commit_success(database, http, flow, &definition, outcome).await
            } else {
                Ok(())
            }
        }
    }
}

/// Persists one completed exchange's checkpoint before its commit (`docs/design/storage.md` and
/// `docs/reference/management-api.md`): the mapped token fields ride the flow row under the held
/// claim, so a restart that recovers the claim resumes into provisioning and commit from the
/// checkpoint instead of re-presenting the consumed device code. A lost claim is ordinary
/// contention: the winner owns the row and its own exchange, so the discard is logged and the
/// caller abandons its outcome.
///
/// # Errors
///
/// Returns [`storage::Error`] when the checkpoint persist fails or [`Error::Internal`] when the
/// claimed flow carries no claim stamp — a claimed flow always does.
///
/// # Panics
///
/// Panics if the exchange outcome fails to serialize, which cannot occur: the outcome is a
/// struct of optional strings, integers, and string maps, a shape serde_json always renders.
async fn checkpoint_completed_exchange(
    database: &Database,
    flow: &OngoingFlow,
    outcome: &ExchangeOutcome,
) -> Result<bool, Error> {
    let claim = flow.device_poll_started_at.ok_or_else(|| Error::Internal {
        message: "the claimed flow carries no claim stamp".to_string(),
    })?;

    #[expect(
        clippy::expect_used,
        reason = "the checkpoint always serializes: the outcome is a struct of optional \
                  strings, integers, and string maps"
    )]
    let checkpoint_json =
        serde_json::to_string(outcome).expect("the exchange checkpoint always serializes");

    let is_persisted = storage::oauth_flows::persist_exchange_checkpoint(
        database,
        flow.id,
        claim,
        &checkpoint_json,
    )
    .await?;

    if !is_persisted {
        debug!(
            "device flow {} lost its claim; exchange outcome discarded",
            flow.id
        );
    }

    Ok(is_persisted)
}

/// Schedules the next attempt after a failed transport round-trip: a timeout doubles only the
/// next attempt's delay while the stored interval is persisted unchanged — the transient
/// backoff never compounds across claims — and everything else keeps the stored interval. The
/// cap bounds only the doubling from above; the stored interval bounds the delay from below,
/// so a flow whose upstream interval exceeds the cap is never polled earlier than it asked
/// (`docs/design/storage.md`).
async fn schedule_retry(
    database: &Database,
    flow: &OngoingFlow,
    is_timeout: bool,
) -> Result<(), Error> {
    let stored = flow
        .device_poll_interval_seconds
        .unwrap_or(DEFAULT_DEVICE_INTERVAL_SECONDS);

    let delay = if is_timeout {
        stored
            .checked_mul(2)
            .map(|doubled| stored.max(doubled.min(BACKOFF_INTERVAL_CAP_SECONDS)))
            .ok_or_else(|| interval_overflow(stored))?
    } else {
        stored
    };

    schedule_retry_at(database, flow, stored, delay).await
}

/// Schedules the next attempt while the caller still owns its claim: `delay_seconds` sets the
/// next attempt instant and `interval_seconds` is the flow interval persisted as the stored
/// schedule. The two differ only on a transport-timeout backoff, whose doubling is transient.
/// A lost claim — the persist matching no row — is ordinary contention: the winning poller
/// owns the schedule, so the discard is logged and ignored.
///
/// # Errors
///
/// Returns [`storage::Error`] when the persist fails or [`Error::Internal`] when the claimed
/// flow carries no claim stamp — a claimed flow always does, the same stored drift
/// [`checkpoint_completed_exchange`] fatalizes.
async fn schedule_retry_at(
    database: &Database,
    flow: &OngoingFlow,
    interval_seconds: i64,
    delay_seconds: i64,
) -> Result<(), Error> {
    let Some(claim) = flow.device_poll_started_at else {
        return Err(Error::Internal {
            message: "the claimed flow carries no claim stamp".to_string(),
        });
    };

    let next_poll_at = next_poll_instant(delay_seconds)?;

    let is_persisted = storage::oauth_flows::persist_device_poll(
        database,
        flow.id,
        claim,
        DevicePollOutcome::Retry {
            next_poll_at,
            interval_seconds,
        },
    )
    .await?;

    if !is_persisted {
        debug!("device flow {} lost its claim; schedule discarded", flow.id);
    }

    Ok(())
}

/// The failure an upstream-reported poll interval raises when it overflows the schedule
/// arithmetic, mirroring how `resolve_expiry` guards the token path: the interval originates in
/// the device-authorization response, so a magnitude this large is a malformed response.
fn interval_overflow(interval_seconds: i64) -> Error {
    Error::Unavailable {
        detail: format!("malformed device flow: `interval` overflow, got {interval_seconds}"),
    }
}

/// The next-poll instant for one interval, in UTC microseconds: the interval scaled to the
/// database clock through the shared clock helper [`unix_time::seconds_from_now`], with the
/// overflow of the upstream-reported magnitude raised as [`interval_overflow`].
fn next_poll_instant(interval_seconds: i64) -> Result<i64, Error> {
    unix_time::seconds_from_now(interval_seconds).ok_or_else(|| interval_overflow(interval_seconds))
}

/// Persists one terminal failure while the caller still owns the claim stamped
/// `claim_started_at`; the flow's result stays until the management client retrieves it or the
/// flow expires. The caller passes the stamp its persist must compare-set against: the held
/// claim, or — on a path that already ran [`storage::oauth_flows::restamp_claim`] — the fresh
/// stamp the re-stamp produced, since the superseded pre-restamp stamp matches no row.
///
/// # Errors
///
/// Returns [`storage::Error`] when the persist fails or [`Error::Internal`] when the claimed
/// flow carries no claim stamp — a claimed flow always does, the same stored drift
/// [`checkpoint_completed_exchange`] fatalizes.
async fn persist_terminal(
    database: &Database,
    flow: &OngoingFlow,
    claim_started_at: Option<i64>,
    code: &str,
    message: &str,
) -> Result<(), Error> {
    let Some(claim) = claim_started_at else {
        return Err(Error::Internal {
            message: "the claimed flow carries no claim stamp".to_string(),
        });
    };

    let is_persisted = storage::oauth_flows::persist_device_poll(
        database,
        flow.id,
        claim,
        DevicePollOutcome::Failed {
            code: code.to_string(),
            message: message.to_string(),
        },
    )
    .await?;

    if is_persisted {
        info!("device flow {} failed terminally: {code}", flow.id);
    } else {
        debug!(
            "device flow {} could not record the terminal outcome; failure discarded",
            flow.id
        );
    }

    Ok(())
}

/// Commits a completed device exchange: the shared completion pipeline folds the outcome
/// through provisioning into the stored credentials, the claim is re-stamped for the commit,
/// and the provider write and the succeeded outcome commit in one transaction. A provisioning
/// failure fails the flow terminally with the gateway-owned `provisioning_failed` pair,
/// persisted with the claim the flow still holds — the failure never reaches the re-stamp — so
/// the poll loop stops at the terminal outcome instead of re-running the exchange; a failure of
/// the commit transaction itself — after the re-stamp — fails the flow the same terminal way,
/// a create flow whose provider write lost to another row's claim of the name recording the
/// operator-fixable `name_conflict` pair and every other failure the gateway-owned
/// `commit_failed` pair, either way because the exchange is consumed upstream; a claim the
/// re-stamp finds stolen abandons the provider write, leaving the newer state untouched. The
/// exchange checkpoint the caller persisted before this pipeline rides through it: the success
/// commit clears it together with the outcome, and every terminal persist clears it with the
/// terminal row.
async fn commit_success(
    database: &Database,
    http: &Client,
    flow: &OngoingFlow,
    definition: &crate::provider::definition::ProviderDefinition,
    outcome: ExchangeOutcome,
) -> Result<(), Error> {
    // A create flow whose provider name another row already holds can never succeed. Leaving
    // the insert's unique-constraint failure to the spawn loop's warning would re-claim and
    // re-fail the flow until the expiry sweep deletes it, with the management client reading
    // "pending" the whole time; the conflict is terminal instead, with its gateway-owned
    // failure pair (`docs/reference/management-api.md`). The constraint stays as the concurrent
    // backstop behind this pre-check, and its loss is classified at the commit path below.
    if flow.purpose == FlowPurpose::Create && is_create_name_taken(database, flow).await? {
        return persist_terminal(
            database,
            flow,
            flow.device_poll_started_at,
            "name_conflict",
            "the provider name the flow creates is already taken",
        )
        .await;
    }

    let held_claim = flow.device_poll_started_at.ok_or_else(|| Error::Internal {
        message: "the claimed flow carries no claim stamp".to_string(),
    })?;

    let product = match completion::complete_exchange(http, definition, outcome, flow).await {
        Ok(product) => product,

        // A provisioning failure after a successful exchange fails the flow terminally
        // (`docs/design/storage.md` and `docs/reference/management-api.md`): the exchange result
        // cannot be resumed — a retry would re-run the exchange against the upstream — so
        // the gateway-owned pair persists with the claim the flow still holds (the failure
        // short-circuits before the re-stamp below) and the poll loop stops at the terminal
        // outcome. A claim stolen while provisioning ran matches no row and discards the
        // outcome, like every persist.
        Err(error) => {
            return persist_terminal(
                database,
                flow,
                Some(held_claim),
                "provisioning_failed",
                &provisioning_failure_message(&error),
            )
            .await;
        }
    };

    // Provisioning ran inside the claim, so the stamp may have aged past the staleness window;
    // re-stamp it for the commit. A refused re-stamp is a stolen claim: the winner owns the
    // row, so the success derived from the superseded exchange is abandoned before any
    // provider write.
    let Some(claim) = storage::oauth_flows::restamp_claim(database, flow.id, held_claim).await?
    else {
        warn!(
            "device flow {} lost its claim during provisioning; success discarded",
            flow.id
        );

        return Ok(());
    };

    // From here the exchange is consumed upstream: no failure of the record may leave the flow
    // pending, or the claim-staleness recovery would hand the flow back to the poll loop and
    // re-run the already-consumed grant exchange — an RFC 8628 token endpoint answers the
    // re-presented device code with a terminal rejection, turning a succeeded grant into a
    // misleading failure. So an error of the commit transaction — the transaction begin, the
    // provider write (its unique constraint included), the claim-conditioned outcome commit,
    // or the commit itself — persists the gateway-owned terminal pair against the fresh claim
    // the row still carries before the error propagates for the spawn loop's warning. The
    // provider write never survived the failed transaction, so the pair records a failure,
    // never a `succeeded` outcome.
    if let Err(error) =
        commit_success_transaction(database, flow, definition, &product, claim).await
    {
        // The failed transaction's provider write may have lost to another row's claim of the
        // create name — the unique constraint is the pre-check's concurrent backstop, and the
        // pre-check-to-write window makes the loss reachable. That loss is operator-fixable
        // (rename and retry), not an internal failure, so a create flow confirms it by
        // re-checking the name — mirroring the pre-check — and records the terminal
        // `name_conflict` pair (`docs/reference/management-api.md`); a probe that fails or still
        // finds the name free keeps the internal-failure diagnosis, so a genuine
        // `commit_failed` is never mislabeled as the conflict.
        let is_name_conflict = flow.purpose == FlowPurpose::Create
            && matches!(is_create_name_taken(database, flow).await, Ok(true));

        let (code, message) = if is_name_conflict {
            (
                "name_conflict",
                "the provider name the flow creates was claimed before the commit".to_string(),
            )
        } else {
            (
                "commit_failed",
                format!(
                    "the exchange succeeded but the gateway failed to record the result: {error}"
                ),
            )
        };

        if let Err(persist_error) =
            persist_terminal(database, flow, Some(claim), code, &message).await
        {
            warn!(
                "device flow {} also failed to record its terminal outcome: {persist_error}",
                flow.id
            );
        }

        return Err(error);
    }

    Ok(())
}

/// Whether another stored provider row already holds the create flow's name: the pre-check
/// ahead of the commit transaction and the commit path's constraint-loss confirmation share one
/// probe, so both `name_conflict` diagnoses rest on the same fact
/// (`docs/reference/management-api.md`). Every caller checks the create purpose before probing, so
/// the name read rides the CHECK-pinned create-name contract.
///
/// # Errors
///
/// Returns [`storage::Error`] when the probe query fails.
async fn is_create_name_taken(
    database: &Database,
    flow: &OngoingFlow,
) -> Result<bool, storage::Error> {
    storage::providers::is_provider_name_taken(database, flow.create_provider_name()).await
}

/// Commits the provider write and the succeeded outcome in one transaction under the fresh
/// post-restamp `claim`: the claim-conditioned outcome commit makes the provider and the
/// consumable result appear together, and a claim lost between the re-stamp and the commit
/// rolls the provider write back — the winner owns the row. A reauthorize flow whose target
/// provider was deleted mid-commit fails terminally under the fresh stamp.
///
/// # Errors
///
/// Returns [`Error`] when the transaction begins, the provider write, the claim-conditioned
/// outcome commit, or the commit itself fails.
async fn commit_success_transaction(
    database: &Database,
    flow: &OngoingFlow,
    definition: &crate::provider::definition::ProviderDefinition,
    product: &completion::CompletionProduct,
    claim: i64,
) -> Result<(), Error> {
    let mut transaction = database.transaction().await?;

    let result_provider_id =
        completion::write_completion_provider(&mut transaction, flow, definition, product).await?;

    let Some(result_provider_id) = result_provider_id else {
        // A concurrent `providers:delete` won: the flow fails with the vanished-target
        // classification, mirroring the refresh path. The failure persists against the fresh
        // post-restamp stamp — the pre-restamp stamp the flow row still carries matches no row.
        transaction.rollback().await?;

        return persist_terminal(
            database,
            flow,
            Some(claim),
            "target_vanished",
            "the provider the flow reauthorizes was deleted",
        )
        .await;
    };

    let is_committed = storage::oauth_flows::commit_device_success_in_transaction(
        &mut transaction,
        flow.id,
        claim,
        result_provider_id,
    )
    .await?;

    if !is_committed {
        // The claim was lost while provisioning ran: the provider write above must not survive.
        transaction.rollback().await?;

        debug!("device flow {} lost its claim; success discarded", flow.id);

        return Ok(());
    }

    transaction.commit().await?;

    info!(
        "device flow {} succeeded: provider {result_provider_id} committed",
        flow.id
    );

    Ok(())
}

/// Whether this engine failure is a transport timeout: the control executor carries the
/// `reqwest::Error::is_timeout` fact through as the typed [`Error::Timeout`] variant, so the
/// backoff decision matches the fact instead of parsing the rendered message.
fn is_timeout(error: &Error) -> bool {
    matches!(error, Error::Timeout { .. })
}

/// The gateway-owned terminal message one completion-pipeline failure stores
/// (`docs/reference/management-api.md`): the error's own rendering — transport, mapping, drift, and
/// provisioning-poll details are gateway-authored at their source — except [`Error::Rejected`],
/// whose `Display` embeds the truncated upstream body excerpt; it renders from the status alone, so
/// no upstream body content ever lands in a terminal result.
fn provisioning_failure_message(error: &Error) -> String {
    match error {
        Error::Rejected { status, .. } => {
            format!("the provisioning request failed with upstream status {status}")
        }
        error => error.to_string(),
    }
}
