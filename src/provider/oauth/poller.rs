//! The background device poller: one task resumes every unexpired pending device flow from its
//! stored schedule at startup and keeps polling newly started flows (README §3, §7.2).
//!
//! Claiming a due flow is one conditional `UPDATE … RETURNING` — the conditional database lease —
//! and a claim older than the control-request timeout is recovered by the next claimant, so
//! competing pollers and restarts never double-exchange an upstream grant. Only the current
//! claimant persists the result or next schedule: every persist conditions on the claim stamp.

use super::{
    completion,
    exchange::{
        DEFAULT_DEVICE_INTERVAL_SECONDS, ExchangeOutcome, ExchangeProduct,
        SLOW_DOWN_PENALTY_SECONDS, execute_exchange,
    },
};
use crate::{
    database::Database,
    http_client,
    provider::{
        Error,
        definition::{
            credential::{CredentialAcquisition, Grant, OAuthRecipe},
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

/// How long a device-flow claim is trusted before another poller steals it: the control-plane
/// client bounds every exchange attempt at 30 seconds, so a claim older than that belongs to a
/// poller that died mid-exchange.
const CLAIM_STALE_AFTER: Duration = http_client::CONTROL_PLANE_TOTAL_TIMEOUT;

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
            flow.device_poll_started_at,
            "internal_error",
            "the flow's provider definition vanished",
        )
        .await;
    };

    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
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
    } = &**recipe;

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

    let product = match execute_exchange(http, exchange, &env).await {
        Ok(product) => product,

        // A transport timeout backs off the next attempt — the stored interval is persisted
        // unchanged, so the transient doubling never compounds across claims (RFC 8628 §3.5
        // reserves the permanent raise for `slow_down`); any other transport or response
        // failure schedules the plain interval retry, so a transient outage never kills the
        // flow.
        Err(error) => {
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
            // (README §5.3): only the configured codes — plus the engine-owned polling codes —
            // end a flow. The retry rides the stored interval until the flow's own expiry
            // bounds it, so a transient upstream rejection never kills the flow mid-window.
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
            commit_success(database, http, flow, &definition, outcome).await
        }
    }
}

/// Schedules the next attempt after a failed transport round-trip: a timeout doubles only the
/// next attempt's delay while the stored interval is persisted unchanged — the transient
/// backoff never compounds across claims — and everything else keeps the stored interval.
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
            .map(|doubled| doubled.min(BACKOFF_INTERVAL_CAP_SECONDS))
            .ok_or_else(|| interval_overflow(stored))?
    } else {
        stored
    };

    schedule_retry_at(database, flow, stored, delay).await
}

