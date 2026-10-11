//! Model row persistence tests: the discovery apply (blanket vanish, upsert of listed rows with
//! base-fact rewrite and override preservation, manual rows untouched, identity preserved), the
//! checked creation and vanished-row claim, the override rewrite, deletion with its route cascade
//! and never-recycled identity, the ordered entry listing, and the effectively-enabled listing's
//! override-over-discovered resolution.

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

use super::{ModelCreate, ModelUpdate};
use crate::{
    compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    database::Database,
    provider::{DiscoveredModel, DiscoveryState, StoredCredentials, definition::DefinitionPayload},
    routing::ModelRouteTarget,
    storage::{
        Error as StorageError,
        api_keys::ApiKeyFields,
        models::{ModelEntry, list_enabled_models_with_owner},
        providers,
    },
    vocabulary::ReasoningLevel,
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
async fn seed_provider(
    database: &Database,
    provider_name: &str,
    model_prefix: &str,
) -> Result<i64, StorageError> {
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

    let provider = providers::insert_provider(
        database,
        providers::NewProvider {
            name: provider_name,
            model_prefix,
            base_url: None,
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    Ok(provider.id)
}

/// Inserts one enabled serving key with an allow-all default, returning its row identity.
async fn seed_api_key(database: &Database) -> Result<i64, StorageError> {
    use crate::access_control::{AccessDecision, ApiKeyAccessControl};

    let policy = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };

    let inserted = crate::storage::api_keys::insert_api_key(
        database,
        &ApiKeyFields {
            name: "fixture-key",
            key: "sk-serving-fixture",
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?;

    Ok(inserted.id)
}

/// Builds one discovered-model fixture with the given capability facts.
fn discovered(
    upstream_id: &str,
    thinking_levels: Option<&[ReasoningLevel]>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    compaction: Option<CompactionSupportFacts>,
) -> DiscoveredModel {
    DiscoveredModel {
        upstream_id: upstream_id.to_string(),
        thinking_levels: thinking_levels
            .map(|levels| ReasoningLevel::canonicalize_levels(levels.iter().copied())),
        max_input_tokens,
        max_output_tokens,
        compaction,
    }
}

/// The compaction base facts of one supported mechanism.
fn supported(mechanism: Mechanism) -> CompactionSupportFacts {
    CompactionSupportFacts::from_pairs([(mechanism, MechanismSupport::Supported)])
        .expect("one known pair builds facts")
}

/// Builds the checked-creation request of a manual row with the given overrides.
fn manual_create<'a>(
    provider_id: i64,
    upstream_id: &'a str,
    is_enabled: bool,
    thinking_levels_override: Option<&'a [ReasoningLevel]>,
    max_input_tokens_override: Option<i64>,
) -> ModelCreate<'a> {
    ModelCreate {
        provider_id,
        upstream_id,
        is_enabled,
        thinking_levels_override,
        max_input_tokens_override,
        max_output_tokens_override: None,
        compaction_support_override: None,
    }
}

/// The entry of `upstream_id` in a refresh result; an absent entry is the failure under test.
fn entry_for(entries: &[ModelEntry], upstream_id: &str) -> Result<ModelEntry, StorageError> {
    entries
        .iter()
        .find(|entry| entry.model.upstream_id == upstream_id)
        .cloned()
        .ok_or_else(|| StorageError::NotFound {
            message: format!("the refresh result carries no entry `{upstream_id}`"),
        })
}

/// Discovery atomically marks unlisted discovered rows vanished, upserts listed rows with fresh
/// base facts while preserving identity, switches, and administrator overrides, and leaves manual
/// rows untouched.
#[tokio::test]
async fn discovery_upserts_vanishes_and_never_touches_manual_rows() -> Result<(), Box<dyn StdError>>
{
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    let first = super::apply_discovered_models(
        &database,
        provider_id,
        &[
            discovered(
                "a",
                Some(&[ReasoningLevel::Low, ReasoningLevel::Medium]),
                Some(100),
                Some(50),
                Some(supported(Mechanism::ResponsesStandaloneCompact)),
            ),
            discovered("b", None, None, None, None),
        ],
    )
    .await?;

    assert_eq!(
        first.len(),
        2,
        "both rows are listed after the first refresh"
    );

    assert!(
        first.iter().all(
            |entry| entry.model.discovery_state == DiscoveryState::Discovered
                && entry.model.is_enabled
        ),
        "discovered rows insert enabled and discovered"
    );

    // The administrator override on `b` must survive the next refresh untouched.
    super::update_model_overrides_checked(
        &database,
        ModelUpdate {
            model_id: entry_for(&first, "b")?.model.id,
            is_enabled: true,
            thinking_levels_override: Some(&[ReasoningLevel::High]),
            max_input_tokens_override: Some(500),
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    let manual = super::insert_model_checked(
        &database,
        manual_create(
            provider_id,
            "manual",
            true,
            Some(&[ReasoningLevel::Low]),
            None,
        ),
    )
    .await?;

    assert_eq!(manual.model.discovery_state, DiscoveryState::Manual);

    let second = super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered(
            "a",
            Some(&[ReasoningLevel::High]),
            Some(111),
            None,
            None,
        )],
    )
    .await?;

    let a = entry_for(&second, "a")?;

    assert_eq!(
        a.model.id,
        entry_for(&first, "a")?.model.id,
        "a re-listed row keeps its identity"
    );

    assert_eq!(
        a.model.thinking_levels,
        Some(vec![ReasoningLevel::High]),
        "the refresh rewrites the base facts to the latest upstream knowledge"
    );

    assert_eq!(
        a.model.max_input_tokens,
        Some(111),
        "an extracted fact this refresh carried replaces the stored one"
    );

    let b = entry_for(&second, "b")?;

    assert_eq!(b.model.discovery_state, DiscoveryState::Vanished);

    assert_eq!(
        b.model.thinking_levels_override,
        Some(vec![ReasoningLevel::High]),
        "the vanished row keeps its administrator override"
    );

    assert_eq!(b.model.max_input_tokens_override, Some(500));

    let manual_after = entry_for(&second, "manual")?;

    assert_eq!(
        manual_after.model.discovery_state,
        DiscoveryState::Manual,
        "a manual row is never marked vanished by an omission"
    );

    assert_eq!(
        manual_after.model.id, manual.model.id,
        "the manual row keeps its identity"
    );

    Ok(())
}

/// Checked creation inserts a manual row with unknown base facts, conflicts on an already-listed
/// natural key, and claims a vanished row into manual state while preserving identity and the
/// last discovered base facts.
#[tokio::test]
async fn creation_conflicts_and_claims_vanished_rows() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    let created = super::insert_model_checked(
        &database,
        manual_create(provider_id, "k", true, None, Some(77)),
    )
    .await?;

    assert_eq!(created.model.discovery_state, DiscoveryState::Manual);

    assert_eq!(
        created.model.thinking_levels, None,
        "a fresh manual row starts unknown"
    );

    assert_eq!(created.model.max_input_tokens_override, Some(77));

    let repeated =
        super::insert_model_checked(&database, manual_create(provider_id, "k", true, None, None))
            .await;

    assert!(
        matches!(&repeated, Err(StorageError::Conflict { .. })),
        "an already-listed natural key conflicts, got {repeated:?}"
    );

    // Discovery lists `k`, then omits it — the row turns discovered and then vanished.
    super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered(
            "k",
            Some(&[ReasoningLevel::Low]),
            Some(42),
            None,
            None,
        )],
    )
    .await?;

    super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered("other", None, None, None, None)],
    )
    .await?;

    let listed = super::list_model_entries(&database, Some(provider_id)).await?;

    let vanished = entry_for(&listed, "k")?;

    assert_eq!(vanished.model.discovery_state, DiscoveryState::Vanished);

    assert_eq!(
        vanished.model.thinking_levels,
        Some(vec![ReasoningLevel::Low])
    );

    assert_eq!(vanished.model.max_input_tokens, Some(42));

    let claimed = super::insert_model_checked(
        &database,
        manual_create(provider_id, "k", false, Some(&[ReasoningLevel::High]), None),
    )
    .await?;

    assert_eq!(
        claimed.model.id, created.model.id,
        "the claim keeps the vanished row's identity"
    );

    assert_eq!(claimed.model.discovery_state, DiscoveryState::Manual);

    assert!(
        !claimed.model.is_enabled,
        "the claim applies the requested switch"
    );

    assert_eq!(
        claimed.model.thinking_levels,
        Some(vec![ReasoningLevel::Low]),
        "the claim keeps the last discovered base facts"
    );

    assert_eq!(
        claimed.model.thinking_levels_override,
        Some(vec![ReasoningLevel::High]),
        "the claim applies the requested overrides"
    );

    Ok(())
}

