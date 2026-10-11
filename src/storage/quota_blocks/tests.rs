//! Durable quota-state tests: the fenced reconcile (exhaustion evidence blocks with its future
//! reset, availability evidence clears the exact key, unknown and monitoring-only observations
//! preserve established blocks, and a stale captured fence applies nothing), the
//! generation-gated dispatch rejection with its reset preservation, the explicit availability
//! reset, and the transactional evidence invalidation a provider mutation performs.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or list is that test's failure report"
)]

/// The fixture definition version the seed guard reuses.
fn fixture_version() -> NonZeroI64 {
    NonZeroI64::new(1).expect("the fixture version is nonzero")
}

use super::{QuotaFence, load_state, reconcile, record_rejection, reset};
use crate::{
    database::Database,
    provider::{
        StoredCredentials,
        definition::DefinitionPayload,
        quota::{Observation, QuotaApplicability, SafeTimestamp},
    },
    storage::Error as StorageError,
};
use std::{error::Error as StdError, num::NonZeroI64};

/// Parses the static-API-key fixture definition payload.
fn api_key_payload() -> DefinitionPayload {
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

/// Inserts the fixture definition once and one enabled static-API-key provider carrying it,
/// returning the provider's row identity.
async fn seed_provider(database: &Database) -> Result<i64, StorageError> {
    let payload = api_key_payload();

    let definition = match crate::storage::provider_definitions::find_definition(
        database,
        "fixture-definition",
        fixture_version(),
    )
    .await?
    {
        Some(stored) => stored,

        None => {
            crate::storage::provider_definitions::insert_definition(
                database,
                crate::storage::provider_definitions::NewDefinition {
                    name: "fixture-definition",
                    base_version: None,
                    payload: &payload,
                    created_at: 1_000,
                },
            )
            .await?
        }
    };

    let credentials = StoredCredentials::ApiKey {
        api_key: "sk-fixture".to_string(),
    };

    let provider = crate::storage::providers::insert_provider(
        database,
        crate::storage::providers::NewProvider {
            name: "acct",
            model_prefix: "",
            base_url: None,
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    Ok(provider.id)
}

/// Parses one quota-entry fixture from its wire JSON — the entry's presentation members
/// (`label`, `scope`, `window`) are private to the quota domain, so the fixture travels through
/// the typed deserialization the extraction itself feeds.
fn entry(json: serde_json::Value) -> crate::provider::quota::QuotaEntry {
    serde_json::from_value(json).expect("fixture quota entry parses")
}

/// Builds the observation of one entry observed at `observed_at`.
fn observation(observed_at: i64, entries: Vec<crate::provider::quota::QuotaEntry>) -> Observation {
    Observation {
        observed_at: SafeTimestamp::try_from(observed_at).expect("fixture stamps are safe"),
        quotas: entries,
    }
}

/// The wire JSON of one counter measurement with the used-over-limit exhaustion evidence.
fn exhausted_counter(
    key: &str,
    routing: serde_json::Value,
    reset: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "key": [key],
        "label": "Fixture",
        "scope": {"type": "unknown"},
        "routing": routing,
        "window": {"type": "unknown"},
        "measurement": {"type": "counter", "used": 100, "limit": 100, "unit": "requests"},
        "reset": reset
    })
}

/// The fixture fence captured before the observation.
async fn captured_fence(database: &Database, provider_id: i64) -> Result<QuotaFence, StorageError> {
    load_state(database, provider_id)
        .await?
        .map(|state| state.fence)
        .ok_or(StorageError::NotFound {
            message: "the fixture provider exists".to_string(),
        })
}

/// Reconcile applies exhaustion evidence as a durable block carrying the future absolute reset,
/// clears the exact key on availability evidence, preserves blocks under unknown and
/// monitoring-only observations, and applies nothing under a stale captured fence.
#[tokio::test]
async fn reconcile_applies_clears_and_preserves_by_evidence() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database).await?;

    let observed_at = 1_000_000_000_000;

    let reset_at = observed_at + 60_000_000;

    let captured = captured_fence(&database, provider_id).await?;

    assert!(
        reconcile(
            &database,
            provider_id,
            captured,
            &observation(
                observed_at,
                vec![entry(exhausted_counter(
                    "rpm",
                    serde_json::json!({"type": "all_models"}),
                    serde_json::json!({"type": "at", "at": reset_at}),
                ))],
            ),
        )
        .await?,
        "the current fence applies the observation"
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.fence.generation, 0,
        "reconcile advances only the revision"
    );

    assert_eq!(state.fence.revision, 1);

    assert_eq!(state.blocks.len(), 1);

    let block = state.blocks.first().expect("one block is stored");

    assert_eq!(block.key, vec!["rpm".to_string()]);

    assert_eq!(block.applies_to, QuotaApplicability::AllModels);

    assert_eq!(
        block.blocked_until,
        Some(reset_at),
        "the future absolute reset is stored"
    );

    // Availability evidence clears the exact key.
    let available = entry(serde_json::json!({
        "key": ["rpm"],
        "label": "Fixture",
        "scope": {"type": "unknown"},
        "routing": {"type": "all_models"},
        "window": {"type": "unknown"},
        "measurement": {"type": "unlimited"},
        "reset": {"type": "unknown"}
    }));

    let captured = captured_fence(&database, provider_id).await?;

    assert!(
        reconcile(
            &database,
            provider_id,
            captured,
            &observation(observed_at + 1, vec![available])
        )
        .await?,
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(
        state.blocks.is_empty(),
        "the availability evidence cleared the block"
    );

    // Re-establish the block, then an unknown measurement must preserve it.
    let captured = captured_fence(&database, provider_id).await?;

    reconcile(
        &database,
        provider_id,
        captured,
        &observation(
            observed_at + 2,
            vec![entry(exhausted_counter(
                "rpm",
                serde_json::json!({"type": "all_models"}),
                serde_json::json!({"type": "unknown"}),
            ))],
        ),
    )
    .await?;

    let unknown = entry(serde_json::json!({
        "key": ["rpm"],
        "label": "Fixture",
        "scope": {"type": "unknown"},
        "routing": {"type": "all_models"},
        "window": {"type": "unknown"},
        "measurement": {"type": "unknown"},
        "reset": {"type": "unknown"}
    }));

    let captured = captured_fence(&database, provider_id).await?;

    assert!(
        reconcile(
            &database,
            provider_id,
            captured,
            &observation(observed_at + 3, vec![unknown])
        )
        .await?,
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.blocks.len(),
        1,
        "an unknown measurement preserves the established block"
    );

    // A monitoring-only observation never blocks, whatever its measurement says.
    let monitoring = entry(serde_json::json!({
        "key": ["labels"],
        "label": "Fixture",
        "scope": {"type": "unknown"},
        "routing": {"type": "monitoring_only"},
        "window": {"type": "unknown"},
        "measurement": {"type": "ratio", "used_fraction": 1.5},
        "reset": {"type": "unknown"}
    }));

    let captured = captured_fence(&database, provider_id).await?;

    assert!(
        reconcile(
            &database,
            provider_id,
            captured,
            &observation(observed_at + 4, vec![monitoring])
        )
        .await?,
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(
        state
            .blocks
            .iter()
            .all(|block| block.key != vec!["labels".to_string()]),
        "monitoring-only observations never inhabit the durable state"
    );

    // A stale captured fence — the generation moved on — applies nothing.
    let stale = QuotaFence {
        generation: 999,
        revision: 0,
    };

    let revision_before = load_state(&database, provider_id)
        .await?
        .expect("the provider exists")
        .fence
        .revision;

    assert!(
        !reconcile(
            &database,
            provider_id,
            stale,
            &observation(observed_at + 5, Vec::new())
        )
        .await?,
        "a stale generation applies nothing"
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.fence.revision, revision_before,
        "the stale reconcile left the fence untouched"
    );

    Ok(())
}

/// A dispatch rejection is recorded only against the current generation, preserves a
/// strictly-future stored reset and drops a passed one, and canonicalizes the stored
/// applicability.
#[tokio::test]
async fn rejection_is_generation_gated_and_preserves_resets() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database).await?;

    let observed_at = 2_000_000_000_000;

    let reset_at = observed_at + 120_000_000;

    let generation = captured_fence(&database, provider_id).await?.generation;

    let applicability = QuotaApplicability::Models {
        upstream_ids: vec!["beta".to_string(), "alpha".to_string()],
    };

    assert!(
        record_rejection(
            &database,
            provider_id,
            generation,
            &["rpm".to_string()],
            &applicability,
            observed_at,
        )
        .await?,
        "the current generation records the rejection"
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.fence.revision, 1,
        "the rejection advanced the revision"
    );

    assert_eq!(
        state.blocks.first().map(|block| block.applies_to.clone()),
        Some(QuotaApplicability::Models {
            upstream_ids: vec!["alpha".to_string(), "beta".to_string()]
        }),
        "the stored applicability is canonicalized"
    );

    assert_eq!(
        state.blocks.first().map(|block| block.blocked_until),
        Some(None),
        "an unknown reset stays null"
    );

    // A strictly-future stored reset survives a newer rejection of the same restriction, and a
    // passed reset is dropped by the rejection that observes it past.
    let future_reset = observed_at + 120_000_000;

    // Install the future reset through a full observation.
    reconcile(
        &database,
        provider_id,
        captured_fence(&database, provider_id).await?,
        &observation(
            observed_at,
            vec![entry(exhausted_counter(
                "rpm",
                serde_json::json!({"type": "all_models"}),
                serde_json::json!({"type": "at", "at": future_reset}),
            ))],
        ),
    )
    .await?;

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.blocks.first().map(|block| block.blocked_until),
        Some(Some(future_reset)),
    );

    assert!(
        record_rejection(
            &database,
            provider_id,
            captured_fence(&database, provider_id).await?.generation,
            &["rpm".to_string()],
            &applicability,
            observed_at + 20,
        )
        .await?,
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.blocks.first().map(|block| block.blocked_until),
        Some(Some(future_reset)),
        "the strictly-future reset survives the newer rejection"
    );

    // A passed reset is not preserved: the new observation's stamp replaces it.
    assert!(
        record_rejection(
            &database,
            provider_id,
            captured_fence(&database, provider_id).await?.generation,
            &["rpm".to_string()],
            &applicability,
            future_reset + 1,
        )
        .await?,
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert_eq!(
        state.blocks.first().map(|block| block.blocked_until),
        Some(None),
    );

    // A newer generation suppresses the rejection entirely.
    super::reset(&database, provider_id).await?;

    let stale_generation = generation;

    assert!(
        !record_rejection(
            &database,
            provider_id,
            stale_generation,
            &["rpm".to_string()],
            &applicability,
            reset_at + 2,
        )
        .await?,
        "a rejection against a stale generation records nothing"
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(state.blocks.is_empty());

    // A deleted account answers false.
    crate::storage::providers::delete_provider(&database, provider_id).await?;

    assert!(
        !record_rejection(
            &database,
            provider_id,
            stale_generation,
            &["rpm".to_string()],
            &applicability,
            reset_at + 3,
        )
        .await?,
        "a deleted account records nothing"
    );

    Ok(())
}

/// The explicit availability reset clears the durable restrictions and advances both fences
/// without touching the administrator serving switch, and an unknown account is not found.
#[tokio::test]
async fn reset_clears_blocks_and_advances_both_fences() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database).await?;

    let observed_at = 3_000_000_000_000;

    let captured = captured_fence(&database, provider_id).await?;

    reconcile(
        &database,
        provider_id,
        captured,
        &observation(
            observed_at,
            vec![entry(exhausted_counter(
                "rpm",
                serde_json::json!({"type": "all_models"}),
                serde_json::json!({"type": "unknown"}),
            ))],
        ),
    )
    .await?;

    let fence = reset(&database, provider_id).await?;

    assert_eq!(
        fence,
        QuotaFence {
            generation: 1,
            revision: 2
        },
        "the reset advanced both fences exactly once past the reconcile's revision"
    );

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(
        state.blocks.is_empty(),
        "the reset cleared the restrictions"
    );

    assert!(
        crate::storage::providers::find_provider_by_id(&database, provider_id)
            .await?
            .expect("the provider exists")
            .is_enabled,
        "the reset never touches the administrator serving switch"
    );

    let unknown = reset(&database, 9_999_999).await;

    assert!(
        matches!(&unknown, Err(StorageError::NotFound { .. })),
        "resetting an unknown account is not found, got {unknown:?}"
    );

    Ok(())
}

