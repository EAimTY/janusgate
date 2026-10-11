//! Flow-begin tests: the authorization-code authorize-URL construction over the persisted flow
//! row, the device-authorization stage against the mock upstream, and the slot initializers
//! (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or serve is that test's failure report"
)]

use super::{BeginFacts, BeginPurpose, begin_flow, initialize_slots};
use crate::{
    provider::{
        ProviderDefinition,
        definition::credential::{Grant, OAuthRecipe},
        tests_support::failure_detail,
    },
    test_support::{mock_upstream::MockUpstream, temp_database},
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeMap, error::Error as StdError};

/// The purpose of one fixture begin: a create flow naming `fixture-provider`.
fn create_purpose() -> BeginPurpose {
    BeginPurpose::Create {
        provider_name: "fixture-provider".to_string(),
        model_prefix: "fixture.".to_string(),
    }
}

/// The recipe's OAuth grant of the fixture definition, destructured for assertions.
fn grant_of(definition: &ProviderDefinition) -> &Grant {
    let OAuthRecipe { grant, .. } = definition
        .oauth_recipe()
        .expect("the fixture recipe is OAuth");

    grant
}

/// Patches the fixture code payload's grant fields.
fn with_grant(patch: Value) -> Value {
    let mut payload = json!({
        "type": "authorization_code",
        "authorize_url": [{"type": "literal", "value": "https://up.test/authorize"}],
        "redirect_uri": "http://localhost:1455/callback",
        "pkce": "none"
    });

    #[expect(clippy::expect_used, reason = "the patch is an object by construction")]
    payload
        .as_object_mut()
        .expect("the patch is an object")
        .extend(patch.as_object().expect("the patch is an object").clone());

    payload
}

/// Parses the authorization-code recipe whose grant carries `grant_patch` and whose exchange
/// and refresh sections address the mock upstream.
fn code_definition(grant_patch: Value) -> ProviderDefinition {
    crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::oauth::tests_support::parse(
            crate::provider::oauth::tests_support::code_payload("http://127.0.0.1:1")
                .tap_grant(grant_patch),
        ),
    )
}

/// Stores the authorization-code recipe whose grant carries `grant_patch`, returning the
/// stored definition beside the database the flow row references it through.
async fn stored_code_definition(
    grant_patch: Value,
) -> Result<
    (
        ProviderDefinition,
        crate::database::Database,
        crate::test_support::TempDatabaseGuard,
    ),
    Box<dyn StdError>,
> {
    let (database, guard) = temp_database().await;

    let definition = code_definition(grant_patch);

    crate::storage::provider_definitions::insert_definition(
        &database,
        crate::storage::provider_definitions::NewDefinition {
            name: definition.name.as_str(),
            base_version: None,
            payload: &definition.payload,
            created_at: definition.created_at,
        },
    )
    .await?;

    Ok((definition, database, guard))
}

/// Replaces the grant member of one parsed payload JSON.
trait TapGrant {
    /// Replaces `/credential/grant` with `patch`.
    fn tap_grant(self, patch: Value) -> Self;
}

impl TapGrant for Value {
    fn tap_grant(mut self, patch: Value) -> Self {
        *self
            .pointer_mut("/credential/grant")
            .expect("the payload carries the grant") = patch;

        self
    }
}

