//! Provisioning-engine tests: the ordered sequence folding into the working state, the
//! guard-skipped conditional arm, the long-running-operation poll with its done, error, and
//! budget paths, and the completion pipeline's credential product
//! (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or serve is that test's failure report"
)]

use super::{ProvisioningEnv, WorkingState, run_steps};
use crate::{
    provider::{
        ProviderDefinition,
        definition::{DefinitionPayload, eval::ValueEnv},
        oauth::{complete_exchange, execute_exchange, tests_support, write_completion_provider},
        tests_support::failure_detail,
    },
    storage::oauth_flows,
    test_support::{mock_upstream::MockUpstream, temp_database},
    unix_time,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error as StdError};

/// The parsed fixture definition of one provisioning sequence over `steps`.
fn provision_definition(base: &str, steps: Value) -> ProviderDefinition {
    crate::provider::tests_support::definition_named(
        "fixture-definition",
        tests_support::parse(tests_support::with_provisioning(
            tests_support::code_payload(base),
            steps,
        )),
    )
}

/// One plain JSON provisioning step GETting `{base}/step` with the given extractions, merged
/// with `extra` (a guard or a poll).
fn json_step(base: &str, extract: Value, extra: Value) -> Value {
    let mut step = json!({
        "request": {
            "method": "GET",
            "url": [{"type": "literal", "value": format!("{base}/step")}]
        },
        "response_encoding": "json",
        "extract": extract
    });

    #[expect(
        clippy::expect_used,
        reason = "the step fixture is an object by construction"
    )]
    step.as_object_mut()
        .expect("the step fixture is an object")
        .extend(
            extra
                .as_object()
                .expect("the extra fixture is an object")
                .clone(),
        );

    step
}

/// The steps of the parsed `definition`.
fn steps_of(definition: &ProviderDefinition) -> &[super::super::provisioning::ProvisioningStep] {
    definition
        .payload
        .provisioning
        .as_deref()
        .expect("the fixture carries provisioning steps")
}

/// The stage constants of one fixture sequence: the current secret and client identity.
fn constants() -> ProvisioningEnv<'static> {
    ProvisioningEnv {
        secret: Some("at-0"),
        client_id: "client-1",
        client_secret: Some("secret-1"),
    }
}

/// The working state over the given starting slots and no base URL.
fn working_state(slots: BTreeMap<String, String>) -> WorkingState {
    WorkingState::new(slots, None)
}

/// The future absolute expiry one fixture provisioning step reports, in Unix seconds.
fn future_expiry_seconds() -> i64 {
    unix_time::unix_now() + 3600
}

/// Step extractions fold into the working state: named slots, the credential tokens, and the
/// upstream-derived base URL with its provenance mark.
#[tokio::test]
async fn extractions_fold_into_the_working_state() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "project": "p-1",
            "home": "https://home.test",
            "token": "at-9"
        })
        .to_string(),
    ])
    .await;

    let definition = provision_definition(
        &mock.base(),
        json!([json_step(
            &mock.base(),
            json!([
                {"path": ["project"], "target": {"type": "slot", "name": "project"},
                 "shape": {"type": "string"}, "required": true},
                {"path": ["home"], "target": {"type": "base_url"},
                 "shape": {"type": "string"}, "required": true},
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]),
            json!({})
        )]),
    );

    let mut state = working_state(BTreeMap::new());

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await?;

    assert_eq!(state.slots.get("project").map(String::as_str), Some("p-1"));

    assert_eq!(state.access_token.as_deref(), Some("at-9"));

    assert_eq!(state.base_url.as_deref(), Some("https://home.test"));

    assert!(
        state.is_base_url_derived,
        "an extraction is the provenance mark"
    );

    Ok(())
}

/// The `skip_if` guard runs a step only while its named slot is absent: the conditional arm of
/// the sequence.
#[tokio::test]
async fn skip_if_guards_the_conditional_arm() -> Result<(), Box<dyn StdError>> {
    let body = json!({"project": "p-1"}).to_string();

    let (mock, recorded) = MockUpstream::start_json(vec![body.clone(), body]).await;

    let steps = json!([
        {
            "request": {"method": "GET", "url": [
                {"type": "literal", "value": format!("{}/first", mock.base())}
            ]},
            "response_encoding": "json",
            "extract": [],
            "skip_if": "project"
        },
        json_step(&mock.base(), json!([]), json!({}))
    ]);

    let definition = provision_definition(&mock.base(), steps);

    let mut guarded = working_state(BTreeMap::from([("project".to_string(), "p-1".to_string())]));

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut guarded,
    )
    .await?;

    assert_eq!(recorded.lock().expect("the mock records").len(), 1);

    let mut unguarded = working_state(BTreeMap::new());

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut unguarded,
    )
    .await?;

    // The recorder is cumulative: the guarded run's one request plus the unguarded run's two.
    assert_eq!(recorded.lock().expect("the mock records").len(), 3);

    Ok(())
}

