//! Quota coordinator tests: the forced refresh reading and reconciling durable restriction
//! blocks, the cached cadence serving without a second upstream request, failed reads keeping
//! the established restrictions, the administrator reset, and the deleted-account contract —
//! against the mock upstream over the real storage tier (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, store, or serve is that test's failure report"
)]

use super::{Coordinator, LastRefresh, RefreshFailure};
use crate::{
    provider::{StoredCredentials, oauth::tests_support, quota::QuotaApplicability},
    storage::quota_blocks,
    test_support::{mock_upstream::MockUpstream, temp_database},
    unix_time::unix_now_micros,
};
use serde_json::json;
use std::{error::Error as StdError, sync::Arc};

/// One quota entry measured as a used fraction with a future reset, applicable to every model
/// of the account: `{"ratio": <magnitude>, "reset_at": <unix-seconds>}` under `plan`.
fn fraction_descriptor() -> serde_json::Value {
    json!({
        "request": {"method": "GET", "url": [
            {"type": "base"}, {"type": "literal", "value": "/v1/quota"}
        ]},
        "sources": [{
            "type": "object",
            "key": "plan",
            "at": ["plan"],
            "is_required": true,
            "entries": [{
                "key": "credits",
                "label": "Monthly credits",
                "at": [],
                "is_required": true,
                "window": {"type": "duration", "seconds": 2_592_000},
                "measurement": {"type": "ratio",
                                "value": {"field": {"base": "entry", "path": ["ratio"],
                                                    "is_required": true},
                                          "encoding": "number"},
                                "scale": "fraction", "direction": "used"},
                "reset": {"type": "field",
                          "field": {"base": "entry", "path": ["reset_at"], "is_required": true},
                          "encoding": "unix_seconds"},
                "routing": {"type": "all_models"}
            }]
        }]
    })
}

/// The quota reading body of `ratio`.
fn quota_body(ratio: f64) -> String {
    json!({"plan": {"ratio": ratio, "reset_at": unix_now_micros() / 1_000_000 + 3600}}).to_string()
}

/// Serves `first` to the first request and `then` to every later one, recording every request.
async fn switching_mock(first: String, then: String) -> MockUpstream {
    let answers = std::sync::atomic::AtomicUsize::new(0);

    let (mock, _) = MockUpstream::start_answering(move || {
        let served = answers.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        crate::test_support::mock_upstream::MockAction::json(if served == 0 {
            first.clone()
        } else {
            then.clone()
        })
    })
    .await;

    mock
}

/// Seeds one enabled API-key provider bound to the quota-carrying payload.
async fn seed_quota_provider(
    database: &crate::database::Database,
    name: &str,
    payload: serde_json::Value,
    base: &str,
) -> Result<i64, Box<dyn StdError>> {
    let provider = crate::provider::tests_support::seed_provider(
        database,
        name,
        payload,
        &StoredCredentials::ApiKey {
            api_key: "key-1".to_string(),
        },
        Some(base),
    )
    .await?;

    Ok(provider.id)
}

/// The coordinator fixture over the shared test configuration: the pooling cadence reads the
/// process-global configuration, so the install runs under the configuration lock — which the
/// fixture releases before any await, the installed configuration being process-global.
async fn coordinator_fixture() -> Result<
    (
        Arc<Coordinator>,
        crate::database::Database,
        crate::test_support::TempDatabaseGuard,
    ),
    Box<dyn StdError>,
> {
    {
        let _lock = crate::test_support::configuration_lock();

        crate::test_support::install_configuration();
    }

    let (database, guard) = temp_database().await;

    Ok((Arc::new(Coordinator::default()), database, guard))
}

