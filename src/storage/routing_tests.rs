//! Transactional route and pool management behavior.

use super::*;
use crate::{
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::test_support::insert_test_provider,
    test_support::create_test_database,
};
use std::error::Error as StdError;

/// Discovery exposes account-local identities and preserves authored aliases across lifecycle
/// changes.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if inventory refresh changes route identity or fails to restore a vanished direct target.
#[tokio::test]
async fn physical_inventory_exposes_models_and_preserves_aliases() -> Result<(), Box<dyn StdError>>
{
    let (database, _guard) = create_test_database().await;
    let a = insert_test_provider(&database, "A").await?;
    let b = insert_test_provider(&database, "B").await?;
    let discovered = crate::provider::DiscoveredModel {
        upstream_id: "M".into(),
        thinking_levels: None,
        max_input_tokens: None,
        max_output_tokens: None,
        compaction: None,
    };
    let a_rows = crate::storage::models::apply_discovered_models(
        &database,
        a,
        std::slice::from_ref(&discovered),
    )
    .await?;
    let b_rows =
        crate::storage::models::apply_discovered_models(&database, b, &[discovered]).await?;
    #[expect(
        clippy::indexing_slicing,
        reason = "each refresh applied its single upstream model"
    )]
    let targets = [
        ModelRouteTarget::Direct {
            model_id: a_rows[0].model.id,
        },
        ModelRouteTarget::Direct {
            model_id: b_rows[0].model.id,
        },
    ];
    assert_eq!(
        list_routes(&database)
            .await?
            .iter()
            .filter(|route| route.is_automatic)
            .count(),
        2
    );
    let [a_target, b_target] = &targets;
    let direct_route = create_route(&database, "M", true, a_target).await?;
    assert!(create_route(&database, "M", true, b_target).await.is_err());
    create_route(&database, "other-M", true, b_target).await?;
    assert_eq!(
        crate::storage::models::list_model_entries(&database, None)
            .await?
            .len(),
        2
    );
    assert_eq!(list_routes(&database).await?.len(), 4);
    let pinned = pin_publication(&database).await?;
    let routes = list_routes(&database)
        .await?
        .into_iter()
        .filter(|route| !route.is_automatic)
        .collect::<Vec<_>>();
    crate::storage::models::apply_discovered_models(
        &database,
        b,
        &[
            crate::storage::models::discovered("M"),
            crate::storage::models::discovered("N"),
        ],
    )
    .await?;
    assert_eq!(
        crate::storage::models::list_model_entries(&database, None)
            .await?
            .len(),
        3
    );
    assert_eq!(
        list_routes(&database)
            .await?
            .into_iter()
            .filter(|route| !route.is_automatic)
            .collect::<Vec<_>>(),
        routes
    );
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.public_id.as_str())
            .collect::<Vec<_>>(),
        ["A-M", "B-M", "B-N", "M", "other-M"]
    );
    crate::storage::models::apply_discovered_models(&database, a, &[]).await?;
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.public_id.as_str())
            .collect::<Vec<_>>(),
        ["B-M", "B-N", "other-M"]
    );
    assert_eq!(
        list_routes(&database)
            .await?
            .into_iter()
            .filter(|route| !route.is_automatic)
            .collect::<Vec<_>>(),
        routes
    );
    crate::storage::models::apply_discovered_models(
        &database,
        a,
        &[crate::storage::models::discovered("M")],
    )
    .await?;
    assert_eq!(
        list_routes(&database)
            .await?
            .into_iter()
            .filter(|route| !route.is_automatic)
            .collect::<Vec<_>>(),
        routes
    );
    assert_eq!(
        crate::storage::api_keys::find_api_key_by_id(&database, pinned.id).await?,
        Some(pinned)
    );
    let restored = crate::gateway::resolve_routing(&database, "M").await?;
    assert_eq!(restored.model_route_id, direct_route);
    assert_eq!(
        ModelRouteTarget::Direct {
            model_id: restored.model.id
        },
        *a_target
    );
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.public_id.as_str())
            .collect::<Vec<_>>(),
        ["A-M", "B-M", "B-N", "M", "other-M"]
    );
    Ok(())
}

