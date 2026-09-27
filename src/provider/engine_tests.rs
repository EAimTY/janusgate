//! Engine golden-path tests against a local mock upstream (README §5.3–§5.6):
//! discovery pagination, provisioning with a long-running operation poll, device-flow
//! classification through the shared control executor, and the shared OAuth completion
//! pipeline.

#![expect(
    clippy::unwrap_used,
    reason = "the fixtures are constructed so each unwrap lands on its documented side"
)]
#![expect(
    clippy::panic,
    reason = "a fixture landing on the wrong side of its check is the failure report"
)]

use crate::{
    database::Database,
    format::UpstreamFormat,
    provider::{
        Error, StoredCredentials,
        definition::{discovery::Discovery, eval::ValueEnv},
        oauth::{self, ProvisioningEnv, WorkingState, run_steps},
    },
    storage,
    test_support::{
        MockUpstream, OAuthDefinitionFixture, OAuthGrantFixture, OAuthRefreshFixture,
        OAuthResponseField, OAuthTarget, create_test_database, insert_provider,
        oauth_definition_payload,
    },
};
use serde_json::json;
use std::collections::BTreeMap;

/// Pins discovery pagination: the token loop walks both pages, strips prefixes, and stops when
/// the next-page token is absent.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the walk diverges from the expectation.
#[tokio::test]
async fn discovery_walks_token_paginated_lists() {
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({
            "data": [{"id": "model-a"}, {"id": "model-b"}],
            "nextPageToken": "page-2"
        }))
        .unwrap(),
        serde_json::to_string(&json!({
            "data": [{"id": "models/model-c"}],
        }))
        .unwrap(),
    ])
    .await;

    let discovery: Discovery = serde_json::from_value(json!({
        "request": {
            "method": "GET",
            "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
        },
        "extract": {
            "mode": "array",
            "array_at": ["data"],
            "id_field": ["id"],
            "strip_prefix": "models/"
        },
        "pagination": {
            "type": "token",
            "request_param": "pageToken",
            "response_path": ["nextPageToken"],
            "max_pages": 5
        }
    }))
    .unwrap();

    let client = crate::http_client::control_plane();
    let env = discovery_env(&mock.base());

    let ids = crate::provider::discovery::run_discovery(&client, &discovery, &env)
        .await
        .unwrap();

    assert_eq!(ids, vec!["model-a", "model-b", "model-c"]);
    mock.shutdown.send(()).ok();
}

/// Pins the provisioning engine: the conditional step is skipped, the poll
/// repeats until done, and the extractions fold into the working state.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the sequence diverges from the expectation.
#[tokio::test]
async fn provisioning_runs_steps_and_polls_the_operation() {
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({"done": false})).unwrap(),
        serde_json::to_string(&json!({"done": true, "response": {"project": {"id": "proj-1"}}}))
            .unwrap(),
    ])
    .await;

    let steps: Vec<crate::provider::definition::provisioning::ProvisioningStep> =
        serde_json::from_value(json!([
            {
                "request": {
                    "method": "POST",
                    "url": [{"type": "base"}, {"type": "literal", "value": "/skip"}]
                },
                "response_encoding": "json",
                "skip_if": "already"
            },
            {
                "request": {
                    "method": "POST",
                    "url": [{"type": "base"}, {"type": "literal", "value": "/onboard"}]
                },
                "response_encoding": "json",
                "poll": {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "base"}, {"type": "literal", "value": "/onboard"}]
                    },
                    "response_encoding": "json",
                    "done_path": ["done"],
                    "error_path": ["error"],
                    "result_extract": [
                        {
                            "path": ["response", "project"],
                            "target": {"type": "slot", "name": "project"},
                            "shape": {"type": "string_or_field", "field": "id"},
                            "required": true
                        }
                    ],
                    "interval_ms": 1,
                    "budget_ms": 5000
                }
            }
        ]))
        .unwrap();

    let client = crate::http_client::control_plane();

    let mut slots = BTreeMap::new();
    slots.insert("already".to_string(), "present".to_string());

    let base_url = Some(mock.base());

    let mut state = WorkingState::new(slots, base_url.clone());

    let constants = ProvisioningEnv {
        secret: Some("test-secret"),
        client_id: "test-client-id",
        client_secret: None,
    };

    run_steps(&client, &steps, &constants, &mut state)
        .await
        .unwrap();

    assert_eq!(
        state.slots.get("project").map(String::as_str),
        Some("proj-1")
    );
    mock.shutdown.send(()).ok();
}

/// Pins the flat form encoding of a provisioning poll (README §5.1, §5.4): the done path names
/// the single form member carrying the done marker, a non-truthy value keeps the poll running,
/// both truthy spellings — `true` and `1`, the string renderings of the truthy scalars the JSON
/// rule accepts — complete it, and the completed response's `result_extract` folds its flat
/// fields into the working state.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the poll diverges from the expectation.
#[tokio::test]
async fn provisioning_polls_a_form_encoded_operation_to_completion() {
    for done in ["true", "1"] {
        let mock = MockUpstream::start(vec![
            // The step's own response, itself form-encoded and carrying no extractions.
            "accepted=1".to_string(),
            // The first poll attempt is still running; the second completes, spelling the done
            // marker `done` and carrying the flat member `result_extract` folds.
            "done=false".to_string(),
            format!("done={done}&project_id=proj-1"),
        ])
        .await;

        let steps: Vec<crate::provider::definition::provisioning::ProvisioningStep> =
            serde_json::from_value(json!([
                {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "base"}, {"type": "literal", "value": "/onboard"}]
                    },
                    "response_encoding": "form",
                    "poll": {
                        "request": {
                            "method": "POST",
                            "url": [{"type": "base"}, {"type": "literal", "value": "/onboard"}]
                        },
                        "response_encoding": "form",
                        "done_path": ["done"],
                        "error_path": ["error"],
                        "result_extract": [
                            {
                                "path": ["project_id"],
                                "target": {"type": "slot", "name": "project"},
                                "shape": {"type": "string"},
                                "required": true
                            }
                        ],
                        "interval_ms": 1,
                        "budget_ms": 5000
                    }
                }
            ]))
            .unwrap();

        let client = crate::http_client::control_plane();

        let mut state = WorkingState::new(BTreeMap::new(), Some(mock.base()));

        let constants = ProvisioningEnv {
            secret: Some("test-secret"),
            client_id: "test-client-id",
            client_secret: None,
        };

        run_steps(&client, &steps, &constants, &mut state)
            .await
            .unwrap();

        assert_eq!(
            state.slots.get("project").map(String::as_str),
            Some("proj-1"),
            "the completed form poll folds its flat result field"
        );

        mock.shutdown.send(()).ok();
    }
}

