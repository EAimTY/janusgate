//! Serving-key policy persistence tests: the complete-policy round-trip with canonical sparse
//! ordering, the enabled-secret listing, the duplicate and missing-reference validation, the
//! atomic whole-state replacement on update, the cascade of the sparse rules on key deletion, and
//! the enabled-key gate behind the admission policy read.

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

use super::ApiKeyFields;
use crate::{
    access_control::{
        AccessDecision, ApiKeyAccessControl, ApiKeyOwnerDefault, ApiKeyRouteOverride,
    },
    database::Database,
    provider::StoredCredentials,
    routing::ProviderPoolMember,
    storage::Error as StorageError,
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

/// Inserts the fixture inventory a policy can reference: one provider, one pool with that
/// provider as a member, and the pool's automatic route.
async fn seed_inventory(database: &Database) -> Result<(i64, i64, i64), StorageError> {
    let provider_id = seed_provider(database).await?;

    let pool_id = crate::storage::routing::create_pool(
        database,
        "pool",
        "",
        true,
        &[ProviderPoolMember {
            provider_id,
            priority: 0,
        }],
    )
    .await?;

    crate::storage::models::apply_discovered_models(
        database,
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

    let route_id = crate::storage::routing::list_routes(database)
        .await?
        .iter()
        .find(|route| route.is_automatic)
        .map(|route| route.id)
        .ok_or(StorageError::NotFound {
            message: "the fixture carries an automatic route".to_string(),
        })?;

    Ok((provider_id, pool_id, route_id))
}

/// The writable fields of one key over `policy`.
fn fields<'a>(
    name: &'a str,
    key: &'a str,
    is_enabled: bool,
    access_control: &'a ApiKeyAccessControl,
) -> ApiKeyFields<'a> {
    ApiKeyFields {
        name,
        key,
        is_enabled,
        access_control,
    }
}

/// A complete policy round-trips through create and list with its sparse collections canonically
/// ordered, explicit pins equal to the key default retained, and the enabled-secret listing
/// carries exactly the enabled keys.
#[tokio::test]
async fn complete_policy_round_trips_canonically() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let (provider_id, pool_id, route_id) = seed_inventory(&database).await?;

    let policy = ApiKeyAccessControl {
        default_access: AccessDecision::Deny,
        owner_defaults: vec![
            ApiKeyOwnerDefault::Pool {
                pool_id,
                access: AccessDecision::Allow,
            },
            ApiKeyOwnerDefault::Provider {
                provider_id,
                access: AccessDecision::Deny,
            },
        ],
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: route_id,
            access: AccessDecision::Allow,
        }],
    };

    let inserted =
        super::insert_api_key(&database, &fields("primary", "sk-serving-1", true, &policy)).await?;

    assert!(inserted.id > 0);

    assert_eq!(inserted.key, "sk-serving-1");

    // The returned policy is canonicalized: provider defaults before pool defaults.
    assert_eq!(
        inserted.access_control.owner_defaults,
        vec![
            ApiKeyOwnerDefault::Provider {
                provider_id,
                access: AccessDecision::Deny
            },
            ApiKeyOwnerDefault::Pool {
                pool_id,
                access: AccessDecision::Allow
            },
        ],
        "the owner defaults are canonicalized providers-first"
    );

    let listed = super::list_api_keys(&database).await?;

    assert_eq!(
        listed,
        vec![inserted.clone()],
        "the listing reads the complete policy back"
    );

    // An explicit decision equal to the key default is an authored pin, never collapsed.
    let pinned = ApiKeyAccessControl {
        default_access: AccessDecision::Deny,
        owner_defaults: vec![ApiKeyOwnerDefault::Provider {
            provider_id,
            access: AccessDecision::Deny,
        }],
        route_overrides: Vec::new(),
    };

    let pinned_key =
        super::insert_api_key(&database, &fields("pinned", "sk-serving-2", true, &pinned)).await?;

    assert_eq!(
        pinned_key.access_control.owner_defaults, pinned.owner_defaults,
        "an explicit pin equal to the inherited default is retained"
    );

    // A disabled key is excluded from the enabled-secret listing.
    super::insert_api_key(
        &database,
        &fields("disabled", "sk-serving-3", false, &policy),
    )
    .await?;

    let mut secrets = super::list_enabled_api_key_secrets(&database).await?;

    secrets.sort_by_key(|(id, _)| *id);

    assert_eq!(
        secrets,
        vec![
            (inserted.id, "sk-serving-1".to_string()),
            (pinned_key.id, "sk-serving-2".to_string()),
        ],
        "the enabled-secret listing carries exactly the enabled keys"
    );

    Ok(())
}

