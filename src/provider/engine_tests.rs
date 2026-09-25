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

use crate::provider::{
    StoredCredentials,
    definition::{discovery::Discovery, eval::ValueEnv},
    oauth::{self},
    provisioning::{self, WorkingState},
};
use serde_json::json;
use std::collections::BTreeMap;
use tokio::io::AsyncReadExt as _;

/// A tiny HTTP/1.1 mock: answers every request from `handler` and shuts down on drop.
struct MockUpstream {
    address: std::net::SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

impl MockUpstream {
    /// Serves `responses` round-robin: one entry per request.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    async fn start(responses: Vec<String>) -> Self {
        let (mock, _) = Self::start_recording(responses).await;

        mock
    }
}

impl MockUpstream {
    /// The `http://` base URL the mock answers at.
    fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Serves `responses` round-robin and records every request's `METHOD /target` line, so a
    /// test can pin the exact URLs the engine rendered.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    async fn start_recording(
        responses: Vec<String>,
    ) -> (Self, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let responses = std::sync::Arc::new(std::sync::Mutex::new(responses));

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let recorded = seen.clone();

        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => accepted,
                };

                let Ok((mut socket, _)) = accepted else {
                    break;
                };

                let responses = responses.clone();
                let seen = recorded.clone();

                tokio::spawn(async move {
                    // Read the request head through its blank line.
                    let mut buffer = Vec::new();
                    let mut chunk = [0_u8; 1024];

                    loop {
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }

                        buffer.extend_from_slice(chunk.get(..read).unwrap_or(&chunk));

                        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }

                    if let Some((method, target)) = std::str::from_utf8(&buffer)
                        .ok()
                        .and_then(|head| head.lines().next())
                        .and_then(|line| {
                            let mut parts = line.split_whitespace();
                            Some((parts.next()?, parts.next()?.to_string()))
                        })
                    {
                        seen.lock().unwrap().push(format!("{method} {target}"));
                    }

                    let index = {
                        let mut guard = responses.lock().unwrap();
                        // The single-response mock serves its one entry forever; the
                        // multi-response one walks in order. The len==0 case is the
                        // fixture-authoring bug the unwrap reports.
                        #[expect(
                            clippy::indexing_slicing,
                            reason = "an empty fixture is a test bug the panic reports"
                        )]
                        if guard.len() > 1 {
                            guard.remove(0)
                        } else {
                            guard[0].clone()
                        }
                    };

                    let _ = socket
                        .send_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                                index.len(),
                                index
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });

        (
            Self {
                address,
                shutdown: shutdown_tx,
            },
            seen,
        )
    }
}

/// Sends everything buffered on `socket`.
trait SendAll {
    async fn send_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
}

impl SendAll for tokio::net::TcpStream {
    async fn send_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;

        self.write_all(bytes).await
    }
}

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

    let constants = provisioning::ProvisioningEnv {
        secret: Some("test-secret"),
        client_id: "test-client-id",
        client_secret: None,
    };

    provisioning::run_steps(&client, &steps, &constants, &mut state)
        .await
        .unwrap();

    assert_eq!(
        state.slots.get("project").map(String::as_str),
        Some("proj-1")
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

    let constants = provisioning::ProvisioningEnv {
        secret: Some("test-secret"),
        client_id: "test-client-id",
        client_secret: None,
    };

    provisioning::run_steps(&client, &steps, &constants, &mut state)
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
) -> Result<oauth::DeviceAuthorization, oauth::Error> {
    let mock = MockUpstream::start(vec![serde_json::to_string(&body).unwrap()]).await;

    let grant = device_grant(&mock.base());

    let slots = BTreeMap::new();

    let env = ValueEnv::for_device_authorization(&slots, None, "client-id", None);

    oauth::execute_device_authorization(&crate::http_client::control_plane(), &grant, &env).await
}

/// Pins the RFC 8628 §3.2 shape of the device-authorization response (README §5.3): a
/// non-positive `interval`, or a present non-positive `expires_in`, is a malformed response
/// that fails the flow start — while positive values and absent members decode per the
/// documented fallback rules.
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

    let decoded = device_authorization_response(json!({
        "device_code": "dc",
        "user_code": "uc",
        "verification_uri": verify
    }))
    .await
    .unwrap();

    assert_eq!(decoded.expires_in_seconds, None);
    assert_eq!(decoded.interval_seconds, None);
}

/// The definition payload of a completion test: an OAuth recipe whose provisioning sequence
/// extracts the access token from one upstream call. `is_with_provisioning` false strips the
/// sequence, leaving the exchange alone — which maps no access token here.
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
                    "url": [{"type": "literal", "value": "https://example.test/token"}]
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

    assert!(matches!(
        error,
        crate::provider::oauth::Error::Internal { .. }
    ));

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

    let outcome = oauth::completed_outcome(
        oauth::execute_exchange(client, &recipe.exchange, &env)
            .await
            .unwrap(),
    )
    .unwrap();

    oauth::complete_exchange(client, definition, outcome, flow)
        .await
        .unwrap()
        .base_url
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
