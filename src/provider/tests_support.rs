//! Shared provider-test fixtures: the payload parser, the definition and provider seeds, and
//! the control-request builders every provider test family reuses. Fixtures specific to one
//! family live beside that family's tests.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a fixture that cannot parse or store is that test's failure report, and a helper \
              asserting on a failure reports a wrong success by panicking"
)]

use crate::{
    database::Database,
    provider::{DefinitionPayload, Provider, ProviderDefinition, StoredCredentials},
};
use serde_json::Value;
use std::{error::Error as StdError, num::NonZeroI64};

/// Parses one definition payload fixture; a fixture that does not parse is the test bug.
pub(crate) fn parse_payload(payload: Value) -> DefinitionPayload {
    serde_json::from_value(payload).expect("fixture definition payload parses")
}

/// Builds the in-memory definition version 1 of `name` over `payload`.
pub(crate) fn definition_named(name: &str, payload: DefinitionPayload) -> ProviderDefinition {
    ProviderDefinition {
        name: name.to_string(),
        #[expect(
            clippy::expect_used,
            reason = "the fixture version is a nonzero literal"
        )]
        version: NonZeroI64::new(1).expect("fixture versions are nonzero literals"),
        payload,
        created_at: 1_000,
    }
}

/// Parses `payload` into the in-memory definition version 1 of `name`.
pub(crate) fn definition_from_json(name: &str, payload: Value) -> ProviderDefinition {
    definition_named(name, parse_payload(payload))
}

/// Inserts one enabled provider row named `name` bound to the parsed `payload` and returns the
/// stored row, carrying the `fixture.` model prefix. The definition version it binds —
/// `fixture-definition` version 1 — is stored first, so a fixture family seeding several
/// providers of one recipe binds the same stored version.
pub(crate) async fn seed_provider(
    database: &Database,
    name: &str,
    payload: Value,
    credentials: &StoredCredentials,
    base_url: Option<&str>,
) -> Result<Provider, Box<dyn StdError>> {
    let definition = definition_from_json("fixture-definition", payload);

    if crate::storage::provider_definitions::find_definition(
        database,
        &definition.name,
        definition.version,
    )
    .await?
    .is_none()
    {
        crate::storage::provider_definitions::insert_definition(
            database,
            crate::storage::provider_definitions::NewDefinition {
                name: definition.name.as_str(),
                base_version: None,
                payload: &definition.payload,
                created_at: definition.created_at,
            },
        )
        .await?;
    }

    let provider = crate::storage::providers::insert_provider(
        database,
        crate::storage::providers::NewProvider {
            name,
            model_prefix: "fixture.",
            base_url,
            credentials,
            definition: &definition,
            is_enabled: true,
        },
    )
    .await?;

    Ok(provider)
}

/// The static-API-key credentials carrying `api_key`.
pub(crate) fn api_key_credentials(api_key: &str) -> StoredCredentials {
    StoredCredentials::ApiKey {
        api_key: api_key.to_string(),
    }
}

/// The rendered failure of a result expected to fail; an unexpected success is the test's
/// failure report. The engine failure types render their full human message, so the returned
/// text is what the assertions inspect.
pub(crate) fn failure_detail<TValue, TError>(result: Result<TValue, TError>) -> String
where
    TError: std::fmt::Display,
{
    match result {
        Ok(_) => panic!("the operation must fail"),

        Err(error) => error.to_string(),
    }
}
