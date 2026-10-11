//! Device-poller tests: the due-flow claim with its staleness recovery, the per-attempt
//! scheduling decisions over every exchange classification — pending, slow-down, non-terminal,
//! and terminal codes, the consumed-grant terminal — and the checkpointed-exchange resume,
//! against the mock upstream over the real storage tier
//! (`docs/design/providers.md` and `docs/design/storage.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or serve is that test's failure report"
)]

use super::{claim_next_due_flow, process, schedule_retry};
use crate::{
    database::Database,
    provider::{ProviderDefinition, StoredCredentials, oauth::tests_support},
    storage,
    storage::oauth_flows::{self, FlowGrant, FlowOutcome, FlowPurpose, NewFlow, OngoingFlow},
    test_support::{TempDatabaseGuard, mock_upstream::MockUpstream, temp_database},
    unix_time,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error as StdError};

/// A stored device-grant definition named `fixture-definition` over the mock `base`, inserted
/// so a flow row can reference it, with the owning database and its lifetime guard.
async fn stored_device_definition(
    base: &str,
) -> Result<(ProviderDefinition, Database, TempDatabaseGuard), Box<dyn StdError>> {
    let (database, guard) = temp_database().await;

    let payload = tests_support::parse(tests_support::device_payload(base));

    let definition =
        crate::provider::tests_support::definition_named("fixture-definition", payload);

    storage::provider_definitions::insert_definition(
        &database,
        storage::provider_definitions::NewDefinition {
            name: definition.name.as_str(),
            base_version: None,
            payload: &definition.payload,
            created_at: definition.created_at,
        },
    )
    .await?;

    Ok((definition, database, guard))
}

/// Inserts one pending device create flow due `late_by_seconds` ago, storing the reported
/// `interval_seconds` and carrying the `dev-1` device code.
async fn due_flow(
    database: &Database,
    definition: &ProviderDefinition,
    state: &str,
    interval_seconds: i64,
    late_by_seconds: i64,
) -> Result<OngoingFlow, Box<dyn StdError>> {
    let now = unix_time::unix_now_micros();

    let flow = oauth_flows::insert_flow(
        database,
        NewFlow {
            definition_name: definition.name.clone(),
            definition_version: definition.version,
            state_token: state.to_string(),
            purpose: FlowPurpose::Create,
            provider_name: Some("fixture-provider".to_string()),
            model_prefix: Some("fixture.".to_string()),
            base_url: None,
            target_provider_id: None,
            grant_type: FlowGrant::Device,
            slots: BTreeMap::new(),
            code_verifier: None,
            device_code: Some("dev-1".to_string()),
            device_user_code: Some("ABCD-1234".to_string()),
            device_verification_uri: Some("https://up.test/device".to_string()),
            device_verification_uri_complete: None,
            device_poll_interval_seconds: Some(interval_seconds),
            device_next_poll_at: Some(now - late_by_seconds * unix_time::MICROS_PER_SEC),
            expires_at: now + 600 * unix_time::MICROS_PER_SEC,
        },
    )
    .await?;

    Ok(flow)
}

/// Re-reads the flow row of `state` with its post-processing facts.
async fn flow_of(database: &Database, state: &str) -> Result<OngoingFlow, Box<dyn StdError>> {
    oauth_flows::find_flow_by_state(database, state, unix_time::unix_now_micros())
        .await?
        .map_or_else(
            || Err(Box::<dyn StdError>::from("the unexpired flow row vanished")),
            Ok,
        )
}

/// Asserts the flow's next-poll instant sits `interval_seconds` ahead of the `before` stamp,
/// within the wall-clock tolerance the awaited poll attempt itself consumes.
fn assert_scheduled(flow: &OngoingFlow, before: i64, interval_seconds: i64) {
    let delay = flow
        .device_next_poll_at
        .expect("the retry schedules the next attempt")
        - before;

    let micros = interval_seconds * unix_time::MICROS_PER_SEC;

    assert!(
        delay >= micros && delay <= micros + 3 * unix_time::MICROS_PER_SEC,
        "the next attempt is {delay} micros out, expected about {micros}"
    );
}

/// The RFC polling code bodies and the fixture token response the mocks serve.
mod bodies {
    use serde_json::json;

    /// The success-status body upstreams report while the user has not approved yet.
    pub(super) fn pending() -> String {
        json!({"error": "authorization_pending"}).to_string()
    }

