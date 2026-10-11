//! Public-model listing tests: structural availability gating (route, pool, provider, and model
//! switches; vanished inventory), the key-policy resolution (route override, typed owner default,
//! key default), the pooled capability composition (numerical minima, level intersection,
//! unknown omission, per-mechanism compaction support), the direct entry's override-over-discovered
//! resolution, and the public-ID ordering.

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
    access_control::{
        AccessDecision, ApiKeyAccessControl, ApiKeyOwnerDefault, ApiKeyRouteOverride,
    },
    compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    database::Database,
    provider::{DiscoveredModel, StoredCredentials},
    routing::ProviderPoolMember,
    storage::{
        Error as StorageError,
        api_keys::ApiKeyFields,
        models::{EnabledModel, ModelUpdate},
        providers::{NewProvider, ProviderUpdate},
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

    let provider = crate::storage::providers::insert_provider(
        database,
        NewProvider {
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

/// Discovers one upstream identifier on the provider with the given capability facts.
async fn discover_one(
    database: &Database,
    provider_id: i64,
    upstream_id: &str,
    thinking_levels: Option<&[ReasoningLevel]>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    compaction: Option<CompactionSupportFacts>,
) -> Result<i64, StorageError> {
    let entries = crate::storage::models::apply_discovered_models(
        database,
        provider_id,
        &[DiscoveredModel {
            upstream_id: upstream_id.to_string(),
            thinking_levels: thinking_levels
                .map(|levels| ReasoningLevel::canonicalize_levels(levels.iter().copied())),
            max_input_tokens,
            max_output_tokens,
            compaction,
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

/// Inserts one enabled serving key with the given policy, returning its row identity.
async fn seed_key(
    database: &Database,
    key: &str,
    policy: &ApiKeyAccessControl,
) -> Result<i64, StorageError> {
    Ok(crate::storage::api_keys::insert_api_key(
        database,
        &ApiKeyFields {
            name: "fixture-key",
            key,
            is_enabled: true,
            access_control: policy,
        },
    )
    .await?
    .id)
}

/// The allow-all policy fixture.
fn allowing_policy() -> ApiKeyAccessControl {
    ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    }
}

/// The member fixture of `provider_id` at `priority`.
fn member(provider_id: i64, priority: u32) -> ProviderPoolMember {
    ProviderPoolMember {
        provider_id,
        priority,
    }
}

/// The row identities the shared fixture builds.
struct Inventory {
    /// The first pool member, storing `m` with known facts.
    first_provider: i64,

    /// The second pool member, storing `m` with conflicting known facts.
    second_provider: i64,

    /// The standalone provider, storing `d` under the `g.` prefix.
    third_provider: i64,

    /// The pool over the first two members.
    pool_id: i64,

    /// The pool's automatic public entry.
    pooled_route_id: i64,

    /// The standalone provider's automatic public entry.
    direct_route_id: i64,
}

/// Builds the shared fixture inventory: the first and second providers both store `m` and pool
/// under an empty prefix; the third provider stores `d` standalone with an input override.
async fn seed_inventory(database: &Database) -> Result<Inventory, Box<dyn StdError>> {
    let first_provider = seed_provider(database, "alpha", "a.").await?;

    let second_provider = seed_provider(database, "beta", "b.").await?;

    let third_provider = seed_provider(database, "gamma", "g.").await?;

    discover_one(
        database,
        first_provider,
        "m",
        Some(&[ReasoningLevel::Low, ReasoningLevel::High]),
        Some(100),
        Some(50),
        Some(
            CompactionSupportFacts::from_pairs([
                (
                    Mechanism::ResponsesStandaloneCompact,
                    MechanismSupport::Supported,
                ),
                (
                    Mechanism::MessagesOnDemandCompaction,
                    MechanismSupport::Supported,
                ),
            ])
            .expect("two known pairs build facts"),
        ),
    )
    .await?;

    discover_one(
        database,
        second_provider,
        "m",
        Some(&[ReasoningLevel::High, ReasoningLevel::Medium]),
        Some(200),
        Some(80),
        Some(
            CompactionSupportFacts::from_pairs([
                (
                    Mechanism::ResponsesStandaloneCompact,
                    MechanismSupport::Supported,
                ),
                (
                    Mechanism::MessagesOnDemandCompaction,
                    MechanismSupport::Unsupported,
                ),
            ])
            .expect("two known pairs build facts"),
        ),
    )
    .await?;

    let third_d = discover_one(database, third_provider, "d", None, Some(30), None, None).await?;

    let pool_id = crate::storage::routing::create_pool(
        database,
        "pool",
        "",
        true,
        &[member(first_provider, 0), member(second_provider, 1)],
    )
    .await?;

    // The third provider carries the override the direct entry must resolve.
    crate::storage::models::update_model_overrides_checked(
        database,
        ModelUpdate {
            model_id: third_d,
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: Some(300),
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    let routes = crate::storage::routing::list_routes(database).await?;

    let route_of = |public_id: &str| {
        routes
            .iter()
            .find(|route| route.is_automatic && route.public_id == public_id)
            .map(|route| route.id)
            .ok_or(StorageError::NotFound {
                message: format!("the automatic entry {public_id} exists"),
            })
    };

    Ok(Inventory {
        first_provider,
        second_provider,
        third_provider,
        pool_id,
        pooled_route_id: route_of("m")?,
        direct_route_id: route_of("g.d")?,
    })
}

/// The listed entry with `public_id`; an absent entry is the failure under test.
fn entry<'a>(
    listed: &'a [EnabledModel],
    public_id: &str,
) -> Result<&'a EnabledModel, StorageError> {
    listed
        .iter()
        .find(|entry| entry.public_id == public_id)
        .ok_or(StorageError::NotFound {
            message: format!("the listing carries {public_id}"),
        })
}

/// The public IDs of a listing in order.
fn public_ids(listed: &[EnabledModel]) -> Vec<&str> {
    listed
        .iter()
        .map(|entry| entry.public_id.as_str())
        .collect()
}

/// Structural availability gates the listing: disabled routes, pools, providers, and models and
/// vanished inventory contribute no entry, and the entries order by public ID.
#[tokio::test]
async fn structural_availability_gates_the_listing() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let inventory = seed_inventory(&database).await?;

    let key = seed_key(&database, "sk-allow", &allowing_policy()).await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert_eq!(
        public_ids(&listed),
        ["g.d", "m"],
        "the complete set orders by public ID with both entries"
    );

    assert_eq!(
        entry(&listed, "g.d")?.owner_name,
        "gamma",
        "a direct entry names its provider"
    );

    assert_eq!(
        entry(&listed, "m")?.owner_name,
        "pool",
        "a pooled entry names its pool"
    );

    // Disabling the pool drops its entry while the direct entry stays.
    crate::storage::routing::update_pool(
        &database,
        inventory.pool_id,
        "pool",
        "",
        false,
        &[
            member(inventory.first_provider, 0),
            member(inventory.second_provider, 1),
        ],
    )
    .await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert_eq!(
        public_ids(&listed),
        ["g.d"],
        "the disabled pool's entry is not listed"
    );

    // Re-enable the pool, then disable one member: the remaining candidate still lists.
    crate::storage::routing::update_pool(
        &database,
        inventory.pool_id,
        "pool",
        "",
        true,
        &[
            member(inventory.first_provider, 0),
            member(inventory.second_provider, 1),
        ],
    )
    .await?;

    crate::storage::providers::update_provider(
        &database,
        inventory.second_provider,
        ProviderUpdate {
            name: "beta",
            model_prefix: "",
            base_url: None,
            is_enabled: false,
            definition: None,
            credentials: None,
        },
    )
    .await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert_eq!(
        entry(&listed, "m")?.max_input_tokens,
        Some(100),
        "the disabled member is no structural candidate, so the remaining one's minimum stands"
    );

    // Vanished inventory is likewise no candidate: the first member's `m` vanishes, leaving the
    // disabled second member — no candidate, no entry.
    crate::storage::models::apply_discovered_models(
        &database,
        inventory.first_provider,
        &[DiscoveredModel {
            upstream_id: "elsewhere".to_string(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }],
    )
    .await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert!(
        !listed.iter().any(|entry| entry.public_id == "m"),
        "a pool without any structural candidate lists no entry"
    );

    Ok(())
}

/// The key policy resolves route override, then typed owner default, then key default: an
/// inherited deny is overridden by a route allow, and an allow default is overridden by an owner
/// deny.
#[tokio::test]
async fn key_policy_resolves_override_then_owner_then_default() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let inventory = seed_inventory(&database).await?;

    // Key default deny with a route allow on the pooled entry: the inherited deny is overridden.
    let route_allow = ApiKeyAccessControl {
        default_access: AccessDecision::Deny,
        owner_defaults: Vec::new(),
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: inventory.pooled_route_id,
            access: AccessDecision::Allow,
        }],
    };

    let key = seed_key(&database, "sk-route-allow", &route_allow).await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert_eq!(
        public_ids(&listed),
        ["m"],
        "the route allow overrides the inherited deny; the direct entry stays denied"
    );

    // Owner default deny over an allow key default hides the owner's entries.
    let owner_deny = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: vec![ApiKeyOwnerDefault::Provider {
            provider_id: inventory.third_provider,
            access: AccessDecision::Deny,
        }],
        route_overrides: Vec::new(),
    };

    let key = seed_key(&database, "sk-owner-deny", &owner_deny).await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert_eq!(
        public_ids(&listed),
        ["m"],
        "the provider deny hides the direct entry; the pooled entry inherits the allow default"
    );

    // A route deny over an allow-everything key hides exactly that route.
    let route_deny = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: inventory.direct_route_id,
            access: AccessDecision::Deny,
        }],
    };

    let key = seed_key(&database, "sk-route-deny", &route_deny).await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    assert_eq!(
        public_ids(&listed),
        ["m"],
        "the route deny hides exactly its route"
    );

    Ok(())
}