/// The PKCE S256 challenge of the fixture verifier (RFC 7636 §4.2).
fn challenge_of(verifier: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Asserts the flow's lifetime sits within `low_seconds`..`high_seconds` of now.
fn assert_expires_within(
    flow: &super::super::flow::BeganFlow,
    low_seconds: i64,
    high_seconds: i64,
) {
    let remaining = flow.flow.expires_at - unix_time::unix_now_micros();

    assert!(
        remaining > low_seconds * unix_time::MICROS_PER_SEC
            && remaining < high_seconds * unix_time::MICROS_PER_SEC,
        "the flow lifetime is {remaining} micros from now"
    );
}

/// The authorization-code begin builds the full authorize URL — standard parameters in order,
/// the S256 challenge pair for a PKCE grant, extra parameters, and the state token last — and
/// persists the flow row carrying the verifier and the ten-minute lifetime.
#[tokio::test]
async fn code_begin_builds_the_url_and_persists_the_flow() -> Result<(), Box<dyn StdError>> {
    let grant = with_grant(json!({
        "pkce": "s256",
        "extra_params": [{"name": "scope", "value": [{"type": "literal", "value": "read write"}]}]
    }));

    let (definition, database, _guard) = stored_code_definition(grant).await?;

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        None,
        BTreeMap::new(),
    )
    .await?;

    let BeginFacts::AuthorizationCode { authorize_url } = &began.facts else {
        panic!("an authorization-code begin carries the authorize URL");
    };

    let Grant::AuthorizationCode { redirect_uri, .. } = grant_of(&definition) else {
        panic!("the fixture grant is an authorization code");
    };

    let verifier = began
        .flow
        .code_verifier
        .as_deref()
        .expect("the S256 begin mints the verifier");

    let expected = format!(
        "https://up.test/authorize?response_type=code&client_id=client-1&redirect_uri={}&\
         code_challenge={}&code_challenge_method=S256&scope=read%20write&state={}",
        crate::provider::control::percent_encode_component(redirect_uri),
        challenge_of(verifier),
        began.flow.state_token
    );

    assert_eq!(authorize_url, &expected);

    assert_eq!(
        began.flow.grant_type,
        crate::storage::oauth_flows::FlowGrant::AuthorizationCode
    );

    assert_expires_within(&began, 599, 601);

    Ok(())
}

/// The no-PKCE begin appends no challenge parameters and stores no verifier.
#[tokio::test]
async fn code_begin_without_pkce_omits_the_challenge() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) =
        stored_code_definition(with_grant(json!({"pkce": "none"}))).await?;

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        None,
        BTreeMap::new(),
    )
    .await?;

    let BeginFacts::AuthorizationCode { authorize_url } = &began.facts else {
        panic!("an authorization-code begin carries the authorize URL");
    };

    assert!(authorize_url.contains("response_type=code"));

    assert!(!authorize_url.contains("code_challenge"));

    assert!(began.flow.code_verifier.is_none());

    Ok(())
}

/// Query rows join ahead of a fragment: a stored authorize base carrying a fragment keeps every
/// authorization parameter in the query component.
#[tokio::test]
async fn authorize_parameters_precede_a_fragment() -> Result<(), Box<dyn StdError>> {
    let (definition, database, _guard) = stored_code_definition(with_grant(json!({
        "authorize_url": [{"type": "literal", "value": "https://up.test/authorize#consent"}]
    })))
    .await?;

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        None,
        BTreeMap::new(),
    )
    .await?;

    let BeginFacts::AuthorizationCode { authorize_url } = &began.facts else {
        panic!("an authorization-code begin carries the authorize URL");
    };

    assert!(
        authorize_url.starts_with("https://up.test/authorize?response_type=code")
            && authorize_url.contains(&format!("&state={}", began.flow.state_token))
            && authorize_url.ends_with("#consent"),
        "the fragment stays behind every parameter: {authorize_url}"
    );

    Ok(())
}

/// The device begin executes the configured authorization request against the mock upstream
/// and persists the reported codes and poll schedule.
#[tokio::test]
async fn device_begin_persists_the_reported_schedule() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![
        json!({
            "device_code": "dev-1",
            "user_code": "ABCD-1234",
            "verification_uri": "https://up.test/device",
            "expires_in": 600,
            "interval": 3
        })
        .to_string(),
    ])
    .await;

    let (database, _guard) = temp_database().await;

    let definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::oauth::tests_support::parse(
            crate::provider::oauth::tests_support::device_payload(&mock.base()),
        ),
    );

    crate::storage::provider_definitions::insert_definition(
        &database,
        crate::storage::provider_definitions::NewDefinition {
            name: definition.name.as_str(),
            base_version: None,
            payload: &definition.payload,
            created_at: definition.created_at,
        },
    )
    .await?;

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        Some("https://flow-base.test".to_string()),
        BTreeMap::new(),
    )
    .await?;

    let BeginFacts::Device {
        user_code,
        verification_uri,
        ..
    } = &began.facts
    else {
        panic!("a device begin carries the verification facts");
    };

    assert_eq!(user_code, "ABCD-1234");

    assert_eq!(verification_uri.as_deref(), Some("https://up.test/device"));

    assert_eq!(began.flow.device_code.as_deref(), Some("dev-1"));

    assert_eq!(began.flow.device_poll_interval_seconds, Some(3));

    assert_expires_within(&began, 599, 601);

    let next_poll_in = began
        .flow
        .device_next_poll_at
        .expect("the begin schedules the first poll")
        - unix_time::unix_now_micros();

    assert!(
        next_poll_in > 2 * unix_time::MICROS_PER_SEC
            && next_poll_in < 4 * unix_time::MICROS_PER_SEC
    );

    let requests = recorded.lock().expect("the mock records");

    let request = requests.first().expect("one request");

    assert_eq!(request.method, "POST");

    assert_eq!(request.target, "/device");

    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );

    assert!(request.body.contains("client_id=client-1"));

    Ok(())
}

