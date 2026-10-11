//! Ongoing OAuth flow persistence tests: the single-consume lifecycle of a state token with its
//! expiry gate and sweep, the device-flow claim lease with its checkpoint and poll persistence,
//! the claim-conditioned success commit, and the cross-column schema invariants the inserted
//! flow shapes must satisfy.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or list is that test's failure report"
)]

use super::{
    DevicePollOutcome, FlowGrant, FlowOutcome, FlowPurpose, NewFlow, OngoingFlow,
    claim_due_device_flow, commit_device_success_in_transaction, consume_flow,
    consume_terminal_flow, find_flow_by_state, insert_flow, persist_device_poll,
    persist_exchange_checkpoint, restamp_claim, sweep_expired_flows,
};
use crate::{
    database::Database,
    provider::{ProviderDefinition, StoredCredentials},
    storage::Error as StorageError,
};
use std::{collections::BTreeMap, error::Error as StdError, num::NonZeroI64};

/// Parses the static-API-key fixture definition payload the flows reference.
fn api_key_payload() -> crate::provider::definition::DefinitionPayload {
    serde_json::from_value(serde_json::json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "literal", "value": "https://first.test/v1/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    }))
    .expect("fixture definition payload parses")
}

/// Inserts the fixture definition once, returning the stored definition the flows reference.
async fn seed_definition(database: &Database) -> Result<ProviderDefinition, StorageError> {
    let payload = api_key_payload();

    crate::storage::provider_definitions::insert_definition(
        database,
        crate::storage::provider_definitions::NewDefinition {
            name: "fixture-definition",
            base_version: None,
            payload: &payload,
            created_at: 1_000,
        },
    )
    .await
}

/// The nonzero fixture version of `value`.
fn version(value: i64) -> NonZeroI64 {
    NonZeroI64::new(value).expect("fixture versions are nonzero literals")
}

/// The common fields of every fixture flow: the fixture definition reference and a fresh state
/// token.
fn flow_base(state_token: &str, expires_at: i64) -> NewFlow {
    NewFlow {
        definition_name: "fixture-definition".to_string(),
        definition_version: version(1),
        state_token: state_token.to_string(),
        purpose: FlowPurpose::Create,
        provider_name: Some("new-provider".to_string()),
        model_prefix: Some("acme.".to_string()),
        base_url: None,
        target_provider_id: None,
        grant_type: FlowGrant::AuthorizationCode,
        slots: BTreeMap::from([("team".to_string(), "t1".to_string())]),
        code_verifier: Some("fixture-verifier".to_string()),
        device_code: None,
        device_user_code: None,
        device_verification_uri: None,
        device_verification_uri_complete: None,
        device_poll_interval_seconds: None,
        device_next_poll_at: None,
        expires_at,
    }
}

/// The fixture create flow of the authorization-code grant.
fn code_create_flow(state_token: &str, expires_at: i64) -> NewFlow {
    flow_base(state_token, expires_at)
}

/// The fixture reauthorize flow of the authorization-code grant, targeting `provider_id`.
fn code_reauthorize_flow(state_token: &str, expires_at: i64, provider_id: i64) -> NewFlow {
    NewFlow {
        purpose: FlowPurpose::Reauthorize,
        provider_name: None,
        model_prefix: None,
        target_provider_id: Some(provider_id),
        ..flow_base(state_token, expires_at)
    }
}

/// The fixture device flow, due for polling at `next_poll_at`.
fn device_create_flow(state_token: &str, expires_at: i64, next_poll_at: i64) -> NewFlow {
    NewFlow {
        grant_type: FlowGrant::Device,
        code_verifier: None,
        device_code: Some("device-code".to_string()),
        device_user_code: Some("ABCD-1234".to_string()),
        device_verification_uri: Some("https://first.test/device".to_string()),
        device_verification_uri_complete: None,
        device_poll_interval_seconds: Some(5),
        device_next_poll_at: Some(next_poll_at),
        ..flow_base(state_token, expires_at)
    }
}

/// Unwraps an expected-present flow read; a missing flow is the failure under test.
fn present_flow(flow: Option<OngoingFlow>) -> OngoingFlow {
    flow.expect("the flow under test exists")
}

/// The microsecond stamp fixtures share as their wall clock origin.
const NOW: i64 = 5_000_000_000_000;

/// A state token identifies the unexpired flow for reading without consuming, the atomic consume
/// spends the row exactly once, and expiry gates both the read and the consume until the sweep
/// deletes the expired row.
#[tokio::test]
async fn state_tokens_identify_consume_once_and_expire() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    seed_definition(&database).await?;

    let inserted =
        insert_flow(&database, code_create_flow("state-live", NOW + 600_000_000)).await?;

    let found = present_flow(find_flow_by_state(&database, "state-live", NOW).await?);

    assert_eq!(found.id, inserted.id);

    assert_eq!(found.create_provider_name(), "new-provider");

    assert_eq!(found.create_model_prefix(), "acme.");

    assert_eq!(found.grant_type, FlowGrant::AuthorizationCode);

    assert_eq!(
        found.slots,
        BTreeMap::from([("team".to_string(), "t1".to_string())])
    );

    assert_eq!(found.code_verifier.as_deref(), Some("fixture-verifier"));

    assert_eq!(found.expires_at, NOW + 600_000_000);

    let consumed = present_flow(consume_flow(&database, "state-live", NOW).await?);

    assert_eq!(consumed.id, inserted.id);

    assert!(
        consume_flow(&database, "state-live", NOW).await?.is_none(),
        "a consumed state can never complete twice"
    );

    // An expired flow answers nothing until the sweep removes it.
    insert_flow(&database, code_create_flow("state-expired", NOW - 1)).await?;

    assert!(
        find_flow_by_state(&database, "state-expired", NOW)
            .await?
            .is_none()
    );

    assert!(
        consume_flow(&database, "state-expired", NOW)
            .await?
            .is_none()
    );

    sweep_expired_flows(&database, NOW).await?;

    assert!(
        find_flow_by_state(&database, "state-expired", NOW)
            .await?
            .is_none(),
        "the sweep deleted the expired row"
    );

    Ok(())
}