/// The policy write rejects duplicate rule identities and references to absent rows with their
/// typed errors, on create and on update alike, leaving the stored state unchanged.
#[tokio::test]
async fn policy_validation_rejects_duplicates_and_missing_references()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let (provider_id, _pool_id, _route_id) = seed_inventory(&database).await?;

    let duplicate = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: vec![
            ApiKeyOwnerDefault::Provider {
                provider_id,
                access: AccessDecision::Allow,
            },
            ApiKeyOwnerDefault::Provider {
                provider_id,
                access: AccessDecision::Deny,
            },
        ],
        route_overrides: Vec::new(),
    };

    let rejected = super::insert_api_key(&database, &fields("key", "sk-1", true, &duplicate)).await;

    assert!(
        matches!(
            &rejected,
            Err(StorageError::DuplicateAccessReference { reference })
                if *reference == super::AccessReference::Provider(provider_id)
        ),
        "a repeated authored rule identity is a duplicate, got {rejected:?}"
    );

    let unknown_provider = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: vec![ApiKeyOwnerDefault::Provider {
            provider_id: 9_999_999,
            access: AccessDecision::Allow,
        }],
        route_overrides: Vec::new(),
    };

    let rejected =
        super::insert_api_key(&database, &fields("key", "sk-1", true, &unknown_provider)).await;

    assert!(
        matches!(
            &rejected,
            Err(StorageError::MissingAccessReference { reference })
                if *reference == super::AccessReference::Provider(9_999_999)
        ),
        "an unknown provider reference is missing, got {rejected:?}"
    );

    let unknown_pool = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: vec![ApiKeyOwnerDefault::Pool {
            pool_id: 9_999_999,
            access: AccessDecision::Allow,
        }],
        route_overrides: Vec::new(),
    };

    let rejected =
        super::insert_api_key(&database, &fields("key", "sk-1", true, &unknown_pool)).await;

    assert!(
        matches!(
            &rejected,
            Err(StorageError::MissingAccessReference { reference })
                if *reference == super::AccessReference::Pool(9_999_999)
        ),
        "an unknown pool reference is missing, got {rejected:?}"
    );

    let unknown_route = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: 9_999_999,
            access: AccessDecision::Allow,
        }],
    };

    let rejected =
        super::insert_api_key(&database, &fields("key", "sk-1", true, &unknown_route)).await;

    assert!(
        matches!(
            &rejected,
            Err(StorageError::MissingAccessReference { reference })
                if *reference == super::AccessReference::Route(9_999_999)
        ),
        "an unknown route reference is missing, got {rejected:?}"
    );

    // The same validation runs on update, against an existing key.
    let initial = ApiKeyAccessControl {
        default_access: AccessDecision::Deny,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };

    let inserted = super::insert_api_key(&database, &fields("key", "sk-1", true, &initial)).await?;

    let rejected = super::update_api_key(
        &database,
        inserted.id,
        fields("key", "sk-1", true, &unknown_route),
    )
    .await;

    assert!(
        matches!(&rejected, Err(StorageError::MissingAccessReference { .. })),
        "the update validates the replacement policy too, got {rejected:?}"
    );

    let unchanged = super::list_api_keys(&database).await?;

    assert_eq!(
        unchanged.first().map(|key| key.access_control.clone()),
        Some(initial),
        "the rejected update leaves the stored policy unchanged"
    );

    Ok(())
}

