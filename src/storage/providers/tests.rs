//! Provider row persistence tests: the insert round-trip and the name-ordered listing, the
//! uniqueness backstop on a taken name, the transactional update with its rename, definition
//! rebind, credential-kind and base-URL gates, and the lease-guarded token-refresh writes over
//! the stored credentials document.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or list is that test's failure report"
)]

use super::{NewProvider, ProviderUpdate};
use crate::{
    database::Database,
    provider::{
        CredentialKind, Provider, StoredCredentials,
        definition::{DefinitionPayload, ProviderDefinition},
    },
    storage::Error as StorageError,
};
use std::{collections::BTreeMap, error::Error as StdError, num::NonZeroI64};

/// Parses the static-API-key fixture payload: a literal-URL chat-completion recipe whose
/// expressions never reference a base URL.
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

/// Parses the base-addressing fixture payload: its inference URL is the base-URL expression, so
/// a provider bound to it must carry a base URL.
fn base_addressing_payload() -> DefinitionPayload {
    serde_json::from_value(serde_json::json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    }))
    .expect("fixture definition payload parses")
}

/// Parses the OAuth fixture payload: an authorization-code recipe with a refresh grant, whose
/// credential kind every OAuth provider row must match.
fn oauth_payload() -> DefinitionPayload {
    serde_json::from_value(serde_json::json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "literal", "value": "https://first.test/v1/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {
            "type": "oauth",
            "client_id": "client-id",
            "grant": {
                "type": "authorization_code",
                "authorize_url": [
                    {"type": "literal", "value": "https://first.test/authorize"}
                ],
                "redirect_uri": "http://127.0.0.1:0/callback",
                "pkce": "s256"
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": "https://first.test/token"}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "authorization_code"}
                        ]},
                        {"name": "code", "value": [{"type": "authorization_code"}]}
                    ]}
                },
                "response": {"encoding": "json", "fields": [
                    {"path": ["access_token"], "target": {"type": "access_token"},
                     "required": true}
                ]},
                "terminal_codes": ["bad_request"]
            },
            "refresh": {
                "type": "grant",
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": "https://first.test/token"}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "refresh_token"}
                        ]},
                        {"name": "refresh_token", "value": [{"type": "refresh_token"}]}
                    ]}
                },
                "terminal_codes": ["bad_request"]
            }
        }
    }))
    .expect("fixture definition payload parses")
}

/// The nonzero fixture version of `value`.
fn version(value: i64) -> NonZeroI64 {
    NonZeroI64::new(value).expect("fixture versions are nonzero literals")
}

/// Inserts one fixture definition version and returns the stored definition.
async fn insert_fixture_definition(
    database: &Database,
    name: &str,
    payload: &DefinitionPayload,
) -> Result<ProviderDefinition, StorageError> {
    crate::storage::provider_definitions::insert_definition(
        database,
        crate::storage::provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload,
            created_at: 1_000,
        },
    )
    .await
}

/// Inserts one static-API-key fixture provider bound to `definition`.
async fn insert_fixture_provider(
    database: &Database,
    name: &str,
    model_prefix: &str,
    definition: &ProviderDefinition,
) -> Result<Provider, StorageError> {
    let credentials = StoredCredentials::ApiKey {
        api_key: "sk-fixture".to_string(),
    };

    super::insert_provider(
        database,
        NewProvider {
            name,
            model_prefix,
            base_url: None,
            credentials: &credentials,
            definition,
            is_enabled: true,
        },
    )
    .await
}

/// Unwraps an expected-present row read; a missing row is the failure under test.
fn present(row: Option<Provider>) -> Provider {
    row.expect("the row under test exists")
}

