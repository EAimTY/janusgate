//! Automatic model-exposure tests, driven through the storage mutations that synchronize the
//! automatic namespace: standalone exposure under the provider prefix, membership suppression and
//! restoration, the pooled union, in-place prefix renames preserving route identity, disabled and
//! vanished inventory keeping their entries, naming conflicts rolling back the whole mutation,
//! and the provider-deletion cascade.

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

use crate::{
    database::Database,
    provider::{DiscoveredModel, StoredCredentials},
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{Error as StorageError, models::ModelCreate, providers, routing::ModelRoute},
};
use std::{error::Error as StdError, num::NonZeroI64};

/// Parses the static-API-key fixture definition payload.
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

/// Discovers the given upstream identifiers on the provider.
async fn discover(
    database: &Database,
    provider_id: i64,
    upstream_ids: &[&str],
) -> Result<(), StorageError> {
    let discovered = upstream_ids
        .iter()
        .map(|upstream_id| DiscoveredModel {
            upstream_id: (*upstream_id).to_string(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        })
        .collect::<Vec<_>>();

    crate::storage::models::apply_discovered_models(database, provider_id, &discovered).await?;

    Ok(())
}

/// The automatic route targeting `target`, if the inventory carries one.
async fn automatic_route(
    database: &Database,
    target: &ModelRouteTarget,
) -> Result<Option<ModelRoute>, StorageError> {
    Ok(crate::storage::routing::list_routes(database)
        .await?
        .into_iter()
        .find(|route| route.is_automatic && &route.target == target))
}

/// The member fixture of `provider_id` at `priority`.
fn member(provider_id: i64, priority: u32) -> ProviderPoolMember {
    ProviderPoolMember {
        provider_id,
        priority,
    }
}

/// A standalone provider exposes each stored model under its prefix; any pool membership
/// suppresses the standalone entries; a prefix rename preserves the surviving route identity;
/// removing the last membership restores standalone exposure under a fresh identity.
#[tokio::test]
async fn membership_suppresses_and_restores_standalone_exposure() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    discover(&database, provider_id, &["m1"]).await?;

    let standalone = crate::storage::routing::list_routes(&database).await?;

    let original = standalone
        .iter()
        .find(|route| route.public_id == "acme.m1")
        .ok_or_else(|| StorageError::NotFound {
            message: "the standalone entry exists".to_string(),
        })?;

    assert!(original.is_automatic);

    let pool_id = crate::storage::routing::create_pool(
        &database,
        "pool",
        "p.",
        true,
        &[member(provider_id, 0)],
    )
    .await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        !routes.iter().any(|route| route.public_id == "acme.m1"),
        "any pool membership suppresses the standalone entry"
    );

    let pooled = routes
        .iter()
        .find(|route| route.public_id == "p.m1")
        .ok_or_else(|| StorageError::NotFound {
            message: "the pool entry exists".to_string(),
        })?;

    assert_eq!(
        pooled.target,
        ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "m1".to_string()
        },
    );

    let pooled_id = pooled.id;

    crate::storage::routing::update_pool(
        &database,
        pool_id,
        "pool",
        "q.",
        true,
        &[member(provider_id, 0)],
    )
    .await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    let renamed = routes
        .iter()
        .find(|route| route.public_id == "q.m1")
        .ok_or_else(|| StorageError::NotFound {
            message: "the renamed pool entry exists".to_string(),
        })?;

    assert_eq!(
        renamed.id, pooled_id,
        "the prefix rename preserves the surviving route identity"
    );

    crate::storage::routing::update_pool(&database, pool_id, "pool", "q.", true, &[]).await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        !routes.iter().any(|route| route.public_id == "q.m1"),
        "the emptied pool loses its entry"
    );

    let restored = routes
        .iter()
        .find(|route| route.public_id == "acme.m1")
        .ok_or_else(|| StorageError::NotFound {
            message: "removing the last membership restores the standalone entry".to_string(),
        })?;

    assert_ne!(
        restored.id, original.id,
        "the departed standalone identity is gone; the restored entry is a fresh row"
    );

    Ok(())
}