/// Pins the form-encoded poll's error surface (README §5.4): the one form member the error path
/// names is the terminal failure payload, rendered verbatim after the form decoding, and it
/// preempts a truthy done marker — a response carrying `done=true` alongside its error member
/// fails on the payload, never completes.
///
/// # Panics
///
/// Panics when a fixture lands on the wrong side of its check.
#[tokio::test]
async fn a_form_encoded_poll_surfaces_its_error_member_as_the_failure_payload() {
    let mock = MockUpstream::start(vec![
        "accepted=1".to_string(),
        "done=true&error=operation%20failed".to_string(),
    ])
    .await;

    let steps: Vec<crate::provider::definition::provisioning::ProvisioningStep> =
        serde_json::from_value(json!([
            {
                "request": {
                    "method": "POST",
                    "url": [{"type": "base"}, {"type": "literal", "value": "/onboard"}]
                },
                "response_encoding": "form",
                "poll": {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "base"}, {"type": "literal", "value": "/onboard"}]
                    },
                    "response_encoding": "form",
                    "done_path": ["done"],
                    "error_path": ["error"],
                    "interval_ms": 1,
                    "budget_ms": 5000
                }
            }
        ]))
        .unwrap();

    let client = crate::http_client::control_plane();

    let mut state = WorkingState::new(BTreeMap::new(), Some(mock.base()));

    let constants = ProvisioningEnv {
        secret: Some("test-secret"),
        client_id: "test-client-id",
        client_secret: None,
    };

    let outcome = run_steps(&client, &steps, &constants, &mut state).await;

    let Err(error) = outcome else {
        panic!("a form poll carrying its error member must fail on the payload");
    };

    assert!(
        error
            .to_string()
            .contains("failed upstream: operation failed"),
        "the error member renders verbatim after the form decoding: {error}"
    );

    mock.shutdown.send(()).ok();
}

/// Pins the per-request environment rebuild (README §5.4): a step's request resolves the slots
/// an earlier step extracted, and the poll request resolves the operation slot its own step
/// placed — slot references never resolve against the flow's initial slots alone, where they
/// would silently render empty.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the rendered requests diverge from the expected
/// URLs.
#[tokio::test]
async fn provisioning_resolves_slots_previous_stages_placed() {
    let (mock, seen) = MockUpstream::start_recording(vec![
        serde_json::to_string(&json!({"operation": "onboard-1"})).unwrap(),
        serde_json::to_string(&json!({"done": true})).unwrap(),
    ])
    .await;

    let steps: Vec<crate::provider::definition::provisioning::ProvisioningStep> =
        serde_json::from_value(json!([
            {
                "request": {
                    "method": "POST",
                    "url": [{"type": "base"}, {"type": "literal", "value": "/start"}]
                },
                "response_encoding": "json",
                "extract": [
                    {
                        "path": ["operation"],
                        "target": {"type": "slot", "name": "operation"},
                        "shape": {"type": "string"},
                        "required": true
                    }
                ]
            },
            {
                "request": {
                    "method": "POST",
                    "url": [
                        {"type": "base"},
                        {"type": "literal", "value": "/onboard/"},
                        {"type": "slot", "name": "operation"}
                    ]
                },
                "response_encoding": "json",
                "poll": {
                    "request": {
                        "method": "POST",
                        "url": [
                            {"type": "base"},
                            {"type": "literal", "value": "/onboard/"},
                            {"type": "slot", "name": "operation"},
                            {"type": "literal", "value": "/poll"}
                        ]
                    },
                    "response_encoding": "json",
                    "done_path": ["done"],
                    "error_path": ["error"],
                    "interval_ms": 1,
                    "budget_ms": 5000
                }
            }
        ]))
        .unwrap();

    let client = crate::http_client::control_plane();

    let base_url = Some(mock.base());

    let mut state = WorkingState::new(BTreeMap::new(), base_url.clone());

    let constants = ProvisioningEnv {
        secret: Some("test-secret"),
        client_id: "test-client-id",
        client_secret: None,
    };

    run_steps(&client, &steps, &constants, &mut state)
        .await
        .unwrap();

    assert_eq!(
        state.slots.get("operation").map(String::as_str),
        Some("onboard-1")
    );

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            "POST /start".to_string(),
            "POST /onboard/onboard-1".to_string(),
            "POST /onboard/onboard-1/poll".to_string(),
        ]
    );

    mock.shutdown.send(()).ok();
}

/// The one-step provisioning sequence one expiry test runs: its response maps
/// `expires_at_unix_seconds` from the `expires_at` member.
///
/// # Panics
///
/// Panics when the fixture fails to parse.
fn expiry_step() -> Vec<crate::provider::definition::provisioning::ProvisioningStep> {
    #[expect(clippy::unwrap_used, reason = "the fixture step always parses")]
    serde_json::from_value(json!([
        {
            "request": {
                "method": "POST",
                "url": [{"type": "base"}, {"type": "literal", "value": "/token"}]
            },
            "response_encoding": "json",
            "extract": [
                {
                    "path": ["expires_at"],
                    "target": {"type": "expires_at_unix_seconds"},
                    "shape": {"type": "integer"},
                    "required": true
                }
            ]
        }
    ]))
    .unwrap()
}

