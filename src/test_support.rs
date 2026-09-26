//! Test support: seeding, OAuth definition fixture building, JSON-pointer read helpers, and the
//! mock upstream transport shared by the tests across the crate.
//!
//! Every helper panics when its fixed fixture fails to parse, validate, or insert, or when a
//! JSON pointer read misses its path — a test bug by construction, so the panic is the failure
//! report.

#![expect(
    clippy::panic,
    reason = "a fixture that cannot parse, validate, or insert is a test bug, and the panic \
              is the failure report"
)]

use crate::{
    database::Database,
    format::UpstreamFormat,
    provider::{Provider, ProviderDefinition, StoredCredentials, definition::DefinitionPayload},
    storage,
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    num::NonZero,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

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

/// The unreachable fixture token endpoint the fixed OAuth definition fixtures exchange and
/// refresh at: a test that needs a live answer starts its own mock and passes its base URL.
pub(crate) const FIXTURE_TOKEN_URL: &str = "http://127.0.0.1:1/token";

/// The response-mapping target an OAuth definition fixture folds a mapped value into.
pub(crate) enum OAuthTarget {
    /// The stored access token.
    AccessToken,

    /// The stored refresh token.
    RefreshToken,

    /// The stored provider base URL.
    BaseUrl,

    /// The named working slot.
    Slot(&'static str),
}

/// One required response-mapping row of an OAuth definition fixture: the single response member
/// `path` folds into `target`.
pub(crate) struct OAuthResponseField {
    /// The response member the row reads.
    path: &'static str,

    /// The target the member's value folds into.
    target: OAuthTarget,
}

impl OAuthResponseField {
    /// Maps the response member `path` to `target`, required.
    pub(crate) const fn new(path: &'static str, target: OAuthTarget) -> Self {
        Self { path, target }
    }

    /// Renders the row's descriptor JSON.
    fn to_json(&self) -> Value {
        let target = match &self.target {
            OAuthTarget::AccessToken => json!({"type": "access_token"}),
            OAuthTarget::RefreshToken => json!({"type": "refresh_token"}),
            OAuthTarget::BaseUrl => json!({"type": "base_url"}),
            OAuthTarget::Slot(name) => json!({"type": "slot", "name": name}),
        };

        json!({
            "path": [self.path],
            "target": target,
            "required": true
        })
    }
}

/// The grant kind of an OAuth definition fixture, carrying the kind's fixed fixture parameters:
/// the authorization-code grant's authorize URL, redirect URI, and absent PKCE, and the device
/// grant's authorization request, response mapping, and expiry fallback.
pub(crate) enum OAuthGrantFixture {
    /// The authorization-code grant, whose exchange posts the `grant_type`, `code`, and
    /// `redirect_uri` form rows.
    AuthorizationCode,

    /// The device grant, whose exchange posts the `device_code` form row plus the device
    /// `grant_type` literal row when `has_grant_type_field` holds.
    Device {
        /// Whether the exchange's form declares the device `grant_type` literal row.
        has_grant_type_field: bool,
    },
}

/// The refresh mode of an OAuth definition fixture.
pub(crate) enum OAuthRefreshFixture<'a> {
    /// The tagged never-renews mode.
    None,

    /// The refresh grant: the fixed `grant_type`/`refresh_token` form posted to `url`, with the
    /// optional declared response mapping — when absent, the refresh response maps through the
    /// exchange's mapping.
    Grant {
        /// The refresh endpoint.
        url: &'a str,

        /// The declared response mapping rows, or none to inherit the exchange's.
        response: Option<Vec<OAuthResponseField>>,
    },

    /// The reprovision mode: the provisioning steps renew the credential.
    Reprovision,
}

/// The deltas an OAuth definition fixture varies around the shared
/// [`oauth_definition_payload`] skeleton.
pub(crate) struct OAuthDefinitionFixture<'a> {
    /// The upstream format the definition speaks.
    pub(crate) format: UpstreamFormat,

    /// The literal appended to the base URL in the inference URL, or none for the bare base.
    pub(crate) inference_suffix: Option<&'static str>,

    /// The OAuth client id.
    pub(crate) client_id: &'static str,

    /// The `(name, generator)` initial-slot pairs, the generator drawn from the closed
    /// `uuid`/`uuid_hex` set; empty declares none.
    pub(crate) initial_slots: &'static [(&'static str, &'static str)],

    /// The grant kind.
    pub(crate) grant: OAuthGrantFixture,

    /// The endpoint the exchange's control request posts to.
    pub(crate) exchange_url: &'a str,

    /// The exchange response's mapped targets besides the skeleton's always-required
    /// `access_token` row.
    pub(crate) response_fields: Vec<OAuthResponseField>,

    /// The terminal codes of the exchange and of a `Grant` refresh.
    pub(crate) terminal_codes: &'static [&'static str],

    /// The refresh mode.
    pub(crate) refresh: OAuthRefreshFixture<'a>,

    /// The provisioning steps array, verbatim, or none.
    pub(crate) provisioning: Option<Value>,
}

