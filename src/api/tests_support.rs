//! Shared serving-test fixtures: the gateway test harness driving the real `/api` router, the
//! storage seeds building providers, models, routes, pools, and serving keys, and the upstream
//! response fixtures the mock upstream serves. Fixtures specific to one surface live beside that
//! surface's tests.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a fixture that cannot install, store, or serve is that test's failure report"
)]

use crate::{
    context::Context,
    database::Database,
    http_client,
    provider::{DiscoveredModel, ProviderDefinition, StoredCredentials},
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{
        api_keys::{self, ApiKeyFields},
        models, provider_definitions, providers, quota_blocks, routing,
    },
    test_support::{
        TempDatabaseGuard, configuration_lock, install_configuration, mock_upstream, temp_database,
    },
};
use axum::{Router, body::Body, response::Response};
use serde_json::{Value, json};
use std::{error::Error as StdError, num::NonZero, path::PathBuf, sync::MutexGuard};
use tower::Service;

/// The serving secret the seeded all-allowing key carries, and the credential the surface tests
/// present.
pub(crate) const KEY_SECRET: &str = "sk-serving-fixture";

/// The static upstream credential the seeded provider rows carry; request-pinning assertions
/// read it back out of the rendered upstream headers.
pub(crate) const UPSTREAM_SECRET: &str = "sk-upstream-fixture";

/// The per-request body read cap, mirroring the outward body limit the handlers enforce.
const BODY_READ_LIMIT: usize = 64 * 1024 * 1024;

/// Installs the process-global test configuration once and initializes the affinity process key,
/// mirroring the server startup order. The configuration lock serializes the installing test
/// threads, so exactly one writes the fixture file and installs while concurrent openers receive
/// the winner's already-installed configuration.
pub(crate) fn install_gateway_configuration() {
    let lock = configuration_lock();

    install_configuration();

    drop(lock);

    crate::affinity::initialize_key();
}

/// The gateway under test: one temporary main-registry database, the request context built the
/// way the server builds it, and the real `/api` router driven in process.
pub(crate) struct TestGateway {
    /// The gateway's database, also cloned into the context and the router's authentication
    /// middleware.
    database: Database,

    /// The request context the router serves through.
    context: Context,

    /// The complete `/api` router with its state applied.
    router: Router,

    /// The throwaway directory the test's api-log rows record into.
    _api_log_directory: PathBuf,
}

impl TestGateway {
    /// Opens the gateway over a fresh temporary database: the shared configuration is installed,
    /// the context is assembled exactly like the server startup assembles it, and the real `/api`
    /// router is built over it. Returns the gateway beside the database's drop guard, which must
    /// outlive every use of the gateway.
    pub(crate) async fn open() -> (Self, TempDatabaseGuard) {
        install_gateway_configuration();

        let (database, guard) = temp_database().await;

        // Every test records its accounting rows into its own throwaway directory, so tests
        // never share api-log files.
        let api_log_directory =
            std::env::temp_dir().join(format!("janusgate-test-api-logs-{}", rand::random::<u64>()));

        let context = Context {
            database: database.clone(),
            api_log_store: crate::storage::api_logs::ApiLogStore::new(
                api_log_directory.clone(),
                NonZero::<u32>::new(1).expect("the fixture retention is a nonzero literal"),
            ),
            affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
            quota_coordinator: std::sync::Arc::new(
                crate::provider::quota::coordinator::Coordinator::default(),
            ),
            data_plane: http_client::data_plane(),
            control_plane: http_client::control_plane(),
        };

        let router = super::router(database.clone()).with_state(context.clone());

        (
            Self {
                database,
                context,
                router,
                _api_log_directory: api_log_directory,
            },
            guard,
        )
    }

    /// The gateway's database handle, for the seeds this file and the surface test modules use.
    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    /// The gateway's request context, for test modules that drive pipeline stages directly.
    pub(crate) fn context(&self) -> &Context {
        &self.context
    }