/// The device begin applies the documented fallbacks: a missing `interval` means five seconds
/// and a missing `expires_in` means the definition's fallback lifetime.
#[tokio::test]
async fn device_begin_applies_the_documented_fallbacks() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "device_code": "dev-2",
            "user_code": "EFGH-5678",
            "verification_uri": "https://up.test/device",
            "verification_uri_complete": "https://up.test/device?code=EFGH-5678"
        })
        .to_string(),
    ])
    .await;

    let (database, _guard) = temp_database().await;

    let definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::oauth::tests_support::parse(
            crate::provider::oauth::tests_support::device_payload(&mock.base()),
        ),
    );

    crate::storage::provider_definitions::insert_definition(
        &database,
        crate::storage::provider_definitions::NewDefinition {
            name: definition.name.as_str(),
            base_version: None,
            payload: &definition.payload,
            created_at: definition.created_at,
        },
    )
    .await?;

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        None,
        BTreeMap::new(),
    )
    .await?;

    assert_eq!(began.flow.device_poll_interval_seconds, Some(5));

    let remaining = began.flow.expires_at - unix_time::unix_now_micros();

    assert!(
        remaining > 899 * unix_time::MICROS_PER_SEC && remaining < 901 * unix_time::MICROS_PER_SEC
    );

    let BeginFacts::Device {
        verification_uri,
        verification_uri_complete,
        ..
    } = &began.facts
    else {
        panic!("a device begin carries the verification facts");
    };

    assert_eq!(verification_uri.as_deref(), Some("https://up.test/device"));

    assert_eq!(
        verification_uri_complete.as_deref(),
        Some("https://up.test/device?code=EFGH-5678")
    );

    Ok(())
}

/// A device authorization response with a non-integer or non-positive duration is malformed
/// (RFC 8628 §3.2): the begin fails instead of scheduling a dead or spin-polling flow.
#[tokio::test]
async fn device_begin_rejects_malformed_durations() -> Result<(), Box<dyn StdError>> {
    for (interval, fragment) in [
        (json!(0), "must be positive"),
        (json!(-2), "must be positive"),
        (json!("soon"), "non-integer"),
    ] {
        let (mock, _) = MockUpstream::start_json(vec![
            json!({
                "device_code": "dev-3",
                "user_code": "IJKL-9012",
                "verification_uri": "https://up.test/device",
                "interval": interval
            })
            .to_string(),
        ])
        .await;

        let (database, _guard) = temp_database().await;

        let definition = crate::provider::tests_support::definition_named(
            "fixture-definition",
            crate::provider::oauth::tests_support::parse(
                crate::provider::oauth::tests_support::device_payload(&mock.base()),
            ),
        );

        let began = begin_flow(
            &database,
            &reqwest::Client::new(),
            &definition,
            create_purpose(),
            None,
            BTreeMap::new(),
        )
        .await;

        let detail = failure_detail(began);

        assert!(detail.contains(fragment), "got: {detail}");
    }

    Ok(())
}

