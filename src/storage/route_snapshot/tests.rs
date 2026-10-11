//! The dispatch-admission snapshot tests: one coherent read resolving the key's enabled policy,
//! the route row, and its concrete target — a direct route's provider-model pair, a pool's
//! authored member order with absent inventory and unavailable accounts retained — and the
//! policy filtering that answers no snapshot for a denied route and the inactive-key failure for
//! a disabled key.

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

use super::TargetSnapshot;
use crate::{
    access_control::{AccessDecision, ApiKeyAccessControl},
    database::Database,
    provider::{DiscoveredModel, StoredCredentials},
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{
        Error as StorageError,
        api_keys::{ApiKeyFields, insert_api_key, update_api_key},
        models::ModelUpdate,
        providers,
    },
    vocabulary::ReasoningLevel,
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

/// Discovers one upstream identifier on the provider.
async fn discover_one(
    database: &Database,
    provider_id: i64,
    upstream_id: &str,
) -> Result<i64, StorageError> {
    let entries = crate::storage::models::apply_discovered_models(
        database,
        provider_id,
        &[DiscoveredModel {
            upstream_id: upstream_id.to_string(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }],
    )
    .await?;

    entries
        .iter()
        .find(|entry| entry.model.upstream_id == upstream_id)
        .map(|entry| entry.model.id)
        .ok_or(StorageError::NotFound {
            message: format!("the refresh listed {upstream_id}"),
        })
}

/// Inserts one enabled serving key with an allow-all default, returning its row identity.
async fn seed_allowing_key(database: &Database) -> Result<i64, StorageError> {
    let policy = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };

    Ok(insert_api_key(
        database,
        &ApiKeyFields {
            name: "allowing",
            key: "sk-allowing",
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?
    .id)
}

/// Inserts one enabled serving key with a deny-all default, returning its row identity.
async fn seed_denying_key(database: &Database) -> Result<i64, StorageError> {
    let policy = ApiKeyAccessControl {
        default_access: AccessDecision::Deny,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };

    Ok(insert_api_key(
        database,
        &ApiKeyFields {
            name: "denying",
            key: "sk-denying",
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?
    .id)
}

/// A direct route's snapshot resolves the route row and its concrete provider-model target;
/// a denied route and an unknown public ID answer no snapshot; a disabled key is the typed
/// inactive-key failure.
#[tokio::test]
async fn direct_snapshot_resolves_target_and_policy() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let provider_id = seed_provider(&database, "acct", "acme.").await?;

    let model_id = discover_one(&database, provider_id, "m").await?;

    crate::storage::models::update_model_overrides_checked(
        &database,
        ModelUpdate {
            model_id,
            is_enabled: true,
            thinking_levels_override: Some(&[ReasoningLevel::Low]),
            max_input_tokens_override: Some(42),
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    let route_id = crate::storage::routing::create_route(
        &database,
        "acme.alias",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    let allowing = seed_allowing_key(&database).await?;

    let snapshot = super::find_admitted_route(&database, "acme.alias", allowing)
        .await?
        .ok_or(StorageError::NotFound {
            message: "the allowed route admits a snapshot".to_string(),
        })?;

    assert_eq!(snapshot.route.id, route_id);

    assert_eq!(snapshot.route.public_id, "acme.alias");

    assert!(snapshot.route.is_enabled);

    let TargetSnapshot::Direct { provider, model } = snapshot.target else {
        return Err(StorageError::NotFound {
            message: "the direct route resolves a direct target".to_string(),
        }
        .into());
    };

    assert_eq!(provider.id, provider_id);

    assert_eq!(provider.name, "acct");

    assert_eq!(model.id, model_id);

    assert_eq!(model.upstream_id, "m");

    // A denied route answers no snapshot: the 404-before-everything admission shape.
    let denying = seed_denying_key(&database).await?;

    assert!(
        super::find_admitted_route(&database, "acme.alias", denying)
            .await?
            .is_none()
    );

    // An unknown public ID answers no snapshot for any key.
    assert!(
        super::find_admitted_route(&database, "missing", allowing)
            .await?
            .is_none()
    );

    // A key disabled since authentication is the typed inactive failure.
    update_api_key(
        &database,
        allowing,
        ApiKeyFields {
            name: "allowing",
            key: "sk-allowing",
            is_enabled: false,
            access_control: &ApiKeyAccessControl {
                default_access: AccessDecision::Allow,
                owner_defaults: Vec::new(),
                route_overrides: Vec::new(),
            },
        },
    )
    .await?;

    let inactive = super::find_admitted_route(&database, "acme.alias", allowing).await;

    assert!(
        matches!(&inactive, Err(StorageError::InactiveApiKey { id }) if *id == allowing),
        "the disabled key is the inactive failure, got {inactive:?}"
    );

    Ok(())
}

/// A pooled route's snapshot carries the pool's serving switch and its members in authored
/// priority order, retaining disabled accounts and members whose inventory lacks the exact
/// upstream model.
#[tokio::test]
async fn pooled_snapshot_retains_authored_order_and_absent_inventory()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let first = seed_provider(&database, "first", "f.").await?;

    let second = seed_provider(&database, "second", "s.").await?;

    let third = seed_provider(&database, "third", "t.").await?;

    discover_one(&database, first, "m").await?;

    discover_one(&database, second, "m").await?;

    // The third member never stores `m`: its snapshot slot carries no model.
    discover_one(&database, third, "other").await?;

    let pool_id = crate::storage::routing::create_pool(
        &database,
        "pool",
        "p.",
        true,
        &[
            ProviderPoolMember {
                provider_id: second,
                priority: 0,
            },
            ProviderPoolMember {
                provider_id: third,
                priority: 3,
            },
            ProviderPoolMember {
                provider_id: first,
                priority: 7,
            },
        ],
    )
    .await?;

    let allowing = seed_allowing_key(&database).await?;

    let snapshot = super::find_admitted_route(&database, "p.m", allowing)
        .await?
        .ok_or(StorageError::NotFound {
            message: "the allowed pool route admits a snapshot".to_string(),
        })?;

    assert_eq!(snapshot.route.public_id, "p.m");

    let TargetSnapshot::Pool {
        id,
        is_enabled,
        members,
    } = snapshot.target
    else {
        return Err(StorageError::NotFound {
            message: "the pooled route resolves a pool target".to_string(),
        }
        .into());
    };

    assert_eq!(id, pool_id);

    assert!(is_enabled);

    let member_providers = members
        .iter()
        .map(|member| {
            (
                member.provider.id,
                member.model.as_ref().map(|model| model.upstream_id.clone()),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        member_providers,
        [
            (second, Some("m".to_string())),
            (third, None),
            (first, Some("m".to_string())),
        ],
        "the members ride the authored priority order with absent inventory retained"
    );

    Ok(())
}