/// Pins the provisioning expiry contract (README §5.4, §7.2): an `expires_at_unix_seconds`
/// extraction is consulted expiry evidence, so a zero, negative, or past timestamp is a
/// malformed provisioning response that fails the step — never a stored always-elapsed expiry
/// that would send every dispatch back into the refresh lease — while a future timestamp folds
/// into the working state in database-clock microseconds.
///
/// # Panics
///
/// Panics when a fixture lands on the wrong side of its check.
#[tokio::test]
async fn provisioning_rejects_a_non_future_expires_at_as_malformed() {
    let steps = expiry_step();

    let constants = ProvisioningEnv {
        secret: Some("test-secret"),
        client_id: "test-client-id",
        client_secret: None,
    };

    for seconds in [0, -1, crate::unix_time::unix_now() - 3_600] {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({ "expires_at": seconds })).unwrap(),
        ])
        .await;

        let client = crate::http_client::control_plane();

        let mut state = WorkingState::new(BTreeMap::new(), Some(mock.base()));

        let outcome = run_steps(&client, &steps, &constants, &mut state).await;

        let Err(error) = outcome else {
            panic!("an expiry of {seconds} seconds is always elapsed and must fail the step");
        };

        assert!(
            error
                .to_string()
                .contains(&format!("expiry must be in the future, got {seconds}")),
            "the rejection names the futurity rule with the offending value: {error}"
        );

        mock.shutdown.send(()).ok();
    }

    // A future timestamp folds into the working state, scaled to database-clock microseconds.
    let seconds = crate::unix_time::unix_now() + 21_600;

    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({ "expires_at": seconds })).unwrap(),
    ])
    .await;

    let client = crate::http_client::control_plane();

    let mut state = WorkingState::new(BTreeMap::new(), Some(mock.base()));

    run_steps(&client, &steps, &constants, &mut state)
        .await
        .unwrap();

    assert_eq!(
        state.expires_at,
        Some(seconds * crate::unix_time::MICROS_PER_SEC)
    );

    mock.shutdown.send(()).ok();
}

/// The environment one discovery test carries.
fn discovery_env(base: &str) -> ValueEnv<'static> {
    let leaked: &'static str = Box::leak(base.to_string().into_boxed_str());
    let slots: &'static BTreeMap<String, String> = Box::leak(Box::new(BTreeMap::new()));

    ValueEnv::for_discovery(slots, Some(leaked), "secret")
}

/// The device grant one device-authorization test runs: the authorization response maps every
/// target, so a fixture controls the `expires_in` and `interval` values verbatim.
///
/// # Panics
///
/// Panics when the fixture fails to parse.
fn device_grant(base: &str) -> crate::provider::definition::credential::Grant {
    #[expect(clippy::unwrap_used, reason = "the fixture grant always parses")]
    serde_json::from_value(json!({
        "type": "device",
        "authorization": {
            "request": {
                "method": "POST",
                "url": [{"type": "literal", "value": format!("{base}/device/code")}]
            },
            "response": {
                "encoding": "json",
                "fields": [
                    {
                        "path": ["device_code"],
                        "target": {"type": "device_code"},
                        "required": true
                    },
                    {
                        "path": ["user_code"],
                        "target": {"type": "user_code"},
                        "required": true
                    },
                    {
                        "path": ["verification_uri"],
                        "target": {"type": "verification_uri"},
                        "required": false
                    },
                    {
                        "path": ["expires_in"],
                        "target": {"type": "expires_in_seconds"},
                        "required": false
                    },
                    {
                        "path": ["interval"],
                        "target": {"type": "interval_seconds"},
                        "required": false
                    }
                ]
            }
        }
    }))
    .unwrap()
}

/// Executes the device authorization of a fixture grant against a mock serving `body`.
///
/// # Panics
///
/// Panics when the mock cannot start.
async fn device_authorization_response(
    body: serde_json::Value,
) -> Result<oauth::DeviceAuthorization, Error> {
    let mock = MockUpstream::start(vec![serde_json::to_string(&body).unwrap()]).await;

    let grant = device_grant(&mock.base());

    let slots = BTreeMap::new();

    let env = ValueEnv::for_device_authorization(&slots, None, "client-id", None);

    oauth::execute_device_authorization(&crate::http_client::control_plane(), &grant, &env).await
}

/// Pins the RFC 8628 §3.2 shape of the device-authorization response (README §5.3): a
/// non-positive `interval`, or a present non-positive `expires_in`, is a malformed response
/// that fails the flow start — while positive values decode per the documented fallback rules
/// in both the string and the JSON-number spelling (RFC 6749 §5.1 renders numerical values as
/// JSON numbers), and a wrong-shaped duration fails the stage.
///
/// # Panics
///
/// Panics when a fixture lands on the wrong side of its check.
#[tokio::test]
async fn device_authorization_rejects_non_positive_durations() {
    let verify = "https://verify.example";

    for (durations, is_accepted) in [
        (json!({"expires_in": "600", "interval": "5"}), true),
        (json!({"interval": "5"}), true),
        (json!({"expires_in": "600"}), true),
        (json!({}), true),
        (json!({"expires_in": 1800, "interval": 5}), true),
        (json!({"expires_in": 1800}), true),
        (json!({"interval": 5}), true),
        (json!({"expires_in": "600", "interval": "0"}), false),
        (json!({"expires_in": "600", "interval": "-5"}), false),
        (json!({"expires_in": "0"}), false),
        (json!({"expires_in": "-1"}), false),
    ] {
        let mut body = json!({
            "device_code": "dc",
            "user_code": "uc",
            "verification_uri": verify
        });

        let body_object = body.as_object_mut().unwrap();

        for (member, value) in durations.as_object().unwrap() {
            body_object.insert(member.clone(), value.clone());
        }

        let outcome = device_authorization_response(body).await;

        assert_eq!(
            outcome.is_ok(),
            is_accepted,
            "durations {durations} must be {}",
            if is_accepted {
                "accepted"
            } else {
                "rejected as malformed"
            }
        );

        if let Err(error) = outcome {
            assert!(
                error.to_string().contains("must be positive"),
                "the rejection names the positivity rule: {error}"
            );
        }
    }

    // The accepted shapes decode their durations verbatim.
    let decoded = device_authorization_response(json!({
        "device_code": "dc",
        "user_code": "uc",
        "verification_uri": verify,
        "expires_in": "600",
        "interval": "5"
    }))
    .await
    .unwrap();

    assert_eq!(decoded.expires_in_seconds, Some(600));
    assert_eq!(decoded.interval_seconds, Some(5));

    // The JSON-number spelling decodes exactly like the string one (RFC 8628 §3.2's own
    // example reports `"expires_in": 1800, "interval": 5`).
    let decoded = device_authorization_response(json!({
        "device_code": "dc",
        "user_code": "uc",
        "verification_uri": verify,
        "expires_in": 1800,
        "interval": 5
    }))
    .await
    .unwrap();

    assert_eq!(decoded.expires_in_seconds, Some(1800));
    assert_eq!(decoded.interval_seconds, Some(5));

    let decoded = device_authorization_response(json!({
        "device_code": "dc",
        "user_code": "uc",
        "verification_uri": verify
    }))
    .await
    .unwrap();

    assert_eq!(decoded.expires_in_seconds, None);
    assert_eq!(decoded.interval_seconds, None);

    // A number is a duration; any other non-string kind stays the wrong shape.
    let outcome = device_authorization_response(json!({
        "device_code": "dc",
        "user_code": "uc",
        "verification_uri": verify,
        "expires_in": true
    }))
    .await;

    let Err(error) = outcome else {
        panic!("a boolean duration is the wrong shape and must fail the stage");
    };

    assert!(
        error.to_string().contains("must be a string"),
        "the rejection names the shape rule: {error}"
    );
}

