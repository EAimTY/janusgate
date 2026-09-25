//! The background device poller: one task resumes every unexpired pending device flow from its
//! stored schedule at startup and keeps polling newly started flows (README §5.6).
//!
//! Claiming a due flow is one conditional `UPDATE … RETURNING` — the conditional database lease —
//! and a claim older than the control-request timeout is recovered by the next claimant, so
//! competing pollers and restarts never double-exchange an upstream grant. Only the current
//! claimant persists the result or next schedule: every persist conditions on the claim stamp.

use super::{Error, completion, flow};
use crate::{
    database::Database,
    http_client,
    provider::definition::{
        credential::{CredentialAcquisition, Grant, OAuthRecipe},
        eval::ValueEnv,
    },
    storage,
    storage::oauth_flows::{DevicePollOutcome, FlowPurpose, OngoingFlow},
    unix_time,
};
use log::{debug, info, warn};
use reqwest::Client;
use std::time::Duration;

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

    let interval = flow
        .device_poll_interval_seconds
        .unwrap_or(flow::DEFAULT_DEVICE_INTERVAL_SECONDS);

    match product {
        flow::ExchangeProduct::Pending => schedule_retry_at(database, flow, interval).await,

        flow::ExchangeProduct::SlowDown => {
            // `slow_down` permanently raises this flow's interval (RFC 8628 §3.5); an interval
            // huge enough to overflow the adjustment is upstream-reported drift, mapped to the
            // upstream-unavailable failure like every overflowing duration.
            let raised = interval
                .checked_add(flow::SLOW_DOWN_PENALTY_SECONDS)
                .ok_or_else(|| interval_overflow(interval))?;

            schedule_retry_at(database, flow, raised).await
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
            commit_success(database, http, flow, &definition, outcome).await
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
    let stored = flow
        .device_poll_interval_seconds
        .unwrap_or(flow::DEFAULT_DEVICE_INTERVAL_SECONDS);

    let interval = if is_timeout {
        stored
            .checked_mul(2)
            .map(|doubled| doubled.min(BACKOFF_INTERVAL_CAP_SECONDS))
            .ok_or_else(|| interval_overflow(stored))?
    } else {
        stored
    };

    schedule_retry_at(database, flow, interval).await
}

/// Persists one retry schedule while the caller still owns its claim. A lost claim is ordinary
/// contention: the winning poller owns the schedule, so the discard is logged and ignored.
async fn schedule_retry_at(
    database: &Database,
    flow: &OngoingFlow,
    interval_seconds: i64,
) -> Result<(), Error> {
    let Some(claim) = flow.device_poll_started_at else {
        return Ok(());
    };

    let next_poll_at = next_poll_instant(interval_seconds)?;

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
/// database clock through the shared engine helper [`flow::seconds_from_now`], with the overflow
/// of the upstream-reported magnitude raised as [`interval_overflow`].
fn next_poll_instant(interval_seconds: i64) -> Result<i64, Error> {
    flow::seconds_from_now(interval_seconds, || interval_overflow(interval_seconds))
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

/// Commits a completed device exchange: the shared completion pipeline folds the outcome
/// through provisioning into the stored credentials, then the provider write and the succeeded
/// outcome commit in one transaction. A provisioning failure fails the flow with a
/// gateway-owned message; a lost claim abandons the provider write, leaving the newer state
/// untouched.
async fn commit_success(
    database: &Database,
    http: &Client,
    flow: &OngoingFlow,
    definition: &crate::provider::definition::ProviderDefinition,
    outcome: flow::ExchangeOutcome,
) -> Result<(), Error> {
    // A create flow whose provider name another row already holds can never succeed. Leaving
    // the insert's unique-constraint failure to the spawn loop's warning would re-claim and
    // re-fail the flow until the expiry sweep deletes it, with the management client reading
    // "pending" the whole time; the conflict is terminal instead, with its gateway-owned
    // failure pair (README §7.2). The constraint stays as the concurrent backstop behind this
    // pre-check.
    if flow.purpose == FlowPurpose::Create {
        let name = flow.provider_name.as_deref().unwrap_or_default();

        if storage::providers::find_provider(database, name)
            .await?
            .is_some()
        {
            return persist_terminal(
                database,
                flow,
                "name_conflict",
                "the provider name the flow creates is already taken",
            )
            .await;
        }
    }

    let product = completion::complete_exchange(http, definition, outcome, flow).await?;

    let claim = flow.device_poll_started_at.ok_or_else(|| Error::Internal {
        message: "the claimed flow carries no claim stamp".to_string(),
    })?;

    let mut transaction = database.transaction().await?;

    let result_provider_id =
        completion::write_completion_provider(&mut transaction, flow, definition, &product).await?;

    let Some(result_provider_id) = result_provider_id else {
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{commit_success, flow};
    use crate::{
        http_client,
        provider::StoredCredentials,
        storage,
        storage::oauth_flows::{
            FlowGrant, FlowOutcome, FlowPurpose, NewFlow, claim_due_device_flow,
            find_flow_by_state, insert_flow,
        },
        test_support,
    };
    use std::collections::BTreeMap;

    /// A create flow whose provider name another row already holds terminates as failed with
    /// the gateway-owned `name_conflict` pair instead of being re-claimed and re-failing until
    /// the expiry sweep deletes it (README §7.2).
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the flow outcome diverges.
    #[tokio::test]
    async fn a_create_flow_whose_name_is_taken_fails_terminally() {
        let (database, _guard) = test_support::create_test_database().await;

        let static_definition = test_support::insert_static_key_definition(
            &database,
            "static-def",
            crate::format::UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        test_support::insert_provider(
            &database,
            &static_definition,
            "taken",
            "t-",
            None,
            StoredCredentials::ApiKey {
                api_key: "sk".to_string(),
            },
        )
        .await;

        let definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("taken".to_string()),
                model_prefix: Some(String::new()),
                base_url: None,
                target_provider_id: None,
                grant_type: FlowGrant::Device,
                slots: BTreeMap::new(),
                code_verifier: None,
                device_code: Some("device-code".to_string()),
                device_user_code: Some("user-code".to_string()),
                device_verification_uri: Some("https://verify.example".to_string()),
                device_verification_uri_complete: None,
                device_poll_interval_seconds: Some(5),
                device_next_poll_at: Some(now - 1),
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the due flow is claimed")]
        let claimed = claim_due_device_flow(&database, now, now - 1)
            .await
            .unwrap()
            .expect("the due flow is claimed");

        commit_success(
            &database,
            &http_client::control_plane(),
            &claimed,
            &definition,
            flow::ExchangeOutcome {
                access_token: Some("exchange-token".to_string()),
                ..flow::ExchangeOutcome::default()
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        assert_eq!(settled.outcome, Some(FlowOutcome::Failed));
        assert_eq!(settled.failure_code.as_deref(), Some("name_conflict"));

        // The conflicting row is untouched: the pre-check fired before any provider write.
        assert!(
            storage::providers::find_provider(&database, "taken")
                .await
                .unwrap()
                .is_some()
        );
    }
}
