//! Provider-definition persistence tests: the version chain appends under a name, prior versions
//! stay immutable history, a stale base version is rejected as a conflict without writing, an
//! unknown name is not found, and a repeated creation under a taken name surfaces the database's
//! uniqueness backstop as the typed storage constraint error.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or list is that test's failure report"
)]

use super::NewDefinition;
use crate::{
    database::Database,
    provider::definition::{DefinitionPayload, ProviderDefinition},
    storage::Error as StorageError,
};
use std::{error::Error as StdError, num::NonZeroI64};

/// Parses the fixture payload of a definition whose inference URL is the literal
/// `https://upstream.test/v1/…`: a static-API-key chat-completion recipe whose expressions never
/// reference a base URL.
fn api_key_payload(upstream: &str) -> DefinitionPayload {
    serde_json::from_value(serde_json::json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [
                {"type": "literal", "value": format!("https://{upstream}/v1/chat/completions")}
            ],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    }))
    .expect("fixture definition payload parses")
}

/// The nonzero fixture version of `value`.
fn version(value: i64) -> NonZeroI64 {
    NonZeroI64::new(value).expect("fixture versions are nonzero literals")
}

/// Inserts one fixture definition version and returns the stored row.
async fn insert_fixture(
    database: &Database,
    name: &str,
    base_version: Option<NonZeroI64>,
    payload: &DefinitionPayload,
    created_at: i64,
) -> Result<ProviderDefinition, StorageError> {
    super::insert_definition(
        database,
        NewDefinition {
            name,
            base_version,
            payload,
            created_at,
        },
    )
    .await
}

/// The inserted fixture definition payload of `name` at version 1.
async fn insert_first_fixture(
    database: &Database,
    name: &str,
    payload: &DefinitionPayload,
) -> Result<ProviderDefinition, StorageError> {
    insert_fixture(database, name, None, payload, 1_000).await
}

/// A definition version round-trips through storage exactly as inserted, prior versions stay
/// immutable after successors append, unknown versions answer `None`, and the listing orders by
/// name then version.
#[tokio::test]
async fn definition_versions_round_trip_and_accumulate() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    let first = insert_first_fixture(&database, "acme", &api_key_payload("first.test")).await?;

    assert_eq!(first.name, "acme");

    assert_eq!(first.version.get(), 1, "a creation inserts version 1");

    assert_eq!(first.payload, api_key_payload("first.test"));

    assert_eq!(first.created_at, 1_000);

    let second = insert_fixture(
        &database,
        "acme",
        Some(version(1)),
        &api_key_payload("second.test"),
        2_000,
    )
    .await?;

    assert_eq!(
        second.version.get(),
        2,
        "a successor appends the next version"
    );

    assert_eq!(second.payload, api_key_payload("second.test"));

    let reloaded_first = super::find_definition(&database, "acme", version(1)).await?;

    assert_eq!(
        reloaded_first,
        Some(first),
        "an appended successor leaves the prior version's stored payload untouched"
    );

    assert_eq!(
        super::find_definition(&database, "acme", version(9)).await?,
        None
    );

    insert_fixture(
        &database,
        "acme",
        Some(version(2)),
        &api_key_payload("third.test"),
        3_000,
    )
    .await?;

    insert_first_fixture(&database, "other", &api_key_payload("first.test")).await?;

    let listed = super::list_definitions(&database).await?;

    let listed_keys = listed
        .iter()
        .map(|definition| (definition.name.as_str(), definition.version.get()))
        .collect::<Vec<_>>();

    assert_eq!(
        listed_keys,
        [("acme", 1), ("acme", 2), ("acme", 3), ("other", 1)],
        "the listing orders by name then version"
    );

    Ok(())
}

/// A successor insert naming a stale base version is rejected as a conflict and writes nothing,
/// and a successor for an unknown name is not found.
#[tokio::test]
async fn stale_base_version_and_unknown_name_are_rejected() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    insert_first_fixture(&database, "acme", &api_key_payload("first.test")).await?;

    insert_fixture(
        &database,
        "acme",
        Some(version(1)),
        &api_key_payload("second.test"),
        2_000,
    )
    .await?;

    let stale = insert_fixture(
        &database,
        "acme",
        Some(version(1)),
        &api_key_payload("stale.test"),
        3_000,
    )
    .await;

    assert!(
        matches!(&stale, Err(StorageError::Conflict { .. })),
        "a stale base version is a conflict, got {stale:?}"
    );

    assert_eq!(
        super::find_definition(&database, "acme", version(3)).await?,
        None,
        "the rejected stale insert writes nothing"
    );

    let unknown = insert_fixture(
        &database,
        "missing",
        Some(version(1)),
        &api_key_payload("first.test"),
        4_000,
    )
    .await;

    assert!(
        matches!(&unknown, Err(StorageError::NotFound { .. })),
        "a successor for an unknown name is not found, got {unknown:?}"
    );

    Ok(())
}

/// A repeated creation under a taken name hits the database's uniqueness backstop and surfaces as
/// the typed storage constraint error — the concurrent backstop behind the management surface's
/// checked pre-check.
#[tokio::test]
async fn taken_name_surfaces_the_constraint_backstop() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = crate::test_support::temp_database().await;

    insert_first_fixture(&database, "acme", &api_key_payload("first.test")).await?;

    let repeated = insert_first_fixture(&database, "acme", &api_key_payload("first.test")).await;

    assert!(
        matches!(&repeated, Err(StorageError::Constraint { .. })),
        "the repeated creation hits the uniqueness backstop, got {repeated:?}"
    );

    Ok(())
}