/// Pins the upstream clients' redirect refusal (README §4): neither upstream HTTP client follows
/// a 3xx — the re-dispatch would carry definition-declared credential headers and request bodies
/// to the Location origin — so the redirection itself is the response the caller sees and flows
/// through the non-2xx path. The device-authorization round-trip therefore fails on the redirect
/// response, and the Location origin never answers.
///
/// # Panics
///
/// Panics when a fixture lands on the wrong side of its check.
#[tokio::test]
async fn upstream_clients_surface_redirects_without_following_them() {
    // The Location origin answers the success shape the device-authorization stage consumes, so
    // a followed redirect would complete the stage on the foreign origin's answer.
    let (target, target_seen) = MockUpstream::start_recording(vec![
        serde_json::to_string(&json!({
            "device_code": "dc",
            "user_code": "uc",
            "verification_uri": "https://verify.example"
        }))
        .unwrap(),
    ])
    .await;

    let redirect = MockUpstream::start_raw(vec![format!(
        "HTTP/1.1 302 Found\r\nLocation: {}/device/code\r\nContent-Length: 0\r\n\
         Connection: close\r\n\r\n",
        target.base()
    )])
    .await;

    let grant = device_grant(&redirect.base());

    let slots = BTreeMap::new();

    let env = ValueEnv::for_device_authorization(&slots, None, "client-id", None);

    let outcome =
        oauth::execute_device_authorization(&crate::http_client::control_plane(), &grant, &env)
            .await;

    let Err(error) = outcome else {
        panic!("a redirect from the token endpoint must surface as a rejection, never be followed");
    };

    assert!(
        error.to_string().contains("status 302"),
        "the rejection is the redirect response itself: {error}"
    );

    // The Location origin never received the re-dispatched request.
    assert_eq!(*target_seen.lock().unwrap(), Vec::<String>::new());

    // The data-plane client refuses redirects the same way: the 3xx is the response.
    let response = crate::http_client::data_plane()
        .get(format!("{}/chat/completions", redirect.base()))
        .send()
        .await
        .unwrap();

    assert!(
        response.status().is_redirection(),
        "the data plane received the redirect itself, not the followed response: {}",
        response.status()
    );

    redirect.shutdown.send(()).ok();
    target.shutdown.send(()).ok();
}

/// The definition payload of a completion test: an OAuth recipe whose provisioning sequence
/// extracts the access token from one upstream call. `is_with_provisioning` false strips the
/// sequence, leaving the exchange alone — which maps no access token here.
///
/// The exchange references the `authorization_code` and `redirect_uri` value parts because the
/// write boundary requires both of every authorization-code grant (RFC 6749 §4.1.3).
///
/// # Panics
///
/// Panics when the fixture cannot be mutated.
fn completion_payload(is_with_provisioning: bool) -> serde_json::Value {
    let mut payload = json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {
            "type": "oauth",
            "client_id": "client-id",
            "grant": {
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://example.test/auth"}],
                "redirect_uri": "http://localhost:1455/auth/callback",
                "pkce": "none"
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": "https://example.test/token"}],
                    "body": {"kind": "form", "fields": [
                        {"name": "code", "value": [{"type": "authorization_code"}]},
                        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}
                    ]}
                },
                "response": {"encoding": "json", "fields": []},
                "terminal_codes": ["invalid_grant"]
            },
            "refresh": {"type": "none"}
        },
        "provisioning": [
            {
                "request": {
                    "method": "POST",
                    "url": [{"type": "base"}, {"type": "literal", "value": "/token"}]
                },
                "response_encoding": "json",
                "extract": [
                    {
                        "path": ["access_token"],
                        "target": {"type": "access_token"},
                        "shape": {"type": "string"},
                        "required": true
                    }
                ]
            }
        ]
    });

    if !is_with_provisioning {
        payload.as_object_mut().unwrap().remove("provisioning");
    }

    payload
}

/// The definition one completion test runs against.
///
/// # Panics
///
/// Panics when the fixture fails to parse.
fn completion_definition(payload: serde_json::Value) -> crate::provider::ProviderDefinition {
    crate::provider::ProviderDefinition {
        name: "completion-test".to_string(),
        version: std::num::NonZeroI64::new(1).unwrap(),
        payload: serde_json::from_value(payload).unwrap(),
        created_at: 0,
    }
}