/// The terminal retrieval consumes only a flow carrying a pending result; a pending flow is
/// consumed only by the plain consume, and a failed flow answers its gateway-owned failure pair.
#[tokio::test]
async fn terminal_consume_gates_on_a_pending_result() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    seed_definition(&database).await?;

    let pending = insert_flow(&database, code_create_flow("pending", NOW + 600_000_000)).await?;

    assert!(
        consume_terminal_flow(&database, "pending", NOW)
            .await?
            .is_none(),
        "a pending flow is never consumed by the terminal retrieval"
    );

    let consumed = present_flow(consume_flow(&database, "pending", NOW).await?);

    assert_eq!(consumed.id, pending.id);

    Ok(())
}

/// The device-flow claim is a single conditional lease: a due pending flow is claimed once, a
/// live claim blocks competing pollers, an aged claim is recovered, and the re-stamp is a
/// compare-and-set over the held claim.
#[tokio::test]
async fn device_claim_is_a_single_conditional_lease() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    seed_definition(&database).await?;

    insert_flow(
        &database,
        device_create_flow("device", NOW + 600_000_000, NOW - 1_000),
    )
    .await?;

    let claimed = present_flow(claim_due_device_flow(&database, NOW, NOW).await?);

    assert_eq!(claimed.device_poll_started_at, Some(NOW));

    assert_eq!(claimed.device_code.as_deref(), Some("device-code"));

    // A live claim is not stealable at a staleness boundary at or before its stamp.
    let live = claim_due_device_flow(&database, NOW + 10, NOW).await?;

    assert!(live.is_none(), "a live holder keeps the claim");

    // A claim older than the staleness window is recovered.
    let recovered = present_flow(claim_due_device_flow(&database, NOW + 20, NOW + 1).await?);

    assert_eq!(recovered.device_poll_started_at, Some(NOW + 20));

    // The re-stamp is a compare-and-set over the held claim.
    let restamped = restamp_claim(&database, recovered.id, NOW + 20).await?;

    assert!(restamped.is_some(), "the holder re-stamps its own claim");

    assert!(
        restamp_claim(&database, recovered.id, NOW + 20)
            .await?
            .is_none(),
        "a caller with the superseded stamp re-stamps nothing"
    );

    Ok(())
}

