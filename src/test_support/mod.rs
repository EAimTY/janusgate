//! Test support: the crate-wide shared test infrastructure — the temporary file and directory
//! guards, the JSON-pointer read helper, and the database seeding fixtures — plus the re-exports
//! of the two fixture families beside it: the OAuth definition payload fixtures
//! ([`oauth_fixtures`]) and the mock upstream HTTP transport ([`mock_upstream`]).
//!
//! Every helper panics when its fixed fixture fails to parse, validate, or insert, or when a
//! JSON pointer read misses its path — a test bug by construction, so the panic is the failure
//! report.

#![expect(
    clippy::panic,
    reason = "a fixture that cannot parse, validate, or insert is a test bug, and the panic \
              is the failure report"
)]

mod mock_upstream;
mod oauth_fixtures;

pub(crate) use self::{
    mock_upstream::{MockAction, MockUpstream},
    oauth_fixtures::{
        FIXTURE_TOKEN_URL, OAuthDefinitionFixture, OAuthGrantFixture, OAuthRefreshFixture,
        OAuthResponseField, OAuthTarget, oauth_definition_payload,
    },
};
use crate::{
    database::Database,
    format::UpstreamFormat,
    provider::{Provider, ProviderDefinition, StoredCredentials, definition::DefinitionPayload},
    storage,
};
use serde_json::{Value, json};
use std::{num::NonZero, path::PathBuf, time::Duration};

/// File guard cleaning up the temporary SQLite test database on drop.
pub(crate) struct TempDbGuard(PathBuf);

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Directory guard removing the temporary test directory tree on drop.
pub(crate) struct TempDirGuard(pub(crate) PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Reads the JSON value at a pointer path of a rendered body.
///
/// # Panics
///
/// Panics if the path is absent: a missing path is the failure under test.
pub(crate) fn at<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    #[expect(
        clippy::expect_used,
        reason = "rendered bodies are under test; a missing path is the failure"
    )]
    value
        .pointer(pointer)
        .expect("the rendered body carries the expected path")
}

/// Creates an initialized temporary SQLite test database.
///
/// # Panics
///
/// Panics if opening the test database fails — a broken fixture environment is the failure
/// report.
pub(crate) async fn create_test_database() -> (Database, TempDbGuard) {
    let db_path = std::env::temp_dir().join(format!("janusgate-test-{}.db", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero literal")]
    let max_connections = NonZero::new(2).unwrap();

    #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
    let database = Database::open(
        &db_path,
        crate::database::schema::Registry::Main,
        max_connections,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    (database, TempDbGuard(db_path))
}

/// Inserts one static-key definition version 1 of the given format and returns the stored
/// definition.
///
/// # Panics
///
/// Panics if the insert fails, the payload does not parse, or the fixture fails validation —
/// none of which can occur for the fixed fixture payloads.
pub(crate) async fn insert_static_key_definition(
    database: &Database,
    name: &str,
    format: crate::format::UpstreamFormat,
) -> ProviderDefinition {
    let payload: DefinitionPayload = serde_json::from_value(json!({
        "format": format.as_str(),
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"},
        "discovery": {
            "request": {
                "method": "GET",
                "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
            },
            "extract": {"mode": "array", "array_at": ["data"], "id_field": ["id"]}
        }
    }))
    .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"));

    insert_definition(database, name, payload).await
}

/// Inserts one OAuth definition version 1 whose refresh grant fails against an unreachable
/// endpoint, for the pre-dispatch refresh-failure scenario.
///
/// # Panics
///
/// Panics if the insert fails or the fixture fails validation — neither can occur for the fixed
/// fixture payload.
pub(crate) async fn insert_failing_refresh_definition(
    database: &Database,
    name: &str,
) -> ProviderDefinition {
    let payload = oauth_definition_payload(OAuthDefinitionFixture {
        format: UpstreamFormat::GeminiGenerateContent,
        inference_suffix: None,
        client_id: "fixture-client",
        initial_slots: &[("project", "uuid_hex")],
        grant: OAuthGrantFixture::AuthorizationCode,
        exchange_url: FIXTURE_TOKEN_URL,
        response_fields: vec![OAuthResponseField::new(
            "refresh_token",
            OAuthTarget::RefreshToken,
        )],
        terminal_codes: &["invalid_grant"],
        refresh: OAuthRefreshFixture::Grant {
            url: FIXTURE_TOKEN_URL,
            response: None,
        },
        provisioning: None,
    });

    insert_definition(database, name, payload).await
}

/// Inserts one provider row of the given definition and credential, returning the stored row.
///
/// # Panics
///
/// Panics if the insert fails, or if the fixture credentials disagree with the definition's
/// credential kind — a test bug the assertion surfaces.
pub(crate) async fn insert_provider(
    database: &Database,
    definition: &ProviderDefinition,
    name: &str,
    model_prefix: &str,
    base_url: Option<&str>,
    credentials: StoredCredentials,
) -> Provider {
    assert_eq!(
        credentials.kind(),
        definition.credential_kind(),
        "fixture credentials must match the definition's credential kind"
    );

    storage::providers::insert_provider(
        database,
        storage::providers::NewProvider {
            name,
            model_prefix,
            base_url,
            credentials: &credentials,
            definition,
            is_enabled: true,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture provider inserts: {error}"))
}

/// Inserts one OAuth definition version 1 — an authorization-code grant whose exchange maps the
/// access token and whose credential never renews — for the flow-begin and completion tests.
///
/// # Panics
///
/// Panics if the insert fails or the fixture fails validation — neither can occur for the fixed
/// fixture payload.
pub(crate) async fn insert_oauth_definition(database: &Database, name: &str) -> ProviderDefinition {
    let payload = oauth_definition_payload(OAuthDefinitionFixture {
        format: UpstreamFormat::OpenAiChatCompletion,
        inference_suffix: Some("/chat/completions"),
        client_id: "fixture-client",
        initial_slots: &[],
        grant: OAuthGrantFixture::AuthorizationCode,
        exchange_url: FIXTURE_TOKEN_URL,
        response_fields: vec![],
        terminal_codes: &["invalid_grant"],
        refresh: OAuthRefreshFixture::None,
        provisioning: None,
    });

    insert_definition(database, name, payload).await
}

/// Validates one fixture payload and inserts it as definition version 1 under `name`.
///
/// # Panics
///
/// Panics if the payload does not validate or the insert fails — neither can occur for the fixed
/// fixture payloads.
async fn insert_definition(
    database: &Database,
    name: &str,
    payload: DefinitionPayload,
) -> ProviderDefinition {
    #[expect(
        clippy::unwrap_used,
        reason = "the fixture definition always validates"
    )]
    payload.validate().unwrap();

    storage::provider_definitions::insert_definition(
        database,
        storage::provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload: &payload,
            created_at: 1,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture definition inserts: {error}"))
}