/// Listings publish explicit routes and aggregate only structural candidates, preserving unknown
/// capabilities and a known empty level intersection independently of temporary quota blocks.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if publication, lifecycle, or conservative capability composition diverges.
#[tokio::test]
async fn public_listings_expose_direct_models_and_combine_structural_candidates()
-> Result<(), Box<dyn StdError>> {
    use crate::compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    };
    let mechanism = Mechanism::ResponsesStandaloneCompact;
    let (database, _guard) = create_test_database().await;
    let a = insert_test_provider(&database, "A").await?;
    let b = insert_test_provider(&database, "B").await?;
    let discovered = crate::provider::DiscoveredModel {
        upstream_id: "M".into(),
        thinking_levels: Some(vec![crate::vocabulary::ReasoningLevel::Low]),
        max_input_tokens: Some(1000),
        max_output_tokens: Some(500),
        compaction: CompactionSupportFacts::from_pairs([(mechanism, MechanismSupport::Supported)]),
    };
    let a_models = crate::storage::models::apply_discovered_models(
        &database,
        a,
        std::slice::from_ref(&discovered),
    )
    .await?;
    crate::storage::models::apply_discovered_models(
        &database,
        b,
        &[crate::provider::DiscoveredModel {
            thinking_levels: Some(vec![crate::vocabulary::ReasoningLevel::High]),
            max_input_tokens: Some(800),
            max_output_tokens: None,
            compaction: None,
            ..discovered
        }],
    )
    .await?;
    assert_eq!(
        crate::storage::models::list_enabled_models_with_owner(
            &database,
            crate::test_support::structural_api_key(&database).await?
        )
        .await?
        .len(),
        2
    );
    let pool = create_pool(
        &database,
        "coding",
        "pool-coding-",
        true,
        &[
            ProviderPoolMember {
                provider_id: b,
                priority: 9,
            },
            ProviderPoolMember {
                provider_id: a,
                priority: 2,
            },
        ],
    )
    .await?;
    create_route(
        &database,
        "coding-M",
        true,
        &ModelRouteTarget::Pool {
            pool_id: pool,
            upstream_id: "M".into(),
        },
    )
    .await?;
    create_route(
        &database,
        "future-N",
        true,
        &ModelRouteTarget::Pool {
            pool_id: pool,
            upstream_id: "N".into(),
        },
    )
    .await?;
    #[expect(
        clippy::indexing_slicing,
        reason = "the discovery returned the single seeded model"
    )]
    let model_id = a_models[0].model.id;
    create_route(
        &database,
        "private-M",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(
        listed
            .iter()
            .map(|m| m.public_id.as_str())
            .collect::<Vec<_>>(),
        vec!["coding-M", "pool-coding-M", "private-M"]
    );
    #[expect(
        clippy::unwrap_used,
        reason = "coding-M is the first of the two verified routes"
    )]
    let pooled = listed
        .iter()
        .find(|route| route.public_id == "coding-M")
        .unwrap();
    assert_eq!(pooled.owner_name, "coding");
    assert_eq!(pooled.max_input_tokens, Some(800));
    assert_eq!(pooled.max_output_tokens, None);
    assert_eq!(pooled.thinking_levels, Some(vec![]));
    assert_eq!(pooled.compaction, None);
    database
        .execute(
            "UPDATE models SET compaction_support = ?1 WHERE provider_id = ?2",
            crate::database::params![
                serde_json::to_string(&CompactionSupportFacts::from_pairs([(
                    mechanism,
                    MechanismSupport::Unsupported
                )]))?,
                b
            ],
        )
        .await?;
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    let pooled_support = listed
        .iter()
        .find(|route| route.public_id == "coding-M")
        .and_then(|route| route.compaction.as_ref())
        .and_then(|facts| facts.get(mechanism));
    assert_eq!(pooled_support, Some(MechanismSupport::Unsupported));
    database
        .execute(
            "UPDATE models SET compaction_support_override = ?1 WHERE provider_id = ?2",
            crate::database::params![
                serde_json::to_string(&CompactionSupportFacts::from_pairs([(
                    mechanism,
                    MechanismSupport::Unknown
                )]))?,
                b
            ],
        )
        .await?;
    database
        .execute(
            r#"INSERT INTO provider_quota_blocks
            VALUES (?1, '["weekly"]', '{"type":"all_models"}', 100, NULL)"#,
            crate::database::params![a],
        )
        .await?;
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(listed.len(), 3);
    let pooled_support = listed
        .iter()
        .find(|route| route.public_id == "coding-M")
        .and_then(|route| route.compaction.as_ref())
        .and_then(|facts| facts.get(mechanism));
    assert_eq!(pooled_support, None);
    database
        .execute(
            "UPDATE models SET discovery_state = 'vanished' WHERE provider_id = ?1",
            crate::database::params![a],
        )
        .await?;
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(listed.len(), 2);
    #[expect(clippy::unwrap_used, reason = "B remains a structural candidate")]
    let remaining_levels = listed
        .iter()
        .find(|route| route.public_id == "coding-M")
        .unwrap()
        .thinking_levels
        .clone();
    assert_eq!(
        remaining_levels,
        Some(vec![crate::vocabulary::ReasoningLevel::High])
    );
    database
        .execute(
            "UPDATE providers SET is_enabled = 0 WHERE id = ?1",
            crate::database::params![b],
        )
        .await?;
    assert!(
        crate::storage::models::list_enabled_models_with_owner(
            &database,
            crate::test_support::structural_api_key(&database).await?
        )
        .await?
        .is_empty()
    );
    assert_eq!(list_routes(&database).await?.len(), 4);
    // Distinct exact IDs sharing one pool must never borrow another model's capabilities.
    database
        .execute(
            "UPDATE providers SET is_enabled = 1 WHERE id = ?1",
            crate::database::params![b],
        )
        .await?;
    crate::storage::models::apply_discovered_models(
        &database,
        b,
        &[crate::provider::DiscoveredModel {
            upstream_id: "N".into(),
            thinking_levels: Some(vec![crate::vocabulary::ReasoningLevel::Low]),
            max_input_tokens: Some(2400),
            max_output_tokens: Some(600),
            compaction: None,
        }],
    )
    .await?;
    create_route(
        &database,
        "alias-N",
        true,
        &ModelRouteTarget::Pool {
            pool_id: pool,
            upstream_id: "N".into(),
        },
    )
    .await?;
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.public_id.as_str())
            .collect::<Vec<_>>(),
        vec!["alias-N", "future-N", "pool-coding-N"]
    );
    assert!(listed.iter().all(|entry| entry.owner_name == "coding"
        && entry.max_input_tokens == Some(2400)
        && entry.max_output_tokens == Some(600)
        && entry.thinking_levels == Some(vec![crate::vocabulary::ReasoningLevel::Low])));
    Ok(())
}

