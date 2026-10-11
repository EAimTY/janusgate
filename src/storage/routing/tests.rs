//! Pool and authored-route persistence tests: atomic pool creation and membership replacement
//! with its name, reference, and priority-uniqueness gates; the authored-route reference gate on
//! pool deletion with automatic-entry removal and standalone restoration; route CRUD with
//! identity-preserving retarget and the automatic-ownership conflict; and the deletion-fence
//! presence read.

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

use super::ProviderPool;
use crate::{
    database::Database,
    provider::StoredCredentials,
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{Error as StorageError, api_keys::ApiKeyFields, providers},
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

/// The member fixture of `provider_id` at `priority`.
fn member(provider_id: i64, priority: u32) -> ProviderPoolMember {
    ProviderPoolMember {
        provider_id,
        priority,
    }
}

/// Pool creation stores the pool with its complete ordered membership, the update replaces both
/// atomically, and the taken-name, unknown-member, duplicate-priority, and unknown-pool gates
/// answer their typed failures.
#[tokio::test]
async fn pool_crud_replaces_membership_behind_its_gates() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let first = seed_provider(&database, "acct", "").await?;

    let second = seed_provider(&database, "other", "").await?;

    let pool_id = super::create_pool(
        &database,
        "pool",
        "p.",
        true,
        &[member(first, 0), member(second, 2)],
    )
    .await?;

    let listed = super::list_pools(&database).await?;

    assert_eq!(
        listed,
        vec![ProviderPool {
            id: pool_id,
            name: "pool".to_string(),
            model_prefix: "p.".to_string(),
            is_enabled: true,
            members: vec![member(first, 0), member(second, 2)],
        }],
        "the stored pool carries its complete ordered membership"
    );

    super::update_pool(
        &database,
        pool_id,
        "renamed",
        "q.",
        false,
        &[member(second, 0), member(first, 5)],
    )
    .await?;

    let listed_pools = super::list_pools(&database).await?;

    let updated = listed_pools.first().ok_or(StorageError::NotFound {
        message: "the updated pool is listed".to_string(),
    })?;

    assert_eq!(updated.name, "renamed");

    assert_eq!(updated.model_prefix, "q.");

    assert!(!updated.is_enabled);

    assert_eq!(
        updated.members,
        vec![member(second, 0), member(first, 5)],
        "the update replaces the whole membership, priorities included"
    );

    let duplicate_name = super::create_pool(&database, "renamed", "", true, &[]).await;

    assert!(
        matches!(&duplicate_name, Err(StorageError::Conflict { .. })),
        "a taken pool name conflicts, got {duplicate_name:?}"
    );

    let unknown_member =
        super::create_pool(&database, "fresh", "", true, &[member(9_999_999, 0)]).await;

    assert!(
        matches!(&unknown_member, Err(StorageError::NotFound { .. })),
        "an unknown member provider is not found, got {unknown_member:?}"
    );

    let duplicate_priority = super::create_pool(
        &database,
        "fresh",
        "",
        true,
        &[member(first, 0), member(second, 0)],
    )
    .await;

    assert!(
        matches!(&duplicate_priority, Err(StorageError::Constraint { .. })),
        "duplicate priorities violate the membership uniqueness, got {duplicate_priority:?}"
    );

    let unknown_pool = super::update_pool(&database, 9_999_999, "x", "", true, &[]).await;

    assert!(
        matches!(&unknown_pool, Err(StorageError::NotFound { .. })),
        "updating an unknown pool is not found, got {unknown_pool:?}"
    );

    Ok(())
}

/// Pool deletion conflicts while an authored route references the pool; without the reference it
/// removes the automatic pool entries and lets the standalone exposure return.
#[tokio::test]
async fn pool_deletion_gates_on_authored_routes_and_restores_standalone()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    crate::storage::models::apply_discovered_models(
        &database,
        provider_id,
        &[crate::provider::DiscoveredModel {
            upstream_id: "m".to_string(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }],
    )
    .await?;

    let pool_id =
        super::create_pool(&database, "pool", "p.", true, &[member(provider_id, 0)]).await?;

    let routes = super::list_routes(&database).await?;

    assert!(
        routes.iter().any(|route| route.public_id == "p.m"),
        "the pool member's model carries the automatic pool entry"
    );

    assert!(
        !routes.iter().any(|route| route.public_id == "acme.m"),
        "the pool membership suppresses the standalone entry"
    );

    let alias = super::create_route(
        &database,
        "alias",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "m".to_string(),
        },
    )
    .await?;

    let gated = super::delete_pool(&database, pool_id).await;

    assert!(
        matches!(&gated, Err(StorageError::Conflict { .. })),
        "a pool referenced by an authored route refuses deletion, got {gated:?}"
    );

    super::delete_route(&database, alias).await?;

    super::delete_pool(&database, pool_id).await?;

    let routes = super::list_routes(&database).await?;

    assert!(
        !routes.iter().any(|route| route.public_id == "p.m"),
        "the pool deletion removes its automatic entries"
    );

    assert!(
        routes.iter().any(|route| route.public_id == "acme.m"),
        "removing the last membership restores the standalone entry"
    );

    let missing = super::delete_pool(&database, pool_id).await;

    assert!(
        matches!(&missing, Err(StorageError::NotFound { .. })),
        "deleting an already-deleted pool is not found, got {missing:?}"
    );

    Ok(())
}