/// The create flow one completion test runs.
///
/// # Panics
///
/// Panics when the fixture's version number is zero.
fn completion_flow(base: String) -> crate::storage::oauth_flows::OngoingFlow {
    crate::storage::oauth_flows::OngoingFlow {
        id: 1,
        definition_name: "completion-test".to_string(),
        definition_version: std::num::NonZeroI64::new(1).unwrap(),
        state_token: "state-token".to_string(),
        purpose: crate::storage::oauth_flows::FlowPurpose::Create,
        provider_name: Some("test-provider".to_string()),
        model_prefix: Some(String::new()),
        base_url: Some(base),
        target_provider_id: None,
        grant_type: crate::storage::oauth_flows::FlowGrant::AuthorizationCode,
        slots: BTreeMap::new(),
        code_verifier: None,
        device_code: None,
        device_user_code: None,
        device_verification_uri: None,
        device_verification_uri_complete: None,
        device_poll_interval_seconds: None,
        device_next_poll_at: None,
        device_poll_started_at: None,
        device_exchange_checkpoint: None,
        outcome: None,
        result_provider_id: None,
        failure_code: None,
        failure_message: None,
        expires_at: 0,
    }
}

/// Pins the completion pipeline's access-token contract (README §5.3): the access token may be
/// supplied by a later provisioning step when the exchange response maps none, while a completed
/// exchange whose provisioning leaves no token is the stored-drift failure.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the pipeline diverges from the contract.
#[tokio::test]
async fn completion_supplies_the_access_token_from_provisioning() {
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({"access_token": "provisioned-token"})).unwrap(),
    ])
    .await;

    let definition = completion_definition(completion_payload(true));

    let flow = completion_flow(mock.base());

    let client = crate::http_client::control_plane();

    let product = oauth::complete_exchange(&client, &definition, Default::default(), &flow)
        .await
        .unwrap();

    let StoredCredentials::OAuth { access_token, .. } = product.credentials else {
        panic!("an OAuth completion produces OAuth credentials");
    };

    assert_eq!(access_token, "provisioned-token");

    // Without the provisioning sequence the exchange alone supplies no access token: the
    // write-time validation guarantees one source, so the pipeline fails with the drift error.
    let bare_definition = completion_definition(completion_payload(false));

    let Err(error) =
        oauth::complete_exchange(&client, &bare_definition, Default::default(), &flow).await
    else {
        panic!("a completion without any access token must fail");
    };

    assert!(matches!(error, crate::provider::Error::Internal { .. }));

    mock.shutdown.send(()).ok();
}

/// The completion payload one base-URL precedence test row runs: the exchange request points at
/// `token_url` and its mapping extracts the access token plus an optional `base_url`; with the
/// provisioning sequence, its step re-extracts the `base_url` from its own response instead of
/// the access token.
///
/// # Panics
///
/// Panics when the fixture cannot be mutated.
fn base_url_completion_payload(token_url: &str, is_with_provisioning: bool) -> serde_json::Value {
    let mut payload = completion_payload(is_with_provisioning);

    payload
        .get_mut("credential")
        .unwrap()
        .get_mut("exchange")
        .unwrap()
        .get_mut("request")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "url".to_string(),
            json!([{"type": "literal", "value": format!("{token_url}/token")}]),
        );

    payload
        .get_mut("credential")
        .unwrap()
        .get_mut("exchange")
        .unwrap()
        .get_mut("response")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "fields".to_string(),
            json!([
                {
                    "path": ["access_token"],
                    "target": {"type": "access_token"},
                    "required": true
                },
                {
                    "path": ["base_url"],
                    "target": {"type": "base_url"},
                    "required": false
                }
            ]),
        );

    if is_with_provisioning {
        *payload.get_mut("provisioning").unwrap().get_mut(0).unwrap() = json!({
            "request": {
                "method": "POST",
                "url": [{"type": "literal", "value": format!("{token_url}/base")}]
            },
            "response_encoding": "json",
            "extract": [
                {
                    "path": ["base_url"],
                    "target": {"type": "base_url"},
                    "shape": {"type": "string"},
                    "required": true
                }
            ]
        });
    }

    payload
}

/// Runs one completion test row's exchange end to end without classifying, returning the raw
/// result so a test can pin a failure side.
///
/// # Panics
///
/// Panics when the fixture's grant diverges — the failure is the report.
async fn raw_exchange_outcome(
    client: &reqwest::Client,
    definition: &crate::provider::ProviderDefinition,
    flow: &crate::storage::oauth_flows::OngoingFlow,
) -> Result<oauth::ExchangeOutcome, Error> {
    let crate::provider::definition::credential::CredentialAcquisition::OAuth(recipe) =
        &definition.payload.credential
    else {
        panic!("the completion fixture carries an OAuth credential");
    };

    let crate::provider::definition::credential::Grant::AuthorizationCode { redirect_uri, .. } =
        &recipe.grant
    else {
        panic!("the completion fixture carries an authorization-code grant");
    };

    let env = ValueEnv::for_exchange(
        &flow.slots,
        flow.base_url.as_deref(),
        recipe.client_id.as_str(),
        recipe.client_secret.as_deref(),
        "test-code",
        flow.code_verifier.as_deref(),
        redirect_uri.as_str(),
    );

    oauth::completed_outcome(oauth::execute_exchange(client, &recipe.exchange, &env).await?)
}

/// Runs one completion test row's exchange end to end: the definition's exchange executes
/// against the mock and its outcome classifies.
///
/// # Panics
///
/// Panics when the fixture's grant diverges or the exchange fails — the failure is the report.
async fn executed_outcome(
    client: &reqwest::Client,
    definition: &crate::provider::ProviderDefinition,
    flow: &crate::storage::oauth_flows::OngoingFlow,
) -> oauth::ExchangeOutcome {
    raw_exchange_outcome(client, definition, flow)
        .await
        .unwrap()
}

/// Runs one completion test row end to end: the definition's exchange executes against the mock,
/// its outcome classifies, and the shared pipeline folds it to the working base URL.
///
/// # Panics
///
/// Panics when the fixture's grant diverges or the exchange or pipeline fails — the failure is
/// the report.
async fn completed_base_url(
    client: &reqwest::Client,
    definition: &crate::provider::ProviderDefinition,
    flow: &crate::storage::oauth_flows::OngoingFlow,
) -> Option<String> {
    let outcome = executed_outcome(client, definition, flow).await;

    oauth::complete_exchange(client, definition, outcome, flow)
        .await
        .unwrap()
        .base_url
}