/// Schedules the next attempt while the caller still owns its claim: `delay_seconds` sets the
/// next attempt instant and `interval_seconds` is the flow interval persisted as the stored
/// schedule. The two differ only on a transport-timeout backoff, whose doubling is transient.
/// A lost claim is ordinary contention: the winning poller owns the schedule, so the discard is
/// logged and ignored.
async fn schedule_retry_at(
    database: &Database,
    flow: &OngoingFlow,
    interval_seconds: i64,
    delay_seconds: i64,
) -> Result<(), Error> {
    let Some(claim) = flow.device_poll_started_at else {
        return Ok(());
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
async fn persist_terminal(
    database: &Database,
    flow: &OngoingFlow,
    claim_started_at: Option<i64>,
    code: &str,
    message: &str,
) -> Result<(), Error> {
    let Some(claim) = claim_started_at else {
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
/// the commit transaction itself — after the re-stamp — fails the flow the same terminal way
/// with the gateway-owned `commit_failed` pair, because the exchange is consumed upstream
/// either way; a claim the re-stamp finds stolen abandons the provider write, leaving the
/// newer state untouched.
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
    // failure pair (README §7.2). The constraint stays as the concurrent backstop behind this
    // pre-check.
    if flow.purpose == FlowPurpose::Create {
        let name = flow.provider_name.as_deref().unwrap_or_default();

        if storage::providers::is_provider_name_taken(database, name).await? {
            return persist_terminal(
                database,
                flow,
                flow.device_poll_started_at,
                "name_conflict",
                "the provider name the flow creates is already taken",
            )
            .await;
        }
    }

    let held_claim = flow.device_poll_started_at.ok_or_else(|| Error::Internal {
        message: "the claimed flow carries no claim stamp".to_string(),
    })?;

    let product = match completion::complete_exchange(http, definition, outcome, flow).await {
        Ok(product) => product,

        // A provisioning failure after a successful exchange fails the flow terminally (README
        // §3, §7.2): the exchange result cannot be resumed — a retry would re-run the exchange
        // against the upstream — so the gateway-owned pair persists with the claim the flow
        // still holds (the failure short-circuits before the re-stamp below) and the poll loop
        // stops at the terminal outcome. A claim stolen while provisioning ran matches no row
        // and discards the outcome, like every persist.
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
        if let Err(persist_error) = persist_terminal(
            database,
            flow,
            Some(claim),
            "commit_failed",
            &format!("the exchange succeeded but the gateway failed to record the result: {error}"),
        )
        .await
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

/// The gateway-owned terminal message one completion-pipeline failure stores (README §7.2):
/// the error's own rendering — transport, mapping, and drift details are gateway-authored —
/// except [`Error::Rejected`], whose `Display` embeds the truncated upstream body excerpt; it
/// renders from the status alone, so no upstream body content ever lands in a terminal result.
fn provisioning_failure_message(error: &Error) -> String {
    match error {
        Error::Rejected { status, .. } => {
            format!("the provisioning request failed with upstream status {status}")
        }
        error => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{ExchangeOutcome, claim_next_due_flow, commit_success, process, schedule_retry};
    use crate::{
        http_client,
        provider::StoredCredentials,
        storage,
        storage::oauth_flows::{
            FlowGrant, FlowOutcome, FlowPurpose, NewFlow, claim_due_device_flow,
            find_flow_by_state, insert_flow, restamp_claim,
        },
        test_support::{self, MockUpstream},
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
            ExchangeOutcome {
                access_token: Some("exchange-token".to_string()),
                ..ExchangeOutcome::default()
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

    /// A transport timeout's backoff is transient (RFC 8628 §3.5 reserves a permanent raise for
    /// `slow_down`): the timeout schedules one doubled delay for the next attempt while the
    /// flow's stored interval persists unchanged, and the next attempt — reading the preserved
    /// base — writes it back, so the interval never ratchets across claims.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the backoff schedule diverges from the contract.
    #[tokio::test]
    async fn a_transport_timeout_backs_off_transitorily_and_never_ratchets_the_stored_interval() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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

        let before = crate::unix_time::unix_now_micros();

        schedule_retry(&database, &claimed, true).await.unwrap();

        let after = crate::unix_time::unix_now_micros();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let backed_off = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row survives the timeout");

        // The stored base interval is untouched; only the next attempt's delay doubled.
        assert_eq!(
            backed_off.device_poll_interval_seconds,
            Some(5),
            "the timeout must not raise the stored interval"
        );

        let doubled = 10 * crate::unix_time::MICROS_PER_SEC;

        assert!(
            backed_off.device_next_poll_at.unwrap() >= before + doubled
                && backed_off.device_next_poll_at.unwrap() <= after + doubled,
            "the next attempt rides the doubled delay"
        );

        assert_eq!(backed_off.device_poll_started_at, None);

        // The next attempt — due at the doubled instant — reads the preserved base and writes
        // it back, so the interval never ratchets.
        let due_at = backed_off.device_next_poll_at.unwrap();

        #[expect(clippy::expect_used, reason = "the due flow is claimed again")]
        let claimed_again =
            claim_due_device_flow(&database, due_at, due_at - crate::unix_time::MICROS_PER_SEC)
                .await
                .unwrap()
                .expect("the flow is due again at the doubled instant");

        assert_eq!(
            claimed_again.device_poll_interval_seconds,
            Some(5),
            "the next attempt reads the preserved base"
        );

        let before = crate::unix_time::unix_now_micros();

        schedule_retry(&database, &claimed_again, false)
            .await
            .unwrap();

        let after = crate::unix_time::unix_now_micros();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row survives the next failure");

        assert_eq!(settled.device_poll_interval_seconds, Some(5));

        let base = 5 * crate::unix_time::MICROS_PER_SEC;

        assert!(
            settled.device_next_poll_at.unwrap() >= before + base
                && settled.device_next_poll_at.unwrap() <= after + base,
            "the next attempt rides the stored interval"
        );
    }

    /// The connection-timeout backoff stops doubling at the cap, and the cap bounds only the
    /// transient next-attempt delay: an upstream-reported interval above it persists unchanged.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the capped schedule diverges from the contract.
    #[tokio::test]
    async fn a_transport_timeout_backoff_caps_the_delay_without_raising_the_stored_interval() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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
                device_poll_interval_seconds: Some(100),
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

        let before = crate::unix_time::unix_now_micros();

        schedule_retry(&database, &claimed, true).await.unwrap();

        let after = crate::unix_time::unix_now_micros();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let backed_off = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row survives the timeout");

        // The doubled delay is capped while the upstream-reported interval persists unchanged.
        assert_eq!(
            backed_off.device_poll_interval_seconds,
            Some(100),
            "the capped backoff must not raise the stored interval"
        );

        let capped = super::BACKOFF_INTERVAL_CAP_SECONDS * crate::unix_time::MICROS_PER_SEC;

        assert!(
            backed_off.device_next_poll_at.unwrap() >= before + capped
                && backed_off.device_next_poll_at.unwrap() <= after + capped,
            "the next attempt rides the capped delay"
        );
    }

    /// The batch's claim staleness threshold derives from each claim's own instant, not the
    /// tick start: a claim that has aged past the staleness window by the time the batch
    /// reaches its flow is recovered by that very claim call, while a claim still inside the
    /// window at the call's own instant is respected — so a slow flow ahead in the batch can
    /// never stretch a dead poller's recovery delay.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the claims diverge from the contract.
    #[tokio::test]
    async fn a_claim_recovers_a_claim_stale_at_the_claim_call_s_own_instant() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        // Two due flows whose claims are fabricated through the same conditional claim the
        // poller uses: one stamped 31 seconds ago — past the staleness window relative to the
        // claim call's own instant — and one stamped 29 seconds ago, still inside it. The
        // fresh fixture's threshold protects the stale stamp, so each stamp lands on its own
        // row.
        let stale_at = now - 31 * crate::unix_time::MICROS_PER_SEC;
        let fresh_at = now - 29 * crate::unix_time::MICROS_PER_SEC;

        for (state_token, claim_at) in [("stale-claim", stale_at), ("fresh-claim", fresh_at)] {
            insert_flow(
                &database,
                NewFlow {
                    definition_name: definition.name.clone(),
                    definition_version: definition.version,
                    state_token: state_token.to_string(),
                    purpose: FlowPurpose::Create,
                    provider_name: Some("created".to_string()),
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
                    device_next_poll_at: Some(claim_at - 1),
                    expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
                },
            )
            .await
            .unwrap();

            #[expect(clippy::expect_used, reason = "the fixture flow is claimed")]
            let claimed = claim_due_device_flow(&database, claim_at, stale_at)
                .await
                .unwrap()
                .expect("the fixture flow is claimed");

            assert_eq!(claimed.state_token, state_token);
        }

        // The poller's claim call derives its threshold from its own instant: the 31-second-old
        // claim is past the window and is recovered, with a fresh stamp.
        #[expect(clippy::expect_used, reason = "the stale claim is recovered")]
        let recovered = claim_next_due_flow(&database)
            .await
            .unwrap()
            .expect("the claim past the window at the call's own instant is recovered");

        assert_eq!(recovered.state_token, "stale-claim");
        assert!(
            recovered.device_poll_started_at.unwrap() > stale_at,
            "the recovery stamps a fresh claim"
        );

        // The 29-second-old claim is respected — still inside the window at the call's own
        // instant — so nothing else is due.
        assert!(
            claim_next_due_flow(&database).await.unwrap().is_none(),
            "a claim inside the window at the call's own instant is respected"
        );
    }

    /// Inserts one device-grant OAuth definition whose exchange points at `token_url` and whose
    /// terminal codes exclude the non-terminal error the fixture's upstream reports. With
    /// `has_failing_provisioning`, the definition also carries one provisioning step whose
    /// required `machine` extraction fails against the mock's second response — a provisioning
    /// failure the exchange itself survives.
    ///
    /// # Panics
    ///
    /// Panics if the fixture payload fails to parse, validate, or insert — a broken fixture is
    /// the failure report.
    async fn insert_device_poll_definition(
        database: &crate::database::Database,
        name: &str,
        token_url: &str,
        has_failing_provisioning: bool,
    ) -> crate::provider::ProviderDefinition {
        let device_grant_type = "urn:ietf:params:oauth:grant-type:device_code";

        // The optional provisioning member splices into the wholesale payload; when the fixture
        // carries no provisioning, the explicit `null` member deserializes to `None` like an
        // absent one.
        let provisioning = has_failing_provisioning.then(|| {
            serde_json::json!([
                {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": format!("{token_url}/provision")}]
                    },
                    "response_encoding": "json",
                    "extract": [
                        {
                            "path": ["machine"],
                            "target": {"type": "slot", "name": "machine"},
                            "shape": {"type": "string"},
                            "required": true
                        }
                    ]
                }
            ])
        });

        let payload_json = serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "oauth",
                "client_id": "fixture-client",
                "grant": {
                    "type": "device",
                    "authorization": {
                        "request": {
                            "method": "POST",
                            "url": [{"type": "literal",
                                     "value": "https://example.test/device/code"}]
                        },
                        "response": {
                            "encoding": "json",
                            "fields": [
                                {
                                    "path": ["device_code"],
                                    "target": {"type": "device_code"},
                                    "required": true
                                },
                                {
                                    "path": ["user_code"],
                                    "target": {"type": "user_code"},
                                    "required": true
                                },
                                {
                                    "path": ["verification_uri"],
                                    "target": {"type": "verification_uri"},
                                    "required": true
                                },
                                {
                                    "path": ["expires_in"],
                                    "target": {"type": "expires_in_seconds"},
                                    "required": false
                                }
                            ]
                        }
                    },
                    "expires_in_fallback_seconds": 900
                },
                "exchange": {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": token_url}],
                        "body": {"kind": "form", "fields": [
                            {
                                "name": "grant_type",
                                "value": [{"type": "literal", "value": device_grant_type}]
                            },
                            {
                                "name": "device_code",
                                "value": [{"type": "device_code"}]
                            }
                        ]}
                    },
                    "response": {
                        "encoding": "json",
                        "fields": [
                            {
                                "path": ["access_token"],
                                "target": {"type": "access_token"},
                                "required": true
                            }
                        ]
                    },
                    "terminal_codes": ["access_denied"]
                },
                "refresh": {"type": "none"}
            },
            "provisioning": provisioning
        });

        #[expect(clippy::unwrap_used, reason = "the fixture payload always parses")]
        let payload: crate::provider::definition::DefinitionPayload =
            serde_json::from_value(payload_json).unwrap();

        #[expect(
            clippy::unwrap_used,
            reason = "the fixture definition always validates"
        )]
        payload.validate().unwrap();

        #[expect(clippy::unwrap_used, reason = "the fixture definition always inserts")]
        storage::provider_definitions::insert_definition(
            database,
            storage::provider_definitions::NewDefinition {
                name,
                base_version: None,
                payload: &payload,
                created_at: 1,
            },
        )
        .await
        .unwrap()
    }

    /// A failure code outside the configured terminal codes is not a terminal outcome (README
    /// §5.3): the attempt schedules a retry at the stored interval and the flow survives until
    /// its own expiry — a transient upstream rejection, such as a 2xx
    /// `temporarily_unavailable` body, never kills the flow mid-window.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the flow's schedule diverges.
    #[tokio::test]
    async fn a_non_terminal_error_code_schedules_a_retry_and_keeps_the_flow() {
        let (database, _guard) = test_support::create_test_database().await;

        let mock = MockUpstream::start(vec![
            serde_json::to_string(&serde_json::json!({
                "error": "temporarily_unavailable"
            }))
            .unwrap(),
        ])
        .await;

        let definition =
            insert_device_poll_definition(&database, "device-def", &mock.base(), false).await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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

        process(&database, &http_client::control_plane(), &claimed)
            .await
            .unwrap();

        #[expect(clippy::expect_used, reason = "the flow row survives")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row survives the non-terminal error");

        // The flow is still pending: no terminal outcome, the retry rides the stored interval,
        // and the claim released.
        assert_eq!(settled.outcome, None);
        assert_eq!(settled.device_poll_interval_seconds, Some(5));
        assert!(settled.device_next_poll_at.unwrap() >= now);
        assert_eq!(settled.device_poll_started_at, None);

        mock.shutdown.send(()).ok();
    }

    /// The claim re-stamp is a compare-and-set on the held stamp (README §3, §5.4): the
    /// owner's re-stamp freshens the stored stamp for the commit, while a re-stamp whose claim
    /// a competing tick has stolen matches no row and reports the loss, leaving the winner's
    /// stamp untouched.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the re-stamp diverges from the contract.
    #[tokio::test]
    async fn restamping_a_held_claim_freshens_it_and_a_stolen_one_fails() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        // The claim is stamped 40 seconds in the past — past the staleness window, the state
        // provisioning longer than the window leaves the row in.
        let claim_at = now - 40 * crate::unix_time::MICROS_PER_SEC;

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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
                device_next_poll_at: Some(claim_at - 1),
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the due flow is claimed")]
        let claimed = claim_due_device_flow(&database, claim_at, claim_at - 1)
            .await
            .unwrap()
            .expect("the due flow is claimed");

        #[expect(clippy::expect_used, reason = "the held claim re-stamps")]
        let fresh = restamp_claim(&database, claimed.id, claim_at)
            .await
            .unwrap()
            .expect("the held claim re-stamps");

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let restamped = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        assert_eq!(restamped.device_poll_started_at, Some(fresh));

        // A competing tick past the staleness window recovers the (fabricated) stale claim.
        let later = now + 40 * crate::unix_time::MICROS_PER_SEC;

        #[expect(clippy::expect_used, reason = "the stale claim is stolen")]
        let stolen = claim_due_device_flow(
            &database,
            later,
            now + 10 * crate::unix_time::MICROS_PER_SEC,
        )
        .await
        .unwrap()
        .expect("the stale claim is stolen");

        assert_eq!(stolen.device_poll_started_at, Some(later));

        assert!(
            restamp_claim(&database, claimed.id, fresh)
                .await
                .unwrap()
                .is_none(),
            "a re-stamp over a stolen claim matches no row"
        );

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        assert_eq!(
            settled.device_poll_started_at,
            Some(later),
            "a failed re-stamp leaves the winner's stamp"
        );
    }

    /// A provisioning sequence that outlives the claim-staleness window still commits (README
    /// §3, §5.4): the claim is re-stamped after provisioning returns and before the commit, so
    /// the success the poller derived is never refused by its own aged claim.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the completion diverges from the contract.
    #[tokio::test]
    async fn provisioning_outliving_the_claim_window_still_commits_the_success() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        // The claim is stamped 40 seconds in the past — the state provisioning longer than the
        // 30-second staleness window leaves the row in when the poller returns.
        let claim_at = now - 40 * crate::unix_time::MICROS_PER_SEC;

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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
                device_next_poll_at: Some(claim_at - 1),
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the due flow is claimed")]
        let claimed = claim_due_device_flow(&database, claim_at, claim_at - 1)
            .await
            .unwrap()
            .expect("the due flow is claimed");

        commit_success(
            &database,
            &http_client::control_plane(),
            &claimed,
            &definition,
            ExchangeOutcome {
                access_token: Some("exchange-token".to_string()),
                ..ExchangeOutcome::default()
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        assert_eq!(settled.outcome, Some(FlowOutcome::Succeeded));
        assert_eq!(settled.device_poll_started_at, None);

        #[expect(clippy::expect_used, reason = "the provider write committed")]
        storage::providers::find_provider(&database, "created")
            .await
            .unwrap()
            .expect("the provider write committed");
    }

    /// A claim a competing poller tick stole while provisioning ran abandons the success
    /// instead of committing: the winner owns the row, so the success derived from the
    /// superseded exchange is discarded with the provider write never attempted (README §3,
    /// §5.4).
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the abandonment diverges from the contract.
    #[tokio::test]
    async fn a_claim_stolen_during_provisioning_abandons_the_success() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        let claim_at = now - 40 * crate::unix_time::MICROS_PER_SEC;

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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
                device_next_poll_at: Some(claim_at - 1),
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the due flow is claimed")]
        let claimed = claim_due_device_flow(&database, claim_at, claim_at - 1)
            .await
            .unwrap()
            .expect("the due flow is claimed");

        // A competing poller tick recovers the stale claim while the first poller is
        // provisioning.
        #[expect(clippy::expect_used, reason = "the stale claim is stolen")]
        let stolen =
            claim_due_device_flow(&database, now, now - 30 * crate::unix_time::MICROS_PER_SEC)
                .await
                .unwrap()
                .expect("the stale claim is stolen");

        assert_eq!(stolen.id, claimed.id);

        commit_success(
            &database,
            &http_client::control_plane(),
            &claimed,
            &definition,
            ExchangeOutcome {
                access_token: Some("exchange-token".to_string()),
                ..ExchangeOutcome::default()
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        assert_eq!(settled.outcome, None, "the winner owns the flow's schedule");

        assert_eq!(settled.device_poll_started_at, Some(now));

        assert!(
            storage::providers::find_provider(&database, "created")
                .await
                .unwrap()
                .is_none(),
            "the abandoned success never wrote the provider"
        );
    }

    /// A reauthorize flow whose target provider vanishes while the commit runs fails
    /// terminally under the fresh re-stamped claim (README §3, §7.2): the commit re-stamps the
    /// claim before the provider write, so the vanished-target failure must persist against the
    /// fresh stamp — persisting against the superseded stamp would leave the flow pending and
    /// unstealable until its expiry sweep.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the flow outcome diverges.
    #[tokio::test]
    async fn a_reauthorize_flow_whose_target_vanishes_mid_commit_fails_under_the_fresh_stamp() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        // The claim is stamped 40 seconds in the past — provisioning outlives the 30-second
        // staleness window, so the commit re-stamps the claim before the provider write.
        let claim_at = now - 40 * crate::unix_time::MICROS_PER_SEC;

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Reauthorize,
                provider_name: None,
                model_prefix: None,
                base_url: None,
                // A target id no provider row holds: the concurrent `providers:delete` has
                // already won by the time the rebind runs.
                target_provider_id: Some(424242),
                grant_type: FlowGrant::Device,
                slots: BTreeMap::new(),
                code_verifier: None,
                device_code: Some("device-code".to_string()),
                device_user_code: Some("user-code".to_string()),
                device_verification_uri: Some("https://verify.example".to_string()),
                device_verification_uri_complete: None,
                device_poll_interval_seconds: Some(5),
                device_next_poll_at: Some(claim_at - 1),
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the due flow is claimed")]
        let claimed = claim_due_device_flow(&database, claim_at, claim_at - 1)
            .await
            .unwrap()
            .expect("the due flow is claimed");

        assert_eq!(claimed.device_poll_started_at, Some(claim_at));

        commit_success(
            &database,
            &http_client::control_plane(),
            &claimed,
            &definition,
            ExchangeOutcome {
                access_token: Some("exchange-token".to_string()),
                ..ExchangeOutcome::default()
            },
        )
        .await
        .unwrap();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        // The vanished-target failure is recorded under the fresh re-stamped claim: the
        // terminal outcome, its gateway-owned pair, and the cleared claim stamp.
        assert_eq!(settled.outcome, Some(FlowOutcome::Failed));
        assert_eq!(settled.failure_code.as_deref(), Some("target_vanished"));
        assert_eq!(
            settled.failure_message.as_deref(),
            Some("the provider the flow reauthorizes was deleted")
        );
        assert_eq!(
            settled.device_poll_started_at, None,
            "the persisted terminal outcome clears the claim stamp"
        );
    }

    /// A provisioning step that fails after a successful exchange fails the flow terminally
    /// (README §3, §7.2): the flow records the gateway-owned `provisioning_failed` pair with
    /// the claim it still holds, the claim is released, and the terminal row is unclaimable —
    /// the poll loop ends instead of re-running the exchange against the upstream every
    /// claim-staleness window or stranding the flow row in its pending state.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the flow outcome diverges.
    #[tokio::test]
    async fn a_provisioning_failure_after_a_completed_exchange_fails_the_flow_terminally() {
        let (database, _guard) = test_support::create_test_database().await;

        // The exchange's response maps the access token; the provisioning step's response
        // carries no `machine` member, so its required extraction fails the step.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&serde_json::json!({"access_token": "exchange-token"})).unwrap(),
            serde_json::to_string(&serde_json::json!({})).unwrap(),
        ])
        .await;

        let definition =
            insert_device_poll_definition(&database, "device-def", &mock.base(), true).await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
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

        process(&database, &http_client::control_plane(), &claimed)
            .await
            .unwrap();

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        // The gateway-owned terminal pair, with no upstream body content in the message.
        assert_eq!(settled.outcome, Some(FlowOutcome::Failed));
        assert_eq!(settled.failure_code.as_deref(), Some("provisioning_failed"));
        assert_eq!(
            settled.failure_message.as_deref(),
            Some("Upstream request failed: provisioning response lacks a required field")
        );

        // The terminal persist released the claim stamp.
        assert_eq!(
            settled.device_poll_started_at, None,
            "the persisted terminal outcome clears the claim stamp"
        );

        // The terminal row is unclaimable, so the poll loop can never re-run the exchange.
        #[expect(clippy::unwrap_used, reason = "the claim query executes")]
        let reclaims = claim_due_device_flow(
            &database,
            now + 60 * crate::unix_time::MICROS_PER_SEC,
            now + 30 * crate::unix_time::MICROS_PER_SEC,
        )
        .await
        .unwrap();

        assert!(
            reclaims.is_none(),
            "a terminal flow is never re-claimed for another exchange"
        );

        // The failed provisioning never wrote the provider.
        assert!(
            storage::providers::find_provider(&database, "created")
                .await
                .unwrap()
                .is_none()
        );

        mock.shutdown.send(()).ok();
    }

    /// A failure of the success commit's transaction after a completed exchange — here a forced
    /// storage error at the provider write, behind the claim re-stamp — fails the flow
    /// terminally (README §3, §7.2): the grant is consumed upstream, so the row records the
    /// gateway-owned `commit_failed` pair under the fresh re-stamped claim the flow still
    /// holds, the claim is released, and the terminal row is unclaimable — the poll loop never
    /// re-runs the already-consumed grant exchange, whose re-presented device code an
    /// RFC-conforming upstream would answer with a misleading terminal rejection. The original
    /// error still propagates for the spawn loop's warning, and the pair records a failure —
    /// the provider write never survived the failed transaction.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the flow outcome diverges.
    #[tokio::test]
    async fn a_commit_failure_after_a_completed_exchange_fails_the_flow_terminally() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = insert_device_poll_definition(
            &database,
            "device-def",
            "http://127.0.0.1:1/token",
            false,
        )
        .await;

        let now = crate::unix_time::unix_now_micros();

        // A reauthorize flow: its commit carries no create-name pre-check, so the forced
        // storage error below surfaces exactly at the transaction's provider write.
        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Reauthorize,
                provider_name: None,
                model_prefix: None,
                base_url: None,
                target_provider_id: Some(1),
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

        // The forced storage error: the provider write's table is gone, so the commit's
        // INSERT-or-UPDATE fails after the exchange completed and the claim was re-stamped,
        // while the flow table the terminal persist writes stays intact.
        database
            .execute("DROP TABLE providers", crate::database::params![])
            .await
            .unwrap();

        let result = commit_success(
            &database,
            &http_client::control_plane(),
            &claimed,
            &definition,
            ExchangeOutcome {
                access_token: Some("exchange-token".to_string()),
                ..ExchangeOutcome::default()
            },
        )
        .await;

        assert!(
            result.is_err(),
            "the commit failure propagates for the spawn loop's warning"
        );

        #[expect(clippy::expect_used, reason = "the flow row persists")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the flow row persists");

        // The gateway-owned terminal pair: the grant succeeded, the record failed — never an
        // overclaiming `succeeded` outcome, never a pending row awaiting a doomed re-run.
        assert_eq!(settled.outcome, Some(FlowOutcome::Failed));
        assert_eq!(settled.failure_code.as_deref(), Some("commit_failed"));
        assert!(
            settled
                .failure_message
                .as_deref()
                .is_some_and(|message| message.starts_with(
                    "the exchange succeeded but the gateway failed to record the result"
                )),
            "the terminal message names the failed record: {:?}",
            settled.failure_message
        );

        assert_eq!(
            settled.device_poll_started_at, None,
            "the persisted terminal outcome clears the claim stamp"
        );

        // The terminal row is unclaimable, so the poll loop can never re-run the consumed
        // exchange.
        #[expect(clippy::unwrap_used, reason = "the claim query executes")]
        let reclaims = claim_due_device_flow(
            &database,
            now + 60 * crate::unix_time::MICROS_PER_SEC,
            now + 30 * crate::unix_time::MICROS_PER_SEC,
        )
        .await
        .unwrap();

        assert!(
            reclaims.is_none(),
            "a terminal flow is never re-claimed for another exchange"
        );
    }
}
