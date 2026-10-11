//! Completion-pipeline tests: the authorization-code mapping of exchange classifications, the
//! working-state fold into the stored credentials — slot retention, base-URL provenance, and
//! the expiry sources — and the reauthorize write's derived-base and vanished-target
//! semantics (`docs/design/providers.md` and `docs/reference/management-api.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or store is that test's failure report"
)]

use super::{CompletionProduct, complete_exchange, completed_outcome, write_completion_provider};
use crate::{
    provider::{
        Error as ProviderError, StoredCredentials,
        definition::ProviderDefinition,
        oauth::{
            exchange::{ExchangeOutcome, ExchangeProduct},
            tests_support,
        },
    },
    storage,
    storage::oauth_flows::{FlowGrant, FlowPurpose, NewFlow, OngoingFlow},
    test_support::{TempDatabaseGuard, temp_database},
    unix_time,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error as StdError};

/// The unreachable URL the no-provisioning fixture recipes render; the completion pipeline
/// without provisioning steps performs no upstream request.
const DEAD_BASE: &str = "http://127.0.0.1:1";

/// Stores the authorization-code recipe of `payload` as `fixture-definition` version 1.
async fn stored_definition(
    payload: serde_json::Value,
) -> Result<
    (
        ProviderDefinition,
        crate::database::Database,
        TempDatabaseGuard,
    ),
    Box<dyn StdError>,
> {
    let (database, guard) = temp_database().await;

    let definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        tests_support::parse(payload),
    );

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

/// Inserts one authorization-code create flow carrying `slots` and the `base_url` working base.
async fn create_flow(
    database: &crate::database::Database,
    definition: &ProviderDefinition,
    slots: BTreeMap<String, String>,
    base_url: Option<&str>,
) -> Result<OngoingFlow, Box<dyn StdError>> {
    let flow = storage::oauth_flows::insert_flow(
        database,
        NewFlow {
            definition_name: definition.name.clone(),
            definition_version: definition.version,
            state_token: "fixture-state".to_string(),
            purpose: FlowPurpose::Create,
            provider_name: Some("fixture-provider".to_string()),
            model_prefix: Some("fixture.".to_string()),
            base_url: base_url.map(str::to_string),
            target_provider_id: None,
            grant_type: FlowGrant::AuthorizationCode,
            slots,
            code_verifier: None,
            device_code: None,
            device_user_code: None,
            device_verification_uri: None,
            device_verification_uri_complete: None,
            device_poll_interval_seconds: None,
            device_next_poll_at: None,
            expires_at: unix_time::unix_now_micros() + 600 * unix_time::MICROS_PER_SEC,
        },
    )
    .await?;

    Ok(flow)
}

/// Inserts one authorization-code reauthorize flow targeting `provider`.
async fn reauthorize_flow(
    database: &crate::database::Database,
    provider: &crate::provider::Provider,
) -> Result<OngoingFlow, Box<dyn StdError>> {
    let flow = storage::oauth_flows::insert_flow(
        database,
        NewFlow {
            definition_name: provider.definition.name.clone(),
            definition_version: provider.definition.version,
            state_token: "fixture-state".to_string(),
            purpose: FlowPurpose::Reauthorize,
            provider_name: None,
            model_prefix: None,
            base_url: provider.base_url.clone(),
            target_provider_id: Some(provider.id),
            grant_type: FlowGrant::AuthorizationCode,
            slots: BTreeMap::new(),
            code_verifier: None,
            device_code: None,
            device_user_code: None,
            device_verification_uri: None,
            device_verification_uri_complete: None,
            device_poll_interval_seconds: None,
            device_next_poll_at: None,
            expires_at: unix_time::unix_now_micros() + 600 * unix_time::MICROS_PER_SEC,
        },
    )
    .await?;

    Ok(flow)
}

/// Seeds one enabled OAuth provider row carrying the stored `https://old.test` base, bound to
/// the fixture recipe, and returns it beside the database and its lifetime guard.
async fn seed_reauthorize_target() -> Result<
    (
        crate::database::Database,
        crate::provider::Provider,
        TempDatabaseGuard,
    ),
    Box<dyn StdError>,
> {
    let (database, guard) = temp_database().await;

    let credentials = StoredCredentials::OAuth {
        access_token: "at-old".to_string(),
        refresh_token: Some("rt-old".to_string()),
        slots: BTreeMap::new(),
        expires_at: None,
        refresh_started_at: None,
    };

    let provider = crate::provider::tests_support::seed_provider(
        &database,
        "fixture-provider",
        tests_support::code_payload(DEAD_BASE),
        &credentials,
        Some("https://old.test"),
    )
    .await?;

    Ok((database, provider, guard))
}

/// The OAuth credentials one fixture product stores.
fn oauth_credentials(access_token: &str) -> StoredCredentials {
    StoredCredentials::OAuth {
        access_token: access_token.to_string(),
        refresh_token: Some("rt-1".to_string()),
        slots: BTreeMap::new(),
        expires_at: None,
        refresh_started_at: None,
    }
}

