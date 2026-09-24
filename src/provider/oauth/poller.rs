//! The background device poller: one task resumes every unexpired pending device flow from its
//! stored schedule at startup and keeps polling newly started flows (README §5.6).
//!
//! Claiming a due flow is one conditional `UPDATE … RETURNING` — the conditional database lease —
//! and a claim older than the control-request timeout is recovered by the next claimant, so
//! competing pollers and restarts never double-exchange an upstream grant. Only the current
//! claimant persists the result or next schedule: every persist conditions on the claim stamp.

use super::{Error, flow};
use crate::{
    database::Database,
    http_client,
    provider::{
        StoredCredentials,
        definition::{
            credential::{CredentialAcquisition, Grant, OAuthRecipe},
            eval::ValueEnv,
        },
        provisioning::{self, WorkingState},
    },
    storage,
    storage::oauth_flows::{DevicePollOutcome, OngoingFlow},
    unix_time,
};
use log::{debug, info, warn};
use reqwest::Client;
use std::{collections::BTreeMap, time::Duration};

/// How long a device-flow claim is trusted before another poller steals it: the control-plane
/// client bounds every exchange attempt at 30 seconds, so a claim older than that belongs to a
/// poller that died mid-exchange.
const CLAIM_STALE_AFTER: Duration = http_client::CONTROL_PLANE_TOTAL_TIMEOUT;

/// The interval a connection-timeout backoff grows to before it stops doubling.
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

            let claim_stale_before =
                now - i64::try_from(CLAIM_STALE_AFTER.as_micros()).unwrap_or(i64::MAX);

            // Every due flow is claimed and driven before the next tick; a claim loop error ends
            // the batch and the next tick retries.
            loop {
                let flow = storage::oauth_flows::claim_due_device_flow(
                    &database,
                    unix_time::unix_now_micros(),
                    claim_stale_before,
                )
                .await;

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

/// Drives one claimed flow's poll attempt: executes the definition's exchange request, classifies
/// the outcome, schedules the next attempt, and — on a completed exchange — provisions and
/// commits the provider write together with the terminal outcome in one transaction.
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
            "internal_error",
            "the flow's provider definition vanished",
        )
        .await;
    };

    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return persist_terminal(
            database,
            flow,
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
        access_token_lifetime_seconds,
        id_token_claims,
        ..
    } = &**recipe;

    let Grant::Device { .. } = grant else {
        return persist_terminal(
            database,
            flow,
            "internal_error",
            "the flow's definition carries no device grant",
        )
        .await;
    };

    let env = ValueEnv {
        base_url: flow.base_url.as_deref(),
        secret: None,
        slots: &flow.slots,
        client_id: Some(client_id.as_str()),
        client_secret: client_secret.as_deref(),
        authorization_code: None,
        code_verifier: None,
        redirect_uri: None,
        oauth_state: None,
        device_code: flow.device_code.as_deref(),
        refresh_token: None,
        request_uuid: None,
        model: None,
    };

    debug!(
        "device flow {} poll attempt due at {:?} against definition `{}`",
        flow.id, flow.device_next_poll_at, definition.name
    );

    let product = match flow::execute_exchange(http, exchange, &env).await {
        Ok(product) => product,

        // A transport timeout backs off exponentially before retrying; any other transport or
        // response failure schedules the plain interval retry, so a transient outage never
        // kills the flow.
        Err(error) => {
            debug!("device flow {} exchange attempt failed: {error}", flow.id);

            return schedule_retry(database, flow, is_timeout(&error)).await;
        }
    };

    let interval = flow.device_poll_interval_seconds.unwrap_or(5);

    match product {
        flow::ExchangeProduct::Pending => schedule_retry_at(database, flow, interval, false).await,

        flow::ExchangeProduct::SlowDown => {
            // `slow_down` permanently raises this flow's interval (RFC 8628 §3.5).
            schedule_retry_at(
                database,
                flow,
                interval + flow::SLOW_DOWN_PENALTY_SECONDS,
                true,
            )
            .await
        }

        flow::ExchangeProduct::Other(code) => {
            // A failure code outside the configuration fails the flow: retrying it is futile.
            persist_terminal(
                database,
                flow,
                "upstream_rejected",
                &format!("the token endpoint rejected the device code with `{code}`"),
            )
            .await
        }

        flow::ExchangeProduct::Terminal(code) => {
            persist_terminal(
                database,
                flow,
                "upstream_rejected",
                &format!(
                    "the token endpoint rejected the device code with `{code}`: start a fresh flow \
                 and authorize it promptly"
                ),
            )
            .await
        }

        flow::ExchangeProduct::Completed(outcome) => {
            commit_success(
                database,
                http,
                flow,
                &definition,
                &env,
                outcome,
                *access_token_lifetime_seconds,
                id_token_claims,
            )
            .await
        }
    }
}

/// Schedules the next attempt after a failed transport round-trip: a timeout doubles the flow's
/// interval with exponential backoff, everything else keeps the stored interval.
async fn schedule_retry(
    database: &Database,
    flow: &OngoingFlow,
    is_timeout: bool,
) -> Result<(), Error> {
    let stored = flow.device_poll_interval_seconds.unwrap_or(5);

    let interval = if is_timeout {
        (stored * 2).min(BACKOFF_INTERVAL_CAP_SECONDS)
    } else {
        stored
    };

    schedule_retry_at(database, flow, interval, interval != stored).await
}