/// A missing required extraction fails the step naming the path; a missing optional one leaves
/// its target unchanged.
#[tokio::test]
async fn required_absences_fail_and_optional_ones_skip() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![json!({"other": "x"}).to_string()]).await;

    let definition = provision_definition(
        &mock.base(),
        json!([json_step(
            &mock.base(),
            json!([
                {"path": ["project"], "target": {"type": "slot", "name": "project"},
                 "shape": {"type": "string"}, "required": true},
                {"path": ["absent"], "target": {"type": "slot", "name": "optional"},
                 "shape": {"type": "string"}, "required": false}
            ]),
            json!({})
        )]),
    );

    let mut state = working_state(BTreeMap::new());

    let failure = run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await;

    let detail = failure_detail(failure);

    assert!(detail.contains("required path"), "got: {detail}");

    let definition = provision_definition(
        &mock.base(),
        json!([json_step(
            &mock.base(),
            json!([
                {"path": ["absent"], "target": {"type": "slot", "name": "optional"},
                 "shape": {"type": "string"}, "required": false}
            ]),
            json!({})
        )]),
    );

    let mut state = working_state(BTreeMap::new());

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await?;

    assert!(!state.slots.contains_key("optional"));

    Ok(())
}

/// The absolute-expiry extraction accepts a future Unix timestamp, and rejects a past one and
/// a present value of the wrong shape — consulted expiry evidence must be in the future.
#[tokio::test]
async fn expiry_extractions_must_be_future_integers() -> Result<(), Box<dyn StdError>> {
    let expiry = future_expiry_seconds();

    let (mock, _) = MockUpstream::start_json(vec![json!({"expiry": expiry}).to_string()]).await;

    let definition = provision_definition(
        &mock.base(),
        json!([json_step(
            &mock.base(),
            json!([
                {"path": ["expiry"], "target": {"type": "expires_at_unix_seconds"},
                 "shape": {"type": "integer"}, "required": true}
            ]),
            json!({})
        )]),
    );

    let mut state = working_state(BTreeMap::new());

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await?;

    assert_eq!(state.expires_at, Some(expiry * unix_time::MICROS_PER_SEC));

    for (payload, fragment) in [
        (json!({"expiry": 1_000_000}), "must be in the future"),
        (json!({"expiry": "soon"}), "must be an integer"),
    ] {
        let (mock, _) = MockUpstream::start_json(vec![payload.to_string()]).await;

        let definition = provision_definition(
            &mock.base(),
            json!([json_step(
                &mock.base(),
                json!([
                    {"path": ["expiry"], "target": {"type": "expires_at_unix_seconds"},
                     "shape": {"type": "integer"}, "required": true}
                ]),
                json!({})
            )]),
        );

        let mut state = working_state(BTreeMap::new());

        let failure = run_steps(
            &reqwest::Client::new(),
            steps_of(&definition),
            &constants(),
            &mut state,
        )
        .await;

        let detail = failure_detail(failure);

        assert!(detail.contains(fragment), "got: {detail}");
    }

    Ok(())
}

/// The poll repeats until its done path turns true, then folds the completed operation's
/// result extractions into the working state.
#[tokio::test]
async fn polls_complete_on_the_done_path() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![
        json!({"done": false}).to_string(),
        json!({"done": true, "region": "eu"}).to_string(),
    ])
    .await;

    let step = json_step(
        &mock.base(),
        json!([]),
        json!({
            "poll": {
                "request": {"method": "GET", "url": [
                    {"type": "literal", "value": format!("{}/poll", mock.base())}
                ]},
                "response_encoding": "json",
                "done_path": ["done"],
                "error_path": [],
                "result_extract": [
                    {"path": ["region"], "target": {"type": "slot", "name": "region"},
                     "shape": {"type": "string"}, "required": true}
                ],
                "interval_ms": 1,
                "budget_ms": 5000
            }
        }),
    );

    let definition = provision_definition(&mock.base(), json!([step]));

    let mut state = working_state(BTreeMap::new());

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await?;

    assert_eq!(state.slots.get("region").map(String::as_str), Some("eu"));

    assert_eq!(recorded.lock().expect("the mock records").len(), 2);

    Ok(())
}