/// Authored-route CRUD validates its target and public name, preserves the row identity across a
/// retarget, refuses to edit or delete an automatic route, and answers not-found for unknown
/// routes.
#[tokio::test]
async fn route_crud_preserves_identity_and_respects_automatic_ownership()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "").await?;

    let pool_id =
        super::create_pool(&database, "pool", "", true, &[member(provider_id, 0)]).await?;

    crate::storage::models::apply_discovered_models(
        &database,
        provider_id,
        &[crate::provider::DiscoveredModel {
            upstream_id: "m".to_string(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }],
    )
    .await?;

    let routes = super::list_routes(&database).await?;

    let automatic_id = routes
        .iter()
        .find(|route| route.is_automatic)
        .map(|route| route.id)
        .ok_or_else(|| StorageError::NotFound {
            message: "the fixture carries no automatic route".to_string(),
        })?;

    let model_id = crate::storage::models::list_model_entries(&database, Some(provider_id))
        .await?
        .first()
        .map(|entry| entry.model.id)
        .ok_or_else(|| StorageError::NotFound {
            message: "the fixture provider carries its discovered model".to_string(),
        })?;

    let direct_id = super::create_route(
        &database,
        "alias",
        true,
        &ModelRouteTarget::Direct {
            model_id: 9_999_999,
        },
    )
    .await;

    assert!(
        matches!(&direct_id, Err(StorageError::NotFound { .. })),
        "a route to an unknown model is not found, got {direct_id:?}"
    );

    let direct_id = super::create_route(
        &database,
        "alias",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    let duplicated = super::create_route(
        &database,
        "alias",
        true,
        &ModelRouteTarget::Direct { model_id: 1 },
    )
    .await;

    assert!(
        matches!(&duplicated, Err(StorageError::Conflict { .. })),
        "a repeated public ID conflicts, got {duplicated:?}"
    );

    super::update_route(
        &database,
        direct_id,
        "alias2",
        false,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "m".to_string(),
        },
    )
    .await?;

    let routes = super::list_routes(&database).await?;

    let retargeted = routes
        .iter()
        .find(|route| route.id == direct_id)
        .ok_or_else(|| StorageError::NotFound {
            message: "the retargeted route keeps its identity".to_string(),
        })?;

    assert_eq!(retargeted.public_id, "alias2");

    assert!(!retargeted.is_enabled);

    assert_eq!(
        retargeted.target,
        ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "m".to_string()
        },
        "the retarget swapped the target under the preserved identity"
    );

    let automatic_edit = super::update_route(
        &database,
        automatic_id,
        "hijacked",
        true,
        &ModelRouteTarget::Direct { model_id: 1 },
    )
    .await;

    assert!(
        matches!(&automatic_edit, Err(StorageError::Conflict { .. })),
        "an automatic route refuses authored edits, got {automatic_edit:?}"
    );

    let automatic_delete = super::delete_route(&database, automatic_id).await;

    assert!(
        matches!(&automatic_delete, Err(StorageError::Conflict { .. })),
        "an automatic route refuses deletion, got {automatic_delete:?}"
    );

    let missing_delete = super::delete_route(&database, 9_999_999).await;

    assert!(
        matches!(&missing_delete, Err(StorageError::NotFound { .. })),
        "deleting an unknown route is not found, got {missing_delete:?}"
    );

    super::delete_route(&database, direct_id).await?;

    Ok(())
}

/// The deletion-fence presence read reports the committed existence of the authenticated key and
/// the authored pool of an affinity namespace.
#[tokio::test]
async fn namespace_presence_reports_deletions() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let _provider_id = seed_provider(&database, "acct", "").await?;

    let pool_id = super::create_pool(&database, "pool", "", true, &[]).await?;

    let api_key_id = seed_api_key(&database).await?;

    let present = super::namespace_presence(&database, api_key_id, pool_id).await?;

    assert!(present.has_api_key);

    assert!(present.has_pool);

    crate::storage::api_keys::delete_api_key(&database, api_key_id).await?;

    let without_key = super::namespace_presence(&database, api_key_id, pool_id).await?;

    assert!(!without_key.has_api_key);

    assert!(without_key.has_pool);

    Ok(())
}