/// The token outcome carrying `access_token` and, when set, a response lifetime and a derived
/// base URL — every other field stays empty.
fn token_outcome(
    access_token: &str,
    expires_in_seconds: Option<i64>,
    base_url: Option<&str>,
) -> ExchangeOutcome {
    let mut outcome = ExchangeOutcome::default();

    outcome.access_token = Some(access_token.to_string());

    outcome.expires_in_seconds = expires_in_seconds;

    outcome.base_url = base_url.map(str::to_string);

    outcome
}

/// The outcome classification mapping renders the authorization-code failures: a completed
/// exchange passes through, the polling classifications are engine drift, a terminal code
/// reports the consumed-grant remedy, and any other code reports the plain rejection.
#[test]
fn the_outcome_mapping_renders_code_grant_classifications() {
    let completed = completed_outcome(ExchangeProduct::Completed(token_outcome(
        "at-1", None, None,
    )))
    .expect("a completed exchange maps through");

    assert_eq!(completed.access_token.as_deref(), Some("at-1"));

    for product in [ExchangeProduct::Pending, ExchangeProduct::SlowDown] {
        let failure = completed_outcome(product)
            .expect_err("a polling classification is engine drift under the code grant");

        assert!(
            matches!(failure, ProviderError::Internal { .. }),
            "the drift classifies as internal, got {failure:?}"
        );
    }

    let terminal = completed_outcome(ExchangeProduct::Terminal("invalid_grant".to_string()))
        .expect_err("a terminal code fails the completion");

    let detail = terminal.to_string();

    assert!(
        detail.contains("expired, already used, or invalid"),
        "the terminal message carries the remedy: {detail}"
    );

    let other = completed_outcome(ExchangeProduct::Other(
        "temporarily_unavailable".to_string(),
    ))
    .expect_err("an unconfigured code fails the completion");

    assert!(
        other.to_string().contains("temporarily_unavailable"),
        "the rejection names the code: {other}"
    );
}

/// The working base URL follows the exchange's derivation with its provenance: an
/// exchange-supplied base replaces the flow's begin-time base and marks the product derived;
/// without one the begin-time base stands unmarked.
#[tokio::test]
async fn the_working_base_follows_the_exchange_derivation() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) =
        stored_definition(tests_support::code_payload(DEAD_BASE)).await?;

    let flow = create_flow(
        &database,
        &definition,
        BTreeMap::new(),
        Some("https://flow-base.test"),
    )
    .await?;

    let derived = complete_exchange(
        &reqwest::Client::new(),
        &definition,
        token_outcome("at-1", None, Some("https://home.test")),
        &flow,
    )
    .await?;

    assert_eq!(derived.base_url.as_deref(), Some("https://home.test"));

    assert!(
        derived.is_base_url_derived,
        "an exchange-supplied base is the provenance mark"
    );

    let echoed = complete_exchange(
        &reqwest::Client::new(),
        &definition,
        token_outcome("at-1", None, None),
        &flow,
    )
    .await?;

    assert_eq!(echoed.base_url.as_deref(), Some("https://flow-base.test"));

    assert!(
        !echoed.is_base_url_derived,
        "the begin-time echo is never a derived base"
    );

    Ok(())
}

/// Empty slot values never reach the stored credentials — an empty slot evaluates exactly like
/// an absent one — while the carried non-empty slots ride along.
#[tokio::test]
async fn empty_slots_never_reach_the_stored_credentials() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) =
        stored_definition(tests_support::code_payload(DEAD_BASE)).await?;

    let slots = BTreeMap::from([
        ("stale".to_string(), String::new()),
        ("carried".to_string(), "kept".to_string()),
    ]);

    let flow = create_flow(&database, &definition, slots, None).await?;

    let product = complete_exchange(
        &reqwest::Client::new(),
        &definition,
        token_outcome("at-1", None, None),
        &flow,
    )
    .await?;

    let StoredCredentials::OAuth { slots, .. } = product.credentials else {
        panic!("the completion stores OAuth credentials");
    };

    assert_eq!(slots.get("carried").map(String::as_str), Some("kept"));

    assert!(
        !slots.contains_key("stale"),
        "an empty slot is dropped, not stored"
    );

    Ok(())
}