/// Pins the flow's slot initialization (README §5.3): the definition's generators run over the
/// stored slots and initialize only the missing names — a name the reauthorization target
/// already carries keeps its stored value, and each generator renders its own spelling.
///
/// # Panics
///
/// Panics when the fixture fails to parse or initialization diverges from the contract.
#[test]
fn initialize_slots_fill_only_the_missing_names() {
    let mut payload = completion_payload(false);

    payload
        .get_mut("credential")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "initial_slots".to_string(),
            json!([
                {"name": "stable", "generator": "uuid_hex"},
                {"name": "hex", "generator": "uuid_hex"},
                {"name": "hyphenated", "generator": "uuid"}
            ]),
        );

    let definition = completion_definition(payload);

    let mut stored = BTreeMap::new();

    stored.insert("stable".to_string(), "kept".to_string());

    let slots = oauth::initialize_slots(&definition, &stored).unwrap();

    // A name the stored slots carry is never re-initialized.
    assert_eq!(slots.get("stable").map(String::as_str), Some("kept"));

    let hex = slots.get("hex").map(String::as_str).unwrap();

    assert_eq!(hex.len(), 32);
    assert!(hex.chars().all(|character| character.is_ascii_hexdigit()));

    let hyphenated = slots.get("hyphenated").map(String::as_str).unwrap();

    assert_eq!(hyphenated.len(), 36);
    assert_eq!(hyphenated.matches('-').count(), 4);
    assert!(
        hyphenated
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
    );
}

/// Pins the exchange mapping's number-tolerant text extraction (RFC 6749 §5.1 renders numerical
/// values as JSON numbers): a token endpoint reporting `expires_in` as a JSON number decodes
/// into the outcome's lifetime exactly like the string spelling (README §5.1).
///
/// # Panics
///
/// Panics when the fixture fails to parse or the exchange diverges from the expectation.
#[tokio::test]
async fn token_exchange_reads_numeric_expires_in() {
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({
            "access_token": "exchange-token",
            "expires_in": 3600
        }))
        .unwrap(),
    ])
    .await;

    let mut payload = completion_payload(false);

    // The exchange points at the mock and maps the access token plus the numeric `expires_in`.
    payload
        .get_mut("credential")
        .unwrap()
        .get_mut("exchange")
        .unwrap()
        .get_mut("request")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "url".to_string(),
            json!([{"type": "literal", "value": format!("{}/token", mock.base())}]),
        );

    payload
        .get_mut("credential")
        .unwrap()
        .get_mut("exchange")
        .unwrap()
        .get_mut("response")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "fields".to_string(),
            json!([
                {
                    "path": ["access_token"],
                    "target": {"type": "access_token"},
                    "required": true
                },
                {
                    "path": ["expires_in"],
                    "target": {"type": "expires_in_seconds"},
                    "required": false
                }
            ]),
        );

    let definition = completion_definition(payload);

    let flow = completion_flow(mock.base());

    let client = crate::http_client::control_plane();

    let outcome = executed_outcome(&client, &definition, &flow).await;

    assert_eq!(outcome.access_token.as_deref(), Some("exchange-token"));
    assert_eq!(outcome.expires_in_seconds, Some(3600));

    mock.shutdown.send(()).ok();
}

/// Pins the consulted-expiry contract (README §5.3, §7.2): a present `expires_in` carrying
/// non-numeric text is a malformed token response that fails the stage — never a silent fall
/// through to the weaker expiry sources — while the integer spellings decode verbatim.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the exchange diverges from the contract.
#[tokio::test]
async fn a_non_integer_expires_in_fails_the_token_response_malformed() {
    for (expires_in, is_accepted) in [
        (json!("3600"), true),
        (json!(3600), true),
        (json!("soon"), false),
    ] {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "access_token": "exchange-token",
                "expires_in": expires_in
            }))
            .unwrap(),
        ])
        .await;

        let mut payload = completion_payload(false);

        payload
            .get_mut("credential")
            .unwrap()
            .get_mut("exchange")
            .unwrap()
            .get_mut("request")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                "url".to_string(),
                json!([{"type": "literal", "value": format!("{}/token", mock.base())}]),
            );

        payload
            .get_mut("credential")
            .unwrap()
            .get_mut("exchange")
            .unwrap()
            .get_mut("response")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                "fields".to_string(),
                json!([
                    {
                        "path": ["access_token"],
                        "target": {"type": "access_token"},
                        "required": true
                    },
                    {
                        "path": ["expires_in"],
                        "target": {"type": "expires_in_seconds"},
                        "required": false
                    }
                ]),
            );

        let definition = completion_definition(payload);

        let flow = completion_flow(mock.base());

        let client = crate::http_client::control_plane();

        let outcome = raw_exchange_outcome(&client, &definition, &flow).await;

        assert_eq!(
            outcome.is_ok(),
            is_accepted,
            "an `expires_in` of {expires_in} must be {}",
            if is_accepted {
                "accepted"
            } else {
                "rejected as malformed"
            }
        );

        if let Err(error) = outcome {
            assert!(
                error.to_string().contains("non-integer `expires_in`"),
                "the rejection names the unparseable lifetime: {error}"
            );
        }

        mock.shutdown.send(()).ok();
    }
}