    /// The success-status body that permanently raises the poll interval.
    pub(super) fn slow_down() -> String {
        json!({"error": "slow_down"}).to_string()
    }

    /// The fixture token response: both tokens and a one-hour lifetime.
    pub(super) fn token_response() -> String {
        json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600}).to_string()
    }
}

/// The claim hands out exactly the due flows, stamps the claim, and never hands the same
/// claimed flow out again while its stamp is fresh; a not-yet-due flow stays untouched.
#[tokio::test]
async fn claims_only_due_flows_and_stamps_them() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) = stored_device_definition("http://127.0.0.1:1").await?;

    let due = due_flow(&database, &definition, "due-state", 3, 1).await?;

    due_flow(&database, &definition, "future-state", 3, -60).await?;

    let claimed = claim_next_due_flow(&database)
        .await?
        .expect("the due flow is claimable");

    assert_eq!(claimed.id, due.id);

    let stamp = claimed
        .device_poll_started_at
        .expect("the claim stamps the row");

    assert!(
        stamp <= unix_time::unix_now_micros(),
        "the stamp is a past instant"
    );

    assert!(
        claim_next_due_flow(&database).await?.is_none(),
        "a freshly claimed flow is not reclaimable and the future flow is not due"
    );

    Ok(())
}

/// A claim older than the staleness window is recovered by the next claimant — the competing
/// pollers never leave a dead poller's flow stranded — while the still-fresh window keeps
/// recovering hands off.
#[tokio::test]
async fn a_stale_claim_is_recovered_by_the_next_claimant() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) = stored_device_definition("http://127.0.0.1:1").await?;

    due_flow(&database, &definition, "stale-state", 3, 1).await?;

    let claimed = claim_next_due_flow(&database)
        .await?
        .expect("the due flow is claimable");

    // Backdate the held claim just past the staleness window: the next claimant recovers the
    // row a dead poller would have abandoned.
    let abandoned_at = unix_time::unix_now_micros()
        - super::CLAIM_STALE_AFTER.as_micros() as i64
        - unix_time::MICROS_PER_SEC;

    database
        .execute(
            "UPDATE ongoing_oauth_flows SET device_poll_started_at = ?1 WHERE id = ?2",
            [abandoned_at, claimed.id],
        )
        .await?;

    let recovered = claim_next_due_flow(&database)
        .await?
        .expect("the stale claim is recoverable");

    assert_eq!(recovered.id, claimed.id);

    assert!(
        recovered
            .device_poll_started_at
            .expect("the recovery re-stamps")
            > abandoned_at,
        "the recovery carries a fresh stamp"
    );

    Ok(())
}

/// The pending classification reschedules at the stored interval and keeps the flow pending:
/// the gateway polls independently of the browser.
#[tokio::test]
async fn pending_reschedules_at_the_stored_interval() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![bodies::pending()]).await;

    let (definition, database, _guard) = stored_device_definition(&mock.base()).await?;

    due_flow(&database, &definition, "pending-state", 3, 1).await?;

    let flow = claim_next_due_flow(&database).await?.expect("due");

    let before = unix_time::unix_now_micros();

    process(&database, &reqwest::Client::new(), &flow).await?;

    let updated = flow_of(&database, "pending-state").await?;

    assert_eq!(
        updated.outcome, None,
        "a pending answer never ends the flow"
    );

    assert_eq!(updated.device_poll_interval_seconds, Some(3));

    assert_scheduled(&updated, before, 3);

    assert_eq!(recorded.lock().expect("the mock records").len(), 1);

    Ok(())
}

/// The slow-down classification permanently raises the stored interval by the penalty and
/// schedules the next attempt at the raised interval (RFC 8628 §3.5).
#[tokio::test]
async fn slow_down_raises_the_stored_interval() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![bodies::slow_down()]).await;

    let (definition, database, _guard) = stored_device_definition(&mock.base()).await?;

    due_flow(&database, &definition, "slow-state", 3, 1).await?;

    let flow = claim_next_due_flow(&database).await?.expect("due");

    let before = unix_time::unix_now_micros();

    process(&database, &reqwest::Client::new(), &flow).await?;

    let updated = flow_of(&database, "slow-state").await?;

    assert_eq!(updated.outcome, None, "a slow-down never ends the flow");

    assert_eq!(
        updated.device_poll_interval_seconds,
        Some(3 + super::super::exchange::SLOW_DOWN_PENALTY_SECONDS),
        "the raised interval persists as the stored schedule"
    );

    assert_scheduled(
        &updated,
        before,
        3 + super::super::exchange::SLOW_DOWN_PENALTY_SECONDS,
    );

    Ok(())
}