/// The client every coordinator run drives.
fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// A forced refresh reads the upstream, normalizes the observation, and reconciles the durable
/// restriction block with its future reset and account-wide applicability.
#[tokio::test]
async fn forced_refresh_reconciles_durable_blocks() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![quota_body(1.0)]).await;

    let payload = tests_support::with_member(
        tests_support::code_payload(&mock.base()),
        "",
        "quota",
        fraction_descriptor(),
    );

    let (coordinator, database, _guard) = coordinator_fixture().await?;

    let provider_id =
        seed_quota_provider(&database, "fixture-provider", payload, &mock.base()).await?;

    let observation = coordinator
        .refresh(&database, &client(), provider_id)
        .await?;

    assert_eq!(observation.quotas.len(), 1);

    let entry = observation.quotas.first().expect("one observation");

    assert_eq!(entry.key, vec!["plan".to_string(), "credits".to_string()]);

    assert_eq!(entry.measurement.is_exhausted(), Some(true));

    let state = quota_blocks::load_state(&database, provider_id)
        .await?
        .expect("the provider carries a fence");

    assert_eq!(state.blocks.len(), 1);

    let block = state.blocks.first().expect("one block");

    assert_eq!(block.key, vec!["plan".to_string(), "credits".to_string()]);

    assert_eq!(block.applies_to, QuotaApplicability::AllModels);

    assert!(block.blocked_until.is_some(), "the future reset schedules");

    let requests = recorded
        .lock()
        .map_err(|error| format!("the recorded requests lock: {error}"))?;

    assert_eq!(requests.len(), 1);

    Ok(())
}

/// A fresh cadence serves the committed evidence without a second upstream request, and the
/// memory metadata carries the succeeded reading beside the durable blocks.
#[tokio::test]
async fn fresh_cadence_serves_the_committed_evidence() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![quota_body(1.0)]).await;

    let payload = tests_support::with_member(
        tests_support::code_payload(&mock.base()),
        "",
        "quota",
        fraction_descriptor(),
    );

    let (coordinator, database, _guard) = coordinator_fixture().await?;

    let provider_id =
        seed_quota_provider(&database, "fixture-provider", payload, &mock.base()).await?;

    let state = coordinator
        .check(&database, &client(), provider_id, "model-a")
        .await?
        .expect("the provider exists");

    assert_eq!(state.blocks.len(), 1);

    let second = coordinator
        .check(&database, &client(), provider_id, "model-a")
        .await?
        .expect("the provider exists");

    assert_eq!(second.blocks.len(), 1);

    assert_eq!(
        recorded.lock().expect("the mock records").len(),
        1,
        "the fresh cadence never re-requests"
    );

    let (blocks, refresh) = coordinator
        .availability(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(blocks.len(), 1);

    assert!(matches!(refresh, LastRefresh::Succeeded { .. }));

    Ok(())
}