    /// Serves one request through the real router.
    pub(crate) async fn send(&self, request: axum::extract::Request) -> Response {
        let mut router = self.router.clone();

        std::future::poll_fn(|cx| {
            <Router as Service<http::Request<Body>>>::poll_ready(&mut router, cx)
        })
        .await
        .expect("the router's readiness never fails");

        router
            .call(request)
            .await
            .expect("the router's service error is infallible")
    }

    /// Serves a POST with a JSON body and the given headers.
    pub(crate) async fn post_json(
        &self,
        path: &str,
        headers: &[(&str, String)],
        body: Value,
    ) -> Response {
        let mut builder = http::Request::builder()
            .method(http::Method::POST)
            .uri(path);

        for (name, value) in headers {
            builder = builder.header(*name, value);
        }

        let request = builder
            .body(Body::from(
                serde_json::to_vec(&body).expect("fixture bodies serialize"),
            ))
            .expect("static request parts are always valid");

        self.send(request).await
    }

    /// Serves a GET with the given headers.
    pub(crate) async fn get(&self, path: &str, headers: &[(&str, String)]) -> Response {
        let mut builder = http::Request::builder().method(http::Method::GET).uri(path);

        for (name, value) in headers {
            builder = builder.header(*name, value);
        }

        let request = builder.body(Body::empty()).expect("static request parts");

        self.send(request).await
    }

    /// Serves a POST whose body is the raw string, for streaming and malformed-body requests.
    pub(crate) async fn post_raw(
        &self,
        path: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> Response {
        let mut builder = http::Request::builder()
            .method(http::Method::POST)
            .uri(path);

        for (name, value) in headers {
            builder = builder.header(*name, value);
        }

        let request = builder
            .body(Body::from(body.to_owned()))
            .expect("static request parts are always valid");

        self.send(request).await
    }
}

/// Collects a response body as lossy text.
pub(crate) async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), BODY_READ_LIMIT)
        .await
        .expect("the response body reads");

    String::from_utf8_lossy(&bytes).into_owned()
}

/// Collects a response body and parses it as JSON.
pub(crate) async fn body_json(response: Response) -> Result<Value, Box<dyn StdError>> {
    let bytes = axum::body::to_bytes(response.into_body(), BODY_READ_LIMIT).await?;

    Ok(serde_json::from_slice(&bytes)?)
}

/// The `Authorization: Bearer <secret>` credential header pair of `secret`.
pub(crate) fn bearer(secret: &str) -> (&'static str, String) {
    ("authorization", format!("Bearer {secret}"))
}

/// The `x-goog-api-key` credential header pair of `secret`.
pub(crate) fn goog_key(secret: &str) -> (&'static str, String) {
    ("x-goog-api-key", secret.to_owned())
}

/// The Anthropic credential header pair of `secret`, beside the required version header.
pub(crate) fn anthropic_headers(secret: &str) -> Vec<(&'static str, String)> {
    vec![
        ("x-api-key", secret.to_owned()),
        ("anthropic-version", "2023-06-01".to_owned()),
    ]
}

/// The all-allowing access policy every seeded default key carries.
pub(crate) fn allow_all_policy() -> crate::access_control::ApiKeyAccessControl {
    crate::access_control::ApiKeyAccessControl {
        default_access: crate::access_control::AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    }
}

/// Inserts one serving key carrying `secret` and `policy`, returning its row identity.
pub(crate) async fn seed_key(
    database: &Database,
    name: &str,
    secret: &str,
    policy: &crate::access_control::ApiKeyAccessControl,
) -> Result<i64, Box<dyn StdError>> {
    let key = api_keys::insert_api_key(
        database,
        &ApiKeyFields {
            name,
            key: secret,
            is_enabled: true,
            access_control: policy,
        },
    )
    .await?;

    Ok(key.id)
}