/// The pooled entry composes per-candidate facts conservatively: numerical limits use the
/// minimum, reasoning levels intersect, an unknown fact on any candidate stays unknown, and the
/// per-mechanism compaction support is supported only when every candidate supports it and
/// unsupported when any rejects it.
#[tokio::test]
async fn pooled_capabilities_compose_conservatively() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let _inventory = seed_inventory(&database).await?;

    let key = seed_key(&database, "sk-allow", &allowing_policy()).await?;

    let listed = crate::storage::models::list_enabled_models_with_owner(&database, key).await?;

    let pooled = entry(&listed, "m")?;

    assert_eq!(
        pooled.max_input_tokens,
        Some(100),
        "numerical limits use the minimum"
    );

    assert_eq!(
        pooled.max_output_tokens,
        Some(50),
        "numerical limits use the minimum"
    );

    assert_eq!(
        pooled.thinking_levels,
        Some(vec![ReasoningLevel::High]),
        "reasoning levels use the intersection"
    );

    let compaction = pooled
        .compaction
        .as_ref()
        .expect("the fixture carries known compaction facts");

    assert_eq!(
        compaction.get(Mechanism::ResponsesStandaloneCompact),
        Some(MechanismSupport::Supported),
        "supported on every candidate stays supported"
    );

    assert_eq!(
        compaction.get(Mechanism::MessagesOnDemandCompaction),
        Some(MechanismSupport::Unsupported),
        "unsupported on any candidate is unsupported"
    );

    assert_eq!(
        compaction.get(Mechanism::ResponsesExplicitTrigger),
        None,
        "a mechanism unknown on any candidate is omitted"
    );

    // The direct entry resolves the override over the discovered fact and omits the unknown.
    let direct = entry(&listed, "g.d")?;

    assert_eq!(
        direct.max_input_tokens,
        Some(300),
        "the override replaces the discovered fact"
    );

    assert_eq!(
        direct.thinking_levels, None,
        "an unknown level set stays unknown"
    );

    assert_eq!(
        direct.compaction, None,
        "no known mechanism renders no facts"
    );

    Ok(())
}
