//! Test fixtures shared by the storage entity modules' test modules: each entity's tests live in
//! its own module beside its statements, and the fixtures more than one of those modules seed
//! from live here once.

use super::{
    Error,
    models::ModelEntry,
    provider_definitions::{NewDefinition, insert_definition},
};
use crate::{
    database::Database,
    format::UpstreamFormat,
    provider::{StoredCredentials, definition::ProviderDefinition},
    test_support::{
        self, FIXTURE_TOKEN_URL, OAuthDefinitionFixture, OAuthDeviceAuthorizationFixture,
        OAuthGrantFixture, OAuthRefreshFixture, OAuthResponseField, OAuthTarget,
        oauth_definition_payload,
    },
};
use std::error::Error as StdError;

/// Inserts one enabled static-key test provider, returning its stored row.
///
/// # Panics
///
/// Panics if the insert fails.
pub(crate) async fn insert_test_provider(
    database: &Database,
    name: &str,
    model_prefix: &str,
) -> Result<i64, Box<dyn StdError>> {
    let definition = test_support::insert_static_key_definition(
        database,
        &format!("def-{name}"),
        UpstreamFormat::OpenAiChatCompletion,
    )
    .await;

    let provider = test_support::insert_provider(
        database,
        &definition,
        name,
        model_prefix,
        Some("http://localhost:8080"),
        StoredCredentials::ApiKey {
            api_key: "test-secret".to_string(),
        },
    )
    .await;

    Ok(provider.id)
}

/// Fetches one listing entry by its upstream identifier, expecting it to be present.
///
/// # Panics
///
/// Panics if the entry is absent; every caller looks up a row the test just verified exists.
pub(crate) fn entry<'a>(entries: &'a [ModelEntry], upstream_id: &str) -> &'a ModelEntry {
    entries
        .iter()
        .find(|entry| entry.model.upstream_id == upstream_id)
        .unwrap_or_else(|| {
            #[expect(
                clippy::panic,
                reason = "every caller looks up a row the test just verified exists"
            )]
            {
                panic!("model row `{upstream_id}` must be listed")
            }
        })
}

/// Builds one fixture OAuth definition version 1: a device grant whose credential never
/// renews.
///
/// # Panics
///
/// Panics if the payload fails to parse or validate — neither can occur for the fixed
/// fixture payload.
pub(crate) fn oauth_definition() -> ProviderDefinition {
    let payload = oauth_definition_payload(OAuthDefinitionFixture {
        format: UpstreamFormat::OpenAiChatCompletion,
        inference_suffix: Some("/chat/completions"),
        client_id: "fixture-client",
        initial_slots: &[],
        grant: OAuthGrantFixture::Device {
            has_grant_type_field: true,
            authorization: OAuthDeviceAuthorizationFixture::default(),
        },
        exchange_url: FIXTURE_TOKEN_URL,
        response_fields: vec![OAuthResponseField::new(
            "refresh_token",
            OAuthTarget::RefreshToken,
        )],
        terminal_codes: &["expired_token", "access_denied"],
        refresh: OAuthRefreshFixture::None,
        provisioning: None,
    });

    #[expect(clippy::unwrap_used, reason = "the fixture payload validates")]
    payload.validate().unwrap();

    #[expect(clippy::unwrap_used, reason = "a positive literal version")]
    let version = std::num::NonZeroI64::new(1).unwrap();

    ProviderDefinition {
        name: "fixture-oauth".to_string(),
        version,
        payload,
        created_at: 1,
    }
}

/// Inserts the fixture definition as version 1: the seeding ritual every storage OAuth test
/// performs before building providers or flows on top of [`oauth_definition`]'s output.
///
/// # Errors
///
/// Returns the storage [`Error`] when the insert fails.
pub(crate) async fn insert_oauth_definition(
    database: &Database,
    definition: &ProviderDefinition,
) -> Result<(), Error> {
    insert_definition(
        database,
        NewDefinition {
            name: definition.name.as_str(),
            base_version: None,
            payload: &definition.payload,
            created_at: definition.created_at,
        },
    )
    .await?;

    Ok(())
}