/// Each pool exposes the union of its members' stored models; disabled and vanished inventory
/// keeps the automatic entry for recovery; removing the last inventory owner removes the entry.
#[tokio::test]
async fn pooled_union_keeps_gated_entries_and_removes_departed_ones()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let first = seed_provider(&database, "acct", "").await?;

    let second = seed_provider(&database, "other", "").await?;

    let pool_id = crate::storage::routing::create_pool(
        &database,
        "pool",
        "p.",
        true,
        &[member(first, 0), member(second, 1)],
    )
    .await?;

    discover(&database, first, &["shared", "first-only"]).await?;

    discover(&database, second, &["shared", "second-only"]).await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    let pooled_names = routes
        .iter()
        .filter(|route| route.is_automatic)
        .map(|route| route.public_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        pooled_names,
        ["p.first-only", "p.second-only", "p.shared"],
        "the pool exposes the union of its members' stored models under its prefix"
    );

    // The `shared` model of the second member turns vanished: the entry stays for recovery.
    discover(&database, second, &["second-only"]).await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        routes.iter().any(|route| route.public_id == "p.shared"),
        "vanished inventory keeps the automatic entry"
    );

    // A disabled pool keeps its entries, too.
    crate::storage::routing::update_pool(
        &database,
        pool_id,
        "pool",
        "p.",
        false,
        &[member(first, 0), member(second, 1)],
    )
    .await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert_eq!(
        routes.iter().filter(|route| route.is_automatic).count(),
        3,
        "a disabled pool keeps its automatic entries"
    );

    // Removing the first member leaves `second-only` owned by the second member alone.
    crate::storage::routing::update_pool(
        &database,
        pool_id,
        "pool",
        "p.",
        false,
        &[member(second, 0)],
    )
    .await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    let pooled_names = routes
        .iter()
        .filter(|route| route.is_automatic)
        .map(|route| route.public_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        pooled_names,
        ["first-only", "p.second-only", "p.shared", "shared"],
        "the departed member's unique model loses its pool entry while the shared model stays; \
         the departed member's standalone exposure returns"
    );

    // Removing the last inventory owner of `shared` removes the entry: the vanished rows keep
    // their entries, so the owner departs by row deletion, never by an omitted discovery.
    let entries = crate::storage::models::list_model_entries(&database, Some(second)).await?;

    for entry in entries {
        if entry.model.upstream_id == "shared" {
            crate::storage::models::delete_model_checked(&database, entry.model.id).await?;
        }
    }

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        !routes.iter().any(|route| route.public_id == "p.shared"),
        "removing the last inventory owner removes the automatic entry"
    );

    assert!(
        routes
            .iter()
            .any(|route| route.public_id == "p.second-only"),
        "the surviving owned entry remains"
    );

    Ok(())
}

/// A naming conflict between the desired automatic entries and an authored alias rolls back the
/// whole owning mutation — the inventory write included — and succeeds once the alias moves
/// aside. An authored alias can never take an automatic entry's name either.
#[tokio::test]
async fn naming_conflicts_roll_back_the_whole_mutation() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "x.").await?;

    let pool_id = crate::storage::routing::create_pool(
        &database,
        "pool",
        "x.",
        true,
        &[member(provider_id, 0)],
    )
    .await?;

    let alias = crate::storage::routing::create_route(
        &database,
        "x.future",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "future".to_string(),
        },
    )
    .await?;

    let conflicted = crate::storage::models::insert_model_checked(
        &database,
        ModelCreate {
            provider_id,
            upstream_id: "future",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await;

    assert!(
        matches!(&conflicted, Err(StorageError::Conflict { .. })),
        "the automatic entry's desired name is taken by the alias, got {conflicted:?}"
    );

    let entries = crate::storage::models::list_model_entries(&database, Some(provider_id)).await?;

    assert!(
        entries.is_empty(),
        "the conflict rolled back the model insertion: {entries:?}"
    );

    crate::storage::routing::update_route(
        &database,
        alias,
        "x.future-alias",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "future".to_string(),
        },
    )
    .await?;

    let inserted = crate::storage::models::insert_model_checked(
        &database,
        ModelCreate {
            provider_id,
            upstream_id: "future",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    let desired = automatic_route(
        &database,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "future".to_string(),
        },
    )
    .await?;

    assert!(
        desired.is_some(),
        "the pool entry exists once the alias moved aside"
    );

    drop(inserted);

    // The authored namespace cannot take an automatic entry's name: the model now carries the
    // automatic `x.future` entry, so the old alias name is refused to a new authored route.
    let hijack = crate::storage::routing::create_route(
        &database,
        "x.future",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "future".to_string(),
        },
    )
    .await;

    assert!(
        matches!(&hijack, Err(StorageError::Conflict { .. })),
        "an authored route cannot take an automatic entry's name, got {hijack:?}"
    );

    Ok(())
}

/// Deleting a provider cascades its physical models and routes: the pool entries of its unique
/// upstream models disappear with it, the shared ones survive through the remaining member, and
/// the standalone exposure of a departed member never returns.
#[tokio::test]
async fn provider_deletion_cascades_models_and_routes() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let first = seed_provider(&database, "acct", "").await?;

    let second = seed_provider(&database, "other", "").await?;

    let _pool_id = crate::storage::routing::create_pool(
        &database,
        "pool",
        "p.",
        true,
        &[member(first, 0), member(second, 1)],
    )
    .await?;

    discover(&database, first, &["shared", "first-only"]).await?;

    discover(&database, second, &["shared"]).await?;

    crate::storage::providers::delete_provider(&database, first).await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        !routes.iter().any(|route| route.public_id == "p.first-only"),
        "the deleted provider's unique model loses its pool entry"
    );

    assert!(
        routes.iter().any(|route| route.public_id == "p.shared"),
        "the shared model's entry survives through the remaining member"
    );

    crate::storage::providers::delete_provider(&database, second).await?;

    let routes = crate::storage::routing::list_routes(&database).await?;

    assert!(
        !routes.iter().any(|route| route.is_automatic),
        "no inventory owner is left: {routes:?}"
    );

    let pools = crate::storage::routing::list_pools(&database).await?;

    assert_eq!(pools.len(), 1, "the pool itself survives its members");

    Ok(())
}