/// Priority replacement is atomic, including a swap, and a missing desired account preserves
/// the pool's prior name, switch, and entire ordered membership.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if atomicity, ordering, or deletion behavior diverges.
#[tokio::test]
async fn membership_replacement_is_atomic() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let a = insert_test_provider(&database, "A").await?;
    let b = insert_test_provider(&database, "B").await?;
    let first = vec![
        ProviderPoolMember {
            provider_id: a,
            priority: 2,
        },
        ProviderPoolMember {
            provider_id: b,
            priority: 9,
        },
    ];
    let id = create_pool(&database, "coding", "pool-coding-", true, &first).await?;
    let swapped = vec![
        ProviderPoolMember {
            provider_id: a,
            priority: 9,
        },
        ProviderPoolMember {
            provider_id: b,
            priority: 2,
        },
    ];
    update_pool(&database, id, "renamed", "pool-renamed-", false, &swapped).await?;
    let pools = list_pools(&database).await?;
    #[expect(clippy::unwrap_used, reason = "the seeded pool is present")]
    let pool = pools.first().unwrap();
    assert_eq!(pool.name, "renamed");
    assert!(!pool.is_enabled);
    assert_eq!(
        pool.members
            .iter()
            .map(|m| m.provider_id)
            .collect::<Vec<_>>(),
        vec![b, a]
    );
    assert!(
        update_pool(
            &database,
            id,
            "lost",
            "pool-lost-",
            true,
            &[ProviderPoolMember {
                provider_id: 9999,
                priority: 0
            }]
        )
        .await
        .is_err()
    );
    assert_eq!(list_pools(&database).await?, pools);
    assert!(
        create_pool(&database, "renamed", "pool-renamed-", true, &[])
            .await
            .is_err()
    );
    delete_pool(&database, id).await?;
    assert!(list_pools(&database).await?.is_empty());
    assert!(
        update_pool(&database, id, "deleted", "pool-deleted-", true, &[])
            .await
            .is_err()
    );
    Ok(())
}