/// Pins the completion pipeline's base-URL precedence (README §5.3, §5.4): an exchange
/// mapping's `base_url` target flows into the working base URL the provider write commits, the
/// flow's stored base survives an exchange response that maps none, and a provisioning
/// extraction supersedes the exchange's.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the pipeline diverges from the precedence.
#[tokio::test]
async fn completion_folds_the_exchange_base_url_into_the_working_base() {
    let client = crate::http_client::control_plane();

    // The exchange response's extracted `base_url` replaces the flow's stored base.
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({
            "access_token": "exchange-token",
            "base_url": "https://exchange.example"
        }))
        .unwrap(),
    ])
    .await;

    let bare = completion_definition(base_url_completion_payload(&mock.base(), false));

    let flow = completion_flow("https://flow.example".to_string());

    let base_url = completed_base_url(&client, &bare, &flow).await;

    assert_eq!(base_url.as_deref(), Some("https://exchange.example"));

    mock.shutdown.send(()).ok();

    // An exchange response that maps no `base_url` leaves the flow's stored base in place.
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({
            "access_token": "exchange-token"
        }))
        .unwrap(),
    ])
    .await;

    let bare = completion_definition(base_url_completion_payload(&mock.base(), false));

    let flow = completion_flow("https://flow.example".to_string());

    let base_url = completed_base_url(&client, &bare, &flow).await;

    assert_eq!(base_url.as_deref(), Some("https://flow.example"));

    mock.shutdown.send(()).ok();

    // A provisioning extraction supersedes the exchange's `base_url`.
    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({
            "access_token": "exchange-token",
            "base_url": "https://exchange.example"
        }))
        .unwrap(),
        serde_json::to_string(&json!({"base_url": "https://provisioned.example"})).unwrap(),
    ])
    .await;

    let provisioned = completion_definition(base_url_completion_payload(&mock.base(), true));

    let flow = completion_flow("https://flow.example".to_string());

    let base_url = completed_base_url(&client, &provisioned, &flow).await;

    assert_eq!(base_url.as_deref(), Some("https://provisioned.example"));

    mock.shutdown.send(()).ok();
}

/// Pins the completion pipeline's expiry precedence (README §5.4, §7.2): a provisioning-extracted
/// `expires_at_unix_seconds` supersedes the response's token data — the exchange's reported
/// `expires_in` is never consulted when the extraction supplies an absolute expiry.
///
/// # Panics
///
/// Panics when the fixture fails to parse or the pipeline diverges from the precedence.
#[tokio::test]
async fn completion_prefers_the_provisioned_expiry_over_the_response_expiry() {
    let seconds = crate::unix_time::unix_now() + 21_600;

    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({ "expires_at": seconds })).unwrap(),
    ])
    .await;

    let mut payload = completion_payload(true);

    // The provisioning step extracts the absolute expiry instead of the access token.
    *payload.get_mut("provisioning").unwrap().get_mut(0).unwrap() = json!({
        "request": {
            "method": "POST",
            "url": [{"type": "base"}, {"type": "literal", "value": "/token"}]
        },
        "response_encoding": "json",
        "extract": [
            {
                "path": ["expires_at"],
                "target": {"type": "expires_at_unix_seconds"},
                "shape": {"type": "integer"},
                "required": true
            }
        ]
    });

    let definition = completion_definition(payload);

    let flow = completion_flow(mock.base());

    let client = crate::http_client::control_plane();

    // The exchange response's token data reports a one-hour lifetime.
    let outcome = oauth::ExchangeOutcome {
        access_token: Some("exchange-token".to_string()),
        expires_in_seconds: Some(3_600),
        ..oauth::ExchangeOutcome::default()
    };

    let product = oauth::complete_exchange(&client, &definition, outcome, &flow)
        .await
        .unwrap();

    let StoredCredentials::OAuth { expires_at, .. } = product.credentials else {
        panic!("an OAuth completion produces OAuth credentials");
    };

    assert_eq!(expires_at, Some(seconds * crate::unix_time::MICROS_PER_SEC));

    mock.shutdown.send(()).ok();
}

/// The OAuth credentials one completion-write test's stored row carries.
fn oauth_credentials(access_token: &str) -> StoredCredentials {
    StoredCredentials::OAuth {
        access_token: access_token.to_string(),
        refresh_token: None,
        slots: BTreeMap::new(),
        expires_at: None,
        refresh_started_at: None,
    }
}

/// The reauthorize flow one completion-write test runs: the begin-time base echo `base`, no
/// create-only naming, and `target_provider_id` as the write's target.
///
/// # Panics
///
/// Panics when the fixture's version number is zero.
fn reauthorize_completion_flow(
    base: String,
    target_provider_id: i64,
) -> crate::storage::oauth_flows::OngoingFlow {
    let mut flow = completion_flow(base);

    flow.purpose = crate::storage::oauth_flows::FlowPurpose::Reauthorize;
    flow.provider_name = None;
    flow.model_prefix = None;
    flow.target_provider_id = Some(target_provider_id);

    flow
}

/// Validates and inserts one OAuth definition fixture whose exchange posts to `exchange_url`
/// and maps the access token plus `extra_fields`, for the completion-write tests.
///
/// # Panics
///
/// Panics if the payload does not validate or the insert fails — neither can occur for the
/// fixed fixture payloads.
async fn insert_completion_write_definition(
    database: &Database,
    name: &str,
    exchange_url: &str,
    extra_fields: Vec<OAuthResponseField>,
) -> crate::provider::ProviderDefinition {
    let payload = oauth_definition_payload(OAuthDefinitionFixture {
        format: UpstreamFormat::OpenAiChatCompletion,
        inference_suffix: Some("/chat/completions"),
        client_id: "fixture-client",
        initial_slots: &[],
        grant: OAuthGrantFixture::AuthorizationCode,
        exchange_url,
        response_fields: extra_fields,
        terminal_codes: &["invalid_grant"],
        refresh: OAuthRefreshFixture::None,
        provisioning: None,
    });

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
    .unwrap()
}