/// A completed operation carrying a terminal failure payload fails the step with the
/// gateway-authored message; the payload itself never reaches the error.
#[tokio::test]
async fn poll_error_payloads_fail_the_step() -> Result<(), Box<dyn StdError>> {
    let payload =
        json!({"done": true, "error": {"status": "failed", "message": "boom"}}).to_string();

    let (mock, _) = MockUpstream::start_json(vec![payload]).await;

    let step = json_step(
        &mock.base(),
        json!([]),
        json!({
            "poll": {
                "request": {"method": "GET", "url": [
                    {"type": "literal", "value": format!("{}/poll", mock.base())}
                ]},
                "response_encoding": "json",
                "done_path": ["done"],
                "error_path": ["error"],
                "interval_ms": 1,
                "budget_ms": 1000
            }
        }),
    );

    let definition = provision_definition(&mock.base(), json!([step]));

    let mut state = working_state(BTreeMap::new());

    let failure = run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await;

    let detail = failure_detail(failure);

    assert!(detail.contains("failed upstream"), "got: {detail}");

    assert!(
        !detail.contains("boom"),
        "the upstream payload stays out of the error"
    );

    Ok(())
}

/// A poll whose done path never turns true fails at its wall-clock budget.
#[tokio::test]
async fn poll_budget_exhaustion_fails_the_step() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![json!({"done": false}).to_string()]).await;

    let step = json_step(
        &mock.base(),
        json!([]),
        json!({
            "poll": {
                "request": {"method": "GET", "url": [
                    {"type": "literal", "value": format!("{}/poll", mock.base())}
                ]},
                "response_encoding": "json",
                "done_path": ["done"],
                "error_path": [],
                "interval_ms": 5,
                "budget_ms": 40
            }
        }),
    );

    let definition = provision_definition(&mock.base(), json!([step]));

    let mut state = working_state(BTreeMap::new());

    let failure = run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await;

    let detail = failure_detail(failure);

    assert!(detail.contains("did not complete in time"), "got: {detail}");

    Ok(())
}

/// A form-encoded poll reads its done path in the string spellings and folds the flat fields.
#[tokio::test]
async fn form_polls_read_the_string_spellings() -> Result<(), Box<dyn StdError>> {
    let body = "done=true&project=p-2";

    let (mock, _) = MockUpstream::start_raw(vec![format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )])
    .await;

    // The step itself reads the same form body the mock serves to its poll.
    let mut step = json_step(
        &mock.base(),
        json!([]),
        json!({
            "poll": {
                "request": {"method": "GET", "url": [
                    {"type": "literal", "value": format!("{}/poll", mock.base())}
                ]},
                "response_encoding": "form",
                "done_path": ["done"],
                "error_path": [],
                "result_extract": [
                    {"path": ["project"], "target": {"type": "slot", "name": "project"},
                     "shape": {"type": "string"}, "required": true}
                ],
                "interval_ms": 1,
                "budget_ms": 1000
            }
        }),
    );

    #[expect(
        clippy::expect_used,
        reason = "the step fixture is an object by construction"
    )]
    step.as_object_mut()
        .expect("the step fixture is an object")
        .insert("response_encoding".to_string(), serde_json::json!("form"));

    let definition = provision_definition(&mock.base(), json!([step]));

    let mut state = working_state(BTreeMap::new());

    run_steps(
        &reqwest::Client::new(),
        steps_of(&definition),
        &constants(),
        &mut state,
    )
    .await?;

    assert_eq!(state.slots.get("project").map(String::as_str), Some("p-2"));

    Ok(())
}

