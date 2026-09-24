//! Engine golden-path tests against a local mock upstream (README §5.4–§5.6):
//! discovery pagination, provisioning with a long-running operation poll, and
//! device-flow classification through the shared control executor.

#![expect(
    clippy::unwrap_used,
    reason = "the fixtures are constructed so each unwrap lands on its documented side"
)]

use crate::provider::{
    definition::{discovery::Discovery, eval::ValueEnv},
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let responses = std::sync::Arc::new(std::sync::Mutex::new(responses));

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

        Self {
            address,
            shutdown: shutdown_tx,
        }
    }
}

impl MockUpstream {
    /// The `http://` base URL the mock answers at.
    fn base(&self) -> String {
        format!("http://{}", self.address)
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

    let working_slots = state.slots.clone();

    let env = ValueEnv {
        base_url: base_url.as_deref(),
        secret: None,
        slots: &working_slots,
        client_id: None,
        client_secret: None,
        authorization_code: None,
        code_verifier: None,
        redirect_uri: None,
        oauth_state: None,
        device_code: None,
        refresh_token: None,
        request_uuid: None,
        model: None,
    };

    provisioning::run_steps(&client, &steps, &env, &mut state)
        .await
        .unwrap();

    assert_eq!(
        state.slots.get("project").map(String::as_str),
        Some("proj-1")
    );
    mock.shutdown.send(()).ok();
}

/// The environment one discovery test carries.
fn discovery_env(base: &str) -> ValueEnv<'static> {
    let leaked: &'static str = Box::leak(base.to_string().into_boxed_str());
    let slots: &'static BTreeMap<String, String> = Box::leak(Box::new(BTreeMap::new()));

    ValueEnv {
        base_url: Some(leaked),
        secret: Some("secret"),
        slots,
        client_id: None,
        client_secret: None,
        authorization_code: None,
        code_verifier: None,
        redirect_uri: None,
        oauth_state: None,
        device_code: None,
        refresh_token: None,
        request_uuid: None,
        model: None,
    }
}