/// Inserts one serving key carrying [`KEY_SECRET`] with the all-allowing default policy.
pub(crate) async fn seed_allowing_key(database: &Database) -> Result<i64, Box<dyn StdError>> {
    seed_key(database, "allowing", KEY_SECRET, &allow_all_policy()).await
}

/// Parses one definition payload fixture; a fixture that does not parse is the test bug.
pub(crate) fn parse_payload(payload: Value) -> crate::provider::DefinitionPayload {
    serde_json::from_value(payload).expect("fixture definition payload parses")
}

/// Inserts one definition named `name` at version 1 over `payload`, returning the stored
/// definition the provider rows bind.
pub(crate) async fn seed_definition(
    database: &Database,
    name: &str,
    payload: Value,
) -> Result<ProviderDefinition, Box<dyn StdError>> {
    let payload = parse_payload(payload);

    let definition = provider_definitions::insert_definition(
        database,
        provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload: &payload,
            created_at: 1_000,
        },
    )
    .await?;

    Ok(definition)
}

/// The static-API-key provider definition over `url_parts` with optional extra inference
/// members (`stream_url`, `response_unwrap`, `upstream_streaming`, `quota_exhaustion`,
/// `render_options`): the chat-completions wire format with one bearer credential header.
pub(crate) fn chat_definition(url_parts: Value, inference_overrides: Value) -> Value {
    let mut inference = json!({
        "url": url_parts,
        "headers": [
            {
                "name": "Authorization",
                "value": [{"type": "literal", "value": "Bearer "}, {"type": "secret"}]
            }
        ],
        "upstream_streaming": "match_client"
    });

    if let Value::Object(overrides) = inference_overrides
        && let Value::Object(inference_object) = &mut inference
    {
        for (name, value) in overrides {
            inference_object.insert(name, value);
        }
    }

    json!({
        "format": "openai_chat_completion",
        "inference": inference,
        "credential": {"type": "api_key"}
    })
}

/// Inserts one enabled static-API-key provider named `name` with `prefix`, bound to `definition`,
/// upstreaming to `base_url`.
pub(crate) async fn seed_provider(
    database: &Database,
    name: &str,
    prefix: &str,
    base_url: &str,
    definition: &ProviderDefinition,
) -> Result<i64, Box<dyn StdError>> {
    let credentials = StoredCredentials::ApiKey {
        api_key: UPSTREAM_SECRET.to_owned(),
    };

    let provider = providers::insert_provider(
        database,
        providers::NewProvider {
            name,
            model_prefix: prefix,
            base_url: Some(base_url),
            credentials: &credentials,
            definition,
            is_enabled: true,
        },
    )
    .await?;

    Ok(provider.id)
}