/// A provider mutation that replaces the credentials or moves the base URL invalidates the
/// obsolete evidence transactionally: the blocks clear and both fences advance.
#[tokio::test]
async fn provider_mutations_invalidate_obsolete_evidence() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database).await?;

    let observed_at = 4_000_000_000_000;

    let captured = captured_fence(&database, provider_id).await?;

    reconcile(
        &database,
        provider_id,
        captured,
        &observation(
            observed_at,
            vec![entry(exhausted_counter(
                "rpm",
                serde_json::json!({"type": "all_models"}),
                serde_json::json!({"type": "unknown"}),
            ))],
        ),
    )
    .await?;

    crate::storage::providers::update_provider(
        &database,
        provider_id,
        crate::storage::providers::ProviderUpdate {
            name: "acct",
            model_prefix: "",
            base_url: Some("https://moved.test"),
            is_enabled: true,
            definition: None,
            credentials: None,
        },
    )
    .await?;

    let state = load_state(&database, provider_id)
        .await?
        .expect("the provider exists");

    assert!(
        state.blocks.is_empty(),
        "the base-URL change cleared the blocks"
    );

    assert_eq!(
        state.fence,
        QuotaFence {
            generation: 1,
            revision: 2
        },
        "the invalidation advanced both fences past the reconcile's revision"
    );

    Ok(())
}
