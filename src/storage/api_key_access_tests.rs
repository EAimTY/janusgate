//! Real-storage permission replacement, identity lifecycle, rollback, and cascade contracts.

use super::{api_keys::*, *};
use crate::{
    access_control::{
        AccessDecision::{Allow, Deny},
        ApiKeyAccessControl, ApiKeyOwnerDefault, ApiKeyRouteOverride, RouteOwner,
    },
    database::params,
    routing::ModelRouteTarget,
    storage::{models::ModelCreate, test_support::insert_test_provider},
    test_support::create_test_database,
};
use std::error::Error as StdError;

/// Complete replacement fields for one test client.
fn fields(policy: &ApiKeyAccessControl) -> ApiKeyFields<'_> {
    ApiKeyFields {
        name: "Client",
        key: "client-secret",
        is_enabled: true,
        access_control: policy,
    }
}

/// Replacement preserves explicit intent, canonicalizes without deduplication, and rolls back
/// every ordinary field and rule on failures. Typed IDs and deletion follow actual identities.
///
/// # Panics
/// Panics if authored policy, transaction atomicity, or foreign-key cascades diverge.
#[tokio::test]
async fn replacement_rollback_identity_and_cascades() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let provider_id = insert_test_provider(&database, "Provider").await?;
    let pool_id = routing::create_pool(&database, "Pool", "pool-", true, &[]).await?;
    assert_eq!(provider_id, pool_id);
    let model = models::insert_model_checked(
        &database,
        ModelCreate {
            provider_id,
            upstream_id: "M",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;
    let route_id = routing::create_route(
        &database,
        "Alias",
        true,
        &ModelRouteTarget::Direct {
            model_id: model.model.id,
        },
    )
    .await?;
    let mut policy = ApiKeyAccessControl {
        default_access: Allow,
        owner_defaults: vec![
            ApiKeyOwnerDefault::Pool {
                pool_id,
                access: Deny,
            },
            ApiKeyOwnerDefault::Provider {
                provider_id,
                access: Allow,
            },
        ],
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: route_id,
            access: Allow,
        }],
    };
    let key = insert_api_key(&database, &fields(&policy)).await?;
    policy.canonicalize();
    assert_eq!(key.access_control, policy);
    assert_eq!(list_api_keys(&database).await?, vec![key.clone()]);
    for access in [Allow, Deny] {
        let mut duplicate = policy.clone();
        duplicate.owner_defaults.push(ApiKeyOwnerDefault::Provider {
            provider_id,
            access,
        });
        assert!(
            matches!(insert_api_key(&database, &ApiKeyFields { key: "rejected", ..fields(&duplicate) }).await,
            Err(Error::DuplicateAccessReference { reference: AccessReference::Provider(id) }) if id == provider_id)
        );
        let mut duplicate = policy.clone();
        duplicate.route_overrides.push(ApiKeyRouteOverride {
            model_route_id: route_id,
            access,
        });
        assert!(
            matches!(update_api_key(&database, key.id, ApiKeyFields { name: "Rejected", key: "rotated", is_enabled: false, access_control: &duplicate }).await,
            Err(Error::DuplicateAccessReference { reference: AccessReference::Route(id) }) if id == route_id)
        );
        assert_eq!(
            find_api_key_by_id(&database, key.id).await?,
            Some(key.clone())
        );
    }
    for reference in [
        AccessReference::Provider(999),
        AccessReference::Pool(999),
        AccessReference::Route(999),
    ] {
        let mut missing = policy.clone();
        match reference {
            AccessReference::Provider(provider_id) => {
                missing.owner_defaults.push(ApiKeyOwnerDefault::Provider {
                    provider_id,
                    access: Allow,
                })
            }
            AccessReference::Pool(pool_id) => {
                missing.owner_defaults.push(ApiKeyOwnerDefault::Pool {
                    pool_id,
                    access: Allow,
                })
            }
            AccessReference::Route(model_route_id) => {
                missing.route_overrides.push(ApiKeyRouteOverride {
                    model_route_id,
                    access: Allow,
                })
            }
        }
        assert!(
            matches!(insert_api_key(&database, &ApiKeyFields { key: "rejected", ..fields(&missing) }).await,
            Err(Error::MissingAccessReference { reference: observed }) if observed == reference)
        );
        assert!(matches!(
            update_api_key(
                &database,
                key.id,
                ApiKeyFields {
                    name: "Rejected",
                    key: "rotated",
                    is_enabled: false,
                    access_control: &missing
                }
            )
            .await,
            Err(Error::MissingAccessReference { .. })
        ));
        assert_eq!(
            find_api_key_by_id(&database, key.id).await?,
            Some(key.clone())
        );
        assert_eq!(find_api_key_by_key(&database, "rejected").await?, None);
    }
    let taken = insert_api_key(
        &database,
        &ApiKeyFields {
            key: "taken-secret",
            ..fields(&policy)
        },
    )
    .await?;
    assert!(matches!(
        update_api_key(
            &database,
            key.id,
            ApiKeyFields {
                key: &taken.key,
                ..fields(&policy)
            }
        )
        .await,
        Err(Error::Conflict { .. })
    ));
    assert_eq!(
        find_api_key_by_id(&database, key.id).await?,
        Some(key.clone())
    );

    // Changing a lower default cannot erase an explicit equal-default pin.
    policy.default_access = Deny;
    update_api_key(&database, key.id, fields(&policy)).await?;
    assert_eq!(
        policy.resolve(route_id, RouteOwner::Provider(provider_id)),
        Allow
    );
    routing::update_route(
        &database,
        route_id,
        "Renamed",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "M".into(),
        },
    )
    .await?;
    assert_eq!(
        find_api_key_by_id(&database, key.id)
            .await?
            .map(|key| key.access_control),
        Some(policy.clone())
    );
    let snapshot = route_snapshot::find_admitted_route(&database, "Renamed", key.id).await?;
    assert!(snapshot.is_some());
    policy.route_overrides.clear();
    update_api_key(&database, key.id, fields(&policy)).await?;
    assert!(
        route_snapshot::find_admitted_route(&database, "Renamed", key.id)
            .await?
            .is_none()
    );
    policy.route_overrides.push(ApiKeyRouteOverride {
        model_route_id: route_id,
        access: Allow,
    });
    update_api_key(&database, key.id, fields(&policy)).await?;
    routing::delete_route(&database, route_id).await?;
    let recreated = routing::create_route(
        &database,
        "Renamed",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "M".into(),
        },
    )
    .await?;
    assert_ne!(route_id, recreated);
    #[expect(clippy::expect_used, reason = "the key survives route deletion")]
    let survived = find_api_key_by_id(&database, key.id)
        .await?
        .expect("surviving key");
    assert!(survived.access_control.route_overrides.is_empty());
    assert_eq!(
        survived
            .access_control
            .resolve(recreated, RouteOwner::Pool(pool_id)),
        Deny
    );
    // Clear only owner defaults: absent collections are full replacements, not patches.
    policy.owner_defaults.clear();
    policy.route_overrides.clear();
    update_api_key(&database, key.id, fields(&policy)).await?;
    assert_eq!(
        find_api_key_by_id(&database, key.id)
            .await?
            .map(|key| key.access_control),
        Some(policy.clone())
    );
    database
        .execute("DELETE FROM api_keys WHERE id = ?1", params![taken.id])
        .await?;
    for table in [
        "api_key_provider_defaults",
        "api_key_pool_defaults",
        "api_key_model_route_overrides",
    ] {
        assert!(
            database
                .query(format!("SELECT api_key_id FROM {table}"), ())
                .await?
                .is_empty()
        );
    }
    // Referenced owner rows cascade independently of same-number identities of the other kind.
    policy.owner_defaults = vec![
        ApiKeyOwnerDefault::Provider {
            provider_id,
            access: Allow,
        },
        ApiKeyOwnerDefault::Pool {
            pool_id,
            access: Allow,
        },
    ];
    update_api_key(&database, key.id, fields(&policy)).await?;
    database
        .execute("DELETE FROM providers WHERE id = ?1", params![provider_id])
        .await?;
    #[expect(clippy::expect_used, reason = "the key survives provider deletion")]
    let survived = find_api_key_by_id(&database, key.id)
        .await?
        .expect("surviving key");
    assert_eq!(
        survived.access_control.owner_defaults,
        vec![ApiKeyOwnerDefault::Pool {
            pool_id,
            access: Allow
        }]
    );
    routing::delete_route(&database, recreated).await?;
    routing::delete_pool(&database, pool_id).await?;
    assert!(
        find_api_key_by_id(&database, key.id)
            .await?
            .is_some_and(|key| key.access_control.owner_defaults.is_empty())
    );
    Ok(())
}