/// A failure code outside the configured terminal codes never ends the flow: the schedule
/// rides the stored interval until the flow's own expiry bounds it.
#[tokio::test]
async fn a_non_terminal_code_keeps_the_schedule() -> Result<(), Box<dyn StdError>> {
    let (mock, _) =
        MockUpstream::start_json(vec![json!({"error": "consent_required"}).to_string()]).await;

    let (definition, database, _guard) = stored_device_definition(&mock.base()).await?;

    due_flow(&database, &definition, "other-state", 3, 1).await?;

    let flow = claim_next_due_flow(&database).await?.expect("due");

    let before = unix_time::unix_now_micros();

    process(&database, &reqwest::Client::new(), &flow).await?;

    let updated = flow_of(&database, "other-state").await?;

    assert_eq!(
        updated.outcome, None,
        "an unconfigured code never ends the flow"
    );

    assert_eq!(updated.device_poll_interval_seconds, Some(3));

    assert_scheduled(&updated, before, 3);

    Ok(())
}

/// A configured terminal code consumes the flow into the gateway-owned `upstream_rejected`
/// pair naming the code, and the flow stops polling.
#[tokio::test]
async fn a_terminal_code_fails_the_flow_with_the_gateway_pair() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) =
        MockUpstream::start_json(vec![json!({"error": "access_denied"}).to_string()]).await;

    let (definition, database, _guard) = stored_device_definition(&mock.base()).await?;

    due_flow(&database, &definition, "terminal-state", 3, 1).await?;

    let flow = claim_next_due_flow(&database).await?.expect("due");

    process(&database, &reqwest::Client::new(), &flow).await?;

    let updated = flow_of(&database, "terminal-state").await?;

    assert_eq!(updated.outcome, Some(FlowOutcome::Failed));

    let (code, message) = updated.failure_pair();

    assert_eq!(code, "upstream_rejected");

    assert!(
        message.contains("rejected the device code with `access_denied`"),
        "the terminal message names the configured code: {message}"
    );

    assert_eq!(recorded.lock().expect("the mock records").len(), 1);

    Ok(())
}

/// A decoded 2xx token response whose mapped fields cannot fold terminates the flow — the
/// single-use device code is consumed upstream whether or not the gateway could use the
/// response, so no retry is scheduled.
#[tokio::test]
async fn an_unusable_token_response_consumes_the_flow() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![json!({}).to_string()]).await;

    let (definition, database, _guard) = stored_device_definition(&mock.base()).await?;

    due_flow(&database, &definition, "consumed-state", 3, 1).await?;

    let flow = claim_next_due_flow(&database).await?.expect("due");

    process(&database, &reqwest::Client::new(), &flow).await?;

    let updated = flow_of(&database, "consumed-state").await?;

    assert_eq!(updated.outcome, Some(FlowOutcome::Failed));

    let (code, message) = updated.failure_pair();

    assert_eq!(code, "exchange_failed");

    assert!(
        message.contains("unusable"),
        "the terminal message diagnoses the unusable response: {message}"
    );

    Ok(())
}

/// A completed exchange is checkpointed before provisioning and committed in one transaction:
/// the provider row appears with the exchanged credentials and the flow carries the succeeded
/// outcome pointing at it.
#[tokio::test]
async fn a_completed_exchange_commits_the_provider() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![bodies::token_response()]).await;

    let (definition, database, _guard) = stored_device_definition(&mock.base()).await?;

    due_flow(&database, &definition, "success-state", 3, 1).await?;

    let flow = claim_next_due_flow(&database).await?.expect("due");

    process(&database, &reqwest::Client::new(), &flow).await?;

    let updated = flow_of(&database, "success-state").await?;

    assert_eq!(updated.outcome, Some(FlowOutcome::Succeeded));

    let provider = storage::providers::find_provider_by_id(&database, updated.result_provider_id())
        .await?
        .expect("the committed provider exists");

    assert_eq!(provider.name, "fixture-provider");

    let StoredCredentials::OAuth {
        access_token,
        refresh_token,
        expires_at,
        ..
    } = &provider.credentials
    else {
        panic!("the completed flow stores OAuth credentials");
    };

    assert_eq!(access_token, "at-1");

    assert_eq!(refresh_token.as_deref(), Some("rt-1"));

    let remaining =
        expires_at.expect("expires_in schedules the refresh") - unix_time::unix_now_micros();

    assert!(
        remaining > 3595 * unix_time::MICROS_PER_SEC
            && remaining < 3601 * unix_time::MICROS_PER_SEC,
        "the lifetime rides expires_in"
    );

    assert_eq!(recorded.lock().expect("the mock records").len(), 1);

    Ok(())
}