/// The override rewrite changes only the administrator-owned columns, preserves discovery state
/// and base facts, and answers not-found for an unknown row.
#[tokio::test]
async fn override_rewrite_leaves_discovery_state_and_base_facts() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    let entries = super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered(
            "a",
            Some(&[ReasoningLevel::Low]),
            Some(100),
            Some(50),
            Some(supported(Mechanism::MessagesOnDemandCompaction)),
        )],
    )
    .await?;

    let model_id = entry_for(&entries, "a")?.model.id;

    let updated = super::update_model_overrides_checked(
        &database,
        ModelUpdate {
            model_id,
            is_enabled: false,
            thinking_levels_override: Some(&[ReasoningLevel::Max]),
            max_input_tokens_override: Some(900),
            max_output_tokens_override: Some(901),
            compaction_support_override: Some(
                &CompactionSupportFacts::from_pairs([(
                    Mechanism::MessagesOnDemandCompaction,
                    MechanismSupport::Unsupported,
                )])
                .expect("one known pair builds facts"),
            ),
        },
    )
    .await?;

    assert!(!updated.model.is_enabled);

    assert_eq!(
        updated.model.thinking_levels,
        Some(vec![ReasoningLevel::Low])
    );

    assert_eq!(updated.model.max_input_tokens, Some(100));

    assert_eq!(updated.model.max_output_tokens, Some(50));

    assert_eq!(
        updated.model.thinking_levels_override,
        Some(vec![ReasoningLevel::Max])
    );

    assert_eq!(updated.model.max_input_tokens_override, Some(900));

    assert_eq!(updated.model.max_output_tokens_override, Some(901));

    assert_eq!(
        updated
            .model
            .compaction_support_override
            .map(|facts| facts.get(Mechanism::MessagesOnDemandCompaction)),
        Some(Some(MechanismSupport::Unsupported)),
        "the override rewrite stores the compaction facts"
    );

    let unknown = super::update_model_overrides_checked(
        &database,
        ModelUpdate {
            model_id: 4_242_424,
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await;

    assert!(
        matches!(&unknown, Err(StorageError::NotFound { .. })),
        "an unknown row is not found, got {unknown:?}"
    );

    Ok(())
}

/// Deletion removes the row and its automatic route, answers not-found for an unknown row, and a
/// rediscovery of the same upstream creates a fresh, never-recycled identity.
#[tokio::test]
async fn deletion_cascades_routes_and_never_recycles_identity() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "").await?;

    let entries = super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered("m", None, None, None, None)],
    )
    .await?;

    let original_id = entry_for(&entries, "m")?.model.id;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        routes.iter().any(|route| route.public_id == "m"
            && route.target
                == ModelRouteTarget::Direct {
                    model_id: original_id
                }),
        "the standalone model carries its automatic direct route"
    );

    super::delete_model_checked(&database, original_id).await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        routes.is_empty(),
        "the deletion removes the model's direct routes: {routes:?}"
    );

    let missing = super::delete_model_checked(&database, original_id).await;

    assert!(
        matches!(&missing, Err(StorageError::NotFound { .. })),
        "deleting an already-deleted row is not found, got {missing:?}"
    );

    let rediscovered = super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered("m", None, None, None, None)],
    )
    .await?;

    let new_id = entry_for(&rediscovered, "m")?.model.id;

    assert!(
        new_id > original_id,
        "the rebuilt row carries a fresh, never-recycled identity ({new_id} must exceed \
         {original_id})"
    );

    Ok(())
}