/// The update replaces ordinary fields and the whole policy atomically, gates on an unknown key
/// and a taken secret, and the deletion cascades the sparse rules while answering whether a row
/// was removed.
#[tokio::test]
async fn update_replaces_whole_state_and_deletion_cascades_rules() -> Result<(), Box<dyn StdError>>
{
    let (database, _guard) = crate::test_support::temp_database().await;

    let (provider_id, _pool_id, route_id) = seed_inventory(&database).await?;

    let with_rules = ApiKeyAccessControl {
        default_access: AccessDecision::Deny,
        owner_defaults: vec![ApiKeyOwnerDefault::Provider {
            provider_id,
            access: AccessDecision::Allow,
        }],
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: route_id,
            access: AccessDecision::Allow,
        }],
    };

    let first =
        super::insert_api_key(&database, &fields("first", "sk-first", true, &with_rules)).await?;

    let empty = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };

    super::update_api_key(
        &database,
        first.id,
        fields("renamed", "sk-second", false, &empty),
    )
    .await?;

    let listed = super::list_api_keys(&database).await?;

    let updated = listed
        .first()
        .ok_or(StorageError::NotFound {
            message: "the updated key is listed".to_string(),
        })?
        .clone();

    assert_eq!(updated.name, "renamed");

    assert_eq!(updated.key, "sk-second");

    assert!(!updated.is_enabled);

    assert_eq!(
        updated.access_control, empty,
        "the update replaced the complete policy"
    );

    let unknown =
        super::update_api_key(&database, 9_999_999, fields("x", "sk-x", true, &empty)).await;

    assert!(
        matches!(&unknown, Err(StorageError::NotFound { .. })),
        "updating an unknown key is not found, got {unknown:?}"
    );

    let second =
        super::insert_api_key(&database, &fields("second", "sk-other", true, &empty)).await?;

    let taken = super::update_api_key(
        &database,
        second.id,
        fields("second", "sk-second", true, &empty),
    )
    .await;

    assert!(
        matches!(&taken, Err(StorageError::Conflict { .. })),
        "a taken secret conflicts, got {taken:?}"
    );

    assert!(
        super::delete_api_key(&database, first.id).await?,
        "the deletion reports the removed row"
    );

    assert!(
        !super::delete_api_key(&database, first.id).await?,
        "a second deletion reports that no row was removed"
    );

    let rules_left = database
        .query(
            "SELECT COUNT(*) FROM api_key_provider_defaults WHERE api_key_id = ?1",
            [first.id],
        )
        .await?
        .first()
        .map(|row| row.get::<i64>(0))
        .transpose()?;

    assert_eq!(
        rules_left,
        Some(0),
        "the deletion cascaded the provider default"
    );

    let overrides_left = database
        .query(
            "SELECT COUNT(*) FROM api_key_model_route_overrides WHERE api_key_id = ?1",
            [first.id],
        )
        .await?
        .first()
        .map(|row| row.get::<i64>(0))
        .transpose()?;

    assert_eq!(
        overrides_left,
        Some(0),
        "the deletion cascaded the route override"
    );

    Ok(())
}

/// The admission policy read loads the complete policy of an enabled key only: a disabled or
/// unknown key is the typed inactive-key failure.
#[tokio::test]
async fn policy_read_gates_on_an_enabled_key() -> Result<(), Box<dyn StdError>> {
    use crate::access_control::RouteOwner;

    let (database, _guard) = crate::test_support::temp_database().await;

    let (provider_id, _pool_id, _route_id) = seed_inventory(&database).await?;

    let empty = ApiKeyAccessControl {
        default_access: AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };

    let disabled =
        super::insert_api_key(&database, &fields("disabled", "sk-disabled", false, &empty)).await?;

    let enabled =
        super::insert_api_key(&database, &fields("enabled", "sk-enabled", true, &empty)).await?;

    let inactive = {
        let mut transaction = database.transaction().await?;

        let outcome = super::read_enabled_policy(&mut transaction, disabled.id).await;

        transaction.commit().await?;

        outcome
    };

    assert!(
        matches!(&inactive, Err(StorageError::InactiveApiKey { id }) if *id == disabled.id),
        "a disabled key is inactive, got {inactive:?}"
    );

    let missing = {
        let mut transaction = database.transaction().await?;

        let outcome = super::read_enabled_policy(&mut transaction, 9_999_999).await;

        transaction.commit().await?;

        outcome
    };

    assert!(
        matches!(&missing, Err(StorageError::InactiveApiKey { .. })),
        "an unknown key is inactive, got {missing:?}"
    );

    let policy = {
        let mut transaction = database.transaction().await?;

        let outcome = super::read_enabled_policy(&mut transaction, enabled.id).await;

        transaction.commit().await?;

        outcome?
    };

    assert_eq!(
        policy.resolve(1, RouteOwner::Provider(provider_id)),
        AccessDecision::Allow,
        "the enabled key's policy resolves its allow default"
    );

    Ok(())
}