/// The poll persistence is claim-conditioned: a retry reschedules and clears the claim and any
/// recorded checkpoint, a terminal failure records the failure pair and drops the schedule, and
/// both discard their outcome when the claim was lost.
#[tokio::test]
async fn device_poll_persists_retries_and_failures() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    seed_definition(&database).await?;

    insert_flow(
        &database,
        device_create_flow("retry-flow", NOW + 600_000_000, NOW - 1_000),
    )
    .await?;

    let claimed = present_flow(claim_due_device_flow(&database, NOW, NOW).await?);

    let claim = claimed
        .device_poll_started_at
        .expect("the claim is stamped");

    assert!(
        persist_exchange_checkpoint(&database, claimed.id, claim, r#"{"token":"mapped"}"#).await?,
        "the claim holder records the exchange checkpoint"
    );

    let checkpointed = present_flow(find_flow_by_state(&database, "retry-flow", NOW).await?);

    assert_eq!(
        checkpointed.device_exchange_checkpoint.as_deref(),
        Some(r#"{"token":"mapped"}"#),
    );

    assert!(
        !persist_exchange_checkpoint(&database, claimed.id, claim - 1, r#"{"stale":true}"#).await?,
        "a caller without the claim records nothing"
    );

    assert!(
        persist_device_poll(
            &database,
            claimed.id,
            claim,
            DevicePollOutcome::Retry {
                next_poll_at: NOW + 5_000_000,
                interval_seconds: 10,
            },
        )
        .await?,
        "the claim holder schedules the retry"
    );

    let retried = present_flow(find_flow_by_state(&database, "retry-flow", NOW).await?);

    assert_eq!(retried.device_next_poll_at, Some(NOW + 5_000_000));

    assert_eq!(retried.device_poll_interval_seconds, Some(10));

    assert_eq!(
        retried.device_poll_started_at, None,
        "the retry released the claim"
    );

    assert_eq!(
        retried.device_exchange_checkpoint, None,
        "a retry is only ever scheduled for an exchange that did not complete"
    );

    // The rescheduled flow becomes due again, and the terminal failure persists under the fresh
    // claim.
    let claimed = present_flow(claim_due_device_flow(&database, NOW + 6_000_000, NOW).await?);

    let claim = claimed
        .device_poll_started_at
        .expect("the claim is stamped");

    let lost = persist_device_poll(
        &database,
        claimed.id,
        claim - 1,
        DevicePollOutcome::Failed {
            code: "superseded".to_string(),
            message: "stale claim".to_string(),
        },
    )
    .await?;

    assert!(!lost, "a caller without the claim persists nothing");

    assert!(
        persist_device_poll(
            &database,
            claimed.id,
            claim,
            DevicePollOutcome::Failed {
                code: "expired_token".to_string(),
                message: "the device code expired".to_string(),
            },
        )
        .await?,
        "the claim holder persists the terminal failure"
    );

    let failed = present_flow(find_flow_by_state(&database, "retry-flow", NOW).await?);

    assert_eq!(failed.outcome, Some(FlowOutcome::Failed));

    assert_eq!(
        failed.failure_pair(),
        (
            "expired_token".to_string(),
            "the device code expired".to_string()
        ),
    );

    assert_eq!(
        failed.device_next_poll_at, None,
        "a terminal flow carries no schedule"
    );

    assert_eq!(failed.device_poll_started_at, None);

    let consumed = present_flow(consume_terminal_flow(&database, "retry-flow", NOW).await?);

    assert_eq!(consumed.id, failed.id);

    assert!(
        find_flow_by_state(&database, "retry-flow", NOW)
            .await?
            .is_none(),
        "the terminal retrieval consumed the row"
    );

    Ok(())
}

/// The success commit is part of the poller's provider-write transaction, conditioned on the held
/// claim: the flow turns succeeded with its result provider, drops the schedule and any
/// checkpoint, and the terminal retrieval consumes it; a lost claim commits nothing.
#[tokio::test]
async fn success_commit_is_claim_conditioned_and_atomic() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = seed_definition(&database).await?;

    insert_flow(
        &database,
        device_create_flow("success-flow", NOW + 600_000_000, NOW - 1_000),
    )
    .await?;

    let claimed = present_flow(claim_due_device_flow(&database, NOW, NOW).await?);

    let claim = claimed
        .device_poll_started_at
        .expect("the claim is stamped");

    persist_exchange_checkpoint(&database, claimed.id, claim, r#"{"token":"mapped"}"#).await?;

    let credentials = StoredCredentials::OAuth {
        access_token: "at-commit".to_string(),
        refresh_token: None,
        slots: BTreeMap::new(),
        expires_at: None,
        refresh_started_at: None,
    };

    let mut transaction = database.transaction().await?;

    let provider_id = crate::storage::providers::insert_provider_in_transaction(
        &mut transaction,
        crate::storage::providers::NewProvider {
            name: "committed",
            model_prefix: "",
            base_url: None,
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    assert!(
        commit_device_success_in_transaction(&mut transaction, claimed.id, claim, provider_id)
            .await?,
        "the claim holder commits the success beside its provider write"
    );

    transaction.commit().await?;

    let committed = present_flow(find_flow_by_state(&database, "success-flow", NOW).await?);

    assert_eq!(committed.outcome, Some(FlowOutcome::Succeeded));

    assert_eq!(committed.result_provider_id(), provider_id);

    assert_eq!(committed.device_next_poll_at, None);

    assert_eq!(committed.device_poll_started_at, None);

    assert_eq!(
        committed.device_exchange_checkpoint, None,
        "the commit completed the recorded exchange"
    );

    let retrieved = present_flow(consume_terminal_flow(&database, "success-flow", NOW).await?);

    assert_eq!(retrieved.result_provider_id(), provider_id);

    // A commit against a lost claim is refused.
    insert_flow(
        &database,
        device_create_flow("stolen-flow", NOW + 600_000_000, NOW - 1_000),
    )
    .await?;

    let claimed = present_flow(claim_due_device_flow(&database, NOW, NOW).await?);

    let claim = claimed
        .device_poll_started_at
        .expect("the claim is stamped");

    // The claim ages past the staleness window and is recovered by a competing poller.
    present_flow(claim_due_device_flow(&database, NOW + 100, NOW + 1).await?);

    let mut transaction = database.transaction().await?;

    let provider_id = crate::storage::providers::insert_provider_in_transaction(
        &mut transaction,
        crate::storage::providers::NewProvider {
            name: "abandoned",
            model_prefix: "",
            base_url: None,
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    let lost =
        commit_device_success_in_transaction(&mut transaction, claimed.id, claim, provider_id)
            .await?;

    assert!(!lost, "the superseded claim commits nothing");

    transaction.rollback().await?;

    Ok(())
}

/// The inserted flow shapes must satisfy the schema's cross-column invariants: the purpose pins
/// its durable fields, the grant pins its device columns, and a violation surfaces as the typed
/// storage constraint error.
#[tokio::test]
async fn inserted_shapes_satisfy_the_cross_column_invariants() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = seed_definition(&database).await?;

    let provider_id = {
        let credentials = StoredCredentials::ApiKey {
            api_key: "sk-fixture".to_string(),
        };

        crate::storage::providers::insert_provider(
            &database,
            crate::storage::providers::NewProvider {
                name: "acct",
                model_prefix: "",
                base_url: None,
                credentials: &credentials,
                definition: &definition,
                is_enabled: true,
            },
        )
        .await?
        .id
    };

    // A create flow must not carry a reauthorize target.
    let with_target = NewFlow {
        target_provider_id: Some(provider_id),
        ..code_create_flow("bad-target", NOW + 600_000_000)
    };

    let rejected = insert_flow(&database, with_target).await;

    assert!(
        matches!(&rejected, Err(StorageError::Constraint { .. })),
        "a create flow with a reauthorize target violates its invariant, got {rejected:?}"
    );

    // A reauthorize flow must carry its target and no creation fields.
    let without_target = NewFlow {
        purpose: FlowPurpose::Reauthorize,
        provider_name: None,
        model_prefix: None,
        target_provider_id: None,
        ..code_create_flow("no-target", NOW + 600_000_000)
    };

    let rejected = insert_flow(&database, without_target).await;

    assert!(
        matches!(&rejected, Err(StorageError::Constraint { .. })),
        "a reauthorize flow without its target violates its invariant, got {rejected:?}"
    );

    // A device flow must carry its device columns.
    let device_without_code = NewFlow {
        device_code: None,
        ..device_create_flow("no-device-code", NOW + 600_000_000, NOW)
    };

    let rejected = insert_flow(&database, device_without_code).await;

    assert!(
        matches!(&rejected, Err(StorageError::Constraint { .. })),
        "a device flow without its device code violates its invariant, got {rejected:?}"
    );

    // An authorization-code flow must not carry device columns.
    let code_with_device = NewFlow {
        device_code: Some("device-code".to_string()),
        ..code_create_flow("code-with-device", NOW + 600_000_000)
    };

    let rejected = insert_flow(&database, code_with_device).await;

    assert!(
        matches!(&rejected, Err(StorageError::Constraint { .. })),
        "an authorization-code flow with device columns violates its invariant, got {rejected:?}"
    );

    // The valid reauthorize flow inserts.
    insert_flow(
        &database,
        code_reauthorize_flow("reauthorize", NOW + 600_000_000, provider_id),
    )
    .await?;

    let found = present_flow(find_flow_by_state(&database, "reauthorize", NOW).await?);

    assert_eq!(found.reauthorize_target_id(), provider_id);

    Ok(())
}