/// Admission and listing retain key state in their snapshot instead of treating missing rules or
/// missing keys as unrestricted; already released snapshots keep their previous authored decision.
///
/// # Panics
/// Panics if revocation, key disablement, or deletion becomes an implicit grant.
#[tokio::test]
async fn enabled_key_snapshots_are_authoritative() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let provider_id = insert_test_provider(&database, "Provider").await?;
    models::insert_model_checked(
        &database,
        ModelCreate {
            provider_id,
            upstream_id: "M",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;
    let mut policy = crate::test_support::allow_all_access();
    let key = insert_api_key(&database, &fields(&policy)).await?;
    let public_id = "Provider-M";
    let admitted = route_snapshot::find_admitted_route(&database, public_id, key.id).await?;
    assert!(admitted.is_some());
    policy.default_access = Deny;
    update_api_key(&database, key.id, fields(&policy)).await?;
    assert!(
        route_snapshot::find_admitted_route(&database, public_id, key.id)
            .await?
            .is_none()
    );
    assert!(
        models::list_enabled_models_with_owner(&database, key.id)
            .await?
            .is_empty()
    );
    assert!(admitted.is_some());
    update_api_key(
        &database,
        key.id,
        ApiKeyFields {
            is_enabled: false,
            ..fields(&policy)
        },
    )
    .await?;
    assert!(matches!(
        route_snapshot::find_admitted_route(&database, public_id, key.id).await,
        Err(Error::InactiveApiKey { .. })
    ));
    assert!(matches!(
        models::list_enabled_models_with_owner(&database, key.id).await,
        Err(Error::InactiveApiKey { .. })
    ));
    delete_api_key(&database, key.id).await?;
    assert!(matches!(
        route_snapshot::find_admitted_route(&database, "absent", key.id).await,
        Err(Error::InactiveApiKey { .. })
    ));
    Ok(())
}

/// Committed policy replacement, retargeting, and deletion cannot mix new ownership with old
/// permissions. A snapshot started before commit can finish; a later snapshot sees revocation.
///
/// # Panics
/// Panics if concurrent changes produce mixed-owner permission or a missing-key implicit grant.
#[tokio::test]
async fn concurrent_route_and_key_changes_remain_coherent() -> Result<(), Box<dyn StdError>> {
    for change in 0..3 {
        let (database, _guard) = create_test_database().await;
        let provider_id = insert_test_provider(&database, "Provider").await?;
        let model = models::insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "M",
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support_override: None,
            },
        )
        .await?;
        let pool_id = routing::create_pool(&database, "Pool", "pool-", true, &[]).await?;
        let route_id = routing::create_route(
            &database,
            "entry",
            true,
            &ModelRouteTarget::Direct {
                model_id: model.model.id,
            },
        )
        .await?;
        let policy = ApiKeyAccessControl {
            default_access: Deny,
            owner_defaults: vec![ApiKeyOwnerDefault::Provider {
                provider_id,
                access: Allow,
            }],
            route_overrides: Vec::new(),
        };
        let key = insert_api_key(&database, &fields(&policy)).await?;
        let mut transaction = database.transaction().await?;
        // Establish the production reader's database snapshot before permitting a commit.
        let _ = api_keys::read_enabled_policy(&mut transaction, key.id).await?;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let writer_barrier = barrier.clone();
        let writer_database = database.clone();
        let writer = tokio::spawn(async move {
            writer_barrier.wait().await;
            match change {
                0 => {
                    update_api_key(
                        &writer_database,
                        key.id,
                        fields(&ApiKeyAccessControl {
                            default_access: Deny,
                            owner_defaults: Vec::new(),
                            route_overrides: Vec::new(),
                        }),
                    )
                    .await?
                }
                1 => {
                    routing::update_route(
                        &writer_database,
                        route_id,
                        "entry",
                        true,
                        &ModelRouteTarget::Pool {
                            pool_id,
                            upstream_id: "M".into(),
                        },
                    )
                    .await?
                }
                _ => {
                    delete_api_key(&writer_database, key.id).await?;
                }
            }
            Ok::<_, Error>(())
        });
        barrier.wait().await;
        writer.await??;
        let old = route_snapshot::read_admitted_route(&mut transaction, "entry", key.id).await?;
        assert!(old.is_some_and(|snapshot| snapshot.owner() == RouteOwner::Provider(provider_id)));
        transaction.commit().await?;
        let current = route_snapshot::find_admitted_route(&database, "entry", key.id).await;
        if change == 2 {
            assert!(matches!(current, Err(Error::InactiveApiKey { .. })));
        } else {
            assert!(current?.is_none());
        }
    }
    Ok(())
}