/// The completion pipeline: the exchange folds through provisioning into the stored
/// credentials — a provisioning-supplied token and absolute expiry win over the response's own
/// token evidence, and the provider write lands with the flow's product.
#[tokio::test]
async fn completion_folds_provisioning_into_the_provider_write() -> Result<(), Box<dyn StdError>> {
    let expiry = future_expiry_seconds();

    let (mock, recorded) = MockUpstream::start_json(vec![
        json!({"access_token": "at-2", "refresh_token": "rt-1", "project": "p-1"}).to_string(),
        json!({"token": "at-3", "expiry": expiry}).to_string(),
    ])
    .await;

    let payload: DefinitionPayload =
        tests_support::parse(tests_support::provisioned_token_payload(&mock.base()));

    let definition =
        crate::provider::tests_support::definition_named("fixture-definition", payload);

    let (database, _guard) = temp_database().await;

    // The flow row references the stored definition version.
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

    let flow = oauth_flows::insert_flow(&database, fixture_flow(&definition)).await?;

    let flow_slots = flow.slots.clone();

    let env = exchange_env(&flow_slots);

    let outcome =
        match execute_exchange(&reqwest::Client::new(), &recipe_exchange(&definition), &env).await?
        {
            crate::provider::oauth::exchange::ExchangeProduct::Completed(outcome) => outcome,

            _ => return Err("the fixture exchange completes".into()),
        };

    let product = complete_exchange(&reqwest::Client::new(), &definition, outcome, &flow).await?;

    let mut transaction = database.transaction().await?;

    let provider_id = write_completion_provider(&mut transaction, &flow, &definition, &product)
        .await?
        .expect("the create flow writes its provider");

    transaction.commit().await?;

    let stored = crate::storage::providers::find_provider_by_id(&database, provider_id)
        .await?
        .expect("the provider was just written");

    let crate::provider::StoredCredentials::OAuth {
        access_token,
        refresh_token,
        slots,
        expires_at,
        refresh_started_at,
    } = &stored.credentials
    else {
        return Err("the completed flow stores OAuth credentials".into());
    };

    // The provisioning-extracted token and absolute expiry win over the response evidence, the
    // flow-carried slot stays aboard, and the exchange's slot rides along.
    assert_eq!(access_token, "at-3");

    // The provisioning step authenticated with the exchange's own token.
    let requests = recorded.lock().expect("the mock records");

    let request = requests.last().expect("the provisioning request");

    assert_eq!(request.header("authorization"), Some("at-2"));

    assert_eq!(*expires_at, Some(expiry * unix_time::MICROS_PER_SEC));

    assert_eq!(refresh_token.as_deref(), Some("rt-1"));

    assert_eq!(slots.get("project").map(String::as_str), Some("p-1"));

    assert_eq!(slots.get("carried").map(String::as_str), Some("kept"));

    assert_eq!(*refresh_started_at, None);

    Ok(())
}

/// The exchange environment over the given flow slots: the fixture grant's code and redirect.
fn exchange_env(slots: &BTreeMap<String, String>) -> ValueEnv<'_> {
    ValueEnv::for_exchange(
        slots,
        Some("https://flow-base.test"),
        "client-1",
        Some("secret-1"),
        "code-1",
        None,
        "http://localhost:1455/callback",
    )
}

/// The fixture recipe's exchange request.
fn recipe_exchange(
    definition: &ProviderDefinition,
) -> crate::provider::definition::credential::Exchange {
    definition
        .oauth_recipe()
        .expect("the fixture recipe is OAuth")
        .exchange
        .clone()
}

/// The fixture create flow row of the authorization-code grant.
fn fixture_flow(definition: &ProviderDefinition) -> oauth_flows::NewFlow {
    #[expect(
        clippy::expect_used,
        reason = "the fixture version is a nonzero literal"
    )]
    let version = std::num::NonZeroI64::new(1).expect("fixture versions are nonzero literals");

    oauth_flows::NewFlow {
        definition_name: definition.name.clone(),
        definition_version: version,
        state_token: "fixture-state".to_string(),
        purpose: oauth_flows::FlowPurpose::Create,
        provider_name: Some("fixture-provider".to_string()),
        model_prefix: Some("fixture.".to_string()),
        base_url: Some("https://flow-base.test".to_string()),
        target_provider_id: None,
        grant_type: oauth_flows::FlowGrant::AuthorizationCode,
        slots: BTreeMap::from([("carried".to_string(), "kept".to_string())]),
        code_verifier: None,
        device_code: None,
        device_user_code: None,
        device_verification_uri: None,
        device_verification_uri_complete: None,
        device_poll_interval_seconds: None,
        device_next_poll_at: None,
        expires_at: unix_time::unix_now_micros() + 600 * unix_time::MICROS_PER_SEC,
    }
}