/// Builds one OAuth [`DefinitionPayload`] fixture from the shared skeleton — the given format
/// and inference URL, the grant's fixed fixture parameters, a form exchange whose response maps
/// the `access_token` target required plus [`OAuthDefinitionFixture::response_fields`] under
/// [`OAuthDefinitionFixture::terminal_codes`], and the refresh mode — so every OAuth definition
/// fixture in the crate restates only its deltas.
///
/// The skeleton's exchange references the `redirect_uri` value part because the write boundary
/// requires that of every authorization-code grant (RFC 6749 §4.1.3); the same boundary
/// requires the device grant's exchange to reference the `device_code` value part (RFC 8628
/// §3.4), which the device rows carry.
///
/// The payload is returned unvalidated: some fixtures are deliberately invalid, so validation
/// stays the caller's decision.
///
/// # Panics
///
/// Panics if the assembled payload does not parse — the skeleton is fixed, so a parse failure is
/// a test bug, and the panic is the failure report.
pub(crate) fn oauth_definition_payload(fixture: OAuthDefinitionFixture<'_>) -> DefinitionPayload {
    let inference_url = match fixture.inference_suffix {
        Some(suffix) => json!([{"type": "base"}, {"type": "literal", "value": suffix}]),
        None => json!([{"type": "base"}]),
    };

    let (grant, exchange_fields) = match fixture.grant {
        OAuthGrantFixture::AuthorizationCode => (
            json!({
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://accounts.example/auth"}],
                "redirect_uri": "http://localhost:1/callback",
                "pkce": "none"
            }),
            vec![
                json!({"name": "grant_type", "value": [
                    {"type": "literal", "value": "authorization_code"}
                ]}),
                json!({"name": "code", "value": [{"type": "authorization_code"}]}),
                json!({"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}),
            ],
        ),

        OAuthGrantFixture::Device {
            has_grant_type_field,
        } => {
            let mut exchange_fields = Vec::new();

            if has_grant_type_field {
                exchange_fields.push(json!({"name": "grant_type", "value": [
                    {"type": "literal", "value": "urn:ietf:params:oauth:grant-type:device_code"}
                ]}));
            }

            exchange_fields
                .push(json!({"name": "device_code", "value": [{"type": "device_code"}]}));

            (
                json!({
                    "type": "device",
                    "authorization": {
                        "request": {
                            "method": "POST",
                            "url": [{"type": "literal", "value": "http://127.0.0.1:1/device"}],
                            "body": {"kind": "form", "fields": [
                                {"name": "client_id", "value": [{"type": "client_id"}]}
                            ]}
                        },
                        "response": {
                            "encoding": "form",
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
                                }
                            ]
                        }
                    },
                    "expires_in_fallback_seconds": 900
                }),
                exchange_fields,
            )
        }
    };

    let exchange = json!({
        "request": {
            "method": "POST",
            "url": [{"type": "literal", "value": fixture.exchange_url}],
            "body": {"kind": "form", "fields": exchange_fields}
        },
        "response": {
            "encoding": "json",
            "fields": std::iter::once(OAuthResponseField::new(
                "access_token",
                OAuthTarget::AccessToken,
            ))
            .chain(fixture.response_fields)
            .map(|field| field.to_json())
            .collect::<Vec<_>>()
        },
        "terminal_codes": fixture.terminal_codes
    });

    let refresh = match fixture.refresh {
        OAuthRefreshFixture::None => json!({"type": "none"}),

        OAuthRefreshFixture::Reprovision => json!({"type": "reprovision"}),

        OAuthRefreshFixture::Grant { url, response } => {
            let mut refresh = json!({
                "type": "grant",
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": url}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "refresh_token"}
                        ]},
                        {"name": "refresh_token", "value": [{"type": "refresh_token"}]}
                    ]}
                }
            });

            #[expect(
                clippy::expect_used,
                reason = "the refresh skeleton literal is always an object"
            )]
            let refresh_object = refresh
                .as_object_mut()
                .expect("the refresh skeleton is an object");

            if let Some(fields) = response {
                refresh_object.insert(
                    "response".to_string(),
                    json!({
                        "encoding": "json",
                        "fields": fields
                            .iter()
                            .map(|field| field.to_json())
                            .collect::<Vec<_>>()
                    }),
                );
            }

            refresh_object.insert("terminal_codes".to_string(), json!(fixture.terminal_codes));

            refresh
        }
    };

    let mut credential = json!({
        "type": "oauth",
        "client_id": fixture.client_id,
        "grant": grant,
        "exchange": exchange,
        "refresh": refresh
    });

    if !fixture.initial_slots.is_empty() {
        #[expect(
            clippy::expect_used,
            reason = "the credential skeleton literal is always an object"
        )]
        let credential_object = credential
            .as_object_mut()
            .expect("the credential skeleton is an object");

        credential_object.insert(
            "initial_slots".to_string(),
            json!(
                fixture
                    .initial_slots
                    .iter()
                    .map(|(name, generator)| json!({
                        "name": name,
                        "generator": generator
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }

    let mut payload = json!({
        "format": fixture.format.as_str(),
        "inference": {
            "url": inference_url,
            "upstream_streaming": "match_client"
        },
        "credential": credential
    });

    if let Some(steps) = fixture.provisioning {
        #[expect(
            clippy::expect_used,
            reason = "the payload skeleton literal is always an object"
        )]
        let payload_object = payload
            .as_object_mut()
            .expect("the payload skeleton is an object");

        payload_object.insert("provisioning".to_string(), steps);
    }

    serde_json::from_value(payload)
        .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"))
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

/// The transport action one [`MockUpstream`] connection takes once its request has arrived.
pub(crate) enum MockAction {
    /// Writes the complete HTTP response — status line, headers, and body — verbatim, then
    /// closes.
    Respond(String),

    /// Answers nothing and closes: the transport-failure shape the gateway maps to 502.
    Close,

    /// Writes `head` verbatim, then repeats `frame` every `interval` until a write against the
    /// departed client fails.
    StreamForever {
        /// The response head written once.
        head: String,

        /// The frame written repeatedly.
        frame: String,

        /// The pause between two frames.
        interval: Duration,
    },

    /// Writes `head` verbatim, waits `delay`, writes `frame` once, and closes — so a client can
    /// leave before any frame was enqueued.
    StreamAfterDelay {
        /// The response head written once.
        head: String,

        /// The single frame written after the delay.
        frame: String,

        /// The pause between the head and the frame.
        delay: Duration,
    },
}

/// The `METHOD /target` lines one [`MockUpstream`] has recorded, one entry per request in
/// arrival order.
pub(crate) type RecordedRequests = Arc<Mutex<Vec<String>>>;

/// A tiny HTTP/1.1 mock upstream the tests dispatch against: it reads every request head plus
/// the body the head declares, records the request's `METHOD /target` line, answers from its
/// response source, and shuts its accept loop down on drop.
pub(crate) struct MockUpstream {
    /// The socket address the mock answers at.
    address: SocketAddr,

    /// The shutdown signal stopping the mock's accept loop early; dropping the mock stops it
    /// too.
    pub(crate) shutdown: oneshot::Sender<()>,
}

impl MockUpstream {
    /// Serves `responses` round-robin: one entry per request, each framed as a 200 JSON
    /// response.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start(responses: Vec<String>) -> Self {
        Self::start_recording(responses).await.0
    }

    /// Serves `responses` round-robin as complete HTTP responses — status line, headers, and
    /// body — sent verbatim: one entry per request, so a fixture controls the response status
    /// itself (a redirect fixture in particular).
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_raw(responses: Vec<String>) -> Self {
        Self::start_serving(canned(responses)).await.0
    }

    /// Serves `responses` round-robin, each entry framed as a 200 JSON response, and records
    /// every request's `METHOD /target` line, so a test can pin the exact URLs the engine
    /// rendered.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_recording(responses: Vec<String>) -> (Self, RecordedRequests) {
        let framed = responses
            .into_iter()
            .map(|body| {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            })
            .collect();

        Self::start_serving(canned(framed)).await
    }

    /// Serves every request from `respond`, so a test flips the answer between scenarios while
    /// the mock stays up. The requests are recorded, but the recorder is dropped.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_responding(
        respond: impl Fn() -> MockAction + Send + Sync + 'static,
    ) -> Self {
        Self::start_serving(Arc::new(respond)).await.0
    }

    /// The `http://` base URL the mock answers at.
    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Binds an ephemeral listener and serves every accepted connection: the request head plus
    /// its declared body is read, the `METHOD /target` line is recorded, and `respond` picks the
    /// transport action. The accept loop ends when the shutdown signal fires — on drop or on an
    /// explicit send.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind or address its listener — a broken mock is the failure
    /// report.
    async fn start_serving(
        respond: Arc<dyn Fn() -> MockAction + Send + Sync>,
    ) -> (Self, RecordedRequests) {
        #[expect(
            clippy::unwrap_used,
            reason = "a mock that cannot bind is the failure report"
        )]
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

        #[expect(clippy::unwrap_used, reason = "a bound listener reports its address")]
        let address = listener.local_addr().unwrap();

        let (shutdown, mut shutdown_received) = oneshot::channel::<()>();

        let seen: RecordedRequests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);

        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_received => break,
                    accepted = listener.accept() => accepted,
                };

                let Ok((mut socket, _)) = accepted else {
                    break;
                };

                let respond = Arc::clone(&respond);
                let seen = Arc::clone(&recorded);

                tokio::spawn(async move {
                    let Some(request_head) = read_request(&mut socket).await else {
                        return;
                    };

                    record_request(&request_head, &seen);

                    serve_action(&mut socket, respond()).await;
                });
            }
        });

        (Self { address, shutdown }, seen)
    }
}