/// A claim recovered with an exchange checkpoint aboard resumes into provisioning and commit
/// from the checkpoint — never re-presenting the consumed device code to the upstream.
#[tokio::test]
async fn a_checkpointed_flow_resumes_without_reexchanging() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) = stored_device_definition("http://127.0.0.1:1").await?;

    due_flow(&database, &definition, "checkpoint-state", 3, 1).await?;

    let claimed = claim_next_due_flow(&database).await?.expect("due");

    let stamp = claimed
        .device_poll_started_at
        .expect("the claim stamps the row");

    let checkpoint = json!({
        "access_token": "at-checkpoint",
        "refresh_token": "rt-checkpoint",
        "expires_in_seconds": 1200,
        "slots": {}
    })
    .to_string();

    assert!(
        storage::oauth_flows::persist_exchange_checkpoint(
            &database,
            claimed.id,
            stamp,
            &checkpoint
        )
        .await?,
        "the checkpoint persist lands on the held claim"
    );

    // The recovered claimant reads the row fresh: the checkpoint the dead poller persisted
    // rides the flow row it hands back to the engine. Any exchange attempt against this mock
    // fails immediately — a re-presented device code would turn the resumed commit into a
    // failure.
    let (mock, recorded) =
        MockUpstream::start(vec![crate::test_support::mock_upstream::MockAction::Close]).await;

    let _ = mock.base();

    // The dead poller's claim ages past the staleness window before the next claimant wakes.
    let abandoned_at = unix_time::unix_now_micros()
        - super::CLAIM_STALE_AFTER.as_micros() as i64
        - unix_time::MICROS_PER_SEC;

    database
        .execute(
            "UPDATE ongoing_oauth_flows SET device_poll_started_at = ?1 WHERE id = ?2",
            [abandoned_at, claimed.id],
        )
        .await?;

    let resumed = claim_next_due_flow(&database)
        .await?
        .expect("the checkpointed flow is recovered");

    process(&database, &reqwest::Client::new(), &resumed).await?;

    let updated = flow_of(&database, "checkpoint-state").await?;

    assert_eq!(updated.outcome, Some(FlowOutcome::Succeeded));

    let provider = storage::providers::find_provider_by_id(&database, updated.result_provider_id())
        .await?
        .expect("the resumed commit wrote the provider");

    let StoredCredentials::OAuth { access_token, .. } = &provider.credentials else {
        panic!("the completed flow stores OAuth credentials");
    };

    assert_eq!(
        access_token, "at-checkpoint",
        "the commit rides the checkpoint"
    );

    assert!(
        recorded.lock().expect("the mock records").is_empty(),
        "the resume never re-presents the consumed device code"
    );

    Ok(())
}

/// The transport-timeout backoff doubles only the next attempt's delay while the stored
/// interval persists unchanged — the transient doubling never compounds across claims — and
/// every other failure keeps the stored interval for both the delay and the stored schedule.
#[tokio::test]
async fn timeout_backoff_doubles_one_attempt_only() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) = stored_device_definition("http://127.0.0.1:1").await?;

    // One flow per arm: a persisted retry releases the claim, so each arm drives its own
    // claimed row.
    due_flow(&database, &definition, "timeout-state", 3, 1).await?;

    due_flow(&database, &definition, "plain-state", 3, 1).await?;

    let timed_out = claim_next_due_flow(&database).await?.expect("due");

    let before = unix_time::unix_now_micros();

    schedule_retry(&database, &timed_out, true).await?;

    let backed_off = flow_of(&database, "timeout-state").await?;

    assert_eq!(
        backed_off.device_poll_interval_seconds,
        Some(3),
        "the stored interval never compounds"
    );

    assert_scheduled(&backed_off, before, 6);

    let plain_flow = claim_next_due_flow(&database).await?.expect("due");

    schedule_retry(&database, &plain_flow, false).await?;

    let plain = flow_of(&database, "plain-state").await?;

    assert_eq!(plain.device_poll_interval_seconds, Some(3));

    assert_scheduled(&plain, before, 3);

    Ok(())
}
