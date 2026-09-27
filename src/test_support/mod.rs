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
        FIXTURE_TOKEN_URL, OAuthDefinitionFixture, OAuthDeviceAuthorizationFixture,
        OAuthGrantFixture, OAuthRefreshFixture, OAuthResponseField, OAuthTarget,
        oauth_definition_payload,
    },
};
use crate::{
    database::Database,
    format::UpstreamFormat,
    provider::{Provider, ProviderDefinition, StoredCredentials, definition::DefinitionPayload},
    storage,
    storage::oauth_flows::{FlowGrant, FlowPurpose, NewFlow, OngoingFlow, insert_flow},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, num::NonZero, path::PathBuf, time::Duration};

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

/// The delta one flow-seeding test applies over the shared device-flow fixture base of
/// [`insert_flow_fixture`]: every field left `None` keeps the base value, and an override
/// replaces it whole. The two doubly wrapped fields carry a base the tests must clear (a
/// reauthorize flow names no provider), so their inner `Option` distinguishes "keep the base"
/// from "clear the field".
#[derive(Default)]
pub(crate) struct FlowFixtureDelta {
    /// Overrides the base state token `"state-token"`.
    pub(crate) state_token: Option<String>,

    /// Overrides the base purpose [`FlowPurpose::Create`].
    pub(crate) purpose: Option<FlowPurpose>,

    /// Overrides the base provider name `Some("created")`; `Some(None)` clears it.
    pub(crate) provider_name: Option<Option<String>>,

    /// Overrides the base model prefix `Some("")`; `Some(None)` clears it.
    pub(crate) model_prefix: Option<Option<String>>,

    /// Overrides the base target provider id `None`.
    pub(crate) target_provider_id: Option<i64>,

    /// Overrides the base device code `"device-code"`.
    pub(crate) device_code: Option<String>,

    /// Overrides the base user code `"user-code"`.
    pub(crate) device_user_code: Option<String>,

    /// Overrides the base stored poll interval of `5` seconds.
    pub(crate) device_poll_interval_seconds: Option<i64>,

    /// Overrides the base next-poll instant `now - 1` — the due-now schedule.
    pub(crate) device_next_poll_at: Option<i64>,

    /// Overrides the base expiry `now + 600` seconds — the ten-minute TTL.
    pub(crate) expires_at: Option<i64>,
}

/// Inserts one device-grant flow row against `definition` and returns the stored row: the
/// fixture base every flow-seeding test shares — a [`FlowPurpose::Create`] flow for a
/// `"created"` provider under the `"state-token"` token, pending with the 5-second interval,
/// due at `now - 1`, and expiring at `now + 600` seconds — narrowed by `delta`.
///
/// # Panics
///
/// Panics if the insert fails — the fixture row always satisfies the schema.
pub(crate) async fn insert_flow_fixture(
    database: &Database,
    definition: &ProviderDefinition,
    now: i64,
    delta: FlowFixtureDelta,
) -> OngoingFlow {
    let FlowFixtureDelta {
        state_token,
        purpose,
        provider_name,
        model_prefix,
        target_provider_id,
        device_code,
        device_user_code,
        device_poll_interval_seconds,
        device_next_poll_at,
        expires_at,
    } = delta;

    insert_flow(
        database,
        NewFlow {
            definition_name: definition.name.clone(),
            definition_version: definition.version,
            state_token: state_token.unwrap_or_else(|| "state-token".to_string()),
            purpose: purpose.unwrap_or(FlowPurpose::Create),
            provider_name: provider_name.unwrap_or_else(|| Some("created".to_string())),
            model_prefix: model_prefix.unwrap_or_else(|| Some(String::new())),
            base_url: None,
            target_provider_id,
            grant_type: FlowGrant::Device,
            slots: BTreeMap::new(),
            code_verifier: None,
            device_code: Some(device_code.unwrap_or_else(|| "device-code".to_string())),
            device_user_code: Some(device_user_code.unwrap_or_else(|| "user-code".to_string())),
            device_verification_uri: Some("https://verify.example".to_string()),
            device_verification_uri_complete: None,
            device_poll_interval_seconds: Some(device_poll_interval_seconds.unwrap_or(5)),
            device_next_poll_at: Some(device_next_poll_at.unwrap_or(now - 1)),
            expires_at: expires_at.unwrap_or(now + 600 * crate::unix_time::MICROS_PER_SEC),
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture flow inserts: {error}"))
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