/// Pins the reauthorize write set (README §7.1): the commit replaces the credentials, but never
/// writes the flow's begin-time base back over the row. A base the row gains while the user
/// approves — a legitimate `providers:update` between `oauth:reauthorize` and the completion —
/// survives the commit: the begin-time echo is stale by completion time, and rewriting it would
/// send every later dispatch of a base-addressing definition to the old origin.
///
/// # Panics
///
/// Panics when the fixtures fail to insert or the committed row diverges from the contract.
#[tokio::test]
async fn a_reauthorization_keeps_a_base_the_row_gained_mid_flow() {
    let (database, _guard) = create_test_database().await;

    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({"access_token": "exchange-token"})).unwrap(),
    ])
    .await;

    let base = mock.base();

    // The flow's definition: the exchange maps the access token alone, so the completion's
    // working base is the begin-time echo alone — no derivation is aboard.
    let definition =
        insert_completion_write_definition(&database, "completion-write", &base, vec![]).await;

    let provider = insert_provider(
        &database,
        &definition,
        "reauthorized",
        "r-",
        Some("https://begin.example"),
        oauth_credentials("old-token"),
    )
    .await;

    let flow = reauthorize_completion_flow("https://begin.example".to_string(), provider.id);

    let client = crate::http_client::control_plane();

    let outcome = executed_outcome(&client, &definition, &flow).await;

    let product = oauth::complete_exchange(&client, &definition, outcome, &flow)
        .await
        .unwrap();

    // While the user approves, `providers:update` legitimately stores a new base; the
    // completion that follows must not echo the begin-time base over it.
    let mut transaction = database.transaction().await.unwrap();

    storage::providers::set_base_url_in_transaction(
        &mut transaction,
        provider.id,
        "https://updated.example",
    )
    .await
    .unwrap();

    transaction.commit().await.unwrap();

    let mut transaction = database.transaction().await.unwrap();

    let written = oauth::write_completion_provider(&mut transaction, &flow, &definition, &product)
        .await
        .unwrap();

    transaction.commit().await.unwrap();

    assert_eq!(written, Some(provider.id), "the reauthorization commits");

    let settled = storage::providers::find_provider_by_id(&database, provider.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        settled.base_url.as_deref(),
        Some("https://updated.example"),
        "the completion must not echo the begin-time base over the mid-flow update"
    );

    assert_eq!(
        settled.credentials.secret(),
        "exchange-token",
        "the reauthorization still commits the credentials"
    );

    mock.shutdown.send(()).ok();
}

/// Pins the derived-base write (README §3, §7.1): a reauthorization whose exchange derives a
/// `base_url` still replaces the stored base, and the derived value lands in the same commit as
/// a definition rebind — so a rebind to a base-referencing target never sits on a missing or
/// stale base, and the flow's begin-time echo never wins over the derivation.
///
/// # Panics
///
/// Panics when the fixtures fail to insert or the committed row diverges from the contract.
#[tokio::test]
async fn a_reauthorization_writes_a_derived_base_in_the_rebinds_commit() {
    let (database, _guard) = create_test_database().await;

    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({
            "access_token": "exchange-token",
            "base_url": "https://derived.example"
        }))
        .unwrap(),
    ])
    .await;

    let base = mock.base();

    // The flow's definition: the exchange maps the access token and a derived `base_url`.
    let definition = insert_completion_write_definition(
        &database,
        "completion-write-derived",
        &base,
        vec![OAuthResponseField::new("base_url", OAuthTarget::BaseUrl)],
    )
    .await;

    // The rebind target: a different definition whose URLs reference the base too.
    let rebind_target = insert_completion_write_definition(
        &database,
        "rebind-target",
        crate::test_support::FIXTURE_TOKEN_URL,
        vec![],
    )
    .await;

    let provider = insert_provider(
        &database,
        &definition,
        "rebound",
        "rb-",
        Some("https://stored.example"),
        oauth_credentials("old-token"),
    )
    .await;

    // The flow's begin-time base differs from both the stored and the derived value: only the
    // derivation may reach the row.
    let flow = reauthorize_completion_flow("https://begin.example".to_string(), provider.id);

    let client = crate::http_client::control_plane();

    let outcome = executed_outcome(&client, &definition, &flow).await;

    let product = oauth::complete_exchange(&client, &definition, outcome, &flow)
        .await
        .unwrap();

    let mut transaction = database.transaction().await.unwrap();

    let written =
        oauth::write_completion_provider(&mut transaction, &flow, &rebind_target, &product)
            .await
            .unwrap();

    transaction.commit().await.unwrap();

    assert_eq!(written, Some(provider.id), "the reauthorization commits");

    let settled = storage::providers::find_provider_by_id(&database, provider.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        settled.definition.name, "rebind-target",
        "the definition reference rebinds"
    );

    assert_eq!(
        settled.base_url.as_deref(),
        Some("https://derived.example"),
        "the derived base lands in the rebind's commit, replacing the stored and begin bases"
    );

    mock.shutdown.send(()).ok();
}

/// Pins the create write (README §7.1): a creation flow stores the flow's begin-time base — the
/// operator-supplied initial value `oauth:start` stored — unchanged by the provenance rule,
/// since the new row has no prior base to lose.
///
/// # Panics
///
/// Panics when the fixtures fail to insert or the committed row diverges from the contract.
#[tokio::test]
async fn a_create_flow_writes_the_flows_begin_base() {
    let (database, _guard) = create_test_database().await;

    let mock = MockUpstream::start(vec![
        serde_json::to_string(&json!({"access_token": "exchange-token"})).unwrap(),
    ])
    .await;

    let base = mock.base();

    let definition =
        insert_completion_write_definition(&database, "completion-write-create", &base, vec![])
            .await;

    let flow = completion_flow("https://flow.example".to_string());

    let client = crate::http_client::control_plane();

    let outcome = executed_outcome(&client, &definition, &flow).await;

    let product = oauth::complete_exchange(&client, &definition, outcome, &flow)
        .await
        .unwrap();

    let mut transaction = database.transaction().await.unwrap();

    let written = oauth::write_completion_provider(&mut transaction, &flow, &definition, &product)
        .await
        .unwrap();

    transaction.commit().await.unwrap();

    let Some(written_provider_id) = written else {
        panic!("a create flow always inserts");
    };

    let created = storage::providers::find_provider_by_id(&database, written_provider_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(created.name, "test-provider");

    assert_eq!(
        created.base_url.as_deref(),
        Some("https://flow.example"),
        "the create flow stores the begin base the operator supplied at oauth:start"
    );

    assert_eq!(created.credentials.secret(), "exchange-token");

    mock.shutdown.send(()).ok();
}