/// The round-robin canned response source: a single entry serves every request, a multi-entry
/// fixture walks its entries in order, and an empty fixture is the test bug the panic reports.
///
/// # Panics
///
/// Panics if the mutex is poisoned — a prior holder panicked while holding the lock — or if the
/// fixture is empty.
fn canned(responses: Vec<String>) -> Arc<dyn Fn() -> MockAction + Send + Sync> {
    let responses = Arc::new(Mutex::new(responses));

    Arc::new(move || {
        #[expect(
            clippy::unwrap_used,
            reason = "a mutex poisoned by a panicking sibling request is the failure report"
        )]
        let mut guard = responses.lock().unwrap();

        #[expect(
            clippy::indexing_slicing,
            reason = "an empty fixture is a test bug the panic reports"
        )]
        let next = if guard.len() > 1 {
            guard.remove(0)
        } else {
            guard[0].clone()
        };

        MockAction::Respond(next)
    })
}

/// Reads one HTTP/1.1 request head — through its blank line — plus the `Content-Length` body
/// the head declares, returning the head text; `None` when the client left before a head
/// arrived. Read failures count as the client's departure.
async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut collected: Vec<u8> = Vec::new();

    let head_end = loop {
        let read = stream.read_buf(&mut collected).await.unwrap_or(0);

        if read == 0 {
            return None;
        }

        if let Some(position) = collected
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            break position + 4;
        }
    };

    #[expect(
        clippy::indexing_slicing,
        reason = "head_end is an in-bounds offset into the same buffer"
    )]
    let head = String::from_utf8_lossy(&collected[..head_end]).into_owned();

    let body_length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;

            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    let mut body_read = collected.len() - head_end;

    while body_read < body_length {
        let read = stream.read_buf(&mut collected).await.unwrap_or(0);

        if read == 0 {
            break;
        }

        body_read += read;
    }

    Some(head)
}