/// Asserts the two provider rows agree on every stored field.
fn assert_same_provider(inserted: &Provider, read_back: &Provider) {
    assert_eq!(read_back.id, inserted.id);
    assert_eq!(read_back.name, inserted.name);
    assert_eq!(read_back.model_prefix, inserted.model_prefix);
    assert_eq!(read_back.base_url, inserted.base_url);
    assert_eq!(read_back.is_enabled, inserted.is_enabled);
    assert_eq!(read_back.credentials, inserted.credentials);
    assert_eq!(read_back.quota_generation, inserted.quota_generation);
    assert_eq!(read_back.quota_revision, inserted.quota_revision);
    assert_eq!(read_back.definition, inserted.definition);
}

/// A provider row round-trips through storage exactly as inserted, reads back by id, lists in
/// name order, and answers the name-taken pre-check.
#[tokio::test]
async fn provider_rows_round_trip_and_list_in_name_order() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = insert_fixture_definition(&database, "def", &api_key_payload()).await?;

    let inserted = insert_fixture_provider(&database, "acct", "acme.", &definition).await?;

    assert!(inserted.id > 0, "the insert assigns a row identity");

    assert_eq!(inserted.name, "acct");

    assert_eq!(inserted.model_prefix, "acme.");

    assert_eq!(inserted.base_url, None);

    assert!(inserted.is_enabled);

    assert_eq!(inserted.credentials.kind(), CredentialKind::ApiKey);

    assert_eq!(inserted.quota_generation, 0, "fresh fences start at zero");

    assert_eq!(inserted.quota_revision, 0, "fresh fences start at zero");

    assert_eq!(inserted.definition, definition);

    let read_back = present(super::find_provider_by_id(&database, inserted.id).await?);

    assert_same_provider(&inserted, &read_back);

    insert_fixture_provider(&database, "zulu", "", &definition).await?;

    let listed = super::list_providers(&database).await?;

    let names = listed
        .iter()
        .map(|provider| provider.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(names, ["acct", "zulu"], "the listing orders by name");

    assert!(super::is_provider_name_taken(&database, "acct").await?);

    assert!(!super::is_provider_name_taken(&database, "other").await?);

    let reloaded = super::reload_provider(&database, inserted.id, "the test").await?;

    assert_eq!(reloaded.id, inserted.id, "the reload re-reads the same row");

    Ok(())
}

/// A repeated creation under a taken name hits the database's uniqueness backstop and surfaces as
/// the typed storage constraint error.
#[tokio::test]
async fn taken_name_surfaces_the_constraint_backstop() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = insert_fixture_definition(&database, "def", &api_key_payload()).await?;

    insert_fixture_provider(&database, "acct", "", &definition).await?;

    let repeated = insert_fixture_provider(&database, "acct", "", &definition).await;

    assert!(
        matches!(&repeated, Err(StorageError::Constraint { .. })),
        "the repeated creation hits the uniqueness backstop, got {repeated:?}"
    );

    Ok(())
}

/// The update renames, rebinds the exact definition version, rewrites the readable fields, and
/// preserves the row identity.
#[tokio::test]
async fn update_renames_and_rebinds_preserving_identity() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition_v1 = insert_fixture_definition(&database, "def", &api_key_payload()).await?;

    let definition_v2 = insert_fixture_definition(&database, "def2", &api_key_payload()).await?;

    let credentials = StoredCredentials::ApiKey {
        api_key: "sk-fixture".to_string(),
    };

    let inserted = super::insert_provider(
        &database,
        NewProvider {
            name: "acct",
            model_prefix: "acme.",
            base_url: Some("https://old.test"),
            credentials: &credentials,
            definition: &definition_v1,
            is_enabled: true,
        },
    )
    .await?;

    super::update_provider(
        &database,
        inserted.id,
        ProviderUpdate {
            name: "renamed",
            model_prefix: "new.",
            base_url: Some("https://new.test"),
            is_enabled: false,
            definition: Some(("def2", version(1))),
            credentials: None,
        },
    )
    .await?;

    let updated = present(super::find_provider_by_id(&database, inserted.id).await?);

    assert_eq!(updated.id, inserted.id, "the update preserves the identity");

    assert_eq!(updated.name, "renamed");

    assert_eq!(updated.model_prefix, "new.");

    assert_eq!(updated.base_url, Some("https://new.test".to_string()));

    assert!(!updated.is_enabled);

    assert_eq!(updated.definition, definition_v2, "the rebind took effect");

    assert!(
        !super::is_provider_name_taken(&database, "acct").await?,
        "the old name is free after the rename"
    );

    Ok(())
}