/// Public names are unique, target aliases are allowed, missing pool inventory is allowed,
/// and changing targets preserves the route identity while guarding referenced pool deletion.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if the checked target and uniqueness contracts diverge.
#[tokio::test]
async fn route_updates_preserve_identity_and_reference_rules() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let provider = insert_test_provider(&database, "A").await?;
    let rows = database
        .query(
            "INSERT INTO models (provider_id, upstream_id, \
                 discovery_state) VALUES (?1, 'M', 'manual') RETURNING id",
            crate::database::params![provider],
        )
        .await?;
    #[expect(
        clippy::indexing_slicing,
        reason = "INSERT RETURNING yields the inserted model"
    )]
    let direct = ModelRouteTarget::Direct {
        model_id: rows[0].get(0)?,
    };
    let id = create_route(&database, "coding-M", true, &direct).await?;
    create_route(&database, "alias-M", true, &direct).await?;
    assert!(
        create_route(&database, "coding-M", false, &direct)
            .await
            .is_err()
    );
    let pool_id = create_pool(&database, "coding", "pool-coding-", true, &[]).await?;
    let target = ModelRouteTarget::Pool {
        pool_id,
        upstream_id: "N".into(),
    };
    update_route(&database, id, "coding-M", true, &target).await?;
    let routes = list_routes(&database)
        .await?
        .into_iter()
        .filter(|route| !route.is_automatic)
        .collect::<Vec<_>>();
    #[expect(clippy::unwrap_used, reason = "the created route exists")]
    let route = routes.iter().find(|r| r.id == id).unwrap();
    assert_eq!(route.public_id, "coding-M");
    assert_eq!(route.target, target);
    assert!(delete_pool(&database, pool_id).await.is_err());
    assert!(
        update_route(&database, id, "alias-M", false, &direct)
            .await
            .is_err()
    );
    assert!(
        update_route(
            &database,
            id,
            "lost",
            false,
            &ModelRouteTarget::Direct { model_id: 9999 }
        )
        .await
        .is_err()
    );
    assert_eq!(
        list_routes(&database)
            .await?
            .into_iter()
            .filter(|route| !route.is_automatic)
            .collect::<Vec<_>>(),
        routes
    );
    delete_route(&database, id).await?;
    delete_pool(&database, pool_id).await?;
    Ok(())
}

/// Dispatch selects real accounts in authored priority order and keeps route identity separate.
/// Unavailable members remain visible in the snapshot; an enabled empty pool is a 503, whereas
/// disabling the same pool hides its routes with 404.
///
/// # Errors
///
/// Returns fixture or database errors.
///
/// # Panics
///
/// Panics if ordered selection, snapshot retention, or outward error contracts diverge.
#[tokio::test]
async fn dispatch_resolves_explicit_routes_with_ordered_physical_targets()
-> Result<(), Box<dyn StdError>> {
    use crate::storage::{
        models,
        route_snapshot::{TargetSnapshot, find_route},
    };
    let (database, _guard) = create_test_database().await;
    let a = insert_test_provider(&database, "A").await?;
    let b = insert_test_provider(&database, "B").await?;
    let missing = insert_test_provider(&database, "missing").await?;
    models::apply_discovered_models(&database, a, &[models::discovered("M")]).await?;
    models::apply_discovered_models(&database, b, &[models::discovered("M")]).await?;
    database
        .execute(
            "UPDATE providers SET is_enabled = 0 WHERE id = ?1",
            crate::database::params![a],
        )
        .await?;
    let pool = create_pool(
        &database,
        "coding",
        "pool-coding-",
        true,
        &[
            ProviderPoolMember {
                provider_id: b,
                priority: 9,
            },
            ProviderPoolMember {
                provider_id: missing,
                priority: 2,
            },
            ProviderPoolMember {
                provider_id: a,
                priority: 0,
            },
        ],
    )
    .await?;
    let route = create_route(
        &database,
        "coding-M",
        true,
        &ModelRouteTarget::Pool {
            pool_id: pool,
            upstream_id: "M".into(),
        },
    )
    .await?;
    let resolved = crate::gateway::resolve_routing(&database, "coding-M").await?;
    assert_eq!(resolved.model_route_id, route);
    assert_eq!(resolved.public_id, "coding-M");
    assert_eq!(resolved.model.upstream_id, "M");
    assert_eq!(resolved.provider.id, b);
    #[expect(clippy::expect_used, reason = "the seeded route exists")]
    let snapshot = find_route(&database, "coding-M")
        .await?
        .expect("route exists");
    if let TargetSnapshot::Pool { id, members, .. } = snapshot.target {
        assert_eq!(id, pool);
        assert_eq!(
            members
                .iter()
                .map(|member| member.provider.id)
                .collect::<Vec<_>>(),
            vec![a, missing, b]
        );
        assert_eq!(
            members
                .iter()
                .filter(|member| member.model.is_none())
                .count(),
            1
        );
    } else {
        return Err("expected pool snapshot".into());
    }
    create_route(
        &database,
        "future-N",
        true,
        &ModelRouteTarget::Pool {
            pool_id: pool,
            upstream_id: "N".into(),
        },
    )
    .await?;
    #[expect(clippy::expect_used, reason = "no pool member has the exact N model")]
    let unavailable = crate::gateway::resolve_routing(&database, "future-N")
        .await
        .expect_err("pool has no structural candidate");
    let outward = crate::api::error::Error::from(unavailable);
    assert_eq!(
        outward.http_status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        crate::api::error::openai::render_body(&outward)
            .code
            .as_deref(),
        Some("pool_unavailable")
    );
    assert_eq!(
        crate::api::error::gemini::render_body(&outward).status,
        "UNAVAILABLE"
    );
    assert_eq!(
        crate::api::error::anthropic::render_body(&outward).r#type,
        "api_error"
    );
    database
        .execute(
            "UPDATE provider_pools SET is_enabled = 0 WHERE id = ?1",
            crate::database::params![pool],
        )
        .await?;
    #[expect(clippy::expect_used, reason = "a disabled pool is not addressable")]
    let disabled = crate::gateway::resolve_routing(&database, "future-N")
        .await
        .expect_err("disabled pool");
    assert_eq!(
        crate::api::error::Error::from(disabled).http_status(),
        axum::http::StatusCode::NOT_FOUND
    );
    Ok(())
}