/// Records the request line's `METHOD /target` pair; a head without a well-formed request line
/// is answered but not recorded.
///
/// # Panics
///
/// Panics if the mutex is poisoned — a prior holder panicked while holding the lock.
fn record_request(head: &str, seen: &RecordedRequests) {
    if let Some(request_line) = head.lines().next() {
        let mut parts = request_line.split_whitespace();

        if let (Some(method), Some(target)) = (parts.next(), parts.next()) {
            #[expect(
                clippy::unwrap_used,
                reason = "a mutex poisoned by a panicking sibling request is the failure report"
            )]
            seen.lock().unwrap().push(format!("{method} {target}"));
        }
    }
}

/// Carries one connection's [`MockAction`] out against `socket`: write failures — a client that
/// already left — end the action where a write is the only exit, and are ignored elsewhere,
/// exactly like a close.
async fn serve_action(socket: &mut TcpStream, action: MockAction) {
    match action {
        MockAction::Respond(response) => {
            let _ = socket.write_all(response.as_bytes()).await;
        }

        MockAction::Close => {}

        MockAction::StreamForever {
            head,
            frame,
            interval,
        } => {
            if socket.write_all(head.as_bytes()).await.is_ok() {
                loop {
                    if socket.write_all(frame.as_bytes()).await.is_err() {
                        break;
                    }

                    tokio::time::sleep(interval).await;
                }
            }
        }

        MockAction::StreamAfterDelay { head, frame, delay } => {
            if socket.write_all(head.as_bytes()).await.is_ok() {
                tokio::time::sleep(delay).await;

                let _ = socket.write_all(frame.as_bytes()).await;
            }
        }
    }
}