/// Discovers one enabled upstream model on `provider_id`, returning its row identity.
pub(crate) async fn seed_model(
    database: &Database,
    provider_id: i64,
    upstream_id: &str,
) -> Result<i64, Box<dyn StdError>> {
    let entries = models::apply_discovered_models(
        database,
        provider_id,
        &[DiscoveredModel {
            upstream_id: upstream_id.to_owned(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }],
    )
    .await?;

    entries
        .iter()
        .find(|entry| entry.model.upstream_id == upstream_id)
        .map(|entry| entry.model.id)
        .ok_or_else(|| "the fixture model row is missing after discovery".into())
}

/// Creates one explicit public route over `target`, returning its row identity.
pub(crate) async fn seed_route(
    database: &Database,
    public_id: &str,
    target: &ModelRouteTarget,
) -> Result<i64, Box<dyn StdError>> {
    Ok(routing::create_route(database, public_id, true, target).await?)
}

/// Creates one enabled pool named `name` with `prefix` and `members`, returning its row identity.
pub(crate) async fn seed_pool(
    database: &Database,
    name: &str,
    prefix: &str,
    members: &[ProviderPoolMember],
) -> Result<i64, Box<dyn StdError>> {
    Ok(routing::create_pool(database, name, prefix, true, members).await?)
}

/// Records one account-wide quota restriction block on `provider_id` without reset evidence,
/// returning the provider's captured quota generation the record committed under.
pub(crate) async fn seed_quota_block(
    database: &Database,
    provider_id: i64,
) -> Result<i64, Box<dyn StdError>> {
    let generation = quota_blocks::load_state(database, provider_id)
        .await?
        .map(|state| state.fence.generation)
        .ok_or_else(|| "the fixture provider row is missing".to_string())?;

    quota_blocks::record_rejection(
        database,
        provider_id,
        generation,
        &["fixture".to_owned(), "exhausted".to_owned()],
        &crate::provider::quota::QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await?;

    Ok(generation)
}

/// The complete non-streaming chat-completions response body the chat upstream fixtures carry:
/// one primary choice with `text` and the given token usage.
pub(crate) fn upstream_chat_json(text: &str, prompt_tokens: u64, completion_tokens: u64) -> Value {
    json!({
        "id": "chatcmpl-upstream",
        "object": "chat.completion",
        "model": "upstream-model-name",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens
        }
    })
}

/// The complete SSE frame sequence of one streaming chat-completions upstream turn: a role
/// chunk, two content chunks, a usage chunk, and the `[DONE]` sentinel.
pub(crate) fn upstream_chat_sse_frames() -> Vec<String> {
    let chunk = |payload: Value| format!("data: {payload}\n\n");

    vec![
        chunk(json!({
            "id": "chatcmpl-upstream",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}}]
        })),
        chunk(json!({
            "id": "chatcmpl-upstream",
            "choices": [{"index": 0, "delta": {"content": "Hello "}}]
        })),
        chunk(json!({
            "id": "chatcmpl-upstream",
            "choices": [{"index": 0, "delta": {"content": "stream"}}]
        })),
        chunk(json!({
            "id": "chatcmpl-upstream",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9}
        })),
        "data: [DONE]\n\n".to_owned(),
    ]
}

/// The complete non-streaming GenerateContent response body the Gemini upstream fixtures carry.
pub(crate) fn upstream_gemini_json(text: &str) -> Value {
    json!({
        "candidates": [
            {
                "content": {"role": "model", "parts": [{"text": text}]},
                "finishReason": "STOP"
            }
        ],
        "usageMetadata": {
            "promptTokenCount": 4,
            "candidatesTokenCount": 3,
            "totalTokenCount": 7
        }
    })
}

/// The complete non-streaming Messages response body the Anthropic upstream fixtures carry.
pub(crate) fn upstream_anthropic_json(text: &str) -> Value {
    json!({
        "id": "msg-upstream",
        "type": "message",
        "role": "assistant",
        "model": "upstream-model-name",
        "content": [{"type": "text", "text": text}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 6}
    })
}

/// Serializes one JSON fixture body into the request bytes a client sends.
pub(crate) fn json_bytes(body: Value) -> Vec<u8> {
    serde_json::to_vec(&body).expect("fixture bodies serialize")
}

/// Locks one mock upstream's recording list: a poisoned mutex is a prior sibling test's panic,
/// which is that test's own failure report.
pub(crate) fn lock_recordings(
    requests: &mock_upstream::RecordedRequests,
) -> MutexGuard<'_, Vec<mock_upstream::RecordedRequest>> {
    #[expect(
        clippy::unwrap_used,
        reason = "a mutex poisoned by a panicking test is that test's failure report"
    )]
    requests.lock().unwrap()
}

/// Parses one recorded upstream request body as JSON: a recorded body that does not parse is
/// the dispatch-pinning test's own failure report.
pub(crate) fn recorded_json(body: &str) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "a recorded body that does not parse is the test's failure report"
    )]
    serde_json::from_str(body).expect("recorded upstream bodies are JSON")
}