/// Discovery exposes standalone models directly and pooled models through their pool prefix.
///
/// # Errors
///
/// Returns fixture and storage errors.
///
/// # Panics
///
/// Panics if membership fails to transfer exposure or rediscovery replaces an unchanged route
/// identity.
#[tokio::test]
async fn discovery_automatically_exposes_prefixed_direct_models() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let a = insert_test_provider(&database, "A").await?;
    let b = insert_test_provider(&database, "B").await?;
    database
        .execute(
            "UPDATE providers SET model_prefix = 'a-' WHERE id = ?1",
            params![a],
        )
        .await?;
    database
        .execute(
            "UPDATE providers SET model_prefix = 'b-' WHERE id = ?1",
            params![b],
        )
        .await?;
    let pool = create_pool(
        &database,
        "optional",
        "pool-optional-",
        true,
        &[ProviderPoolMember {
            provider_id: b,
            priority: 0,
        }],
    )
    .await?;
    for provider_id in [a, b] {
        crate::storage::models::apply_discovered_models(
            &database,
            provider_id,
            &[crate::storage::models::discovered("M")],
        )
        .await?;
    }
    let a_route = crate::storage::route_snapshot::find_route(&database, "a-M").await?;
    assert!(a_route.is_some());
    assert!(
        crate::storage::route_snapshot::find_route(&database, "pool-optional-M")
            .await?
            .is_some()
    );
    let listed = crate::storage::models::list_enabled_models_with_owner(
        &database,
        crate::test_support::structural_api_key(&database).await?,
    )
    .await?;
    assert_eq!(
        listed
            .iter()
            .map(|model| model.public_id.as_str())
            .collect::<Vec<_>>(),
        ["a-M", "pool-optional-M"]
    );
    update_pool(&database, pool, "optional", "pool-optional-", true, &[]).await?;
    crate::storage::models::apply_discovered_models(
        &database,
        b,
        &[
            crate::storage::models::discovered("M"),
            crate::storage::models::discovered("N"),
        ],
    )
    .await?;
    assert!(
        crate::storage::route_snapshot::find_route(&database, "b-N")
            .await?
            .is_some()
    );
    assert_eq!(
        crate::storage::route_snapshot::find_route(&database, "a-M")
            .await?
            .map(|route| route.route.id),
        a_route.map(|route| route.route.id)
    );
    Ok(())
}