/// A device authorization response carrying neither verification URI target is malformed: the
/// administrator has nothing to approve through.
#[tokio::test]
async fn device_begin_requires_a_verification_uri() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "device_code": "dev-4",
            "user_code": "MNOP-3456"
        })
        .to_string(),
    ])
    .await;

    let (database, _guard) = temp_database().await;

    let definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::oauth::tests_support::parse(
            crate::provider::oauth::tests_support::device_payload(&mock.base()),
        ),
    );

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        None,
        BTreeMap::new(),
    )
    .await;

    let detail = failure_detail(began);

    assert!(detail.contains("lacks a verification URI"), "got: {detail}");

    Ok(())
}

/// A device authorization response missing a required mapped value fails the begin with the
/// gateway-owned message naming the value.
#[tokio::test]
async fn device_begin_reports_a_missing_device_code() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "user_code": "QRST-7890",
            "verification_uri": "https://up.test/device"
        })
        .to_string(),
    ])
    .await;

    let (database, _guard) = temp_database().await;

    let definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::oauth::tests_support::parse(
            crate::provider::oauth::tests_support::device_payload(&mock.base()),
        ),
    );

    let began = begin_flow(
        &database,
        &reqwest::Client::new(),
        &definition,
        create_purpose(),
        None,
        BTreeMap::new(),
    )
    .await;

    let detail = failure_detail(began);

    assert!(
        detail.contains("lacks a `device_code` value"),
        "got: {detail}"
    );

    Ok(())
}

/// The slot initializers run once per new flow, fill only the missing names over the stored
/// slots, and render the UUID generator's two spellings; an API-key definition is stored drift.
#[test]
fn initial_slots_fill_only_the_missing_names() {
    let payload = crate::provider::oauth::tests_support::parse(
        crate::provider::oauth::tests_support::code_payload("http://127.0.0.1:1"),
    );

    let mut payload = payload;

    payload.credential = serde_json::from_value(json!({"type": "oauth",
        "client_id": "client-1",
        "client_secret": "s",
        "grant": {"type": "authorization_code",
                  "authorize_url": [{"type": "literal", "value": "https://up.test/authorize"}],
                  "redirect_uri": "http://localhost:1455/callback",
                  "pkce": "none"},
        "exchange": {
            "request": {"method": "POST",
             "url": [{"type": "literal", "value": "https://up.test/token"}],
                        "body": {"kind": "form", "fields": [
                            {"name": "code", "value": [{"type": "authorization_code"}]},
                            {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
                            {"name": "client_id", "value": [{"type": "client_id"}]},
                            {"name": "client_secret", "value": [{"type": "client_secret"}]}
                        ]}},
            "response": {"encoding": "json", "fields": [
                {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
                {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true}
            ]},
            "terminal_codes": ["invalid_grant"]
        },
        "refresh": {"type": "none"},
        "initial_slots": [
            {"name": "device", "generator": "uuid_hex"},
            {"name": "session", "generator": "uuid"}
        ]
    }))
    .expect("the recipe fixture parses");

    let definition =
        crate::provider::tests_support::definition_named("fixture-definition", payload);

    let stored = BTreeMap::from([("device".to_string(), "kept".to_string())]);

    let initialized = initialize_slots(&definition, &stored).expect("the recipe is OAuth");

    assert_eq!(initialized.get("device").map(String::as_str), Some("kept"));

    let session = initialized
        .get("session")
        .expect("the missing slot is filled");

    assert_eq!(
        session.len(),
        36,
        "the uuid generator renders the hyphenated form"
    );

    let hex_definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::oauth::tests_support::parse(
            crate::provider::oauth::tests_support::code_payload("http://127.0.0.1:1"),
        ),
    );

    // The canonical fixture recipe configures no initial slots: its stored slots pass through.
    let passthrough = initialize_slots(&hex_definition, &stored).expect("the recipe is OAuth");

    assert_eq!(passthrough, stored);

    let api_key_definition = crate::provider::tests_support::definition_named(
        "fixture-definition",
        crate::provider::tests_support::parse_payload(serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": [{"type": "literal", "value": "https://up.test/v1/chat/completions"}],
                "upstream_streaming": "match_client"
            },
            "credential": {"type": "api_key"}
        })),
    );

    let drift = initialize_slots(&api_key_definition, &stored);

    assert!(drift.is_err(), "an api-key definition is stored drift");
}