/// The update rejects a taken name, an unknown definition reference, and a rebind whose
/// credential kind disagrees with the stored credentials — each leaving all state unchanged.
#[tokio::test]
async fn update_gates_names_references_and_credential_kinds() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = insert_fixture_definition(&database, "def", &api_key_payload()).await?;

    let _oauth_definition =
        insert_fixture_definition(&database, "oauth-def", &oauth_payload()).await?;

    let _first = insert_fixture_provider(&database, "acct", "", &definition).await?;

    let second = insert_fixture_provider(&database, "other", "", &definition).await?;

    let taken = super::update_provider(
        &database,
        second.id,
        ProviderUpdate {
            name: "acct",
            model_prefix: "",
            base_url: None,
            is_enabled: true,
            definition: None,
            credentials: None,
        },
    )
    .await;

    assert!(
        matches!(&taken, Err(StorageError::Conflict { message }) if message.contains("already taken")),
        "a rename to a taken name conflicts, got {taken:?}"
    );

    let unknown = super::update_provider(
        &database,
        second.id,
        ProviderUpdate {
            name: "other",
            model_prefix: "",
            base_url: None,
            is_enabled: true,
            definition: Some(("missing", version(1))),
            credentials: None,
        },
    )
    .await;

    assert!(
        matches!(&unknown, Err(StorageError::NotFound { .. })),
        "a rebind to an unknown definition is not found, got {unknown:?}"
    );

    let mismatched = super::update_provider(
        &database,
        second.id,
        ProviderUpdate {
            name: "other",
            model_prefix: "",
            base_url: None,
            is_enabled: true,
            definition: Some(("oauth-def", version(1))),
            credentials: None,
        },
    )
    .await;

    assert!(
        matches!(&mismatched, Err(StorageError::Conflict { .. })),
        "a rebind whose credential kind disagrees with the stored credentials conflicts, \
         got {mismatched:?}"
    );

    let unchanged = present(super::find_provider_by_id(&database, second.id).await?);

    assert_same_provider(&second, &unchanged);

    assert_eq!(unchanged.definition, definition);

    Ok(())
}