/// A healthy reconciliation clears the established block, and a failed read leaves the
/// established restrictions intact while recording its closed failure category.
#[tokio::test]
async fn reconciliation_clears_and_failures_keep_blocks() -> Result<(), Box<dyn StdError>> {
    // The first read is exhausted; every later read is healthy.
    let mock = switching_mock(quota_body(1.0), quota_body(0.5)).await;

    let payload = tests_support::with_member(
        tests_support::code_payload(&mock.base()),
        "",
        "quota",
        fraction_descriptor(),
    );

    let (coordinator, database, _guard) = coordinator_fixture().await?;

    let provider_id =
        seed_quota_provider(&database, "fixture-provider", payload, &mock.base()).await?;

    coordinator
        .refresh(&database, &client(), provider_id)
        .await?;

    assert_eq!(
        quota_blocks::load_state(&database, provider_id)
            .await?
            .expect("fence")
            .blocks
            .len(),
        1
    );

    // The forced second read bypasses the cadence, reconciles the healthy measurement, and
    // deletes the block.
    let observation = coordinator
        .refresh(&database, &client(), provider_id)
        .await?;

    assert_eq!(
        observation
            .quotas
            .first()
            .expect("one observation")
            .measurement
            .is_exhausted(),
        Some(false)
    );

    assert!(
        quota_blocks::load_state(&database, provider_id)
            .await?
            .expect("fence")
            .blocks
            .is_empty()
    );

    // A forced read against an upstream that answers nothing fails, keeps the (empty) state,
    // and records the transport category.
    let (dead, _) =
        MockUpstream::start(vec![crate::test_support::mock_upstream::MockAction::Close]).await;

    crate::storage::providers::update_provider(
        &database,
        provider_id,
        crate::storage::providers::ProviderUpdate {
            name: "fixture-provider",
            model_prefix: "fixture.",
            base_url: Some(&dead.base()),
            is_enabled: true,
            definition: None,
            credentials: None,
        },
    )
    .await?;

    let failed = coordinator.refresh(&database, &client(), provider_id).await;

    assert!(failed.is_err(), "a closed upstream fails the read");

    let (_, refresh) = coordinator
        .availability(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(
        matches!(
            refresh,
            LastRefresh::Failed {
                reason: RefreshFailure::Transport,
                ..
            }
        ),
        "the closed upstream records the transport category"
    );

    Ok(())
}

/// The administrator reset clears the durable evidence and installs the grace record.
#[tokio::test]
async fn reset_clears_blocks_and_installs_grace() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![quota_body(1.0)]).await;

    let payload = tests_support::with_member(
        tests_support::code_payload(&mock.base()),
        "",
        "quota",
        fraction_descriptor(),
    );

    let (coordinator, database, _guard) = coordinator_fixture().await?;

    let provider_id =
        seed_quota_provider(&database, "fixture-provider", payload, &mock.base()).await?;

    coordinator
        .refresh(&database, &client(), provider_id)
        .await?;

    assert_eq!(
        quota_blocks::load_state(&database, provider_id)
            .await?
            .expect("fence")
            .blocks
            .len(),
        1
    );

    coordinator.reset(&database, provider_id).await?;

    let (blocks, refresh) = coordinator
        .availability(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(blocks.is_empty(), "the reset clears the blocks");

    assert!(matches!(refresh, LastRefresh::Reset { .. }));

    Ok(())
}

/// A provider whose exact definition declares no quota query classifies as unsupported, and a
/// deleted account answers `None` from the check and from availability.
#[tokio::test]
async fn unsupported_and_deleted_accounts_classify() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![json!({"plan": {}}).to_string()]).await;

    let (coordinator, database, _guard) = coordinator_fixture().await?;

    let provider_id = seed_quota_provider(
        &database,
        "fixture-provider",
        tests_support::code_payload(&mock.base()),
        &mock.base(),
    )
    .await?;

    let failed = coordinator.refresh(&database, &client(), provider_id).await;

    assert!(
        matches!(
            failed,
            Err(crate::provider::quota::Error::NotConfigured { .. })
        ),
        "a definition without quota viewing classifies as NotConfigured"
    );

    let vanished = seed_quota_provider(
        &database,
        "vanished-provider",
        tests_support::code_payload(&mock.base()),
        &mock.base(),
    )
    .await?;

    crate::storage::providers::delete_provider(&database, vanished).await?;

    assert_eq!(
        coordinator
            .check(&database, &client(), vanished, "m")
            .await?,
        None
    );

    assert_eq!(coordinator.availability(&database, vanished).await?, None);

    Ok(())
}

/// A successful read installs the account's in-memory metadata, and the deleted-account path
/// drops it: the availability read of a vanished account forgets the slot entirely.
#[tokio::test]
async fn forget_drops_deleted_account_metadata() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![quota_body(0.5)]).await;

    let payload = tests_support::with_member(
        tests_support::code_payload(&mock.base()),
        "",
        "quota",
        fraction_descriptor(),
    );

    let (coordinator, database, _guard) = coordinator_fixture().await?;

    let provider_id =
        seed_quota_provider(&database, "fixture-provider", payload, &mock.base()).await?;

    assert!(
        coordinator.slots().is_empty(),
        "a fresh coordinator carries no metadata"
    );

    coordinator
        .refresh(&database, &client(), provider_id)
        .await?;

    assert!(
        coordinator.slots().contains_key(&provider_id),
        "the read installs the account's metadata"
    );

    crate::storage::providers::delete_provider(&database, provider_id).await?;

    assert_eq!(
        coordinator.availability(&database, provider_id).await?,
        None
    );

    assert!(
        !coordinator.slots().contains_key(&provider_id),
        "the deleted account's metadata is dropped"
    );

    Ok(())
}