/// The expiry resolves from the response lifetime first, then the configured access-token
/// lifetime, else no refresh is scheduled at all.
#[tokio::test]
async fn expiry_resolves_from_the_response_then_the_configured_lifetime()
-> Result<(), Box<dyn StdError>> {
    let assert_remaining = |product: &super::CompletionProduct, seconds: i64| {
        let StoredCredentials::OAuth { expires_at, .. } = &product.credentials else {
            panic!("the completion stores OAuth credentials");
        };

        let remaining = expires_at.expect("an expiry source schedules the refresh")
            - unix_time::unix_now_micros();

        assert!(
            remaining > (seconds - 1) * unix_time::MICROS_PER_SEC
                && remaining < (seconds + 1) * unix_time::MICROS_PER_SEC,
            "the expiry sits {remaining} micros out, expected about {seconds} seconds"
        );
    };

    let (definition, database, _guard) =
        stored_definition(tests_support::code_payload(DEAD_BASE)).await?;

    let flow = create_flow(&database, &definition, BTreeMap::new(), None).await?;

    let from_response = complete_exchange(
        &reqwest::Client::new(),
        &definition,
        token_outcome("at-1", Some(3600), None),
        &flow,
    )
    .await?;

    assert_remaining(&from_response, 3600);

    let lifetime_payload = tests_support::with_member(
        tests_support::code_payload(DEAD_BASE),
        "/credential",
        "access_token_lifetime_seconds",
        json!(120),
    );

    let (definition, database, _guard) = stored_definition(lifetime_payload).await?;

    let flow = create_flow(&database, &definition, BTreeMap::new(), None).await?;

    let from_lifetime = complete_exchange(
        &reqwest::Client::new(),
        &definition,
        token_outcome("at-1", None, None),
        &flow,
    )
    .await?;

    assert_remaining(&from_lifetime, 120);

    let (definition, database, _guard) =
        stored_definition(tests_support::code_payload(DEAD_BASE)).await?;

    let flow = create_flow(&database, &definition, BTreeMap::new(), None).await?;

    let unscheduled = complete_exchange(
        &reqwest::Client::new(),
        &definition,
        token_outcome("at-1", None, None),
        &flow,
    )
    .await?;

    let StoredCredentials::OAuth {
        expires_at: unscheduled_expiry,
        ..
    } = unscheduled.credentials
    else {
        panic!("the completion stores OAuth credentials");
    };

    assert_eq!(
        unscheduled_expiry, None,
        "no expiry evidence means no scheduled refresh, never immediate expiry"
    );

    Ok(())
}

/// A reauthorize write replaces the stored base URL only when the flow derived one, rebinding
/// the credentials in the same transaction.
#[tokio::test]
async fn the_reauthorize_write_replaces_only_a_derived_base() -> Result<(), Box<dyn StdError>> {
    let (database, provider, _guard) = seed_reauthorize_target().await?;

    let flow = reauthorize_flow(&database, &provider).await?;

    let product = CompletionProduct {
        credentials: oauth_credentials("at-new"),
        base_url: Some("https://derived.test".to_string()),
        is_base_url_derived: true,
    };

    let mut transaction = database.transaction().await?;

    let written =
        write_completion_provider(&mut transaction, &flow, &provider.definition, &product)
            .await?
            .expect("the target persists");

    transaction.commit().await?;

    assert_eq!(written, provider.id);

    let stored = storage::providers::find_provider_by_id(&database, provider.id)
        .await?
        .expect("the target persists");

    assert_eq!(stored.base_url.as_deref(), Some("https://derived.test"));

    assert_eq!(stored.credentials.secret(), "at-new", "the rebind lands");

    Ok(())
}

/// The begin-time base a flow echoes never replaces the stored base: only a derived base is a
/// reauthorize write candidate, so a concurrent base update is never clobbered.
#[tokio::test]
async fn the_begin_time_base_never_replaces_the_stored_base() -> Result<(), Box<dyn StdError>> {
    let (database, provider, _guard) = seed_reauthorize_target().await?;

    let flow = reauthorize_flow(&database, &provider).await?;

    let product = CompletionProduct {
        credentials: oauth_credentials("at-new"),
        base_url: Some("https://flow-echo.test".to_string()),
        is_base_url_derived: false,
    };

    let mut transaction = database.transaction().await?;

    write_completion_provider(&mut transaction, &flow, &provider.definition, &product)
        .await?
        .expect("the target persists");

    transaction.commit().await?;

    let stored = storage::providers::find_provider_by_id(&database, provider.id)
        .await?
        .expect("the target persists");

    assert_eq!(
        stored.base_url.as_deref(),
        Some("https://old.test"),
        "the stale echo never clobbers the stored base"
    );

    assert_eq!(stored.credentials.secret(), "at-new", "the rebind lands");

    Ok(())
}

/// A reauthorize flow whose target provider was deleted before completion reports the vanished
/// target: the caller rolls back and classifies the loss its surface's way.
#[tokio::test]
async fn a_vanished_reauthorize_target_fails_the_write() -> Result<(), Box<dyn StdError>> {
    let (database, provider, _guard) = seed_reauthorize_target().await?;

    let flow = reauthorize_flow(&database, &provider).await?;

    storage::providers::delete_provider(&database, provider.id).await?;

    let product = CompletionProduct {
        credentials: oauth_credentials("at-new"),
        base_url: None,
        is_base_url_derived: false,
    };

    let mut transaction = database.transaction().await?;

    let written =
        write_completion_provider(&mut transaction, &flow, &provider.definition, &product).await?;

    transaction.rollback().await?;

    assert_eq!(written, None, "the vanished target reports None");

    Ok(())
}