/// A provider bound to a base-addressing definition must always carry a base URL: the update
/// refuses to clear it.
#[tokio::test]
async fn update_requires_the_base_url_of_a_base_addressing_definition()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition =
        insert_fixture_definition(&database, "based", &base_addressing_payload()).await?;

    let credentials = StoredCredentials::ApiKey {
        api_key: "sk-fixture".to_string(),
    };

    let inserted = super::insert_provider(
        &database,
        NewProvider {
            name: "acct",
            model_prefix: "",
            base_url: Some("https://base.test"),
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    let cleared = super::update_provider(
        &database,
        inserted.id,
        ProviderUpdate {
            name: "acct",
            model_prefix: "",
            base_url: None,
            is_enabled: true,
            definition: None,
            credentials: None,
        },
    )
    .await;

    assert!(
        matches!(&cleared, Err(StorageError::Conflict { .. })),
        "clearing the base URL of a base-addressing provider conflicts, got {cleared:?}"
    );

    let kept = present(super::find_provider_by_id(&database, inserted.id).await?);

    assert_eq!(
        kept.base_url,
        Some("https://base.test".to_string()),
        "the rejected update leaves the base URL stored"
    );

    Ok(())
}

/// The token-refresh lease is granted only to an elapsed expiry without a live holder, the
/// renewed credentials and the lease clear are persisted only while the caller still owns the
/// lease, and a `null` expiry never grants one.
#[tokio::test]
async fn refresh_lease_is_guarded_end_to_end() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = insert_fixture_definition(&database, "def", &oauth_payload()).await?;

    let now = 1_000_000_000_000;

    let credentials = StoredCredentials::OAuth {
        access_token: "at-1".to_string(),
        refresh_token: Some("rt-1".to_string()),
        slots: BTreeMap::new(),
        expires_at: Some(now - 100),
        refresh_started_at: None,
    };

    let inserted = super::insert_provider(
        &database,
        NewProvider {
            name: "acct",
            model_prefix: "",
            base_url: None,
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    let mut transaction = database.transaction().await?;

    let lease = super::try_acquire_refresh_lease(&mut transaction, inserted.id, now, 0, 0).await?;

    assert_eq!(
        lease,
        Some(now),
        "an elapsed expiry without a holder grants the lease"
    );

    transaction.commit().await?;

    let mut transaction = database.transaction().await?;

    let live_holder =
        super::try_acquire_refresh_lease(&mut transaction, inserted.id, now + 10, now, 0).await?;

    assert_eq!(
        live_holder, None,
        "a holder with a stamp at the staleness boundary keeps the lease"
    );

    let reacquired =
        super::try_acquire_refresh_lease(&mut transaction, inserted.id, now + 20, now + 1, 0)
            .await?;

    assert_eq!(
        reacquired,
        Some(now + 20),
        "a holder older than the staleness boundary loses the lease"
    );

    transaction.commit().await?;

    let renewed = StoredCredentials::OAuth {
        access_token: "at-2".to_string(),
        refresh_token: Some("rt-2".to_string()),
        slots: BTreeMap::new(),
        expires_at: Some(now + 3_600_000_000),
        refresh_started_at: None,
    };

    assert!(
        super::clear_refresh_lease(&database, inserted.id, now + 20).await?,
        "the lease owner clears the stamp while it is still held"
    );

    assert!(
        !super::clear_refresh_lease(&database, inserted.id, now + 20).await?,
        "a second clear finds no lease of its own"
    );

    // The cleared lease grants a fresh one, under which the renewal persists.
    let mut transaction = database.transaction().await?;

    let fresh_lease =
        super::try_acquire_refresh_lease(&mut transaction, inserted.id, now + 30, 0, 0).await?;

    transaction.commit().await?;

    assert_eq!(fresh_lease, Some(now + 30));

    assert!(
        super::persist_renewed_credentials_for_lease(&database, inserted.id, now + 30, &renewed)
            .await?,
        "the lease owner persists the renewed credentials"
    );

    assert_eq!(
        present(super::find_provider_by_id(&database, inserted.id).await?).credentials,
        renewed,
        "the renewed credentials are stored verbatim, lease cleared"
    );

    let superseded = StoredCredentials::OAuth {
        access_token: "at-3".to_string(),
        refresh_token: None,
        slots: BTreeMap::new(),
        expires_at: Some(now),
        refresh_started_at: None,
    };

    assert!(
        !super::persist_renewed_credentials_for_lease(&database, inserted.id, 999, &superseded)
            .await?,
        "a caller without the lease persists nothing"
    );

    assert_eq!(
        present(super::find_provider_by_id(&database, inserted.id).await?).credentials,
        renewed,
        "the discarded write leaves the stored credentials untouched"
    );

    Ok(())
}

/// A provider whose stored expiry is `null` — the no-scheduled-refresh state — or still beyond
/// the freshness margin is never granted a lease.
#[tokio::test]
async fn lease_requires_a_known_elapsed_expiry() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = insert_fixture_definition(&database, "def", &oauth_payload()).await?;

    let now = 1_000_000_000_000;

    for (name, expires_at) in [("no-schedule", None), ("fresh", Some(now + 10_000))] {
        let credentials = StoredCredentials::OAuth {
            access_token: "at".to_string(),
            refresh_token: None,
            slots: BTreeMap::new(),
            expires_at,
            refresh_started_at: None,
        };

        let inserted = super::insert_provider(
            &database,
            NewProvider {
                name,
                model_prefix: "",
                base_url: None,
                credentials: &credentials,
                definition: &definition,
                is_enabled: true,
            },
        )
        .await?;

        let mut transaction = database.transaction().await?;

        let lease =
            super::try_acquire_refresh_lease(&mut transaction, inserted.id, now, 0, 100).await?;

        assert_eq!(lease, None, "provider {name} is not granted a lease");

        transaction.commit().await?;
    }

    Ok(())
}

/// The reprovision patch rewrites the token facts under the caller's lease, preserves the stored
/// refresh token when the sequence extracted no rotation, replaces the base URL only when
/// carried, and discards everything when the lease was lost.
#[tokio::test]
async fn reprovision_patch_is_lease_guarded_and_rotationless() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let definition = insert_fixture_definition(&database, "def", &oauth_payload()).await?;

    let now = 1_000_000_000_000;

    let credentials = StoredCredentials::OAuth {
        access_token: "at-1".to_string(),
        refresh_token: Some("rt-keep".to_string()),
        slots: BTreeMap::new(),
        expires_at: Some(now - 100),
        refresh_started_at: None,
    };

    let inserted = super::insert_provider(
        &database,
        NewProvider {
            name: "acct",
            model_prefix: "",
            base_url: Some("https://old.test"),
            credentials: &credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    let mut transaction = database.transaction().await?;

    let lease = super::try_acquire_refresh_lease(&mut transaction, inserted.id, now, 0, 0).await?;

    transaction.commit().await?;

    let lease = lease.expect("the elapsed fixture credential grants its lease");

    assert!(
        super::persist_reprovision_for_lease(
            &database,
            inserted.id,
            lease,
            "at-provisioned",
            Some(now + 500),
            None,
            Some("https://provisioned.test"),
        )
        .await?,
        "the lease owner persists the reprovision patch"
    );

    let provisioned = present(super::find_provider_by_id(&database, inserted.id).await?);

    assert_eq!(
        provisioned.credentials,
        StoredCredentials::OAuth {
            access_token: "at-provisioned".to_string(),
            refresh_token: Some("rt-keep".to_string()),
            slots: BTreeMap::new(),
            expires_at: Some(now + 500),
            refresh_started_at: None,
        },
        "the patch rewrites the token facts and preserves the rotationless refresh token"
    );

    assert_eq!(
        provisioned.base_url,
        Some("https://provisioned.test".to_string()),
        "the carried base URL replaces the stored one"
    );

    // The patch cleared the lease stamp, so the next renewal reacquires it; the rotationless
    // reprovision with no base URL leaves the stored one alone.
    let mut transaction = database.transaction().await?;

    let second_lease =
        super::try_acquire_refresh_lease(&mut transaction, inserted.id, now + 1_000, 0, 0).await?;

    transaction.commit().await?;

    let second_lease = second_lease.expect("the patched credential grants a fresh lease");

    assert!(
        super::persist_reprovision_for_lease(
            &database,
            inserted.id,
            second_lease,
            "at-again",
            None,
            Some("rt-rotated"),
            None,
        )
        .await?,
        "the fresh lease owner persists the rotating patch"
    );

    let rotated = present(super::find_provider_by_id(&database, inserted.id).await?);

    assert_eq!(
        rotated.credentials,
        StoredCredentials::OAuth {
            access_token: "at-again".to_string(),
            refresh_token: Some("rt-rotated".to_string()),
            slots: BTreeMap::new(),
            expires_at: None,
            refresh_started_at: None,
        },
        "the carried rotation replaces the stored refresh token"
    );

    assert_eq!(
        rotated.base_url,
        Some("https://provisioned.test".to_string()),
        "an absent base URL keeps the stored one"
    );

    assert!(
        !super::persist_reprovision_for_lease(
            &database,
            inserted.id,
            second_lease,
            "at-lost",
            None,
            None,
            None,
        )
        .await?,
        "a caller without the lease persists nothing"
    );

    Ok(())
}