/// Prefix rewrites preserve automatic identities through overlapping names and roll back conflicts.
///
/// # Errors
///
/// Returns fixture and storage errors.
///
/// # Panics
///
/// Panics if automatic ownership, stable identity, or transaction rollback diverges.
#[tokio::test]
async fn prefix_changes_preserve_identity_and_roll_back_conflicts() -> Result<(), Box<dyn StdError>>
{
    use crate::storage::providers::{ProviderUpdate, update_provider};
    let (database, _guard) = create_test_database().await;
    let provider_id = insert_test_provider(&database, "A").await?;
    database
        .execute(
            "UPDATE providers SET model_prefix = '' WHERE id = ?1",
            params![provider_id],
        )
        .await?;
    crate::storage::models::apply_discovered_models(
        &database,
        provider_id,
        &[
            crate::storage::models::discovered("M"),
            crate::storage::models::discovered("xM"),
        ],
    )
    .await?;
    let pinned = pin_publication(&database).await?;
    let before = list_routes(&database).await?;
    for route in &before {
        assert!(
            update_route(&database, route.id, "forbidden", true, &route.target)
                .await
                .is_err()
        );
        assert!(delete_route(&database, route.id).await.is_err());
    }
    update_provider(
        &database,
        provider_id,
        ProviderUpdate {
            model_prefix: "x",
            name: "A",
            base_url: Some("http://localhost:8080"),
            is_enabled: true,
            definition: None,
            credentials: None,
        },
    )
    .await?;
    assert_eq!(
        crate::storage::api_keys::find_api_key_by_id(&database, pinned.id).await?,
        Some(pinned)
    );
    let after = list_routes(&database).await?;
    for original in &before {
        let renamed = after.iter().find(|route| route.id == original.id);
        assert_eq!(
            renamed.map(|route| route.public_id.as_str()),
            Some(format!("x{}", original.public_id).as_str())
        );
        assert_eq!(renamed.map(|route| &route.target), Some(&original.target));
    }
    #[expect(clippy::expect_used, reason = "discovery exposed two models")]
    let target = &after.first().expect("automatic entry").target;
    create_route(&database, "blocked-M", true, target).await?;
    let snapshot = list_routes(&database).await?;
    assert!(
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                model_prefix: "blocked-",
                name: "renamed",
                base_url: Some("http://localhost:8080"),
                is_enabled: false,
                definition: None,
                credentials: None,
            }
        )
        .await
        .is_err()
    );
    assert_eq!(list_routes(&database).await?, snapshot);
    #[expect(
        clippy::expect_used,
        reason = "the provider remains after a rolled-back update"
    )]
    let provider = crate::storage::providers::find_provider_by_id(&database, provider_id)
        .await?
        .expect("provider");
    assert_eq!(provider.model_prefix, "x");
    assert_eq!(provider.name, "A");
    assert!(provider.is_enabled);
    Ok(())
}