/// Persists one retry schedule while the caller still owns its claim. A lost claim is ordinary
/// contention: the winning poller owns the schedule, so the discard is logged and ignored.
async fn schedule_retry_at(
    database: &Database,
    flow: &OngoingFlow,
    interval_seconds: i64,
    _interval_changed: bool,
) -> Result<(), Error> {
    let Some(claim) = flow.device_poll_started_at else {
        return Ok(());
    };

    let next_poll_at =
        unix_time::unix_now_micros() + interval_seconds * crate::unix_time::MICROS_PER_SEC;

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

/// Persists one terminal failure while the caller still owns its claim; the flow's result stays
/// until the management client retrieves it or the flow expires.
async fn persist_terminal(
    database: &Database,
    flow: &OngoingFlow,
    code: &str,
    message: &str,
) -> Result<(), Error> {
    let Some(claim) = flow.device_poll_started_at else {
        return Ok(());
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
        debug!("device flow {} lost its claim; failure discarded", flow.id);
    }

    Ok(())
}

/// Commits a completed device exchange: provisions, then writes the provider and the succeeded
/// outcome in one transaction. A provisioning failure fails the flow with a gateway-owned
/// message; a lost claim abandons the provider write, leaving the newer state untouched.
#[allow(clippy::too_many_arguments)]
async fn commit_success(
    database: &Database,
    http: &Client,
    flow: &OngoingFlow,
    definition: &crate::provider::definition::ProviderDefinition,
    env: &ValueEnv<'_>,
    outcome: flow::ExchangeOutcome,
    configured_lifetime: Option<std::num::NonZeroU64>,
    id_token_claims: &[crate::provider::definition::credential::IdTokenClaim],
) -> Result<(), Error> {
    // The working slots absorb the exchange's slots and the identity-token claims.
    let mut working_slots = flow.slots.clone();

    if let Some(id_token) = &outcome.id_token {
        flow::apply_id_token_claims(id_token, id_token_claims, &mut working_slots);
    }

    working_slots.extend(outcome.slots);

    let mut state = WorkingState::new(working_slots, flow.base_url.clone());

    state.access_token = outcome.access_token;
    state.refresh_token = outcome.refresh_token;

    if let Some(steps) = &definition.payload.provisioning {
        provisioning::run_steps(http, steps, env, &mut state).await?;
    }

    // The access token must exist before the provider write; the definition's validation
    // guarantees the exchange or a provisioning step supplies it.
    let Some(access_token) = state.access_token.clone() else {
        return persist_terminal(
            database,
            flow,
            "internal_error",
            "the completed exchange produced no access token",
        )
        .await;
    };

    let expires_at = match state.expires_at {
        Some(expires_at) => Some(expires_at),

        None => flow::resolve_expiry(
            outcome.expires_in_seconds,
            Some(&access_token),
            configured_lifetime,
        )?,
    };

    let mut credentials_slots: BTreeMap<String, String> = state.slots.clone();

    credentials_slots.retain(|_, value| !value.is_empty());

    let credentials = StoredCredentials::OAuth {
        access_token,
        refresh_token: state.refresh_token,
        slots: credentials_slots,
        expires_at,
        refresh_started_at: None,
    };

    let base_url = state.base_url.clone();

    let claim = flow.device_poll_started_at.ok_or_else(|| Error::Internal {
        message: "the claimed flow carries no claim stamp".to_string(),
    })?;

    let mut transaction = database.transaction().await?;

    let result_provider_id = match flow.purpose {
        storage::oauth_flows::FlowPurpose::Create => {
            let name = flow.provider_name.clone().unwrap_or_default();
            let model_prefix = flow.model_prefix.clone().unwrap_or_default();

            // A concurrent creation of the same name is the unique constraint's call; the flow
            // fails with the conflict instead of racing.
            storage::providers::insert_provider_in_transaction(
                &mut transaction,
                storage::providers::NewProvider {
                    name: &name,
                    model_prefix: &model_prefix,
                    base_url: base_url.as_deref(),
                    credentials: &credentials,
                    definition,
                    is_enabled: true,
                },
            )
            .await?
        }

        storage::oauth_flows::FlowPurpose::Reauthorize => {
            let target = flow.target_provider_id.ok_or_else(|| Error::Internal {
                message: "the reauthorize flow carries no target provider".to_string(),
            })?;

            let is_updated = storage::providers::rebind_credentials_in_transaction(
                &mut transaction,
                target,
                definition,
                &credentials,
            )
            .await?;

            if !is_updated {
                // A concurrent `providers:delete` won: the flow fails with the vanished-target
                // classification, mirroring the refresh path.
                transaction.rollback().await?;

                return persist_terminal(
                    database,
                    flow,
                    "target_vanished",
                    "the provider the flow reauthorizes was deleted",
                )
                .await;
            }

            target
        }
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

/// Whether this engine failure is a transport timeout: the control executor surfaces a connect
/// or total timeout as an unavailable failure whose message names the transport timeout.
fn is_timeout(error: &Error) -> bool {
    match error {
        Error::Unavailable { detail } => detail.contains("timed out") || detail.contains("timeout"),
        _ => false,
    }
}