/// The entry listing carries provider identity, orders by provider id then upstream id, includes
/// vanished rows, and an unknown provider filter answers an empty list.
#[tokio::test]
async fn entry_listing_orders_and_filters_by_provider() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let first_provider = seed_provider(&database, "acct", "").await?;

    let second_provider = seed_provider(&database, "zulu", "").await?;

    super::apply_discovered_models(
        &database,
        second_provider,
        &[discovered("b", None, None, None, None)],
    )
    .await?;

    super::apply_discovered_models(
        &database,
        first_provider,
        &[
            discovered("z", None, None, None, None),
            discovered("a", None, None, None, None),
        ],
    )
    .await?;

    // Re-discover only `z` on the first provider: `a` turns vanished and stays listed.
    super::apply_discovered_models(
        &database,
        first_provider,
        &[discovered("z", None, None, None, None)],
    )
    .await?;

    let all = super::list_model_entries(&database, None).await?;

    let order = all
        .iter()
        .map(|entry| (entry.provider_id, entry.model.upstream_id.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(
        order,
        [
            (first_provider, "a"),
            (first_provider, "z"),
            (second_provider, "b")
        ],
        "the listing orders by provider id then upstream id and includes vanished rows"
    );

    assert_eq!(
        all.first().map(|entry| entry.provider_name.as_str()),
        Some("acct")
    );

    let filtered = super::list_model_entries(&database, Some(second_provider)).await?;

    assert_eq!(filtered.len(), 1);

    assert_eq!(
        filtered
            .first()
            .map(|entry| entry.model.upstream_id.as_str()),
        Some("b")
    );

    let unknown = super::list_model_entries(&database, Some(9_999_999)).await?;

    assert!(
        unknown.is_empty(),
        "an unknown provider filter lists nothing"
    );

    Ok(())
}

/// The effectively-enabled listing resolves overrides over discovered facts, and an unknown
/// declaration stays unknown.
#[tokio::test]
async fn enabled_listing_resolves_override_over_discovered() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let api_key_id = seed_api_key(&database).await?;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    let entries = super::apply_discovered_models(
        &database,
        provider_id,
        &[discovered(
            "m",
            Some(&[ReasoningLevel::Low]),
            Some(100),
            None,
            None,
        )],
    )
    .await?;

    super::update_model_overrides_checked(
        &database,
        ModelUpdate {
            model_id: entry_for(&entries, "m")?.model.id,
            is_enabled: true,
            thinking_levels_override: Some(&[ReasoningLevel::High]),
            max_input_tokens_override: Some(300),
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    let listed = list_enabled_models_with_owner(&database, api_key_id).await?;

    let listed_model = listed.first().ok_or(StorageError::NotFound {
        message: "the enabled model is listed".to_string(),
    })?;

    assert_eq!(listed_model.public_id, "acme.m");

    assert_eq!(listed_model.owner_name, "acct");

    assert_eq!(
        listed_model.thinking_levels,
        Some(vec![ReasoningLevel::High]),
        "the override replaces the discovered level set"
    );

    assert_eq!(listed_model.max_input_tokens, Some(300));

    assert_eq!(
        listed_model.max_output_tokens, None,
        "an unknown declaration is omitted, never defaulted"
    );

    Ok(())
}