/// Pool exposure replaces member prefixes and follows the union of exact physical model IDs.
///
/// # Errors
///
/// Returns fixture, schema, and storage errors.
///
/// # Panics
///
/// Panics if pool discovery exposes a missing member or leaves a member prefix accessible.
#[tokio::test]
async fn pool_exposure_tracks_member_inventory_and_suppresses_member_prefixes()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let a = insert_test_provider(&database, "A").await?;
    let b = insert_test_provider(&database, "B").await?;
    for provider_id in [a, b] {
        crate::storage::models::apply_discovered_models(
            &database,
            provider_id,
            &[crate::storage::models::discovered("M")],
        )
        .await?;
    }
    let pinned = pin_publication(&database).await?;
    let pool_id = create_pool(
        &database,
        "coding",
        "coding-",
        true,
        &[
            ProviderPoolMember {
                provider_id: a,
                priority: 0,
            },
            ProviderPoolMember {
                provider_id: b,
                priority: 1,
            },
        ],
    )
    .await?;
    let surviving = crate::storage::api_keys::find_api_key_by_id(&database, pinned.id)
        .await?
        .ok_or("publication key")?;
    assert!(surviving.access_control.route_overrides.is_empty());
    crate::storage::models::apply_discovered_models(
        &database,
        b,
        &[
            crate::storage::models::discovered("M"),
            crate::storage::models::discovered("N"),
        ],
    )
    .await?;
    assert!(
        crate::storage::route_snapshot::find_route(&database, "A-M")
            .await?
            .is_none()
    );
    assert!(
        crate::storage::route_snapshot::find_route(&database, "B-M")
            .await?
            .is_none()
    );
    #[expect(
        clippy::expect_used,
        reason = "pool discovery exposes the new exact model"
    )]
    let route = crate::storage::route_snapshot::find_route(&database, "coding-N")
        .await?
        .expect("pooled model");
    let route_id = route.route.id;
    let pool_policy = crate::access_control::ApiKeyAccessControl {
        default_access: crate::access_control::AccessDecision::Allow,
        owner_defaults: vec![crate::access_control::ApiKeyOwnerDefault::Pool {
            pool_id,
            access: crate::access_control::AccessDecision::Allow,
        }],
        route_overrides: vec![crate::access_control::ApiKeyRouteOverride {
            model_route_id: route_id,
            access: crate::access_control::AccessDecision::Deny,
        }],
    };
    crate::storage::api_keys::update_api_key(
        &database,
        pinned.id,
        crate::storage::api_keys::ApiKeyFields {
            name: &pinned.name,
            key: &pinned.key,
            is_enabled: true,
            access_control: &pool_policy,
        },
    )
    .await?;

    #[expect(
        clippy::panic,
        reason = "the automatic pool entry must target its pool"
    )]
    let crate::storage::route_snapshot::TargetSnapshot::Pool { members, .. } = route.target else {
        panic!("pool target")
    };
    assert_eq!(
        members
            .iter()
            .filter(|member| member
                .model
                .as_ref()
                .is_some_and(|model| model.is_effectively_enabled(member.provider.is_enabled)))
            .map(|member| member.provider.id)
            .collect::<Vec<_>>(),
        [b]
    );
    update_pool(
        &database,
        pool_id,
        "coding",
        "renamed-",
        true,
        &[
            ProviderPoolMember {
                provider_id: a,
                priority: 0,
            },
            ProviderPoolMember {
                provider_id: b,
                priority: 1,
            },
        ],
    )
    .await?;
    assert_eq!(
        crate::storage::route_snapshot::find_route(&database, "renamed-N")
            .await?
            .map(|route| route.route.id),
        Some(route_id)
    );
    assert_eq!(
        crate::storage::api_keys::find_api_key_by_id(&database, pinned.id)
            .await?
            .map(|key| key.access_control),
        Some(pool_policy)
    );
    let alias = create_route(
        &database,
        "blocked-M",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "M".into(),
        },
    )
    .await?;
    let before_pools = list_pools(&database).await?;
    let before_routes = list_routes(&database).await?;
    assert!(
        update_pool(
            &database,
            pool_id,
            "changed",
            "blocked-",
            false,
            &[ProviderPoolMember {
                provider_id: a,
                priority: 0
            }]
        )
        .await
        .is_err()
    );
    assert_eq!(list_pools(&database).await?, before_pools);
    assert_eq!(list_routes(&database).await?, before_routes);
    update_pool(
        &database,
        pool_id,
        "coding",
        "coding-",
        true,
        &[ProviderPoolMember {
            provider_id: a,
            priority: 0,
        }],
    )
    .await?;
    assert!(
        crate::storage::route_snapshot::find_route(&database, "coding-N")
            .await?
            .is_none()
    );
    assert!(
        crate::storage::route_snapshot::find_route(&database, "B-N")
            .await?
            .is_some()
    );
    assert!(
        crate::storage::route_snapshot::find_route(&database, "A-M")
            .await?
            .is_none()
    );
    assert!(delete_pool(&database, pool_id).await.is_err());
    delete_route(&database, alias).await?;
    delete_pool(&database, pool_id).await?;
    assert!(
        crate::storage::route_snapshot::find_route(&database, "A-M")
            .await?
            .is_some()
    );
    Ok(())
}

/// Pins the current publication with explicit denial for lifecycle preservation assertions.
async fn pin_publication(
    database: &crate::database::Database,
) -> Result<crate::storage::api_keys::ApiKey, Box<dyn StdError>> {
    let policy = crate::access_control::ApiKeyAccessControl {
        default_access: crate::access_control::AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: list_routes(database)
            .await?
            .into_iter()
            .map(|route| crate::access_control::ApiKeyRouteOverride {
                model_route_id: route.id,
                access: crate::access_control::AccessDecision::Deny,
            })
            .collect(),
    };
    Ok(crate::storage::api_keys::insert_api_key(
        database,
        &crate::storage::api_keys::ApiKeyFields {
            name: "Pinned publication",
            key: "publication-secret",
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?)
}
