//! Acceptance tests for the remote-compaction pipeline (README §6.8): every scenario drives the
//! real serving pipeline — parse, routing, restore, strategy resolution, and the outcome renderings
//! — end to end against a body-capturing mock upstream, asserting dispatch counts, call
//! order, request bodies, emitted lifecycles, reconstructed next inputs, and
//! the per-dispatch accounting the shipped behavior owes.

#![expect(
    clippy::expect_used,
    reason = "fixtures parse, seed, and render values constructed to succeed; a broken \
              fixture is the test's own failure report"
)]
#![expect(
    clippy::panic,
    reason = "a failing fixture assertion or pattern-match is the test failure report"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "fixtures index values they constructed themselves"
)]

use crate::{
    compaction::{history, summary::DEFAULT_SUMMARY_PROMPT},
    compaction_model::{
        boundary::BoundaryPreparation,
        envelope::{
            EnvelopePayload, GatewaySummaryPayload, NativeReplayWindow, NativeWindowPayload,
            ReplacementMode,
        },
        identity::{InstructionRole, ItemIdentity},
        mechanism::{Mechanism, MechanismSupport},
        sealing,
    },
    context::Context,
    format::{
        BoundaryPlacement, CompactionBoundary, NativeArtifact, ParsedCompaction, UpstreamFormat,
    },
    ir::{IrMessage, IrPart, IrRole},
    provider::{
        Model, ModelRouting, Provider, StoredCredentials,
        definition::{DefinitionPayload, ProviderDefinition},
    },
    storage::{
        api_keys::{self, ApiKeyFields},
        models::{ModelCreate, insert_model_checked},
    },
    test_support::{
        self, MockAction, RecordedHttpRequest, RecordedHttpRequests, TempDbGuard,
        insert_definition, insert_provider,
    },
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::num::NonZeroI64;
use tower::ServiceExt as _;

/// The gateway API key the scenarios authenticate with.
const API_KEY: &str = "sk-acceptance";

/// The public model id every scenario routes to.
const MODEL: &str = "test-model";

/// The native compaction ciphertext the scripted windows carry.
const NATIVE_STATE: &str = "native-state";

/// One running scenario: the routing app, its seeded database, the mock
/// upstream, and the recorded requests.
struct Scenario {
    /// Shared runtime cache, permitting explicit evidence fixtures without a conversation store.
    affinity_cache: std::sync::Arc<crate::affinity::Cache>,
    /// Authenticated key identity for namespace-scoped evidence fixtures.
    api_key_id: i64,
    /// The routing app; cloned per request.
    app: axum::Router<()>,

    /// The seeded database with its cleanup guard.
    _database: (crate::database::Database, TempDbGuard),

    /// The mock upstream; dropped at the scenario's end.
    _mock: crate::test_support::MockUpstream,

    /// Every request the mock received, in arrival order.
    recorded: RecordedHttpRequests,
}

/// Starts one scenario: the shared configuration (installed once per process),
/// a fresh database, a scripted mock upstream, and the seeded provider row.
///
/// `compaction` is the definition's compaction section document, `facts` the
/// model row's compaction-support override facts, and `max_input_tokens` the
/// model's effective input capacity. `always_streaming` pins the definition's
/// upstream streaming mode to `always` instead of `match_client`.
///
/// # Panics
///
/// Panics when a fixture insert, the mock bind, or the configuration install
/// fails — a broken fixture environment is the failure report.
async fn scenario(
    format: UpstreamFormat,
    compaction: Option<Value>,
    facts: Option<&crate::compaction_model::support_facts::CompactionSupportFacts>,
    max_input_tokens: Option<i64>,
    always_streaming: bool,
    actions: Vec<MockAction>,
) -> Scenario {
    scenario_with_quota_policy(
        format,
        compaction,
        facts,
        max_input_tokens,
        always_streaming,
        actions,
        None,
    )
    .await
}

/// Starts a scenario with an authored inference rejection policy.
///
/// # Panics
///
/// Panics when a typed fixture cannot seed or render.
#[allow(clippy::too_many_arguments)]
async fn scenario_with_quota_policy(
    format: UpstreamFormat,
    compaction: Option<Value>,
    facts: Option<&crate::compaction_model::support_facts::CompactionSupportFacts>,
    max_input_tokens: Option<i64>,
    always_streaming: bool,
    actions: Vec<MockAction>,
    quota_exhaustion: Option<crate::provider::definition::quota_exhaustion::QuotaExhaustion>,
) -> Scenario {
    // The configuration is process-global: hold the lock only across the
    // install, so the end-to-end scenario's reloads can never interleave with
    // an installation while every later caller fast-paths to the installed
    // value.
    let _configuration = test_support::configuration_lock();

    let database_path = std::env::temp_dir().join(format!(
        "janusgate-compaction-acceptance-{}.sqlite",
        rand::random::<u64>()
    ));

    test_support::install_test_configuration(&database_path, None);

    drop(_configuration);

    let (database, guard) = test_support::create_test_database().await;

    let (mock, recorded) =
        test_support::MockUpstream::start_scripted_recording_bodies(actions).await;

    let base = mock.base();

    let mut payload = definition_payload(format, compaction, always_streaming);
    payload.inference.quota_exhaustion = quota_exhaustion;

    let definition = insert_definition(&database, "acceptance", payload).await;

    let provider = insert_provider(
        &database,
        &definition,
        "acceptance",
        Some(&base),
        StoredCredentials::ApiKey {
            api_key: String::from("upstream-secret"),
        },
    )
    .await;

    let model = insert_model_checked(
        &database,
        ModelCreate {
            provider_id: provider.id,
            upstream_id: MODEL,
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: max_input_tokens,
            max_output_tokens_override: None,
            compaction_support_override: facts,
        },
    )
    .await
    .expect("the fixture model inserts");
    crate::storage::routing::create_route(
        &database,
        MODEL,
        true,
        &crate::routing::ModelRouteTarget::Direct {
            model_id: model.model.id,
        },
    )
    .await
    .expect("fixture route publishes");

    let api_key_id = api_keys::insert_api_key(
        &database,
        &ApiKeyFields {
            name: "acceptance",
            key: API_KEY,
            is_enabled: true,
            access_control: &crate::test_support::allow_all_access(),
        },
    )
    .await
    .expect("the fixture api key inserts");

    let affinity_cache = std::sync::Arc::new(crate::affinity::Cache::default());
    let context = Context {
        database: database.clone(),
        api_log_store: None,
        affinity_cache: affinity_cache.clone(),
        quota_coordinator: std::sync::Arc::new(
            crate::provider::quota::coordinator::Coordinator::default(),
        ),
        data_plane: reqwest::Client::new(),
        control_plane: reqwest::Client::new(),
    };

    // The API router nests at `/api` exactly as the server mounts it, so the
    // scenarios address the real surface paths.
    let app = axum::Router::new()
        .nest("/api", crate::api::router(database.clone()))
        .with_state(context);

    Scenario {
        affinity_cache,
        api_key_id: api_key_id.id,
        app,
        _database: (database, guard),
        _mock: mock,
        recorded,
    }
}

/// Builds the fixture definition payload of `format` whose inference URL
/// addresses the provider's base and whose compaction section is `compaction`.
///
/// # Panics
///
/// Panics if the fixture payload fails to parse, which its shape prevents.
fn definition_payload(
    format: UpstreamFormat,
    compaction: Option<Value>,
    always_streaming: bool,
) -> DefinitionPayload {
    let suffix = match format {
        UpstreamFormat::OpenAiResponse => "/responses",
        UpstreamFormat::AnthropicMessages => "/messages",
        _ => "/chat/completions",
    };

    serde_json::from_value(json!({
        "format": format.as_str(),
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": suffix}],
            "upstream_streaming": if always_streaming { "always" } else { "match_client" },
        },
        "credential": {"type": "api_key"},
        "compaction": compaction,
    }))
    .expect("the fixture definition payload parses")
}

/// The compaction section document declaring the standalone compact operation
/// with the given classifier statuses.
///
/// # Panics
///
/// Panics if the fixture document fails to parse, which its shape prevents.
fn standalone_section(statuses: Vec<u16>) -> Value {
    json!({
        "strategy": "native_preferred",
        "summary_budget": 4096,
        "retention_budget": 1,
        "mechanisms": {
            "responses_standalone_compact": {
                "default": "supported",
                "operation": {
                    "url": [{"type": "base"}, {"type": "literal", "value": "/responses/compact"}],
                    "unsupported": {"statuses": statuses},
                },
            },
        },
    })
}

/// The compaction section document carrying no mechanism declaration: the
/// unknown-support posture whose one-token retention target makes the whole
/// history of the compact fixtures the summarized prefix.
///
/// # Panics
///
/// Panics if the fixture document fails to parse, which its shape prevents.
fn summarize_only_section() -> Value {
    json!({
        "strategy": "native_preferred",
        "summary_budget": 4096,
        "retention_budget": 1,
    })
}

/// The compaction section document declaring both Messages mechanisms
/// supported with their canonical beta overlays.
///
/// # Panics
///
/// Panics if the fixture document fails to parse, which its shape prevents.
fn messages_mechanisms_section(on_demand: &str, threshold: &str) -> Value {
    json!({
        "mechanisms": {
            "messages_on_demand_compaction": {
                "default": on_demand,
                "overlay_headers": [
                    {
                        "name": "anthropic-beta",
                        "value": [{"type": "literal", "value": "compact-2026-09-04"}],
                    }
                ],
            },
            "messages_threshold_compaction": {
                "default": threshold,
                "overlay_headers": [
                    {
                        "name": "anthropic-beta",
                        "value": [{"type": "literal", "value": "compact-2026-01-12"}],
                    }
                ],
            },
        },
    })
}

/// The automatic compaction declaration document.
///
/// # Panics
///
/// Panics if the fixture document fails to parse, which its shape prevents.
fn automatic_section(support: &str) -> Value {
    json!({
        "mechanisms": {
            "responses_automatic_compaction": {"default": support},
        },
    })
}

/// Model support facts overriding one mechanism's support.
///
/// # Panics
///
/// Panics if the fixture facts fail to construct, which the non-empty pairs
/// prevent.
fn facts(
    pairs: &[(Mechanism, MechanismSupport)],
) -> crate::compaction_model::support_facts::CompactionSupportFacts {
    crate::compaction_model::support_facts::CompactionSupportFacts::from_pairs(
        pairs.iter().copied(),
    )
    .expect("the fixture pairs are non-empty")
}

/// The `Authorization`/`Content-Type` header set of an OpenAI-surface request.
fn openai_headers() -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {API_KEY}")),
        ("Content-Type", String::from("application/json")),
    ]
}

/// The header set of an Anthropic-surface request, with the beta header when
/// the request carries a control a beta gates.
fn messages_headers(beta: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("x-api-key", String::from(API_KEY)),
        ("anthropic-version", String::from("2023-06-01")),
        ("Content-Type", String::from("application/json")),
    ];

    if let Some(beta) = beta {
        headers.push(("anthropic-beta", beta.to_string()));
    }

    headers
}

/// Sends one request through the scenario's app, returning the status and the
/// body text.
///
/// # Panics
///
/// Panics when the routing app fails — a broken app is the failure report.
async fn post(
    scenario: &Scenario,
    path: &str,
    headers: &[(&str, String)],
    body: String,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method("POST").uri(path);

    for (name, value) in headers {
        builder = builder.header(*name, value);
    }

    let request = builder.body(Body::from(body)).expect("the request builds");

    let response = scenario
        .app
        .clone()
        .oneshot(request)
        .await
        .expect("the routing app serves");

    let status = response.status();

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .expect("the response body reads");

    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Sends one JSON request through the scenario's app, returning the status and
/// the parsed body.
///
/// # Panics
///
/// Panics as [`post`], or when a body fails to parse — an unparseable body is
/// under test.
async fn post_json(
    scenario: &Scenario,
    path: &str,
    headers: &[(&str, String)],
    body: Value,
) -> (StatusCode, Value) {
    let (status, text) = post(scenario, path, headers, body.to_string()).await;

    let parsed = serde_json::from_str::<Value>(&text)
        .unwrap_or_else(|error| panic!("the response body parses ({error}): {text}"));

    (status, parsed)
}

/// Sends one OpenAI-surface JSON request.
///
/// # Panics
///
/// Panics as [`post_json`].
async fn post_openai(scenario: &Scenario, path: &str, body: Value) -> (StatusCode, Value) {
    post_json(scenario, path, &openai_headers(), body).await
}

/// Sends one Anthropic-surface JSON request with the given beta header.
///
/// # Panics
///
/// Panics as [`post_json`].
async fn post_messages(
    scenario: &Scenario,
    beta: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let headers = messages_headers(beta);

    post_json(scenario, "/api/anthropic-v1/messages", &headers, body).await
}

/// Reads the recorded requests.
///
/// # Panics
///
/// Panics if the recorder mutex is poisoned — a panicking sibling is the
/// failure report.
fn recorded(scenario: &Scenario) -> Vec<RecordedHttpRequest> {
    scenario
        .recorded
        .lock()
        .expect("the recorder reads")
        .clone()
}

/// Parses the recorded body at `index` as JSON.
///
/// # Panics
///
/// Panics when the index is absent or the body does not parse — both are the
/// failure under test.
fn recorded_body(scenario: &Scenario, index: usize) -> Value {
    let requests = recorded(scenario);

    serde_json::from_str(&requests[index].body).expect("the recorded body parses")
}

/// Asserts the recorded request count and returns the requests.
///
/// # Panics
///
/// Panics when the count diverges.
#[must_use]
fn assert_count(scenario: &Scenario, count: usize) -> Vec<RecordedHttpRequest> {
    let requests = recorded(scenario);

    assert_eq!(requests.len(), count, "recorded requests: {requests:?}");

    requests
}

/// Asserts the recorded request count where the requests themselves are not
/// read.
///
/// # Panics
///
/// Panics when the count diverges.
fn assert_dispatches(scenario: &Scenario, count: usize) {
    let requests = recorded(scenario);

    assert_eq!(requests.len(), count, "recorded requests: {requests:?}");
}

/// Distinguishes a required account's quota failure from pool exhaustion or an upstream 429.
///
/// # Panics
///
/// Panics if the normal outward envelope loses the established-account failure semantics.
fn assert_bound_quota_error(body: &Value, is_messages: bool) {
    if is_messages {
        assert_eq!(body["error"]["type"], "rate_limit_error", "{body}");
        assert_eq!(
            body["error"]["message"],
            format!("The model `{MODEL}` has exhausted quota on its required account."),
            "{body}"
        );
    } else {
        assert_eq!(
            body["error"]["code"], "bound_account_quota_exhausted",
            "{body}"
        );
    }
}

/// Frames one raw HTTP/1.1 JSON response of the given status line, with a
/// correct `Content-Length`.
///
/// # Panics
///
/// Panics if the fixture fails to render, which its shape prevents.
fn raw_json_response(status_line: &str, body: Value) -> MockAction {
    let body = body.to_string();

    MockAction::Respond(format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
}

/// Parses every `data:` payload of an SSE body into JSON.
///
/// # Panics
///
/// Panics when a data line fails to parse — the frame shape is under test.
fn sse_payloads(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .map(str::to_string)
        })
        .map(|data| serde_json::from_str(data.trim()).expect("the frame payload parses"))
        .collect()
}

/// Builds one completed Responses upstream body the ordinary pipeline parses.
///
/// # Panics
///
/// Panics if the fixture fails to render, which its shape prevents.
fn responses_completed(output: Value, input_tokens: u64, output_tokens: u64) -> MockAction {
    MockAction::respond_json(
        json!({
            "id": "resp_upstream",
            "status": "completed",
            "output": output,
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
        })
        .to_string(),
    )
}

/// Builds one completed Messages upstream body.
///
/// # Panics
///
/// Panics if the fixture fails to render, which its shape prevents.
fn anthropic_message(
    content: Value,
    stop_reason: &str,
    input_tokens: u64,
    output_tokens: u64,
) -> MockAction {
    MockAction::respond_json(
        json!({
            "id": "msg_upstream",
            "type": "message",
            "role": "assistant",
            "model": MODEL,
            "content": content,
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
        })
        .to_string(),
    )
}

/// Builds one completed Chat Completions upstream body.
///
/// # Panics
///
/// Panics if the fixture fails to render, which its shape prevents.
fn chat_completed(
    text: &str,
    finish_reason: &str,
    input_tokens: u64,
    output_tokens: u64,
) -> MockAction {
    MockAction::respond_json(
        json!({
            "id": "chatcmpl-upstream",
            "object": "chat.completion",
            "created": 1_700_000_000,
            "model": MODEL,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": finish_reason,
            }],
            "usage": {
                "prompt_tokens": input_tokens,
                "completion_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens,
            },
        })
        .to_string(),
    )
}

/// The canonical native compact window: one retained message and one native
/// compaction item.
///
/// # Panics
///
/// Panics if the fixture fails to render, which its shape prevents.
fn window_output() -> Value {
    json!([
        {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "kept answer"}],
        },
        {
            "type": "compaction",
            "id": "cmp_native",
            "encrypted_content": NATIVE_STATE,
        },
    ])
}

/// The standalone compact response the mock answers with.
///
/// # Panics
///
/// Panics if the fixture fails to render, which its shape prevents.
fn compact_response(output: Value, input_tokens: u64, output_tokens: u64) -> MockAction {
    MockAction::respond_json(
        json!({
            "id": "resp_compact",
            "object": "response.compaction",
            "created_at": 5,
            "output": output,
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
        })
        .to_string(),
    )
}

/// A text IR message.
fn text_message(role: IrRole, text: &str) -> IrMessage {
    IrMessage {
        role,
        parts: vec![IrPart::Text {
            text: String::from(text),
        }],
    }
}

/// A user text item of a Responses input array.
fn user_item(text: &str) -> Value {
    json!({"type": "message", "role": "user", "content": text})
}

/// An assistant text item of a Responses input array.
fn assistant_text_item(text: &str) -> Value {
    json!({"type": "message", "role": "assistant", "content": text})
}

/// The completed assistant output message item an Responses body carries.
fn responses_message_item(text: &str) -> Value {
    json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}],
    })
}

/// The `compaction` item carrying `state` as its opaque replay payload.
fn compaction_item(state: &str) -> Value {
    json!({
        "type": "compaction",
        "id": "cmp_gateway",
        "encrypted_content": state,
    })
}

/// Whether `haystack` carries no textual occurrence of `needle` anywhere in
/// its serialized form.
///
/// # Panics
///
/// Panics when the needle occurs — the presence is the failure under test.
fn assert_absent(haystack: &Value, needle: &str) {
    assert!(
        !haystack.to_string().contains(needle),
        "`{needle}` must be absent: {haystack}"
    );
}

/// Seals a gateway summary checkpoint for the tests that replay one, carrying
/// `summary_text` over the given preserved instructions and retained unit,
/// binding the visible content when `bind` is set.
///
/// # Panics
///
/// Panics if sealing fails — the live configuration carries the fixture keys.
fn seal_summary(
    summary_text: &str,
    preserved: &[(&str, InstructionRole)],
    retained: Option<IrMessage>,
    bind: bool,
) -> String {
    let _configuration = test_support::configuration_lock();

    test_support::install_global_configuration();

    let instruction_items: Vec<crate::format::ParsedInstructionItem> = preserved
        .iter()
        .map(|(text, role)| crate::format::ParsedInstructionItem {
            role: *role,
            text: String::from(*text),
        })
        .collect();

    let instruction_overlap: Vec<ItemIdentity> = ItemIdentity::for_instruction_window(
        instruction_items
            .iter()
            .map(|item| (item.role, item.text.as_str())),
    );

    let retained_units = retained
        .map(|message| vec![vec![message]])
        .unwrap_or_default();

    let plan = history::GatewayCheckpointPlan {
        summary_input_messages: Vec::new(),
        retained_units,
        retained_state_items: Vec::new(),
        preserved_instructions: instruction_items
            .iter()
            .zip(instruction_overlap.iter())
            .map(
                |(item, identity)| crate::compaction_model::envelope::PreservedInstruction {
                    identity: identity.clone(),
                    role: item.role,
                    text: item.text.clone(),
                },
            )
            .collect(),
        overlap_identities: instruction_overlap,
        summary_budget: 4096,
        retention_budget: 8192,
    };

    history::seal_gateway_checkpoint(
        &plan,
        String::from(summary_text),
        ReplacementMode::HistoricalCheckpoint,
        bind.then_some(summary_text),
    )
    .expect("the fixture checkpoint seals")
    .1
}

/// Seals a wrapped native window envelope for the tests that replay one.
///
/// # Panics
///
/// Panics if sealing fails — the live configuration carries the fixture keys.
fn seal_window(
    native_format: UpstreamFormat,
    window: NativeReplayWindow,
    bind: Option<&str>,
) -> String {
    let _configuration = test_support::configuration_lock();

    test_support::install_global_configuration();

    history::seal_envelope(
        crate::compaction_model::envelope::EnvelopePayload::NativeWindow(NativeWindowPayload {
            retained_messages: Vec::new(),
            retained_state_items: Vec::new(),
            producer_instance_id: String::from("provider:1"),
            upstream_model_id: String::from(MODEL),
            mechanism: Mechanism::ResponsesStandaloneCompact,
            native_format,
            required_prefix_identity: None,
            window,
        }),
        ReplacementMode::CanonicalWindow,
        bind,
    )
    .expect("the fixture window seals")
    .1
}

/// Scenario 1 — native standalone with retained messages and reasoning: the
/// compact passthrough carries the complete output window, a replay of that
/// window reaches the upstream verbatim (no pruning, no encrypted-reasoning
/// loss), and a second compaction of the restored history succeeds.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn native_standalone_survives_compact_replay_and_recompaction() {
    let window = json!([
        {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "kept answer"}],
        },
        {
            "type": "reasoning",
            "id": "rs_native",
            "summary": [{"type": "summary_text", "text": "thought"}],
            "encrypted_content": "reasoning-state",
        },
        {
            "type": "compaction",
            "id": "cmp_native",
            "encrypted_content": NATIVE_STATE,
        },
    ]);

    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(Vec::new())),
        None,
        None,
        false,
        vec![
            compact_response(window.clone(), 120, 30),
            responses_completed(json!([responses_message_item("continued")]), 30, 4),
            compact_response(window, 90, 20),
        ],
    )
    .await;

    // Compact: the native passthrough.
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({
            "model": MODEL,
            "input": [user_item("old question"), assistant_text_item("old answer")],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "response.compaction");
    assert_eq!(body["output"].as_array().expect("output").len(), 3);
    assert_eq!(body["output"][1]["encrypted_content"], "reasoning-state");
    assert_eq!(body["output"][2]["encrypted_content"], NATIVE_STATE);
    assert_eq!(body["usage"]["input_tokens"], 120);

    // The compact dispatch hit the operation URL with the canonical window.
    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /responses/compact");

    let compact_body = recorded_body(&scenario, 0);
    assert_eq!(compact_body["model"], MODEL);
    assert_eq!(compact_body["input"].as_array().expect("input").len(), 2);
    assert!(compact_body.get("stream").is_none());

    // Replay: the client carries the compact output forward as its input.
    let replay_input = body["output"].clone();

    let (status, _replayed) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({"model": MODEL, "input": replay_input, "store": false}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 2);
    assert_eq!(requests[1].line(), "POST /responses");

    // The window reaches the upstream verbatim: every item once, reasoning
    // ciphertext included.
    let upstream = recorded_body(&scenario, 1);
    assert_eq!(upstream["input"], replay_input);

    // Compact again over the restored history.
    let (status, second) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({"model": MODEL, "input": replay_input}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["object"], "response.compaction");

    let requests = assert_count(&scenario, 3);
    assert_eq!(requests[2].line(), "POST /responses/compact");

    // A replay artifact and a bound native window never confer route permission.
    api_keys::update_api_key(
        &scenario._database.0,
        scenario.api_key_id,
        ApiKeyFields {
            name: "revoked",
            key: API_KEY,
            is_enabled: true,
            access_control: &crate::access_control::ApiKeyAccessControl {
                default_access: crate::access_control::AccessDecision::Deny,
                owner_defaults: Vec::new(),
                route_overrides: Vec::new(),
            },
        },
    )
    .await
    .expect("revoke replay key");
    for path in [
        "/api/openai-v1/responses",
        "/api/openai-v1/responses/compact",
    ] {
        let (status, _) = post_openai(
            &scenario,
            path,
            json!({"model": MODEL, "input": replay_input.clone()}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let _requests = assert_count(&scenario, 3);
    }

    drop(scenario);
}

/// Scenario 2 — a Codex explicit trigger adapted to the xAI standalone
/// compact: one completed compaction item and terminal response, exactly one
/// compact dispatch, and a replay that restores the canonical window once
/// while the covered turns stay gone.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn codex_trigger_adapts_to_the_standalone_window() {
    // The trigger mechanism is declared unsupported; the standalone support is
    // a model fact over an unknown declaration default.
    let section = json!({
        "mechanisms": {
            "responses_explicit_trigger": {"default": "unsupported"},
            "responses_standalone_compact": {
                "default": "unknown",
                "operation": {
                    "url": [{"type": "base"}, {"type": "literal", "value": "/responses/compact"}],
                },
            },
        },
    });

    let standalone_facts = facts(&[(
        Mechanism::ResponsesStandaloneCompact,
        MechanismSupport::Supported,
    )]);

    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(section),
        Some(&standalone_facts),
        None,
        false,
        vec![compact_response(window_output(), 200, 50)],
    )
    .await;

    let (status, text) = post(
        &scenario,
        "/api/openai-v1/responses",
        &openai_headers(),
        json!({
            "model": MODEL,
            "input": [
                user_item("old question one"),
                user_item("old question two"),
                {"type": "compaction_trigger"},
            ],
            "stream": true,
            "store": false,
        })
        .to_string(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // Exactly one dispatch: the standalone compact, never the trigger
    // passthrough.
    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /responses/compact");

    let compact_body = recorded_body(&scenario, 0);
    let input = compact_body["input"].as_array().expect("input");
    assert_eq!(input.len(), 2, "the compact body carries the window alone");

    // The synthesized lifecycle: exactly one compaction item and a completed
    // terminal carrying the same item.
    let payloads = sse_payloads(&text);

    let types: Vec<&str> = payloads
        .iter()
        .filter_map(|payload| payload["type"].as_str())
        .collect();

    assert_eq!(
        types,
        vec![
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.output_item.done",
            "response.completed",
        ]
    );

    let item = payloads[3]["item"].clone();
    assert_eq!(item["type"], "compaction");

    let carrier = item["encrypted_content"].as_str().expect("the carrier");
    assert!(carrier.starts_with(sealing::CARRIER_NAMESPACE));

    assert_eq!(payloads[4]["response"]["output"], json!([item]));
    assert_eq!(payloads[4]["response"]["status"], "completed");

    // Replay: the covered turns are superseded by the wrapped window.
    let (status, _replayed) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item("old question one"),
                user_item("old question two"),
                compaction_item(carrier),
                user_item("pending turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 2);
    assert_eq!(requests[1].line(), "POST /responses");

    // One ordinary generation whose upstream input restores the canonical
    // window once: window items first, the covered turns gone, the new user
    // last.
    let upstream = recorded_body(&scenario, 1);
    let upstream_input = upstream["input"].as_array().expect("input");

    assert_eq!(upstream_input.len(), 3);
    assert_eq!(
        upstream_input[0]["content"][0]["text"], "kept answer",
        "the window's message replays once"
    );
    assert_eq!(upstream_input[1]["encrypted_content"], NATIVE_STATE);
    assert_eq!(upstream_input[2]["content"][0]["text"], "pending turn");

    assert_absent(&upstream, "old question one");
    assert_absent(&upstream, "old question two");

    drop(scenario);
}

/// Scenario 3 — a Responses trigger backed by Claude's on-demand compaction:
/// the gateway renders the on-demand call under the operation beta, wraps the
/// block into a Responses carrier, the replay restores the leading Claude
/// block under the same beta, and re-compaction works.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn responses_trigger_backed_by_claude_on_demand() {
    let claude_compaction = || {
        anthropic_message(
            json!([{"type": "compaction", "content": "claude summary", "signature": "sig-native"}]),
            "end_turn",
            300,
            40,
        )
    };

    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(messages_mechanisms_section("supported", "supported")),
        None,
        None,
        false,
        vec![
            claude_compaction(),
            anthropic_message(
                json!([{"type": "text", "text": "continued"}]),
                "end_turn",
                20,
                5,
            ),
            claude_compaction(),
        ],
    )
    .await;

    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [user_item("old question"), {"type": "compaction_trigger"}],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // One on-demand dispatch under the operation beta, carrying the compaction
    // control member.
    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /messages");
    assert_eq!(
        requests[0].header("anthropic-beta"),
        Some("compact-2026-09-04")
    );

    let on_demand = recorded_body(&scenario, 0);
    assert_eq!(on_demand["compaction"], json!({"type": "summarize"}));
    assert_eq!(on_demand["messages"].as_array().expect("messages").len(), 1);

    // The block is wrapped into one Responses carrier item.
    let carrier = body["output"][0]["encrypted_content"]
        .as_str()
        .expect("the carrier");
    assert!(carrier.starts_with(sealing::CARRIER_NAMESPACE));
    assert_eq!(body["usage"]["input_tokens"], 300);

    // Replay: the leading Claude block is restored under the operation beta.
    let (status, _) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item("old question"),
                compaction_item(carrier),
                user_item("pending turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 2);
    assert_eq!(requests[1].line(), "POST /messages");
    assert_eq!(
        requests[1].header("anthropic-beta"),
        Some("compact-2026-09-04")
    );

    let upstream = recorded_body(&scenario, 1);
    let first_block = &upstream["messages"][0]["content"][0];
    assert_eq!(first_block["type"], "compaction");
    assert_eq!(first_block["signature"], "sig-native");
    assert_eq!(first_block["content"], "claude summary");

    assert_absent(&upstream, "old question");
    let last = upstream["messages"]
        .as_array()
        .expect("messages")
        .last()
        .expect("messages");

    assert_eq!(
        last["content"][0]["text"], "pending turn",
        "the fresh turn follows the restored block"
    );

    // Re-compaction over the restored history works.
    let (status, _) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                compaction_item(carrier),
                user_item("pending turn"),
                {"type": "compaction_trigger"},
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 3);
    assert_eq!(requests[2].line(), "POST /messages");
    assert_eq!(
        requests[2].header("anthropic-beta"),
        Some("compact-2026-09-04")
    );

    drop(scenario);
}

/// An on-demand mechanism whose declaration carries no overlay headers and no
/// supported default still dispatches natively when the model's support facts
/// resolve it supported: the declaration's existence gates the stage and the
/// effective support resolves independently of the declared default, an
/// absent overlay dispatching with none (the checkpoint adapter's own rule).
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn on_demand_model_facts_dispatch_without_declared_overlay_headers() {
    // The declaration exists with an unknown default and no overlay headers —
    // the write-valid posture whose support the model facts alone resolve.
    let section = json!({
        "mechanisms": {
            "messages_on_demand_compaction": {},
        },
    });

    let on_demand_facts = facts(&[(
        Mechanism::MessagesOnDemandCompaction,
        MechanismSupport::Supported,
    )]);

    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(section),
        Some(&on_demand_facts),
        None,
        false,
        vec![anthropic_message(
            json!([{"type": "compaction", "content": "claude summary", "signature":
            "sig-native"}]),
            "end_turn",
            300,
            40,
        )],
    )
    .await;

    let (status, body) = post_messages(
        &scenario,
        Some("compact-2026-09-04"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "old question"}],
            "compaction": {"type": "summarize"},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // One dispatch — the native on-demand call — carrying no overlay header.
    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /messages");
    assert_eq!(requests[0].header("anthropic-beta"), None);

    let on_demand = recorded_body(&scenario, 0);
    assert_eq!(on_demand["compaction"], json!({"type": "summarize"}));

    // The native block renders as the compact-only artifact.
    assert_eq!(body["stop_reason"], "compaction");
    assert_eq!(body["content"][0]["type"], "compaction");
    assert_eq!(body["content"][0]["content"], "claude summary");
    assert_eq!(body["content"][0]["signature"], "sig-native");

    drop(scenario);
}

/// Scenario 4 — a Messages on-demand request backed by the native Responses
/// standalone compact: the gateway calls the beta-free compact operation,
/// wraps the whole window into a Messages carrier block, and the replay
/// restores the complete window upstream.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn messages_on_demand_backed_by_native_responses_window() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(Vec::new())),
        None,
        None,
        false,
        vec![
            compact_response(window_output(), 400, 60),
            responses_completed(json!([responses_message_item("answered")]), 40, 6),
        ],
    )
    .await;

    let (status, body) = post_messages(
        &scenario,
        Some("compact-2026-09-04"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "old question"}],
            "compaction": {"type": "summarize"},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // One compact dispatch at the standalone operation, beta-free.
    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /responses/compact");
    assert_eq!(requests[0].header("anthropic-beta"), None);

    // The whole window rides a Messages carrier block.
    assert_eq!(body["stop_reason"], "compaction");

    let block = &body["content"][0];
    assert_eq!(block["type"], "compaction");

    let carrier = block["signature"].as_str().expect("the carrier");
    assert!(carrier.starts_with(sealing::CARRIER_NAMESPACE));

    assert_eq!(
        block["content"],
        crate::format::anthropic::messages::GATEWAY_CARRIER_DESCRIPTION
    );
    assert_eq!(body["usage"]["input_tokens"], 0);
    assert_eq!(body["usage"]["output_tokens"], 0);

    // Replay: the carrier block restores the complete window upstream.
    let (status, _) = post_messages(
        &scenario,
        None,
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "old question"},
                {
                    "role": "assistant",
                    "content": [{
                        "type": "compaction",
                        "content": crate::format::anthropic::messages::GATEWAY_CARRIER_DESCRIPTION,
                        "signature": carrier,
                    }],
                },
                {"role": "user", "content": "pending turn"},
            ],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 2);
    assert_eq!(requests[1].line(), "POST /responses");

    let upstream = recorded_body(&scenario, 1);
    let input = upstream["input"].as_array().expect("input");

    assert_eq!(input.len(), 3);
    assert_eq!(input[0]["content"][0]["text"], "kept answer");
    assert_eq!(input[1]["encrypted_content"], NATIVE_STATE);
    assert_eq!(input[2]["content"][0]["text"], "pending turn");

    assert_absent(&upstream, "old question");

    drop(scenario);
}

/// Scenario 5 — the summary fallback through a non-native inference format:
/// no compact call is made, the summary dispatch carries the default prompt
/// without tools, a compact-and-continue lifecycle delivers the summary plus
/// the retained state to the continuation, and custom instructions replace
/// the prompt.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn summary_fallback_through_chat_completions() {
    let scenario = scenario(
        UpstreamFormat::OpenAiChatCompletion,
        None,
        None,
        None,
        false,
        vec![
            chat_completed("the older work, summarized", "stop", 10, 5),
            chat_completed("second summary", "stop", 11, 5),
            chat_completed("continued answer", "stop", 30, 6),
            chat_completed("custom summary", "stop", 12, 4),
        ],
    )
    .await;

    let big = "x".repeat(150_001);

    // (a) A trigger request: exactly one internal summary dispatch, no compact
    // call, the default prompt, no tools.
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item(&big),
                user_item("pending question"),
                {"type": "compaction_trigger"},
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /chat/completions");

    let summary_body = recorded_body(&scenario, 0);
    assert!(
        summary_body.get("tools").is_none(),
        "no tools ride the summary"
    );

    let messages = summary_body["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], DEFAULT_SUMMARY_PROMPT);
    assert_eq!(
        messages.last().expect("messages")["content"][0]["text"],
        crate::compaction::summary::SUMMARY_CLOSING_INSTRUCTION
    );

    assert!(
        body["output"][0]["encrypted_content"]
            .as_str()
            .expect("the carrier")
            .starts_with(sealing::CARRIER_NAMESPACE)
    );
    assert_eq!(body["usage"]["input_tokens"], 10);

    let carrier = body["output"][0]["encrypted_content"].clone();

    // (b) A threshold compact-and-continue: the summary plus the retained
    // state reach the continuation upstream.
    let (status, continued) = post_messages(
        &scenario,
        Some("compact-2026-01-12"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": big},
                {"role": "user", "content": "pending question"},
            ],
            "context_management": {"edits": [{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
                "pause_after_compaction": false,
            }]},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // Three dispatches so far: the trigger's summary, this summary, this
    // continuation.
    let requests = assert_count(&scenario, 3);
    assert_eq!(requests[1].line(), "POST /chat/completions");
    assert_eq!(requests[2].line(), "POST /chat/completions");

    let continuation = recorded_body(&scenario, 2);
    let messages = continuation["messages"].as_array().expect("messages");

    // The summary heads the continuation and the pending turn follows; the
    // covered prefix is gone.
    assert!(
        messages.iter().any(
            |message| message["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("second summary"))))
        ),
        "the summary rides the continuation: {continuation}"
    );
    assert_absent(&continuation, &"x".repeat(64));

    assert_eq!(continued["stop_reason"], "end_turn");

    let _ = carrier;

    // (c) Custom instructions replace the default summarization prompt.
    let (status, _) = post_messages(
        &scenario,
        Some("compact-2026-09-04"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": big},
                {"role": "user", "content": "pending question"},
            ],
            "compaction": {"type": "summarize", "instructions": "focus on the decisions"},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 4);
    assert_eq!(requests[3].line(), "POST /chat/completions");

    let custom = recorded_body(&scenario, 3);
    assert_eq!(custom["messages"][0]["role"], "system");
    assert_eq!(custom["messages"][0]["content"], "focus on the decisions");
    assert_absent(&custom, DEFAULT_SUMMARY_PROMPT);

    drop(scenario);
}

/// Scenario 6 — unknown support sends only the summary inference; a
/// positively classified rejection sends exactly one native attempt and one
/// summary, both accounted for.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn unknown_support_and_classified_rejection() {
    let big = "y".repeat(150_001);

    // (a) No facts anywhere: only the summary dispatch.
    let unknown = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(summarize_only_section()),
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("summarized")]),
            10,
            5,
        )],
    )
    .await;

    let (status, body) = post_openai(
        &unknown,
        "/api/openai-v1/responses/compact",
        json!({"model": MODEL, "input": [user_item(&big), user_item("pending question")]}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&unknown, 1);
    assert_eq!(
        requests[0].line(),
        "POST /responses",
        "summary only, no probe"
    );

    let summary_body = recorded_body(&unknown, 0);
    assert!(
        summary_body["instructions"]
            .as_str()
            .expect("instructions")
            .contains("summarizing the earlier part"),
        "the default summarization prompt rides the summary dispatch"
    );

    // The gateway carrier's usage is the summary call's usage: one accounted
    // dispatch.
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["output_tokens"], 5);

    drop(unknown);

    // (b) A configured classifier and a 400 answer: exactly one native
    // attempt, then one summary.
    let rejected = raw_json_response(
        "400 Bad Request",
        json!({"error": {"message": "compact unsupported"}}),
    );

    let classified = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(vec![400])),
        None,
        None,
        false,
        vec![
            rejected,
            responses_completed(json!([responses_message_item("fallback summary")]), 10, 5),
        ],
    )
    .await;

    let (status, body) = post_openai(
        &classified,
        "/api/openai-v1/responses/compact",
        json!({"model": MODEL, "input": [user_item(&big), user_item("pending question")]}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&classified, 2);
    assert_eq!(requests[0].line(), "POST /responses/compact");
    assert_eq!(requests[1].line(), "POST /responses");

    // Both dispatches are accounted: the failed native attempt reported no
    // usage, so the billed totals are the summary's alone — no fabricated
    // counts.
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["output_tokens"], 5);

    assert!(
        body["output"][0]["encrypted_content"]
            .as_str()
            .expect("the carrier")
            .starts_with(sealing::CARRIER_NAMESPACE)
    );

    drop(classified);
}

/// Scenario 7 — arbitrary errors never fall back, and a tampered gateway
/// carrier is rejected with no upstream dispatch at all.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn arbitrary_errors_and_tampered_state() {
    let big = "z".repeat(150_001);

    // (a) A 401 from the compact endpoint mirrors verbatim, with no summary
    // fallback.
    let unauthorized =
        raw_json_response("401 Unauthorized", json!({"error": {"message": "bad key"}}));

    let auth = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(Vec::new())),
        None,
        None,
        false,
        vec![unauthorized],
    )
    .await;

    let (status, body) = post_openai(
        &auth,
        "/api/openai-v1/responses/compact",
        json!({"model": MODEL, "input": [user_item(&big)]}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body.to_string().contains("bad key"),
        "the upstream error body mirrors: {body}"
    );

    assert_dispatches(&auth, 1);

    drop(auth);

    // (b) A network close maps to 502, with no fallback.
    let network = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(Vec::new())),
        None,
        None,
        false,
        vec![MockAction::Close],
    )
    .await;

    let (status, _) = post_openai(
        &network,
        "/api/openai-v1/responses/compact",
        json!({"model": MODEL, "input": [user_item(&big)]}),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);

    assert_dispatches(&network, 1);

    drop(network);

    // (c) A tampered gateway carrier in history: a client-state rejection and
    // no upstream dispatch at all.
    let tampered = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(json!([]), 1, 1)],
    )
    .await;

    let carrier = seal_summary(
        "intact summary",
        &[("be terse", InstructionRole::System)],
        None,
        false,
    );

    let mut damaged = carrier.clone().into_bytes();

    let position = sealing::CARRIER_NAMESPACE.len() + 4;

    damaged[position] ^= 0x01;

    // Authenticate every recognizable carrier before selecting a target, including old state
    // superseded by a newer valid checkpoint and requests naming an unpublished route.
    for path in [
        "/api/openai-v1/responses",
        "/api/openai-v1/responses/compact",
    ] {
        for model in [MODEL, "not-published"] {
            for has_later_checkpoint in [false, true] {
                let mut input = vec![
                    user_item("hello"),
                    compaction_item(&String::from_utf8_lossy(&damaged)),
                ];
                if has_later_checkpoint {
                    input.push(compaction_item(&carrier));
                }
                let (status, body) =
                    post_openai(&tampered, path, json!({"model":model,"input":input})).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {model}: {body}");
                assert!(
                    body.to_string()
                        .contains("compaction_invalid_gateway_state"),
                    "the stable carrier error precedes selection: {body}"
                );
            }
        }
        let (status, _) = post_openai(
            &tampered,
            path,
            json!({"model":"not-published","input":[compaction_item(&carrier)]}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "valid state still requires a public route"
        );
    }

    assert_dispatches(&tampered, 0);

    drop(tampered);
}

/// Scenario 8 — below/at/above the emulated Messages threshold through the
/// gateway summary: below sends one generation; at/above with pause compacts
/// once and stops; without pause it compacts and continues in one lifecycle
/// with no loop.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn threshold_below_pauses_and_continues_without_a_loop() {
    let big = "w".repeat(150_001);

    let below_body = json!({
        "model": MODEL,
        "max_tokens": 100,
        "messages": [{"role": "user", "content": "hello"}],
        "context_management": {"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 50_000},
            "pause_after_compaction": true,
        }]},
    });

    // (a) Below the threshold: exactly one generation dispatch.
    let low = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(
            json!([{"type": "text", "text": "answered"}]),
            "end_turn",
            3,
            1,
        )],
    )
    .await;

    let (status, body) = post_messages(&low, Some("compact-2026-01-12"), below_body.clone()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "end_turn");

    let requests = assert_count(&low, 1);
    assert_eq!(requests[0].line(), "POST /messages");

    let generation = recorded_body(&low, 0);
    assert!(
        generation.get("context_management").is_none(),
        "a below-threshold generation carries no threshold controls"
    );

    drop(low);

    let above_body = json!({
        "model": MODEL,
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": big},
            {"role": "user", "content": "pending question"},
        ],
        "context_management": {"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 50_000},
            "pause_after_compaction": true,
        }]},
    });

    // (b) At/above with pause: exactly one summary dispatch and the compaction
    // stop, with no answer dispatch.
    let paused = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(
            json!([{"type": "text", "text": "the summary"}]),
            "end_turn",
            10,
            5,
        )],
    )
    .await;

    let (status, body) = post_messages(&paused, Some("compact-2026-01-12"), above_body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "compaction");
    assert_eq!(body["content"][0]["type"], "compaction");
    assert_eq!(body["usage"]["input_tokens"], 0);
    assert_eq!(body["usage"]["output_tokens"], 0);
    assert_eq!(body["usage"]["iterations"][0]["type"], "compaction");
    assert_eq!(body["usage"]["iterations"][0]["input_tokens"], 10);

    assert_dispatches(&paused, 1);

    drop(paused);

    // (c) Without pause: one summary plus one generation in one lifecycle —
    // blocks, then the answer; exactly two dispatches, no loop.
    let continuing = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![
            anthropic_message(
                json!([{"type": "text", "text": "the summary"}]),
                "end_turn",
                10,
                5,
            ),
            anthropic_message(
                json!([{"type": "text", "text": "the answer"}]),
                "end_turn",
                20,
                7,
            ),
        ],
    )
    .await;

    let continue_body = json!({
        "model": MODEL,
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": big},
            {"role": "user", "content": "pending question"},
        ],
        "context_management": {"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 50_000},
        }]},
    });

    let (status, body) =
        post_messages(&continuing, Some("compact-2026-01-12"), continue_body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["content"][0]["type"], "compaction");
    assert_eq!(body["content"][1]["type"], "text");
    assert_eq!(body["content"][1]["text"], "the answer");

    let requests = assert_count(&continuing, 2);
    assert_eq!(requests[0].line(), "POST /messages");
    assert_eq!(requests[1].line(), "POST /messages");

    // The continuation receives the summary and the pending turn, and never
    // the compaction controls.
    let continuation = recorded_body(&continuing, 1);
    let messages = continuation["messages"].as_array().expect("messages");

    assert!(
        messages.iter().any(
            |message| message["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("the summary"))))
        ),
        "the summary rides the continuation: {continuation}"
    );
    assert_absent(&continuation, "context_management");
    assert_absent(&continuation, &"w".repeat(64));

    drop(continuing);
}

/// Scenario 9 — native automatic compaction preserves distinct items, order,
/// and usage; a replay carries both boundaries upstream; and a null-content
/// Messages block does not erase history.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn native_automatic_iterations_and_null_blocks() {
    // (a) Native automatic passthrough: the controls ride upstream verbatim
    // and the outward body preserves both compaction items in order.
    let automatic_output = json!([
        {"type": "compaction", "id": "cmp_a", "encrypted_content": "state-a"},
        responses_message_item("between"),
        {"type": "compaction", "id": "cmp_b", "encrypted_content": "state-b"},
        responses_message_item("after"),
    ]);

    let native = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(automatic_section("supported")),
        None,
        None,
        false,
        vec![responses_completed(automatic_output.clone(), 100, 25)],
    )
    .await;

    let (status, body) = post_openai(
        &native,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [user_item("old question"), user_item("pending turn")],
            "context_management": [{"type": "compaction", "compact_threshold": 90_000}],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&native, 1);
    assert_eq!(requests[0].line(), "POST /responses");

    let upstream = recorded_body(&native, 0);
    assert_eq!(
        upstream["context_management"],
        json!([{"type": "compaction", "compact_threshold": 90_000}]),
        "the automatic controls ride the upstream body verbatim"
    );

    let output = body["output"].as_array().expect("output");

    let compactions: Vec<&Value> = output
        .iter()
        .filter(|item| item["type"] == "compaction")
        .collect();

    assert_eq!(
        compactions,
        vec![
            &json!({"type": "compaction", "id": "cmp_a", "encrypted_content": "state-a"}),
            &json!({"type": "compaction", "id": "cmp_b", "encrypted_content": "state-b"}),
        ],
        "both distinct blocks, in order"
    );

    let serialized = body.to_string();
    assert!(serialized.contains("between") && serialized.contains("after"));

    assert_eq!(body["usage"]["input_tokens"], 100);
    assert_eq!(body["usage"]["output_tokens"], 25);

    // (b) The replay carries both boundaries upstream in order — the latest
    // usable boundary is the upstream producer's call.
    let (status, _) = post_openai(
        &native,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item("old question"),
                compaction_item("state-a"),
                user_item("middle turn"),
                compaction_item("state-b"),
                user_item("pending turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&native, 2);
    assert_eq!(requests[1].line(), "POST /responses");

    let upstream = recorded_body(&native, 1);
    let input = upstream["input"].as_array().expect("input");

    let types: Vec<&str> = input
        .iter()
        .filter_map(|item| item["type"].as_str())
        .collect();

    assert_eq!(
        types,
        vec!["message", "compaction", "message", "compaction", "message"]
    );
    assert_eq!(input[1]["encrypted_content"], "state-a");
    assert_eq!(input[3]["encrypted_content"], "state-b");

    drop(native);

    // (c) A null-content compaction block does not erase history: the replay
    // continues over the full window.
    let null_block = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(
            json!([{"type": "text", "text": "answered"}]),
            "end_turn",
            8,
            2,
        )],
    )
    .await;

    let (status, _) = post_messages(
        &null_block,
        Some("compact-2026-01-12"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "kept turn"},
                {
                    "role": "assistant",
                    "content": [{"type": "compaction", "content": null, "signature": "sig-native"}],
                },
                {"role": "user", "content": "pending turn"},
            ],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&null_block, 1);
    assert_eq!(requests[0].line(), "POST /messages");

    let upstream = recorded_body(&null_block, 0);

    let serialized = upstream.to_string();
    assert!(
        serialized.contains("kept turn")
            && serialized.contains("pending turn")
            && serialized.contains("sig-native"),
        "the null block erases no history and rides the replay: {upstream}"
    );

    drop(null_block);
}

/// Scenario 10 — superseded native state: a newer usable gateway checkpoint
/// makes an older, target-incompatible wrapped blob irrelevant, the same blob
/// alone is rejected, and a null no-op block cannot do the checkpoint's work.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn a_newer_checkpoint_supersedes_incompatible_native_state() {
    // Producer A: a wrapped Claude on-demand block — replayable only against
    // the Claude wire.
    let old_blob = seal_window(
        UpstreamFormat::AnthropicMessages,
        NativeReplayWindow::AnthropicOnDemandBlock {
            block: json!({"type": "compaction", "content": "claude state", "signature": "sig-a"}),
        },
        None,
    );

    // The newer checkpoint: a wrapped OpenAI Responses window.
    let newer = seal_window(
        UpstreamFormat::OpenAiResponse,
        NativeReplayWindow::ResponsesWindow {
            items: window_output().as_array().expect("the window").clone(),
        },
        None,
    );

    let covered = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("answered")]),
            6,
            2,
        )],
    )
    .await;

    // (a) The old blob covered by the newer checkpoint: the replay succeeds —
    // the old producer's incompatibility with the target never surfaces.
    let (status, _) = post_openai(
        &covered,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item("covered old"),
                compaction_item(&old_blob),
                user_item("middle turn"),
                compaction_item(&newer),
                user_item("pending turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&covered, 1);
    assert_eq!(requests[0].line(), "POST /responses");

    let upstream = recorded_body(&covered, 0);
    let input = upstream["input"].as_array().expect("input");

    assert_eq!(input.len(), 3);
    assert_eq!(input[0]["content"][0]["text"], "kept answer");
    assert_eq!(input[1]["encrypted_content"], NATIVE_STATE);
    assert_eq!(input[2]["content"][0]["text"], "pending turn");

    assert_absent(&upstream, "claude state");
    assert_absent(&upstream, "covered old");
    assert_absent(&upstream, "middle turn");

    drop(covered);

    // (b) The same old blob without the newer checkpoint: the wrapped window
    // speaks another wire format than the target — an explicit rejection.
    let alone = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(json!([]), 1, 1)],
    )
    .await;

    let (status, body) = post_openai(
        &alone,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item("covered old"),
                compaction_item(&old_blob),
                user_item("pending turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains("compaction_inaccessible_state"),
        "the stable code rides the message: {body}"
    );

    assert_dispatches(&alone, 0);

    drop(alone);

    // (c) A null no-op block cannot do the checkpoint's work: the history it
    // cannot cover still rejects the compaction that would have to summarize
    // the opaque state.
    let big_null = "n".repeat(150_001);

    let null = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(json!([]), "end_turn", 1, 1)],
    )
    .await;

    let (status, body) = post_messages(
        &null,
        Some("compact-2026-09-04"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {
                    "role": "assistant",
                    "content": [{"type": "compaction", "content": null, "signature": "sig-native"}],
                },
                {"role": "user", "content": big_null},
                {"role": "user", "content": "pending turn"},
            ],
            "compaction": {"type": "summarize"},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains("compaction_inaccessible_state"),
        "the stable code rides the message: {body}"
    );

    assert_dispatches(&null, 0);

    drop(null);
}

/// Scenario 11 — client-retained overlapping history: the covered prefix and
/// the covered instruction are superseded by proven identity overlap, a newly
/// injected instruction survives, the summary and retained units restore once,
/// and identical fresh-suffix messages remain distinct.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn client_retained_overlap_supersedes_and_preserves() {
    let carrier = seal_summary(
        "prior work summarized",
        &[("base rules", InstructionRole::System)],
        Some(text_message(IrRole::User, "kept turn")),
        false,
    );

    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("answered")]),
            9,
            3,
        )],
    )
    .await;

    let (status, _) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "instructions": "base rules",
            "input": [
                user_item("covered old"),
                compaction_item(&carrier),
                {"type": "message", "role": "developer", "content": "fresh developer context"},
                user_item("new turn"),
                user_item("new turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /responses");

    let upstream = recorded_body(&scenario, 0);

    // The covered instruction is superseded; the newly injected developer
    // instruction survives beside the checkpoint's preserved one.
    assert_eq!(
        upstream["instructions"],
        "base rules\n\nfresh developer context"
    );

    let input = upstream["input"].as_array().expect("input");

    // Summary, retained unit once, and both identical fresh messages.
    assert_eq!(input.len(), 4);

    let summary_text = input[0]["content"][0]["text"]
        .as_str()
        .expect("the summary");
    assert!(summary_text.contains("prior work summarized"));

    assert_eq!(input[1]["content"][0]["text"], "kept turn");
    assert_eq!(input[2]["content"][0]["text"], "new turn");
    assert_eq!(input[3]["content"][0]["text"], "new turn");

    assert_absent(&upstream, "covered old");

    drop(scenario);
}

/// Scenario 12 — tool and media boundaries: a tool exchange crossing the
/// retention boundary stays whole, an orphan tool result fails explicitly, and
/// an image in the to-be-summarized prefix fails the measurement explicitly.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn tool_pairs_survive_and_orphans_and_images_fail_explicitly() {
    let big = "q".repeat(150_001);

    // (a) The tool exchange rides the retained region whole.
    let pair = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![
            anthropic_message(
                json!([{"type": "text", "text": "the summary"}]),
                "end_turn",
                10,
                5,
            ),
            anthropic_message(
                json!([{"type": "text", "text": "the answer"}]),
                "end_turn",
                20,
                6,
            ),
        ],
    )
    .await;

    let (status, _) = post_messages(
        &pair,
        Some("compact-2026-01-12"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": big},
                {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "t1", "name": "lookup",
                    "input": {"query": "facts"},
                }]},
                {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "t1", "content": "result data",
                }]},
                {"role": "user", "content": "pending question"},
            ],
            "context_management": {"edits": [{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
            }]},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&pair, 2);
    assert_eq!(requests[0].line(), "POST /messages");
    assert_eq!(requests[1].line(), "POST /messages");

    let continuation = recorded_body(&pair, 1);
    let serialized = continuation.to_string();

    assert!(
        serialized.contains("t1") && serialized.contains("result data"),
        "the whole tool exchange rides the continuation: {continuation}"
    );
    assert_absent(&continuation, &"q".repeat(64));

    drop(pair);

    // (b) An orphan tool result is structurally invalid history.
    let orphan = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(json!([]), "end_turn", 1, 1)],
    )
    .await;

    let (status, body) = post_messages(
        &orphan,
        Some("compact-2026-09-04"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "ghost", "content": "orphan",
                }]},
            ],
            "compaction": {"type": "summarize"},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains("compaction_unusable_history"),
        "the stable code rides the message: {body}"
    );

    assert_dispatches(&orphan, 0);

    drop(orphan);

    // (c) An image in the to-be-summarized prefix fails the threshold
    // measurement explicitly.
    let image = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(json!([]), "end_turn", 1, 1)],
    )
    .await;

    let (status, body) = post_messages(
        &image,
        Some("compact-2026-01-12"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": [{
                    "type": "image",
                    "source": {"type": "url", "url": "https://upstream.test/a.png"},
                }]},
                {"role": "user", "content": "pending question"},
            ],
            "context_management": {"edits": [{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
            }]},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string()
            .contains("compaction_unsupported_measurement"),
        "the stable code rides the message: {body}"
    );

    assert_dispatches(&image, 0);

    drop(image);
}

/// Scenario 13 — refusal, truncation, empty, and tool-producing summaries
/// produce no artifact; the native no-summary outcome keeps its own
/// semantics.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn unusable_summaries_fail_and_native_no_summary_stands() {
    let big = "r".repeat(150_001);

    let threshold_request = |summary_answer: MockAction| async move {
        scenario(
            UpstreamFormat::AnthropicMessages,
            None,
            None,
            None,
            false,
            vec![summary_answer],
        )
        .await
    };

    let request_body = json!({
        "model": MODEL,
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": big},
            {"role": "user", "content": "pending question"},
        ],
        "context_management": {"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 50_000},
        }]},
    });

    for (name, answer) in [
        (
            "truncation",
            anthropic_message(
                json!([{"type": "text", "text": "partial"}]),
                "max_tokens",
                10,
                5,
            ),
        ),
        (
            "empty",
            anthropic_message(json!([{"type": "text", "text": ""}]), "end_turn", 10, 5),
        ),
        (
            "tool call",
            anthropic_message(
                json!([{"type": "tool_use", "id": "c1", "name": "lookup", "input": {}}]),
                "tool_use",
                10,
                5,
            ),
        ),
    ] {
        let scenario = threshold_request(answer).await;

        let (status, body) =
            post_messages(&scenario, Some("compact-2026-01-12"), request_body.clone()).await;

        assert_eq!(status, StatusCode::BAD_GATEWAY, "{name}");
        assert!(
            body.to_string().contains("compaction_summary_unusable"),
            "{name}: the stable code rides the message: {body}"
        );

        // One dispatch — the failed summary — and no continuation, no
        // artifact.
        assert_dispatches(&scenario, 1);

        drop(scenario);
    }

    // The native on-demand no-summary outcome: neither an error nor a
    // successful compaction artifact — the consumed response renders with its
    // own stop reason and content.
    let native = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(messages_mechanisms_section("supported", "supported")),
        None,
        None,
        false,
        vec![anthropic_message(
            json!([
                {"type": "compaction", "content": null, "signature": "sig-native"},
                {"type": "text", "text": "plain answer"},
            ]),
            "end_turn",
            8,
            2,
        )],
    )
    .await;

    let (status, body) = post_messages(
        &native,
        Some("compact-2026-09-04"),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "old question"}],
            "compaction": {"type": "summarize"},
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "end_turn", "the native stop mirrors");
    assert_eq!(body["content"][0]["type"], "compaction");
    assert_eq!(body["content"][0]["content"], Value::Null);
    assert_eq!(body["content"][1]["text"], "plain answer");
    assert_eq!(body["usage"]["input_tokens"], 8);
    assert_eq!(body["usage"]["output_tokens"], 2);

    assert_dispatches(&native, 1);

    drop(native);
}

/// Scenario 14 — the trigger lifecycle is equivalent as JSON and as SSE: the
/// same item identity, state, order, stop, and usage within each lifecycle;
/// and a native trigger passthrough whose stream lacks its terminal completion
/// fails instead of degrading.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn trigger_lifecycles_are_equivalent_and_missing_terminals_fail() {
    let big = "s".repeat(150_001);

    let trigger_body = |streaming: bool| {
        json!({
            "model": MODEL,
            "input": [
                user_item(&big),
                user_item("pending question"),
                {"type": "compaction_trigger"},
            ],
            "stream": streaming,
            "store": false,
        })
    };

    // JSON and SSE over the gateway-summary trigger path.
    let json_scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(summarize_only_section()),
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("summarized")]),
            10,
            5,
        )],
    )
    .await;

    let (status, json_body) = post_openai(
        &json_scenario,
        "/api/openai-v1/responses",
        trigger_body(false),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body["status"], "completed");

    let json_item = json_body["output"][0].clone();
    assert_eq!(json_item["type"], "compaction");

    drop(json_scenario);

    let sse_scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(summarize_only_section()),
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("summarized")]),
            10,
            5,
        )],
    )
    .await;

    let (status, sse_text) = post(
        &sse_scenario,
        "/api/openai-v1/responses",
        &openai_headers(),
        trigger_body(true).to_string(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let payloads = sse_payloads(&sse_text);

    let types: Vec<&str> = payloads
        .iter()
        .filter_map(|payload| payload["type"].as_str())
        .collect();

    assert_eq!(
        types,
        vec![
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.output_item.done",
            "response.completed",
        ]
    );

    // One lifecycle, one identity: the item event and the terminal output
    // carry the same item.
    let streamed_item = payloads[3]["item"].clone();
    assert_eq!(payloads[4]["response"]["output"], json!([streamed_item]));
    assert_eq!(payloads[4]["response"]["status"], "completed");

    // Same state shape and usage across the two representations.
    assert_eq!(streamed_item["type"], json_item["type"]);
    assert_eq!(payloads[4]["response"]["usage"]["input_tokens"], 10);
    assert_eq!(payloads[4]["response"]["usage"]["output_tokens"], 5);
    assert_eq!(json_body["usage"]["input_tokens"], 10);
    assert_eq!(json_body["usage"]["output_tokens"], 5);

    drop(sse_scenario);

    // The native trigger passthrough without its terminal completion fails.
    let partial_stream = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
         Content-Length: 4000\r\nConnection: close\r\n\r\n\
         event: response.output_item.added\ndata: {\"type\":\
         \"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"compaction\",\
         \"id\":\"cmp_native\",\"encrypted_content\":\"state\"}}\n\n";

    let missing = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(json!({"mechanisms": {"responses_explicit_trigger": {"default": "supported"}}})),
        None,
        None,
        true,
        vec![MockAction::Respond(partial_stream.to_string())],
    )
    .await;

    let (status, _) = post(
        &missing,
        "/api/openai-v1/responses",
        &openai_headers(),
        json!({
            "model": MODEL,
            "input": [user_item("old question"), {"type": "compaction_trigger"}],
            "stream": true,
            "store": false,
        })
        .to_string(),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "a missing terminal completion fails the compaction"
    );

    assert_dispatches(&missing, 1);

    drop(missing);
}

/// Scenario 15 — a continuation failure after a complete checkpoint fails the
/// outer lifecycle, and the delivered checkpoint itself remains replayable.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn continuation_failure_leaves_the_checkpoint_replayable() {
    let big = "t".repeat(150_001);

    // The continuation's upstream delivers its stream head, then breaks the
    // body against its declared length.
    let broken_stream = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
         Content-Length: 4000\r\nConnection: close\r\n\r\n\
         event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\
         \"msg_upstream\",\"model\":\"test-model\"}}\n\n";

    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![
            anthropic_message(
                json!([{"type": "text", "text": "the summary"}]),
                "end_turn",
                10,
                5,
            ),
            MockAction::Respond(broken_stream.to_string()),
            anthropic_message(
                json!([{"type": "text", "text": "final answer"}]),
                "end_turn",
                20,
                6,
            ),
        ],
    )
    .await;

    let (status, stream) = post(
        &scenario,
        "/api/anthropic-v1/messages",
        &messages_headers(Some("compact-2026-01-12")),
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "stream": true,
            "messages": [
                {"role": "user", "content": big},
                {"role": "user", "content": "pending question"},
            ],
            "context_management": {"edits": [{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
            }]},
        })
        .to_string(),
    )
    .await;

    // The SSE lifecycle renders the checkpoint before the failure and never a
    // completed answer.
    assert_eq!(status, StatusCode::OK);
    assert!(
        stream.contains("\"error\""),
        "the failure renders: {stream}"
    );
    assert!(!stream.contains("final answer"));

    // The delivered checkpoint remains replayable.
    let payloads = sse_payloads(&stream);

    let block = payloads
        .iter()
        .find_map(|payload| {
            let block = payload.get("content_block")?;

            (block["type"] == "compaction").then(|| block.clone())
        })
        .expect("the checkpoint block rendered");

    let carrier = block["signature"].as_str().expect("the carrier");
    assert!(carrier.starts_with(sealing::CARRIER_NAMESPACE));

    let (status, replayed) = post_messages(
        &scenario,
        None,
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "covered old"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "the summary", "signature": carrier},
                ]},
                {"role": "user", "content": "pending new"},
            ],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["stop_reason"], "end_turn");
    assert_eq!(replayed["content"][0]["text"], "final answer");

    let requests = assert_count(&scenario, 3);
    assert_eq!(requests[2].line(), "POST /messages");

    let upstream = recorded_body(&scenario, 2);
    let serialized = upstream.to_string();

    assert!(
        serialized.contains("the summary") && serialized.contains("pending new"),
        "the checkpoint restores: {upstream}"
    );
    assert_absent(&upstream, "covered old");

    drop(scenario);
}

/// Scenario 16 — restart, rotation, and state migration: a carrier sealed
/// under the first key unseals under both configured keys and the live
/// configuration, fails under a resolving set that dropped its key, and a
/// gateway summary replays across a target-format switch.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn keys_rotate_and_summaries_replay_across_formats() {
    use crate::compaction_model::sealing::{UnsealError, unseal};

    // Seal under the live configuration's first key.
    let (_guard, carrier) = {
        let _configuration = test_support::configuration_lock();

        test_support::install_global_configuration();

        history::seal_envelope(
            EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
                summary_text: String::from("rotation summary"),
                preserved_instructions: Vec::new(),
                retained_units: Vec::new(),
                retained_state_items: Vec::new(),
                overlap_identities: Vec::new(),
            }),
            ReplacementMode::CanonicalWindow,
            None,
        )
        .expect("the fixture envelope seals")
    };

    // Rotation: both keys configured — the carrier unseals, live and via a
    // direct resolve.
    let both: crate::config::schema::CompactionConfig = serde_json::from_value(json!({
        "keys": [
            {"id": "test-primary", "key": test_support::FIXTURE_SEALING_KEY},
            {"id": "test-spare", "key": test_support::FIXTURE_SEALING_KEY_ROTATED},
        ],
    }))
    .expect("the fixture key configuration parses");

    let keys = crate::compaction_model::sealing::SealingKeys::build(&both);

    assert!(unseal(&keys, &carrier, None).is_ok());
    assert!(sealing::unseal_with_live_config(&carrier, None).is_ok());

    // Restart-equivalent readers and removed keys: a resolving set without the
    // sealing key fails the carrier distinctly.
    let spare_only: crate::config::schema::CompactionConfig = serde_json::from_value(json!({
        "keys": [{"id": "test-spare", "key": test_support::FIXTURE_SEALING_KEY_ROTATED}],
    }))
    .expect("the fixture key configuration parses");

    let keys = crate::compaction_model::sealing::SealingKeys::build(&spare_only);

    assert!(matches!(
        unseal(&keys, &carrier, None),
        Err(UnsealError::UnknownKey)
    ));

    // A gateway summary replays across the target-format switch: a
    // Responses-produced checkpoint restores against a Claude upstream.
    let summary_carrier = seal_summary(
        "cross format summary",
        &[("carry the objective", InstructionRole::System)],
        Some(text_message(IrRole::User, "retained turn")),
        true,
    );

    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(
            json!([{"type": "text", "text": "answered"}]),
            "end_turn",
            7,
            2,
        )],
    )
    .await;

    let (status, replayed) = post_messages(
        &scenario,
        None,
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "covered old"},
                {"role": "assistant", "content": [
                    {
                        "type": "compaction",
                        "content": "cross format summary",
                        "signature": summary_carrier,
                    },
                ]},
                {"role": "user", "content": "pending turn"},
            ],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["stop_reason"], "end_turn");

    let requests = assert_count(&scenario, 1);
    assert_eq!(requests[0].line(), "POST /messages");

    let upstream = recorded_body(&scenario, 0);
    let serialized = upstream.to_string();

    assert!(
        serialized.contains("cross format summary")
            && serialized.contains("retained turn")
            && serialized.contains("pending turn"),
        "the summary checkpoint restores across formats: {upstream}"
    );
    assert_absent(&upstream, "covered old");
    assert_eq!(upstream["system"], "carry the objective");

    drop(scenario);
}

/// Scenario 17 — native producer/model mismatch: a wrapped artifact whose
/// payload format differs from the target is rejected, while raw
/// same-protocol passthrough remains the caller's responsibility.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn producer_mismatch_rejects_wrapped_state_and_raw_passthrough_remains() {
    // (a) A wrapped OpenAI Responses window replayed against the Claude target.
    let wrapped = seal_window(
        UpstreamFormat::OpenAiResponse,
        NativeReplayWindow::ResponsesWindow {
            items: window_output().as_array().expect("the window").clone(),
        },
        Some("bound content"),
    );

    let wrapped_scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(json!([]), "end_turn", 1, 1)],
    )
    .await;

    let (status, body) = post_messages(
        &wrapped_scenario,
        None,
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "covered old"},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "bound content", "signature": wrapped},
                ]},
                {"role": "user", "content": "pending turn"},
            ],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains("compaction_inaccessible_state"),
        "the stable code rides the message: {body}"
    );

    assert_dispatches(&wrapped_scenario, 0);

    drop(wrapped_scenario);

    // (b) The raw same-protocol passthrough remains: an xAI window replayed
    // against the openai_response upstream rides verbatim.
    let raw = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("answered")]),
            5,
            2,
        )],
    )
    .await;

    let (status, _) = post_openai(
        &raw,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [
                user_item("old question"),
                compaction_item("xai-state"),
                user_item("pending turn"),
            ],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let requests = assert_count(&raw, 1);
    assert_eq!(requests[0].line(), "POST /responses");

    let upstream = recorded_body(&raw, 0);
    let input = upstream["input"].as_array().expect("input");

    assert_eq!(input.len(), 3);
    assert_eq!(input[1]["encrypted_content"], "xai-state");

    drop(raw);
}

/// Scenario 18 — accounting: the compaction iteration (180,000 in / 3,500
/// out) and the message iteration (23,000 in / 1,000 out) bill 203,000/4,500
/// without double counting, the client-rendered top-level usage stays the
/// message stage's, and a compact-only reply renders zero top-level usage
/// beside its compaction iterations.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn the_ledger_bills_every_iteration_without_double_counting() {
    let big = "u".repeat(150_001);

    let threshold_body = |pause: bool| {
        json!({
            "model": MODEL,
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": big},
                {"role": "user", "content": "pending question"},
            ],
            "context_management": {"edits": [{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
                "pause_after_compaction": pause,
            }]},
        })
    };

    // Compact-and-continue: compaction + message iterations, top-level usage
    // from the message stage alone.
    let continuing = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![
            anthropic_message(
                json!([{"type": "text", "text": "the summary"}]),
                "end_turn",
                180_000,
                3_500,
            ),
            anthropic_message(
                json!([{"type": "text", "text": "the answer"}]),
                "end_turn",
                23_000,
                1_000,
            ),
        ],
    )
    .await;

    let (status, body) = post_messages(
        &continuing,
        Some("compact-2026-01-12"),
        threshold_body(false),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    // The top-level message fields remain the message stage's.
    assert_eq!(body["usage"]["input_tokens"], 23_000);
    assert_eq!(body["usage"]["output_tokens"], 1_000);

    // The iterations array is authoritative for the billed totals.
    let iterations = body["usage"]["iterations"].as_array().expect("iterations");
    assert_eq!(iterations.len(), 2);
    assert_eq!(iterations[0]["type"], "compaction");
    assert_eq!(iterations[0]["input_tokens"], 180_000);
    assert_eq!(iterations[0]["output_tokens"], 3_500);
    assert_eq!(iterations[1]["type"], "message");
    assert_eq!(iterations[1]["input_tokens"], 23_000);
    assert_eq!(iterations[1]["output_tokens"], 1_000);

    let billed_input: u64 = iterations
        .iter()
        .filter_map(|iteration| iteration["input_tokens"].as_u64())
        .sum();
    let billed_output: u64 = iterations
        .iter()
        .filter_map(|iteration| iteration["output_tokens"].as_u64())
        .sum();

    assert_eq!((billed_input, billed_output), (203_000, 4_500));

    assert_dispatches(&continuing, 2);

    drop(continuing);

    // Compact-only with pause: the compaction iteration alone, zero top-level
    // usage.
    let paused = scenario(
        UpstreamFormat::AnthropicMessages,
        None,
        None,
        None,
        false,
        vec![anthropic_message(
            json!([{"type": "text", "text": "the summary"}]),
            "end_turn",
            180_000,
            3_500,
        )],
    )
    .await;

    let (status, body) =
        post_messages(&paused, Some("compact-2026-01-12"), threshold_body(true)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "compaction");
    assert_eq!(body["usage"]["input_tokens"], 0);
    assert_eq!(body["usage"]["output_tokens"], 0);

    let iterations = body["usage"]["iterations"].as_array().expect("iterations");
    assert_eq!(iterations.len(), 1);
    assert_eq!(iterations[0]["type"], "compaction");
    assert_eq!(iterations[0]["input_tokens"], 180_000);
    assert_eq!(iterations[0]["output_tokens"], 3_500);

    assert_dispatches(&paused, 1);

    drop(paused);
}

/// A configured `count_tokens` operation drives the gateway threshold's
/// measurement: the count dispatch rides first, its measurement decides
/// the evaluation, and the counter iteration is distinguished from
/// inference. A failing counter falls back to the documented text
/// estimator instead of failing the request.
///
/// # Panics
///
/// Panics if an assertion fails — the assertion is the failure report.
#[tokio::test]
async fn the_native_counter_measures_and_falls_back_to_the_estimator() {
    let big = "w".repeat(150_001);

    let above_body = json!({
        "model": MODEL,
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": big},
            {"role": "user", "content": "pending question"},
        ],
        "context_management": {"edits": [{
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 50_000},
            "pause_after_compaction": true,
        }]},
    });

    // (a) The counter answers: its measurement decides, the summary
    // follows, and the dispatch order is count then summarize.
    let counted = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(json!({
            "count_tokens": {
                "url": [{"type": "base"}, {"type": "literal", "value": "/messages/count_tokens"}],
                "headers": [{"name": "x-api-key", "value": [{"type": "secret"}]}],
            },
        })),
        None,
        None,
        false,
        vec![
            raw_json_response(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n",
                json!({"input_tokens": 60_000}),
            ),
            anthropic_message(
                json!([{"type": "text", "text": "the summary"}]),
                "end_turn",
                10,
                5,
            ),
        ],
    )
    .await;

    let (status, body) =
        post_messages(&counted, Some("compact-2026-01-12"), above_body.clone()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "compaction");

    let requests = assert_count(&counted, 2);
    assert_eq!(requests[0].line(), "POST /messages/count_tokens");
    assert_eq!(requests[1].line(), "POST /messages");

    // The count body carries the window verbatim and no generation-only
    // control.
    let count_body = recorded_body(&counted, 0);
    assert_eq!(count_body["model"], MODEL);
    assert!(count_body.get("max_tokens").is_none());
    assert!(count_body.get("stream").is_none());

    drop(counted);

    // (b) The counter fails: the estimator fallback still evaluates, and
    // the above-threshold window compacts anyway.
    let failing = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(json!({
            "count_tokens": {
                "url": [{"type": "base"}, {"type": "literal", "value": "/messages/count_tokens"}],
                "headers": [{"name": "x-api-key", "value": [{"type": "secret"}]}],
            },
        })),
        None,
        None,
        false,
        vec![
            raw_json_response(
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n",
                json!({"error": {"type": "not_found_error"}}),
            ),
            anthropic_message(
                json!([{"type": "text", "text": "the summary"}]),
                "end_turn",
                10,
                5,
            ),
        ],
    )
    .await;

    let (status, body) = post_messages(&failing, Some("compact-2026-01-12"), above_body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stop_reason"], "compaction");

    let requests = assert_count(&failing, 2);
    assert_eq!(requests[0].line(), "POST /messages/count_tokens");
    assert_eq!(requests[1].line(), "POST /messages");

    drop(failing);
}
/// Review regression: top-level instructions must survive carrier restoration.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_top_level_instructions_survive_replay() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(summarize_only_section()),
        None,
        None,
        false,
        vec![
            responses_completed(json!([responses_message_item("summary")]), 20, 5),
            responses_completed(json!([responses_message_item("answer")]), 4, 2),
        ],
    )
    .await;
    let (status, compact) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({
                "model":MODEL,
        "instructions":"Always answer in Chinese.",
        "input":[user_item("old question"),
        assistant_text_item("old answer")]
            }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut input = compact["output"].as_array().expect("output").clone();
    input.push(user_item("new question"));
    let (status, _) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model":MODEL,
        "instructions":"Always answer in Chinese.",
        "input":input
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        recorded_body(&scenario, 1)["instructions"],
        "Always answer in Chinese."
    );
}

/// Review regression: the active native window must belong to the selected producer.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_reject_foreign_producer_same_format() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("answer")]),
            4,
            2,
        )],
    )
    .await;
    let (_, carrier) = history::seal_native_window(
        NativeWindowPayload {
            retained_messages: Vec::new(),
            retained_state_items: Vec::new(),
            producer_instance_id: "provider:99999".into(),
            upstream_model_id: "another-model".into(),
            mechanism: Mechanism::ResponsesStandaloneCompact,
            native_format: UpstreamFormat::OpenAiResponse,
            required_prefix_identity: None,
            window: NativeReplayWindow::ResponsesWindow {
                items: vec![json!({"type":"compaction",
"id":"cmp_foreign",
"encrypted_content":"foreign-native-secret"})],
            },
        },
        ReplacementMode::CanonicalWindow,
        None,
    )
    .expect("seal");
    let (status, _) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
                "model":MODEL,
        "input":[{"type":"compaction",
        "encrypted_content":carrier},
        user_item("new question")]
            }),
    )
    .await;
    assert!(
        !status.is_success(),
        "foreign producer state was sent upstream"
    );
    assert_dispatches(&scenario, 0);
}

/// Review regression: native trigger unsupported responses must use their classifier.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_trigger_unsupported_falls_back() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(json!({
            "retention_budget":1,
        "mechanisms":{"responses_explicit_trigger":{"default":"supported",
        "unsupported":{"codes":["compaction_unsupported"]}}}
        })),
        None,
        None,
        false,
        vec![
            MockAction::Respond(
                "HTTP/1.1 400 Bad Request\r\n\
Content-Type: application/json\r\n\
Content-Length: 43\r\n\
Connection: close\r\n\
\r\n\
{\"error\":{\"code\":\"compaction_unsupported\"}}"
                    .into(),
            ),
            responses_completed(json!([responses_message_item("summary")]), 20, 5),
        ],
    )
    .await;
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
                "model":MODEL,
        "input":[user_item("old question"),
        assistant_text_item("old answer"),
        {"type":"compaction_trigger"}]
            }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_dispatches(&scenario, 2);
}

/// Scenario — a classified-unsupported exact trigger keeps the adapter chain
/// eligible: the standalone adapter compacts after the trigger boundary falls
/// through, exactly as the decision order table reads.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn classified_unsupported_trigger_keeps_the_standalone_adapter_eligible() {
    let section = json!({
        "strategy": "native_preferred",
        "summary_budget": 4096,
        "retention_budget": 1,
        "mechanisms": {
            "responses_explicit_trigger": {
                "default": "supported",
                "unsupported": {"statuses": [400]},
            },
            "responses_standalone_compact": {
                "default": "supported",
                "operation": {
                    "url": [{"type": "base"}, {"type": "literal", "value": "/responses/compact"}],
                },
            },
        },
    });

    let rejected = raw_json_response(
        "400 Bad Request",
        json!({"error": {"code": "compaction_unsupported"}}),
    );

    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(section),
        None,
        None,
        false,
        vec![rejected, compact_response(window_output(), 120, 30)],
    )
    .await;

    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({
            "model": MODEL,
            "input": [user_item("old question"), {"type": "compaction_trigger"}],
            "store": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");

    // Two dispatches in order: the classified trigger attempt, then the
    // standalone adapter — never the gateway summary.
    let requests = assert_count(&scenario, 2);
    assert_eq!(requests[0].line(), "POST /responses");
    assert_eq!(requests[1].line(), "POST /responses/compact");

    // The adapter's canonical window seals into the one compact-only carrier
    // item, with the compact's usage as the billed totals.
    assert_eq!(body["status"], "completed", "{body}");

    let item = body["output"][0].clone();
    assert_eq!(item["type"], "compaction");

    let carrier = item["encrypted_content"].as_str().expect("the carrier");
    assert!(carrier.starts_with(sealing::CARRIER_NAMESPACE));

    assert_eq!(body["usage"]["input_tokens"], 120);
    assert_eq!(body["usage"]["output_tokens"], 30);

    drop(scenario);
}

/// Scenario — re-compaction preserves instruction identity continuity: a
/// client that drops one covered instruction and re-compacts receives a new
/// checkpoint whose coverage identities still match the surviving instruction,
/// so the next replay never duplicates the instruction context.
///
/// # Panics
///
/// Panics when the pipeline diverges from the scenario's contract.
#[tokio::test]
async fn recompaction_preserves_instruction_identity_continuity() {
    let summary = |text: &str| responses_completed(json!([responses_message_item(text)]), 10, 5);

    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(summarize_only_section()),
        None,
        None,
        false,
        vec![summary("first"), summary("second"), summary("third")],
    )
    .await;

    // Compact: two covered instructions (a top-level instruction and a system
    // input item) beside the covered history.
    let (status, first) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({
            "model": MODEL,
            "instructions": "directive A",
            "input": [
                {"type": "message", "role": "system", "content": "system B"},
                user_item("old question"),
            ],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{first}");

    let first_carrier = first["output"][0]["encrypted_content"]
        .as_str()
        .expect("the first carrier");

    let payload = crate::compaction_model::sealing::unseal_with_live_config(first_carrier, None)
        .expect("the first carrier unseals");

    let crate::compaction_model::envelope::EnvelopePayload::GatewaySummary(first_payload) =
        &payload.payload
    else {
        panic!("the first carrier seals a summary envelope");
    };

    assert_eq!(
        first_payload
            .preserved_instructions
            .iter()
            .map(|instruction| instruction.text.as_str())
            .collect::<Vec<_>>(),
        vec!["directive A", "system B"],
    );

    // Re-compact with one covered instruction dropped: the restored window's
    // instruction context rides the sealed per-item identities, not one joined
    // identity.
    let (status, second) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({
            "model": MODEL,
            "instructions": "directive A",
            "input": [compaction_item(first_carrier), user_item("pending turn")],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{second}");

    let second_carrier = second["output"][0]["encrypted_content"]
        .as_str()
        .expect("the second carrier");

    let payload = crate::compaction_model::sealing::unseal_with_live_config(second_carrier, None)
        .expect("the second carrier unseals");

    let crate::compaction_model::envelope::EnvelopePayload::GatewaySummary(second_payload) =
        &payload.payload
    else {
        panic!("the second carrier seals a summary envelope");
    };

    assert_eq!(
        second_payload
            .preserved_instructions
            .iter()
            .map(|instruction| instruction.text.as_str())
            .collect::<Vec<_>>(),
        vec!["directive A", "system B"],
        "the re-compact plan reuses the payload's per-item identities"
    );

    // Replay the second checkpoint with the client still carrying only the
    // surviving instruction: the covered instruction set is superseded by
    // identity overlap, and the instruction context restores exactly once.
    let (status, third) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({
            "model": MODEL,
            "instructions": "directive A",
            "input": [compaction_item(second_carrier), user_item("another turn")],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{third}");

    let third_carrier = third["output"][0]["encrypted_content"]
        .as_str()
        .expect("the third carrier");

    let payload = crate::compaction_model::sealing::unseal_with_live_config(third_carrier, None)
        .expect("the third carrier unseals");

    let crate::compaction_model::envelope::EnvelopePayload::GatewaySummary(third_payload) =
        &payload.payload
    else {
        panic!("the third carrier seals a summary envelope");
    };

    assert_eq!(
        third_payload
            .preserved_instructions
            .iter()
            .map(|instruction| instruction.text.as_str())
            .collect::<Vec<_>>(),
        vec!["directive A", "system B"],
        "the replayed checkpoint never duplicates the instruction context"
    );

    drop(scenario);
}

/// Review regression: native threshold pause preserves the stop reason and iteration usage.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_native_threshold_preserves_pause_and_usage() {
    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(messages_mechanisms_section("supported", "supported")),
        None,
        None,
        false,
        vec![MockAction::respond_json(
            json!({
                "id":"msg_native",
            "type":"message",
            "role":"assistant",
            "model":MODEL,
                "content":[{"type":"compaction",
            "content":"summary",
            "signature":"native-signature"}],
                "stop_reason":"compaction",
            "usage":{"input_tokens":0,
            "output_tokens":0,
            "iterations":[{"type":"compaction",
            "input_tokens":180000,
            "output_tokens":3500}]}
            })
            .to_string(),
        )],
    )
    .await;
    let (status, body) = post_messages(
        &scenario,
        Some("compact-2026-01-12"),
        json!({
            "model":MODEL,
        "max_tokens":100,
        "messages":[{"role":"user",
        "content":"old history"}],
            "context_management":{"edits":[{"type":"compact_20260112",
        "pause_after_compaction":true}]}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stop_reason"], "compaction");
    assert_eq!(body["usage"]["iterations"][0]["input_tokens"], 180000);
}

/// Review regression: emulated on-demand usage includes billed compaction work.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_on_demand_usage_includes_summary() {
    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(summarize_only_section()),
        None,
        None,
        false,
        vec![anthropic_message(
            json!([{"type":"text",
"text":"summary"}]),
            "end_turn",
            20,
            5,
        )],
    )
    .await;
    let (status, body) = post_messages(
        &scenario,
        Some("compact-2026-09-04"),
        json!({
            "model":MODEL,
        "max_tokens":100,
        "messages":[{"role":"user",
        "content":"old question"}],
        "compaction":{"type":"summarize"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["usage"]["iterations"][0]["input_tokens"], 20);
}

/// Review regression: consecutive markers at one message offset select the latest envelope.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_latest_same_offset_checkpoint_wins() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("answer")]),
            4,
            2,
        )],
    )
    .await;
    let carrier = |summary: &str| {
        history::seal_envelope(
            EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
                summary_text: summary.into(),
                preserved_instructions: vec![],
                retained_units: vec![],
                retained_state_items: vec![],
                overlap_identities: vec![],
            }),
            ReplacementMode::HistoricalCheckpoint,
            None,
        )
        .expect("seal")
        .1
    };
    let first = carrier("obsolete summary");
    let second = carrier("latest summary");
    let (status, _) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
        "input":[
                {"type":"compaction",
        "encrypted_content":first},
        {"type":"compaction",
        "encrypted_content":second},
        user_item("next")
            ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = recorded_body(&scenario, 0).to_string();
    assert!(upstream.contains("latest summary"), "{upstream}");
    assert!(!upstream.contains("obsolete summary"));
}

/// Review regression: native state in retained units survives sealing and replay.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_retained_native_state_survives_sealing() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![],
    )
    .await;
    let request = crate::ir::IrRequest {
        messages: vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "old ".repeat(100),
                }],
            },
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text { text: "new".into() }],
            },
        ],
        state_items: vec![crate::ir::StateItem {
            offset: 1,
            item: json!({"type":"reasoning",
"encrypted_content":"retained-secret"}),
            is_gateway: false,
            is_compaction: false,
        }],
        ..Default::default()
    };
    let plan = history::plan_gateway_checkpoint(
        &request,
        &history::CheckpointInstructions::Parsed(&[]),
        false,
        history::Budgets {
            summary: 4096,
            retention: 1,
        },
    )
    .expect("plan");
    assert_eq!(plan.retained_state_items.len(), 1);
    let (_, carrier) = history::seal_gateway_checkpoint(
        &plan,
        "summary".into(),
        ReplacementMode::CanonicalWindow,
        None,
    )
    .expect("seal");
    use crate::format::ClientFormat as _;
    let parsed = crate::format::openai::response::Format::parse_request(
        &serde_json::to_vec(&json!({"model":MODEL,
"input":[{"type":"compaction",
"encrypted_content":carrier}]}))
        .expect("json"),
        &crate::format::RouteContext::empty(),
    )
    .expect("parse");
    let mut restored = parsed.ir;
    history::prepare(
        &mut restored,
        parsed.compaction.as_ref().expect("compaction"),
        UpstreamFormat::OpenAiResponse,
    )
    .expect("restore");
    assert_eq!(restored.state_items.len(), 1, "retained native state lost");
    drop(scenario);
}

/// Review regression: threshold streaming obtains its summary from compaction_delta.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_threshold_stream_retains_compaction_delta() {
    let frame = |event: &str, value: Value| format!("event: {event}\ndata: {value}\n\n");
    let body = [
        frame(
            "message_start",
            json!({"message":{"id":"msg_sse",
"model":MODEL,
"usage":{"input_tokens":0,
"output_tokens":0}}}),
        ),
        frame(
            "content_block_start",
            json!({"index":0,
"content_block":{"type":"compaction",
"content":null}}),
        ),
        frame(
            "content_block_delta",
            json!({"index":0,
"delta":{"type":"compaction_delta",
"content":"complete summary"}}),
        ),
        frame("content_block_stop", json!({"index":0})),
        frame(
            "message_delta",
            json!({"delta":{"stop_reason":"compaction"},
"usage":{"output_tokens":0,
"iterations":[{"type":"compaction",
"input_tokens":180000,
"output_tokens":3500}]}}),
        ),
        frame("message_stop", json!({})),
    ]
    .concat();
    let action = MockAction::Respond(format!(
        "HTTP/1.1 200 OK\r\n\
Content-Type: text/event-stream\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\
\r\n\
{body}",
        body.len()
    ));
    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(messages_mechanisms_section("supported", "supported")),
        None,
        None,
        true,
        vec![action],
    )
    .await;
    let (status, body) = post_messages(
        &scenario,
        Some("compact-2026-01-12"),
        json!({"model":MODEL,
"max_tokens":100,
"messages":[{"role":"user",
"content":"history"}],
"context_management":{"edits":[{"type":"compact_20260112",
"pause_after_compaction":true}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"][0]["content"], "complete summary");
}

/// Review regression: parallel tool calls and all corresponding outputs form one retained unit.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[test]
fn review_retention_preserves_parallel_tool_exchange() {
    let call = |id: &str| IrMessage {
        role: IrRole::Assistant,
        parts: vec![IrPart::ToolCall {
            call_id: id.into(),
            name: "tool".into(),
            arguments: "{}".into(),
            thought_signature: None,
        }],
    };
    let output = |id: &str| IrMessage {
        role: IrRole::Tool,
        parts: vec![IrPart::ToolOutput {
            call_id: id.into(),
            content: "ok".into(),
            is_error: false,
        }],
    };
    let request = crate::ir::IrRequest {
        messages: vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "old ".repeat(100),
                }],
            },
            call("A"),
            call("B"),
            output("A"),
            output("B"),
        ],
        ..Default::default()
    };
    for (budget, expected_count) in [(4, 0), (100, 4)] {
        let plan = history::plan_gateway_checkpoint(
            &request,
            &history::CheckpointInstructions::Parsed(&[]),
            false,
            history::Budgets {
                summary: 4096,
                retention: budget,
            },
        )
        .expect("plan");
        let kept: Vec<_> = plan
            .retained_units
            .iter()
            .flat_map(|messages| messages.iter())
            .collect();
        assert_eq!(
            kept.len(),
            expected_count,
            "a parallel exchange is entirely retained or entirely summarized"
        );
        for message in &kept {
            for part in &message.parts {
                if let IrPart::ToolOutput { call_id, .. } = part {
                    let has_retained_call = kept.iter().any(|message| {
                        message.parts.iter().any(|part| {
                            matches!(part, IrPart::ToolCall {
                                call_id: retained_id, ..
                            } if retained_id == call_id)
                        })
                    });
                    assert!(has_retained_call, "retained result lost its call: {kept:?}");
                }
            }
        }
    }
}

/// Review regression: native compact bodies address the upstream model,
/// never its public alias.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_native_compact_uses_upstream_model() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(vec![])),
        None,
        None,
        false,
        vec![compact_response(window_output(), 20, 5)],
    )
    .await;
    scenario
        ._database
        .0
        .execute(
            "UPDATE model_routes SET public_id = 'public-alias' WHERE is_automatic = 0",
            (),
        )
        .await
        .expect("alias");
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({"model":"public-alias",
"input":[user_item("old question")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(recorded_body(&scenario, 0)["model"], MODEL);
}

/// Review regression: an official compaction output item can replay with created_by intact.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn review_native_created_by_round_trips() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(vec![])),
        None,
        None,
        false,
        vec![
            compact_response(
                json!([{"type":"compaction",
"id":"cmp_native",
"encrypted_content":"opaque",
"created_by":"upstream-actor"}]),
                20,
                5,
            ),
            responses_completed(json!([responses_message_item("answer")]), 4, 2),
        ],
    )
    .await;
    let (status, compacted) = post_openai(
        &scenario,
        "/api/openai-v1/responses/compact",
        json!({"model":MODEL,
"input":[user_item("old question")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
"input":compacted["output"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        recorded_body(&scenario, 1)["input"][0]["created_by"],
        "upstream-actor"
    );
}
/// Automatic passthrough retries classifier-positive rejections once,
/// with controls stripped.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn automatic_and_threshold_rejections_use_one_gateway_fallback() {
    let mut automatic = automatic_section("supported");
    automatic["retention_budget"] = json!(1);
    automatic["mechanisms"]["responses_automatic_compaction"]["unsupported"] =
        json!({"statuses":[400]});
    let response = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(automatic),
        None,
        None,
        false,
        vec![
            MockAction::Respond(
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                    .into(),
            ),
            responses_completed(json!([responses_message_item("summary")]), 20, 5),
            responses_completed(json!([responses_message_item("answer")]), 8, 3),
        ],
    )
    .await;
    let (status, body) = post_openai(
        &response,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
"input":[user_item(&"old ".repeat(100)),
user_item("pending")],
"context_management":[{"type":"compaction",
"compact_threshold":1}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(recorded(&response).len(), 3);
    assert!(
        recorded_body(&response, 1)
            .get("context_management")
            .is_none()
    );
    assert!(
        recorded_body(&response, 2)
            .get("context_management")
            .is_none()
    );
    let mut threshold = messages_mechanisms_section("unknown", "supported");
    threshold["retention_budget"] = json!(1);
    threshold["mechanisms"]["messages_threshold_compaction"]["unsupported"] =
        json!({"statuses":[400]});
    let messages = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(threshold),
        None,
        None,
        false,
        vec![
            MockAction::Respond(
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                    .into(),
            ),
            anthropic_message(
                json!([{"type":"text",
"text":"summary"}]),
                "end_turn",
                20,
                5,
            ),
            anthropic_message(
                json!([{"type":"text",
"text":"answer"}]),
                "end_turn",
                8,
                3,
            ),
        ],
    )
    .await;
    let (status, body) = post_messages(
        &messages,
        Some("compact-2026-01-12"),
        json!({"model":MODEL,
"max_tokens":100,
"messages":[{"role":"user",
"content":"old ".repeat(40000)},
{"role":"user",
"content":"pending"}],
"context_management":{"edits":[{"type":"compact_20260112",
"trigger":{"type":"input_tokens",
"value":50000}}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(recorded(&messages).len(), 3);
    assert!(
        recorded_body(&messages, 1)
            .get("context_management")
            .is_none()
    );
    assert!(
        recorded_body(&messages, 2)
            .get("context_management")
            .is_none()
    );
}
/// Emulated thresholds use an explicit native adapter and retain the active turn across replay.
///
/// # Panics
///
/// Panics if the regression contract fails.
#[tokio::test]
async fn threshold_adapters_preserve_the_pending_turn_and_native_window() {
    let response = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(standalone_section(vec![])),
        None,
        None,
        false,
        vec![
            compact_response(window_output(), 20, 5),
            responses_completed(json!([responses_message_item("answer")]), 8, 3),
            responses_completed(json!([responses_message_item("next")]), 9, 4),
        ],
    )
    .await;
    let (status, body) = post_openai(
        &response,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
"input":[user_item(&"old ".repeat(100)),
user_item("pending")],
"context_management":[{"type":"compaction",
"compact_threshold":1}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(recorded(&response)[0].target.contains("/responses/compact"));
    let continuation = recorded_body(&response, 1);
    assert!(
        continuation["input"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["encrypted_content"] == NATIVE_STATE)
    );
    assert!(continuation.to_string().contains("pending"));
    assert!(!recorded_body(&response, 0).to_string().contains("pending"));
    let carrier = body["output"]
        .as_array()
        .expect("output")
        .iter()
        .find(|item| item["type"] == "compaction")
        .expect("checkpoint")
        .clone();
    let (status, body) = post_openai(
        &response,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
"input":[carrier,
user_item("fresh")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(recorded_body(&response, 2).to_string().contains("pending"));
    let messages = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(messages_mechanisms_section("supported", "unknown")),
        None,
        None,
        false,
        vec![
            anthropic_message(
                json!([{"type":"compaction",
"content":"native summary",
"signature":"signature"}]),
                "compaction",
                20,
                5,
            ),
            anthropic_message(
                json!([{"type":"text",
"text":"answer"}]),
                "end_turn",
                8,
                3,
            ),
        ],
    )
    .await;
    let (status, body) = post_messages(
        &messages,
        Some("compact-2026-01-12"),
        json!({"model":MODEL,
"max_tokens":100,
"messages":[{"role":"user",
"content":"old ".repeat(40000)},
{"role":"user",
"content":"pending"}],
"context_management":{"edits":[{"type":"compact_20260112",
"trigger":{"type":"input_tokens",
"value":50000}}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        recorded_body(&messages, 0)["compaction"]["type"],
        "summarize"
    );
    assert!(!recorded_body(&messages, 0).to_string().contains("pending"));
    assert!(recorded_body(&messages, 1).to_string().contains("pending"));
    assert!(
        recorded_body(&messages, 1)
            .to_string()
            .contains("signature")
    );
}
/// Readable export preserves pending text and rejects opaque state and damaged older carriers.
///
/// # Panics
///
/// Panics if the export contract or scenario setup fails.
#[tokio::test]
async fn readable_export_requires_policy_and_preserves_only_representable_state() {
    let mut section = summarize_only_section();
    section["can_export_native_summary"] = json!(true);
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        Some(section),
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("answer")]),
            8,
            3,
        )],
    )
    .await;
    let payload = |block| NativeWindowPayload {
        producer_instance_id: "provider:999".into(),
        upstream_model_id: "foreign-model".into(),
        mechanism: Mechanism::MessagesOnDemandCompaction,
        native_format: UpstreamFormat::AnthropicMessages,
        required_prefix_identity: None,
        retained_messages: vec![text_message(IrRole::User, "pending")],
        retained_state_items: vec![],
        window: NativeReplayWindow::AnthropicOnDemandBlock { block },
    };
    let (_, carrier) = history::seal_native_window(
        payload(json!({"type":"compaction",
"content":"readable summary",
"signature":"native"})),
        ReplacementMode::HistoricalCheckpoint,
        None,
    )
    .expect("carrier");
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
"input":[{"type":"compaction",
"encrypted_content":carrier},
user_item("fresh")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = recorded_body(&scenario, 0).to_string();
    assert!(
        sent.contains("readable summary") && sent.contains("pending") && sent.contains("fresh")
    );
    assert!(!sent.contains("native"));
    for block in [
        json!({"type":"compaction",
"content":"summary",
"encrypted_content":"cipher"}),
        json!({"type":"compaction",
"content":"summary",
"tool_changes":[{"type":"tool_addition"}]}),
    ] {
        let (_, carrier) = history::seal_native_window(
            payload(block),
            ReplacementMode::HistoricalCheckpoint,
            None,
        )
        .expect("carrier");
        let (status, body) = post_openai(
            &scenario,
            "/api/openai-v1/responses",
            json!({"model":MODEL,
"input":[{"type":"compaction",
"encrypted_content":carrier},
user_item("fresh")]}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({"model":MODEL,
"input":[{"type":"compaction",
"encrypted_content":"janusgate.compaction.v1.missing.invalid"},
{"type":"compaction",
"encrypted_content":carrier},
user_item("fresh")]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(recorded(&scenario).len(), 1);
}
/// Builds an in-memory direct-route routing fixture whose provider speaks
/// `format` and whose compaction section permits readable native-summary
/// export — the policy the `prepare_for_target` export fixtures exercise
/// without a serving pipeline.
///
/// # Panics
///
/// Panics if the fixture definition payload fails to parse, which its shape
/// prevents.
fn export_routing(format: UpstreamFormat) -> ModelRouting {
    let mut section = summarize_only_section();
    section["can_export_native_summary"] = json!(true);

    ModelRouting {
        model_route_id: 1,
        public_id: String::from(MODEL),
        provider: Provider {
            id: 999,
            name: String::from("acceptance"),
            model_prefix: String::new(),
            base_url: None,
            credentials: StoredCredentials::ApiKey {
                api_key: String::from("upstream-secret"),
            },
            is_enabled: true,
            quota_generation: 0,
            quota_revision: 0,
            definition: ProviderDefinition {
                name: String::from("acceptance"),
                version: NonZeroI64::new(1).expect("one is positive"),
                payload: definition_payload(format, Some(section), false),
                created_at: 1,
            },
        },
        model: Model {
            id: 1,
            upstream_id: String::from(MODEL),
            is_enabled: true,
            discovery_state: crate::provider::DiscoveryState::Manual,
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support: None,
            compaction_support_override: None,
        },
    }
}
/// The raw-native readable export probes the boundary's own state slot: an
/// unrelated opaque state item sharing the boundary offset but preceding the
/// compaction block in encounter order is never mistaken for the block, so an
/// authorized export of a readable block exports instead of silently replaying
/// incompatible native state to a non-Anthropic target.
///
/// # Panics
///
/// Panics if the export contract or the fixture setup fails.
#[test]
fn readable_export_probes_the_boundary_slot_not_any_same_offset_item() {
    // Two opaque items share the boundary offset: the reasoning item precedes
    // the compaction block in vector order, so an offset-only probe matches
    // the reasoning item and misses the readable block.
    let mut request = crate::ir::IrRequest {
        messages: vec![
            text_message(IrRole::User, "old"),
            text_message(IrRole::User, "pending"),
        ],
        state_items: vec![
            crate::ir::StateItem {
                offset: 1,
                item: json!({"type":"reasoning",
"encrypted_content":"covered reasoning"}),
                is_gateway: false,
                is_compaction: false,
            },
            crate::ir::StateItem {
                offset: 1,
                item: json!({"type":"compaction",
"content":"readable summary",
"signature":"native"}),
                is_gateway: false,
                is_compaction: true,
            },
        ],
        ..Default::default()
    };
    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Native {
            offset: 1,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: false,
                placement: BoundaryPlacement::Historical,
            },
        }],
        instruction_items: Vec::new(),
    };
    let preparation =
        BoundaryPreparation::authenticate(&parsed).expect("the native boundary authenticates");

    let (boundary, _) = history::prepare_for_target(
        &mut request,
        &parsed,
        &export_routing(UpstreamFormat::OpenAiResponse),
        &preparation,
    )
    .expect("the readable block exports");

    // The export happened: the readable summary heads the restored window and
    // no native state rides to the non-Anthropic target. Same-offset state the
    // boundary's encounter order precedes is covered with the prefix.
    assert!(
        boundary.is_none(),
        "an exported readable block establishes no native boundary"
    );
    assert_eq!(request.messages.len(), 2);
    let IrPart::Text { text } = &request.messages[0].parts[0] else {
        panic!("the summary message renders text");
    };
    assert!(text.contains("readable summary"), "{text}");
    assert_eq!(request.messages[1], text_message(IrRole::User, "pending"));
    assert!(
        request.state_items.is_empty(),
        "same-offset state the boundary precedes is covered: {:?}",
        request.state_items
    );
}
/// Fresh-suffix state sharing the boundary offset — state the boundary's
/// encounter order does not precede — rides the readable export at its
/// recomputed offset, exactly as the restore paths treat it; it never vanishes
/// silently from a successful restored window.
///
/// # Panics
///
/// Panics if the export contract or the fixture setup fails.
#[test]
fn readable_export_rides_same_offset_fresh_state_at_its_recomputed_offset() {
    let _configuration_guard = crate::test_support::configuration_lock();
    let _configuration = crate::test_support::install_global_configuration();

    let (_, carrier) = history::seal_native_window(
        NativeWindowPayload {
            producer_instance_id: "provider:888".into(),
            upstream_model_id: "foreign-model".into(),
            mechanism: Mechanism::MessagesOnDemandCompaction,
            native_format: UpstreamFormat::AnthropicMessages,
            required_prefix_identity: None,
            retained_messages: vec![text_message(IrRole::User, "pending")],
            retained_state_items: vec![],
            window: NativeReplayWindow::AnthropicOnDemandBlock {
                block: json!({"type":"compaction",
"content":"readable summary",
"signature":"native"}),
            },
        },
        ReplacementMode::HistoricalCheckpoint,
        None,
    )
    .expect("carrier");

    // The carrier precedes the opaque item at the shared offset: the item is
    // fresh-suffix state, not covered prefix.
    let mut request = crate::ir::IrRequest {
        messages: vec![
            text_message(IrRole::User, "old"),
            text_message(IrRole::User, "fresh"),
        ],
        state_items: vec![
            crate::ir::StateItem {
                offset: 1,
                item: json!({"type":"compaction",
"encrypted_content":carrier.clone()}),
                is_gateway: true,
                is_compaction: true,
            },
            crate::ir::StateItem {
                offset: 1,
                item: json!({"type":"reasoning",
"encrypted_content":"fresh reasoning"}),
                is_gateway: false,
                is_compaction: false,
            },
        ],
        ..Default::default()
    };
    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Gateway {
            offset: 1,
            carrier,
            visible_content: None,
        }],
        instruction_items: Vec::new(),
    };
    let preparation =
        BoundaryPreparation::authenticate(&parsed).expect("the carrier authenticates");

    let (boundary, _) = history::prepare_for_target(
        &mut request,
        &parsed,
        &export_routing(UpstreamFormat::OpenAiResponse),
        &preparation,
    )
    .expect("the readable block exports");

    // The restored window: summary, the payload's retained message, the fresh
    // turn — and the fresh same-offset state riding at its recomputed offset
    // (one summary message plus one retained message precede it).
    assert!(boundary.is_none());
    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.state_items.len(), 1);
    assert!(!request.state_items[0].is_gateway);
    assert_eq!(request.state_items[0].item["type"], "reasoning");
    assert_eq!(
        request.state_items[0].offset, 2,
        "the fresh state rides at its recomputed offset"
    );
}
/// Threshold and on-demand streaming use their own block framing and retain pause usage.
///
/// # Panics
///
/// Panics if the streamed protocol differs from its contract.
#[tokio::test]
async fn messages_streams_frame_threshold_and_on_demand_compaction_distinctly() {
    for is_threshold in [false, true] {
        let action = if is_threshold {
            let frame = |event: &str, data: Value| format!("event: {event}\ndata: {data}\n\n");
            let stream = [
                frame(
                    "message_start",
                    json!({"message":{"id":"msg",
"model":MODEL,
"usage":{"input_tokens":0,
"output_tokens":0}}}),
                ),
                frame(
                    "content_block_start",
                    json!({"index":0,
"content_block":{"type":"compaction",
"content":null,
"signature":"signature"}}),
                ),
                frame(
                    "content_block_delta",
                    json!({"index":0,
"delta":{"type":"compaction_delta",
"content":"native summary"}}),
                ),
                frame("content_block_stop", json!({"index":0})),
                frame(
                    "message_delta",
                    json!({"delta":{"stop_reason":"compaction"},
"usage":{"output_tokens":0,
"iterations":[{"type":"compaction",
"input_tokens":20,
"output_tokens":5}]}}),
                ),
                frame("message_stop", json!({})),
            ]
            .concat();
            MockAction::Respond(format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\r\n{stream}",
                stream.len()
            ))
        } else {
            anthropic_message(
                json!([{"type":"compaction",
"content":"native summary",
"signature":"signature"}]),
                "compaction",
                20,
                5,
            )
        };
        let scenario = scenario(
            UpstreamFormat::AnthropicMessages,
            Some(messages_mechanisms_section("supported", "supported")),
            None,
            None,
            false,
            vec![action],
        )
        .await;
        let mut body = json!({"model":MODEL,
"stream":true,
"max_tokens":100,
"messages":[{"role":"user",
"content":"history"}]});
        let beta = if is_threshold {
            body["context_management"] = json!({"edits":[{"type":"compact_20260112",
"pause_after_compaction":true}]});
            "compact-2026-01-12"
        } else {
            body["compaction"] = json!({"type":"summarize"});
            "compact-2026-09-04"
        };
        let (status, stream) = post(
            &scenario,
            "/api/anthropic-v1/messages",
            &messages_headers(Some(beta)),
            body.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{stream}");
        let frames = sse_payloads(&stream);
        if is_threshold {
            let start = frames
                .iter()
                .find(|frame| frame["type"] == "content_block_start")
                .expect("start");
            assert_eq!(start["content_block"]["content"], Value::Null);
            assert_eq!(start["content_block"]["signature"], "signature");
            assert_eq!(
                frames
                    .iter()
                    .filter(|frame| frame["delta"]["type"] == "compaction_delta")
                    .count(),
                1
            );
            let terminal = frames
                .iter()
                .find(|frame| frame["type"] == "message_delta")
                .expect("terminal");
            assert_eq!(terminal["delta"]["stop_reason"], "compaction");
            assert_eq!(terminal["usage"]["iterations"][0]["input_tokens"], 20);
        } else {
            assert!(
                frames
                    .iter()
                    .any(|frame| frame["content_block"]["content"] == "native summary")
            );
            assert!(
                !frames
                    .iter()
                    .any(|frame| frame["delta"]["type"] == "compaction_delta")
            );
        }
    }
}

/// Native counting measures a restored opaque window and addresses the upstream model alias.
///
/// # Panics
///
/// Panics if the native count path rejects replayable state or dispatches an incorrect model.
#[tokio::test]
async fn native_counter_measures_restored_state_with_the_upstream_model() {
    let mut section = messages_mechanisms_section("supported", "unknown");
    section["count_tokens"] = json!({
        "url": [{"type":"base"}, {"type":"literal","value":"/messages/count_tokens"}]
    });
    let scenario = scenario(
        UpstreamFormat::AnthropicMessages,
        Some(section),
        None,
        None,
        false,
        vec![
            MockAction::respond_json(json!({"input_tokens":1}).to_string()),
            anthropic_message(json!([{"type":"text","text":"answer"}]), "end_turn", 8, 3),
        ],
    )
    .await;
    scenario
        ._database
        .0
        .execute(
            "UPDATE model_routes SET public_id = 'public-alias' WHERE is_automatic = 0",
            (),
        )
        .await
        .expect("alias");
    let routing = crate::gateway::resolve_routing(&scenario._database.0, "public-alias")
        .await
        .expect("routing");
    let visible = crate::format::anthropic::messages::GATEWAY_CARRIER_DESCRIPTION;
    let (_, carrier) = history::seal_native_window(
        NativeWindowPayload {
            producer_instance_id: format!("provider:{}", routing.provider.id),
            upstream_model_id: MODEL.into(),
            mechanism: Mechanism::MessagesOnDemandCompaction,
            native_format: UpstreamFormat::AnthropicMessages,
            required_prefix_identity: None,
            retained_messages: vec![],
            retained_state_items: vec![],
            window: NativeReplayWindow::AnthropicOnDemandBlock {
                block: json!({
                    "type":"compaction",
                    "content":"native summary",
                    "signature":"native signature"
                }),
            },
        },
        ReplacementMode::HistoricalCheckpoint,
        Some(visible),
    )
    .expect("carrier");
    let block = crate::format::anthropic::messages::gateway_carrier_block(Some(visible), &carrier);
    let (status, body) = post_messages(
        &scenario,
        Some("compact-2026-01-12"),
        json!({
            "model":"public-alias", "max_tokens":100,
            "messages":[{"role":"assistant","content":[block]},{"role":"user","content":"pending"}],
            "context_management":{"edits":[{"type":"compact_20260112"}]}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(recorded(&scenario)[0].target, "/messages/count_tokens");
    let count_body = recorded_body(&scenario, 0);
    assert_eq!(count_body["model"], MODEL);
    assert!(count_body.to_string().contains("native signature"));
    assert!(
        recorded_body(&scenario, 1)
            .get("context_management")
            .is_none()
    );
    assert_eq!(recorded(&scenario).len(), 2);
}

/// Exact quota rejection evidence wins over native-unsupported fallback and also records direct
/// generation failures. Restriction recipes resolve the physical model, even through a public
/// alias; these direct routes still mirror the original upstream error.
///
/// # Panics
///
/// Panics if policy precedence, actual dispatch count, or durable model applicability diverges.
#[tokio::test]
async fn classified_quota_rejection_precedes_native_fallback_and_records_direct_evidence() {
    let long = "x".repeat(150_001);
    let mut automatic = automatic_section("supported");
    automatic["mechanisms"]["responses_automatic_compaction"]["unsupported"] =
        json!({"statuses":[429]});
    let mut threshold = messages_mechanisms_section("unknown", "supported");
    threshold["mechanisms"]["messages_threshold_compaction"]["unsupported"] =
        json!({"statuses":[429]});
    let cases = [
        (
            UpstreamFormat::OpenAiResponse,
            standalone_section(vec![429]),
            "/api/openai-v1/responses",
            None,
            json!({"model":"public-alias","input":[user_item(&long)]}),
        ),
        (
            UpstreamFormat::OpenAiResponse,
            standalone_section(vec![429]),
            "/api/openai-v1/responses/compact",
            None,
            json!({"model":"public-alias","input":[user_item(&long),user_item("pending")]}),
        ),
        (
            UpstreamFormat::OpenAiResponse,
            automatic,
            "/api/openai-v1/responses",
            None,
            json!({"model":"public-alias","input":[user_item(&long),user_item("pending")],
                "context_management":[{"type":"compaction","compact_threshold":1}]}),
        ),
        (
            UpstreamFormat::AnthropicMessages,
            threshold,
            "/api/anthropic-v1/messages",
            Some("compact-2026-01-12"),
            json!({"model":"public-alias","max_tokens":100,
                "messages":[{"role":"user","content":long},{"role":"user","content":"pending"}],
                "context_management":{"edits":[{"type":"compact_20260112",
                    "trigger":{"type":"input_tokens","value":50000}}]}}),
        ),
    ];
    for (format, section, path, beta, request) in cases {
        let policy = serde_json::from_value(json!({"rules":[{
            "name":"per-model","statuses":[429],
            "code":{"at":["error","code"],"value":"model_quota_exhausted"},
            "restriction_key":[{"type":"literal","value":"models"},
                {"type":"requested_model"}],
            "applies_to":{"type":"requested_model"}
        }]}))
        .expect("the exact quota rule parses");
        let instance = scenario_with_quota_policy(
            format,
            Some(section),
            None,
            None,
            false,
            vec![
                raw_json_response(
                    "429 Too Many Requests",
                    json!({"error":{"code":"model_quota_exhausted"}}),
                ),
                responses_completed(json!([responses_message_item("must not summarize")]), 10, 5),
            ],
            Some(policy),
        )
        .await;
        instance
            ._database
            .0
            .execute(
                "UPDATE model_routes SET public_id = 'public-alias' WHERE is_automatic = 0",
                (),
            )
            .await
            .expect("the fixture public route is renamed");
        let (status, _) = if format == UpstreamFormat::AnthropicMessages {
            post_messages(&instance, beta, request).await
        } else {
            post_openai(&instance, path, request).await
        };
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_dispatches(&instance, 1);
        let providers = crate::storage::providers::list_providers(&instance._database.0)
            .await
            .expect("the account list reads");
        let provider = providers.first().expect("the fixture account exists");
        let state = crate::storage::quota_blocks::load_state(&instance._database.0, provider.id)
            .await
            .expect("quota state reads")
            .expect("the account exists");
        let [block]: [_; 1] = state
            .blocks
            .try_into()
            .expect("one model restriction is recorded");
        assert_eq!(block.key, ["models", MODEL]);
        assert_eq!(
            block.applies_to,
            crate::provider::quota::QuotaApplicability::Models {
                upstream_ids: vec![MODEL.into()],
            }
        );
        assert_eq!(block.blocked_until, None);
    }
}

/// Fresh compaction operations may abandon only their first quota-rejected dispatch. The
/// replacement account owns the actual artifact and subsequent exact retry; recovery of the
/// earlier account cannot move the operation back.
///
/// # Panics
///
/// Panics if fixture construction, stage eligibility, or account commitment differs.
#[tokio::test]
async fn fresh_compaction_quota_rejection_selects_the_next_account() {
    for operation in [
        "standalone",
        "summary",
        "automatic",
        "on_demand",
        "threshold",
    ] {
        let is_messages = matches!(operation, "on_demand" | "threshold");
        let section = match operation {
            "standalone" => standalone_section(vec![429]),
            "summary" => summarize_only_section(),
            "automatic" => automatic_section("supported"),
            _ => messages_mechanisms_section("supported", "supported"),
        };
        let policy = serde_json::from_value(json!({"rules":[{
            "name":"account-quota","statuses":[429],
            "code":{"at":["error","code"],"value":"account_quota_exhausted"},
            "restriction_key":[{"type":"literal","value":"account"}],
            "applies_to":{"type":"all_models"}
        }]}))
        .expect("quota policy");
        let instance = scenario_with_quota_policy(
            if is_messages {
                UpstreamFormat::AnthropicMessages
            } else {
                UpstreamFormat::OpenAiResponse
            },
            Some(section),
            None,
            None,
            false,
            vec![raw_json_response(
                "429 Too Many Requests",
                json!({"error":{"code":"account_quota_exhausted"}}),
            )],
            Some(policy),
        )
        .await;
        let success = match operation {
            "standalone" => compact_response(
                json!([{"type":"compaction","id":"cmp_second","encrypted_content":"second state"}]),
                8,
                3,
            ),
            "on_demand" | "threshold" => anthropic_message(
                json!([{"type":"compaction","content":"second summary","signature":"sig_second"}]),
                "compaction",
                8,
                3,
            ),
            _ => responses_completed(json!([responses_message_item("second summary")]), 8, 3),
        };
        let accounts = add_second_pool_account(&instance, vec![success]).await;
        let long = "w".repeat(150_001);
        let request = match operation {
            "automatic" => json!({"model":MODEL,"input":[user_item(&long),user_item("pending")],
                "context_management":[{"type":"compaction","compact_threshold":1}]}),
            "on_demand" => json!({"model":MODEL,"max_tokens":100,
                "messages":[{"role":"user","content":"history"},
                    {"role":"user","content":"pending"}],
                "compaction":{"type":"summarize"}}),
            "threshold" => json!({"model":MODEL,"max_tokens":100,
                "messages":[{"role":"user","content":long},{"role":"user","content":"pending"}],
                "context_management":{"edits":[{"type":"compact_20260112",
                    "trigger":{"type":"input_tokens","value":50000},
                    "pause_after_compaction":true}]}}),
            _ => json!({"model":MODEL,"input":[user_item("history"),user_item("pending")]}),
        };
        let path = if matches!(operation, "standalone" | "summary") {
            "/api/openai-v1/responses/compact"
        } else {
            "/api/openai-v1/responses"
        };
        let (status, body) = if is_messages {
            post_messages(
                &instance,
                Some(if operation == "on_demand" {
                    "compact-2026-09-04"
                } else {
                    "compact-2026-01-12"
                }),
                request.clone(),
            )
            .await
        } else {
            post_openai(&instance, path, request.clone()).await
        };
        assert_eq!(status, StatusCode::OK, "{operation}: {body}");
        assert_dispatches(&instance, 1);
        assert_eq!(
            accounts.second_recorded.lock().expect("recording").len(),
            1,
            "{operation}"
        );
        crate::storage::quota_blocks::reset(&instance._database.0, accounts.first.provider.id)
            .await
            .expect("recover first account");
        crate::storage::quota_blocks::record_rejection(
            &instance._database.0,
            accounts.second.id,
            accounts.second.quota_generation,
            &["account".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("exhaust committed account");
        let (status, body) = if is_messages {
            post_messages(
                &instance,
                Some(if operation == "on_demand" {
                    "compact-2026-09-04"
                } else {
                    "compact-2026-01-12"
                }),
                request,
            )
            .await
        } else {
            post_openai(&instance, path, request).await
        };
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{operation}: {body}");
        assert_bound_quota_error(&body, is_messages);
        assert_dispatches(&instance, 1);
        assert_eq!(accounts.second_recorded.lock().expect("recording").len(), 1);
    }
}

/// Counter failures, unsupported-native attempts, and accepted summaries already spend a stage
/// on the selected account. A later classified quota rejection never starts another account.
///
/// # Panics
///
/// Panics if a later rejection is retried or the original quota evidence is lost.
#[tokio::test]
async fn compaction_quota_rejections_after_earlier_stages_do_not_switch_accounts() {
    for operation in ["counter", "unsupported", "summary_accepted"] {
        let is_messages = operation == "counter";
        let section = match operation {
            "counter" => json!({"count_tokens":{"url":[{"type":"base"},
                {"type":"literal","value":"/messages/count_tokens"}]},"retention_budget":1}),
            "unsupported" => standalone_section(vec![400]),
            _ => summarize_only_section(),
        };
        let first_action = match operation {
            "counter" => raw_json_response(
                "429 Too Many Requests",
                json!({"error":{"code":"account_quota_exhausted"}}),
            ),
            "unsupported" => raw_json_response("400 Bad Request", json!({"error":"unsupported"})),
            _ => responses_completed(json!([responses_message_item("accepted summary")]), 8, 3),
        };
        let policy = serde_json::from_value(json!({"rules":[{
            "name":"account-quota","statuses":[429],
            "code":{"at":["error","code"],"value":"account_quota_exhausted"},
            "restriction_key":[{"type":"literal","value":"account"}],
            "applies_to":{"type":"all_models"}
        }]}))
        .expect("policy");
        let instance = scenario_with_quota_policy(
            if is_messages {
                UpstreamFormat::AnthropicMessages
            } else {
                UpstreamFormat::OpenAiResponse
            },
            Some(section),
            None,
            None,
            false,
            vec![
                first_action,
                raw_json_response(
                    "429 Too Many Requests",
                    json!({"error":{"code":"account_quota_exhausted"}}),
                ),
            ],
            Some(policy),
        )
        .await;
        let accounts = add_second_pool_account(
            &instance,
            vec![responses_completed(
                json!([responses_message_item("must not run")]),
                4,
                2,
            )],
        )
        .await;
        let long = "w".repeat(150_001);
        let (status, body) = if is_messages {
            post_messages(
                &instance,
                Some("compact-2026-01-12"),
                json!({"model":MODEL,
                "max_tokens":100,"messages":[{"role":"user","content":long},
                    {"role":"user","content":"pending"}],
                "context_management":{"edits":[{"type":"compact_20260112",
                    "trigger":{"type":"input_tokens","value":50000},
                    "pause_after_compaction":true}]}}),
            )
            .await
        } else if operation == "unsupported" {
            post_openai(
                &instance,
                "/api/openai-v1/responses/compact",
                json!({"model":MODEL,"input":[user_item("history"),user_item("pending")]}),
            )
            .await
        } else {
            post_openai(
                &instance,
                "/api/openai-v1/responses",
                json!({"model":MODEL,
                "input":[user_item(&long),user_item("pending")],
                "context_management":[{"type":"compaction","compact_threshold":1}]}),
            )
            .await
        };
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{operation}: {body}");
        assert_dispatches(&instance, 2);
        assert!(
            accounts
                .second_recorded
                .lock()
                .expect("recording")
                .is_empty()
        );
        let state = crate::storage::quota_blocks::load_state(
            &instance._database.0,
            accounts.first.provider.id,
        )
        .await
        .expect("blocks")
        .expect("account");
        assert_eq!(state.blocks.len(), 1, "{operation}");
    }
}

/// Two independently usable accounts sharing one authored definition and exact pool model.
struct PoolAccounts {
    /// Earlier account used to prove recovery cannot move an established binding.
    first: crate::provider::ModelRouting,
    /// Later account whose accepted response establishes the operation.
    second: crate::provider::Provider,
    /// Mock lifetime for the later account.
    second_mock: test_support::MockUpstream,
    /// Captured later-account data-plane calls.
    second_recorded: RecordedHttpRequests,
    /// Authored namespace identity.
    pool_id: i64,
}

/// Publishes the fixture route through two ordered independently credentialed accounts.
///
/// # Panics
///
/// Panics if the authored fixture cannot be inserted or routed.
async fn add_second_pool_account(scenario: &Scenario, actions: Vec<MockAction>) -> PoolAccounts {
    let database = &scenario._database.0;
    let first = crate::gateway::resolve_routing(database, MODEL)
        .await
        .expect("first account");
    let (second_mock, second_recorded) =
        test_support::MockUpstream::start_scripted_recording_bodies(actions).await;
    let base = second_mock.base();
    let definition = insert_definition(
        database,
        "second",
        first.provider.definition.payload.clone(),
    )
    .await;
    let second = insert_provider(
        database,
        &definition,
        "second",
        Some(&base),
        StoredCredentials::ApiKey {
            api_key: "second-secret".into(),
        },
    )
    .await;
    insert_model_checked(
        database,
        ModelCreate {
            provider_id: second.id,
            upstream_id: MODEL,
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await
    .expect("second model");
    let pool_id = crate::storage::routing::create_pool(
        database,
        "native pool",
        "pool-native pool-",
        true,
        &[
            crate::routing::ProviderPoolMember {
                provider_id: first.provider.id,
                priority: 0,
            },
            crate::routing::ProviderPoolMember {
                provider_id: second.id,
                priority: 1,
            },
        ],
    )
    .await
    .expect("pool");
    crate::storage::routing::update_route(
        database,
        first.model_route_id,
        MODEL,
        true,
        &crate::routing::ModelRouteTarget::Pool {
            pool_id,
            upstream_id: MODEL.into(),
        },
    )
    .await
    .expect("pooled route");
    PoolAccounts {
        first,
        second,
        second_mock,
        second_recorded,
        pool_id,
    }
}

/// Authenticated active native provenance selects its own pool member, even after cache loss;
/// a disabled producer cannot borrow another member, while a later portable checkpoint releases
/// the superseded requirement.
///
/// # Panics
///
/// Panics when fixture construction or the account-selection contract fails.
#[tokio::test]
async fn active_native_window_selects_its_pool_producer() {
    let scenario = scenario(
        UpstreamFormat::OpenAiResponse,
        None,
        None,
        None,
        false,
        vec![responses_completed(
            json!([responses_message_item("portable answer")]),
            4,
            2,
        )],
    )
    .await;
    let database = &scenario._database.0;
    let PoolAccounts {
        first,
        second,
        second_mock,
        second_recorded,
        pool_id,
    } = add_second_pool_account(
        &scenario,
        vec![responses_completed(
            json!([responses_message_item("native answer")]),
            4,
            2,
        )],
    )
    .await;
    let base = second_mock.base();
    let native_payload = NativeWindowPayload {
        retained_messages: Vec::new(),
        retained_state_items: Vec::new(),
        producer_instance_id: format!("provider:{}", second.id),
        upstream_model_id: MODEL.into(),
        mechanism: Mechanism::ResponsesStandaloneCompact,
        native_format: UpstreamFormat::OpenAiResponse,
        required_prefix_identity: None,
        window: NativeReplayWindow::ResponsesWindow {
            items: vec![json!({"type":"compaction", "encrypted_content":NATIVE_STATE})],
        },
    };
    let (_, carrier) = history::seal_native_window(
        native_payload.clone(),
        ReplacementMode::CanonicalWindow,
        None,
    )
    .expect("native carrier");
    let request = json!({
        "model": MODEL,
        "input": [
            {"type": "compaction", "encrypted_content": carrier},
            user_item("continue")
        ]
    });
    let (status, body) = post_openai(&scenario, "/api/openai-v1/responses", request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_dispatches(&scenario, 0);
    assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    // Contrary cached artifact evidence cannot override the authenticated producer.
    let parsed =
        <crate::format::openai::response::Format as crate::format::ClientFormat>::parse_request(
            request.to_string().as_bytes(),
            &crate::format::RouteContext::default(),
        )
        .expect("replay parse");
    let preparation = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
        parsed.compaction.as_ref().expect("state"),
    )
    .expect("authenticate")
    .routing_preparation(&parsed)
    .expect("active projection");
    let namespace = crate::affinity::Namespace {
        api_key_id: scenario.api_key_id,
        pool_id,
    };
    let lease = match scenario.affinity_cache.acquire(
        namespace,
        &crate::affinity::Evidence {
            proven_provider: Some(first.provider.id),
            ..crate::affinity::Evidence::default()
        },
    ) {
        crate::affinity::Decision::Bound(lease) => lease,
        _ => panic!("fixture provenance binds"),
    };
    lease.register(&[crate::affinity::Anchor::Artifact(
        preparation.artifact.expect("artifact"),
    )]);
    let (status, body) = post_openai(&scenario, "/api/openai-v1/responses", request.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "routing_state_conflict");
    assert_dispatches(&scenario, 0);
    assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    // Structural eligibility is checked only after all active account requirements agree.
    // Disabling the authenticated producer must not hide contrary live artifact evidence.
    for is_enabled in [false, true] {
        crate::storage::providers::update_provider(
            database,
            second.id,
            crate::storage::providers::ProviderUpdate {
                model_prefix: &second.model_prefix,
                name: "second",
                base_url: Some(&base),
                is_enabled,
                definition: None,
                credentials: None,
            },
        )
        .await
        .expect("change native producer eligibility");
        let (status, body) =
            post_openai(&scenario, "/api/openai-v1/responses", request.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{is_enabled}: {body}");
        assert_eq!(body["error"]["code"], "routing_state_conflict");
        assert_dispatches(&scenario, 0);
        assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    }
    scenario.affinity_cache.remove_pool(pool_id);
    for mismatch in ["producer", "model", "format"] {
        let mut payload = native_payload.clone();
        match mismatch {
            "producer" => payload.producer_instance_id = "provider:99999".into(),
            "model" => payload.upstream_model_id = "another-model".into(),
            _ => payload.native_format = UpstreamFormat::AnthropicMessages,
        }
        let (_, incompatible) =
            history::seal_native_window(payload, ReplacementMode::CanonicalWindow, None)
                .expect("incompatible carrier");
        let (status, body) = post_openai(
            &scenario,
            "/api/openai-v1/responses",
            json!({"model":MODEL,"input":[
                {"type":"compaction","encrypted_content":incompatible}, user_item("continue")
            ]}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{mismatch}: {body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("compaction_inaccessible_state")),
            "{mismatch}: {body}"
        );
    }
    let fence = crate::storage::quota_blocks::load_state(database, second.id)
        .await
        .expect("quota state")
        .expect("second account")
        .fence;
    crate::storage::quota_blocks::record_rejection(
        database,
        second.id,
        fence.generation,
        &["native account limit".into()],
        &crate::provider::quota::QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await
    .expect("quota block");
    let (status, body) = post_openai(&scenario, "/api/openai-v1/responses", request.clone()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_bound_quota_error(&body, false);
    assert_dispatches(&scenario, 0);
    assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    crate::storage::providers::update_provider(
        database,
        second.id,
        crate::storage::providers::ProviderUpdate {
            model_prefix: &second.model_prefix,
            name: "second",
            base_url: Some(&base),
            is_enabled: false,
            definition: None,
            credentials: None,
        },
    )
    .await
    .expect("disable native producer");
    let (status, body) = post_openai(&scenario, "/api/openai-v1/responses", request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "bound_account_unavailable", "{body}");
    assert_dispatches(&scenario, 0);
    assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    let cached_summary = seal_summary("cached portable checkpoint", &[], None, false);
    let cached_request = json!({"model":MODEL,"input":[
        {"type":"compaction","encrypted_content":carrier},
        {"type":"compaction","encrypted_content":cached_summary}, user_item("continue")
    ]});
    let parsed =
        <crate::format::openai::response::Format as crate::format::ClientFormat>::parse_request(
            cached_request.to_string().as_bytes(),
            &crate::format::RouteContext::default(),
        )
        .expect("portable replay parse");
    let preparation = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
        parsed.compaction.as_ref().expect("state"),
    )
    .expect("authenticate")
    .routing_preparation(&parsed)
    .expect("active portable projection");
    let lease = match scenario.affinity_cache.acquire(
        namespace,
        &crate::affinity::Evidence {
            proven_provider: Some(second.id),
            ..crate::affinity::Evidence::default()
        },
    ) {
        crate::affinity::Decision::Bound(lease) => lease,
        _ => panic!("fixture portable anchor binds"),
    };
    lease.register(&[crate::affinity::Anchor::Artifact(
        preparation.artifact.expect("portable artifact"),
    )]);
    let (status, body) = post_openai(&scenario, "/api/openai-v1/responses", cached_request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "bound_account_unavailable", "{body}");
    assert_dispatches(&scenario, 0);
    scenario.affinity_cache.remove_pool(pool_id);
    let summary = seal_summary("portable checkpoint", &[], None, false);
    let (status, body) = post_openai(
        &scenario,
        "/api/openai-v1/responses",
        json!({"model":MODEL,"input":[
            {"type":"compaction","encrypted_content":carrier},
            {"type":"compaction","encrypted_content":summary}, user_item("continue")
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_dispatches(&scenario, 1);
    assert_eq!(second_recorded.lock().expect("recording").len(), 1);
}

/// Authenticated preparation projects only the latest client-visible checkpoint and its
/// uncovered suffix. Outer resource IDs and covered native state cannot alter routing evidence.
///
/// # Panics
///
/// Panics if fixture parsing, authentication, or active-window identity violates the contract.
#[test]
fn routing_preparation_uses_latest_visible_artifact_and_uncovered_window() {
    use crate::format::{ClientFormat as _, RouteContext, openai::response::Format};
    let carrier = seal_summary("invisible expanded summary", &[], None, false);
    let older = seal_summary("superseded summary", &[], None, false);
    let suffix = vec![
        user_item("new question"),
        json!({
            "type":"reasoning","id":"rs_live","encrypted_content":"uncovered state","summary":[]
        }),
    ];
    let prepare = |input: Vec<Value>| {
        let parsed = Format::parse_request(
            json!({"model":MODEL,"input":input}).to_string().as_bytes(),
            &RouteContext::default(),
        )
        .expect("parsed client history");
        let boundary = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
            parsed.compaction.as_ref().expect("state facts"),
        )
        .expect("authenticated boundary");
        boundary
            .routing_preparation(&parsed)
            .expect("routing preparation")
    };
    let mut full = vec![
        user_item("covered question"),
        json!({
            "type":"compaction","id":"old-native","encrypted_content":"covered-native"
        }),
        json!({"type":"compaction","encrypted_content":older}),
        json!({
            "type":"compaction","id":"outer-one","encrypted_content":carrier
        }),
    ];
    full.extend(suffix.clone());
    let full = prepare(full);
    let mut omitted =
        vec![json!({"type":"compaction","id":"outer-two","encrypted_content":carrier})];
    omitted.extend(suffix.clone());
    let omitted = prepare(omitted);
    let retry = |preparation: &crate::compaction_model::boundary::RoutingPreparation| {
        preparation
            .projector
            .evidence()
            .retry
            .map(|retry| retry.digest)
    };
    assert!(retry(&full).is_some());
    assert!(retry(&full) == retry(&omitted));
    assert!(full.artifact.is_some());
    assert!(full.artifact == omitted.artifact);
    let mut changed = vec![json!({"type":"compaction","encrypted_content":carrier})];
    changed.extend([user_item("different uncovered turn"), suffix[1].clone()]);
    let changed = prepare(changed);
    assert!(changed.artifact == full.artifact);
    assert!(retry(&changed) != retry(&full));
    let raw = |content: &str| {
        prepare(vec![
            json!({"type":"compaction","encrypted_content":content}),
            user_item("new question"),
        ])
    };
    let first = raw("raw-one");
    let repeated = raw("raw-one");
    let other = raw("raw-two");
    assert!(first.artifact == repeated.artifact);
    assert!(first.artifact != other.artifact);
    assert!(retry(&first) == retry(&repeated));
    assert!(retry(&first) != retry(&other));
    let no_op = crate::format::anthropic::messages::Format::parse_request(
        json!({"model":MODEL,"max_tokens":16,"messages":[
            {"role":"assistant","content":[{"type":"compaction","content":null}]},
            {"role":"user","content":"new question"}
        ]})
        .to_string()
        .as_bytes(),
        &RouteContext::default().with_anthropic_betas(vec!["compact-2026-09-04".into()]),
    )
    .expect("native null block parses");
    let boundary = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
        no_op.compaction.as_ref().expect("no-op facts"),
    )
    .expect("native null boundary");
    let no_op_projection = boundary
        .routing_preparation(&no_op)
        .expect("no-op preparation");
    assert!(no_op_projection.artifact.is_none());
    assert!(
        retry(&no_op_projection)
            == no_op
                .affinity
                .project(&no_op.ir.messages)
                .evidence()
                .retry
                .map(|retry| retry.digest)
    );
}

/// Successful internal data-plane headers establish the account even when body parsing fails;
/// recovering an earlier account cannot move a retry of that compaction operation.
///
/// # Panics
///
/// Panics if internal headers fail to commit or a bound retry dispatches on another account.
#[tokio::test]
async fn internal_compaction_headers_commit_before_failed_body_parsing() {
    for operation in ["standalone", "summary", "on_demand", "counter"] {
        let (format, section) = match operation {
            "standalone" => (UpstreamFormat::OpenAiResponse, standalone_section(vec![])),
            "summary" => (UpstreamFormat::OpenAiResponse, summarize_only_section()),
            "counter" => (
                UpstreamFormat::AnthropicMessages,
                json!({"count_tokens":{
                    "url":[{"type":"base"},{"type":"literal","value":"/messages/count_tokens"}]
                }}),
            ),
            _ => (
                UpstreamFormat::AnthropicMessages,
                messages_mechanisms_section("supported", "unsupported"),
            ),
        };
        let scenario = scenario(
            format,
            Some(section),
            None,
            None,
            false,
            vec![MockAction::respond_json("not-json".into())],
        )
        .await;
        let actions = if operation == "counter" {
            vec![
                MockAction::respond_json("not-json".into()),
                raw_json_response(
                    "500 Internal Server Error",
                    json!({"error":"summary rejected"}),
                ),
            ]
        } else {
            vec![MockAction::respond_json("not-json".into())]
        };
        let PoolAccounts {
            first,
            second,
            second_mock: _second_mock,
            second_recorded,
            ..
        } = add_second_pool_account(&scenario, actions).await;
        let database = &scenario._database.0;
        crate::storage::quota_blocks::record_rejection(
            database,
            first.provider.id,
            0,
            &["earlier restriction".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("block earlier account");
        let request = if operation == "counter" {
            json!({"model":MODEL,"max_tokens":100,
            "messages":[{"role":"user","content":"w".repeat(150_001)},
                {"role":"user","content":"pending turn"}],
            "context_management":{"edits":[{"type":"compact_20260112",
                "trigger":{"type":"input_tokens","value":50_000},
                "pause_after_compaction":true
            }]}})
        } else if operation == "on_demand" {
            json!({"model":MODEL,"max_tokens":100,
                "messages":[{"role":"user","content":"older history"},
                    {"role":"user","content":"pending turn"}],
                "compaction":{"type":"summarize"}})
        } else {
            json!({"model":MODEL,"input":[user_item("older history"), user_item("pending turn")]})
        };
        let (status, body) = if matches!(operation, "on_demand" | "counter") {
            let beta = if operation == "counter" {
                "compact-2026-01-12"
            } else {
                "compact-2026-09-04"
            };
            post_messages(&scenario, Some(beta), request.clone()).await
        } else {
            post_openai(
                &scenario,
                "/api/openai-v1/responses/compact",
                request.clone(),
            )
            .await
        };
        let expected = if operation == "counter" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_GATEWAY
        };
        assert_eq!(status, expected, "{operation}: {body}");
        assert_dispatches(&scenario, 0);
        let expected_calls = if operation == "counter" { 2 } else { 1 };
        assert_eq!(
            second_recorded.lock().expect("recording").len(),
            expected_calls
        );
        crate::storage::quota_blocks::reset(database, first.provider.id)
            .await
            .expect("earlier account recovers");
        crate::storage::quota_blocks::record_rejection(
            database,
            second.id,
            0,
            &["later restriction".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("selected account exhausts");
        let (status, body) = if matches!(operation, "on_demand" | "counter") {
            let beta = if operation == "counter" {
                "compact-2026-01-12"
            } else {
                "compact-2026-09-04"
            };
            post_messages(&scenario, Some(beta), request).await
        } else {
            post_openai(&scenario, "/api/openai-v1/responses/compact", request).await
        };
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{operation}: {body}");
        assert_dispatches(&scenario, 0);
        assert_bound_quota_error(&body, matches!(operation, "on_demand" | "counter"));
        assert_eq!(
            second_recorded.lock().expect("recording").len(),
            expected_calls
        );
    }
}

/// Actual compact-only output establishes artifact and history bindings independently of input;
/// after cache loss, portable summaries and raw native artifacts become unbound again.
///
/// # Panics
///
/// Panics if discarded history, account recovery, or cache loss violates artifact ownership.
#[tokio::test]
async fn completed_compaction_artifacts_and_windows_bind_without_covered_history() {
    use crate::format::{ClientFormat as _, RouteContext, openai::response::Format};
    for case in [
        "summary",
        "trigger_summary",
        "trigger_summary_sse",
        "trigger_native",
        "trigger_native_sse",
        "native",
        "native_window",
        "native_null",
        "native_missing_id",
        "native_unreplayable",
    ] {
        let is_trigger = case.starts_with("trigger_");
        let is_native = !matches!(case, "summary" | "trigger_summary" | "trigger_summary_sse");
        let section = if case.starts_with("trigger_native") {
            json!({"mechanisms":{"responses_explicit_trigger":{"default":"supported"}}})
        } else if is_native {
            standalone_section(vec![])
        } else {
            summarize_only_section()
        };
        let scenario = scenario(
            UpstreamFormat::OpenAiResponse,
            Some(section),
            None,
            None,
            false,
            vec![responses_completed(
                json!([responses_message_item("new account answer")]),
                4,
                2,
            )],
        )
        .await;
        let action = if case.starts_with("trigger_native") {
            responses_completed(window_output(), 20, 5)
        } else if case == "native_window" {
            compact_response(
                json!([
                    {"type":"message","role":"system","content":[
                        {"type":"input_text","text":""},
                        {"type":"input_text","text":"retained instructions"}
                    ]},
                    {"type":"message","role":"user","content":[
                        {"type":"input_text","text":"look 👋"},
                        {"type":"input_image","image_url":"data:image/png;base64,AQ==",
                            "detail":"auto"},
                        {"type":"input_image","image_url":"https://example.invalid/image"}
                    ]},
                    {"type":"function_call","call_id":"retained-call","name":"read",
                        "arguments":"{\"z\":false,\"a\":[1,2]}","thought_signature":"signature"},
                    {"type":"function_call_output","call_id":"retained-call","output":[
                        {"type":"input_text","text":""},
                        {"type":"input_text","text":"first result"},
                        {"type":"input_text","text":""},
                        {"type":"input_text","text":"last result"}
                    ]},
                    {"type":"message","role":"assistant","content":[
                        {"type":"output_text","text":"kept ","annotations":[],"logprobs":null},
                        {"type":"output_text","text":"answer"}
                    ],"status":"completed","phase":"final_answer","id":"ignored-resource"},
                    {"type":"message","role":"assistant","content":" continued"},
                    {"type":"reasoning","id":null,"encrypted_content":"retained-reasoning",
                        "summary":[{"z":[null,false,1.5,"foreign text"],"a":{"b":"borrowed"}}],
                        "content":[{"type":"reasoning_text","text":"native reasoning"}],
                        "ignored_vendor_field":"ignored"},
                    {"type":"reasoning","encrypted_content":"default summary"},
                    {"type":"compaction_summary","encrypted_content":"superseded native state"},
                    {"type":"message","role":"assistant",
                        "content":"retained before latest boundary"},
                    {"type":"compaction","encrypted_content":NATIVE_STATE,
                        "id":null,"created_by":null},
                    {"type":"function_call","call_id":"new-call","name":"finish"},
                    {"type":"function_call_output","call_id":"new-call","output":"done"},
                    {"type":"message","role":"assistant","content":"after latest boundary"}
                ]),
                20,
                5,
            )
        } else if case == "native_unreplayable" {
            let mut output = window_output().as_array().expect("window fixture").clone();
            output.push(json!({"type":"function_call","call_id":"broken-call",
                "name":"read","arguments":"[1]"}));
            compact_response(json!(output), 20, 5)
        } else if case == "native_null" {
            compact_response(
                json!([{"type":"compaction","id":null,
                "encrypted_content":NATIVE_STATE,"created_by":null}]),
                20,
                5,
            )
        } else if case == "native_missing_id" {
            compact_response(
                json!([{"type":"compaction","encrypted_content":NATIVE_STATE}]),
                20,
                5,
            )
        } else if is_native {
            compact_response(window_output(), 20, 5)
        } else {
            responses_completed(json!([responses_message_item("portable summary")]), 20, 5)
        };
        let PoolAccounts {
            first,
            second,
            second_mock: _second_mock,
            second_recorded,
            pool_id,
        } = add_second_pool_account(&scenario, vec![action]).await;
        let database = &scenario._database.0;
        crate::storage::quota_blocks::record_rejection(
            database,
            first.provider.id,
            0,
            &["earlier restriction".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("block earlier account");
        let mut input = vec![
            user_item("covered old history"),
            user_item("covered pending turn"),
        ];
        if is_trigger {
            input.push(json!({"type":"compaction_trigger"}));
        }
        let endpoint = if is_trigger {
            "/api/openai-v1/responses"
        } else {
            "/api/openai-v1/responses/compact"
        };
        let request = json!({"model":MODEL,"input":input});
        let (status, compacted) = if case.ends_with("_sse") {
            let mut request = request;
            request["stream"] = json!(true);
            let (status, text) =
                post(&scenario, endpoint, &openai_headers(), request.to_string()).await;
            let terminal = sse_payloads(&text)
                .into_iter()
                .find(|event| event["type"] == "response.completed")
                .expect("completed trigger response");
            (status, terminal["response"].clone())
        } else {
            post_openai(&scenario, endpoint, request).await
        };
        assert_eq!(status, StatusCode::OK, "{compacted}");
        assert_dispatches(&scenario, 0);
        assert_eq!(second_recorded.lock().expect("recording").len(), 1);
        // Inspect actual returned history without an artifact hint or the request's covered
        // input masking missing registration. Sealed tokens use authenticated preparation;
        // raw native windows also have their complete ordinary codec projection.
        let mut returned = compacted["output"]
            .as_array()
            .expect("returned window")
            .clone();
        if case == "native_unreplayable" {
            returned.pop();
        }
        let replay = Format::parse_request(
            json!({"model":MODEL,"input":returned})
                .to_string()
                .as_bytes(),
            &RouteContext::default(),
        )
        .expect("returned window replays");
        let mut histories = Vec::new();
        if is_native {
            let mut projector = crate::affinity::fingerprint::Projector::new();
            replay.affinity.append_window(
                &replay.ir.messages,
                replay.ir.state_items.iter(),
                0,
                &mut projector,
            );
            histories.push(("complete", projector.evidence().history));
        }
        let active = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
            replay.compaction.as_ref().expect("returned boundary"),
        )
        .expect("authenticate returned boundary")
        .routing_preparation(&replay)
        .expect("actual routing projection");
        histories.push(("active", active.projector.evidence().history));
        for (projection, history) in histories {
            let decision = scenario.affinity_cache.acquire(
                crate::affinity::Namespace {
                    api_key_id: scenario.api_key_id,
                    pool_id,
                },
                &crate::affinity::Evidence {
                    history,
                    ..crate::affinity::Evidence::default()
                },
            );
            if case == "native_unreplayable" {
                assert!(
                    matches!(decision, crate::affinity::Decision::Fresh(_)),
                    "{case}: {projection} history must not bind a partial window"
                );
            } else {
                let crate::affinity::Decision::Bound(binding) = decision else {
                    panic!("{case}: {projection} history must bind without artifact lookup");
                };
                assert_eq!(binding.provider_id(), second.id);
            }
        }
        crate::storage::quota_blocks::reset(database, first.provider.id)
            .await
            .expect("earlier account recovers");
        crate::storage::quota_blocks::record_rejection(
            database,
            second.id,
            0,
            &["selected restriction".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("selected account exhausts");
        let mut input = compacted["output"]
            .as_array()
            .expect("canonical window")
            .clone();
        if case == "native_unreplayable" {
            input.pop();
        }
        input.push(user_item("new uncovered question"));
        let request = json!({"model":MODEL,"input":input});
        let (status, body) =
            post_openai(&scenario, "/api/openai-v1/responses", request.clone()).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{case}: {body}");
        assert_dispatches(&scenario, 0);
        assert_eq!(second_recorded.lock().expect("recording").len(), 1);
        scenario.affinity_cache.remove_pool(pool_id);
        let (status, body) = post_openai(&scenario, "/api/openai-v1/responses", request).await;
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        assert_dispatches(&scenario, 1);
        assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    }
}

/// Actual Messages compact-only JSON/SSE blocks establish their account without covered input;
/// normalized native fields and sealed gateway identity follow the same replay preparation.
///
/// # Panics
///
/// Panics if a returned block fails to replay or loses its actual producer while hints are live.
#[tokio::test]
async fn messages_compact_only_blocks_bind_without_covered_history() {
    for is_native in [false, true] {
        for is_streaming in [false, true] {
            let format = if is_native {
                UpstreamFormat::AnthropicMessages
            } else {
                UpstreamFormat::OpenAiResponse
            };
            let section = if is_native {
                messages_mechanisms_section("supported", "unsupported")
            } else {
                summarize_only_section()
            };
            let action = if is_native {
                MockAction::respond_json(
                    json!({"id":"native-message","type":"message",
                        "role":"assistant","model":MODEL,"stop_reason":"compaction",
                        "content":[{"type":"compaction","content":"native summary",
                            "signature":"native signature","encrypted_content":null,
                            "tool_changes":[{"z":2,"a":"tool"}]}],
                        "usage":{"input_tokens":20,"output_tokens":5}
                    })
                    .to_string(),
                )
            } else {
                responses_completed(json!([responses_message_item("portable summary")]), 20, 5)
            };
            let scenario = scenario(
                format,
                Some(section),
                None,
                None,
                false,
                vec![responses_completed(
                    json!([responses_message_item("new account answer")]),
                    4,
                    2,
                )],
            )
            .await;
            let PoolAccounts {
                first,
                second,
                second_mock: _second_mock,
                second_recorded,
                pool_id,
            } = add_second_pool_account(&scenario, vec![action]).await;
            let database = &scenario._database.0;
            crate::storage::quota_blocks::record_rejection(
                database,
                first.provider.id,
                0,
                &["earlier restriction".into()],
                &crate::provider::quota::QuotaApplicability::AllModels,
                crate::unix_time::unix_now_micros(),
            )
            .await
            .expect("block earlier account");
            let request = json!({"model":MODEL,"max_tokens":100,"stream":is_streaming,
                "messages":[{"role":"user","content":"covered old input"}],
                "compaction":{"type":"summarize"}});
            let (status, block) = if is_streaming {
                let (status, text) = post(
                    &scenario,
                    "/api/anthropic-v1/messages",
                    &messages_headers(Some("compact-2026-09-04")),
                    request.to_string(),
                )
                .await;
                let start = sse_payloads(&text)
                    .into_iter()
                    .find(|event| event["type"] == "content_block_start")
                    .expect("complete block");
                (status, start["content_block"].clone())
            } else {
                let (status, body) =
                    post_messages(&scenario, Some("compact-2026-09-04"), request).await;
                (status, body["content"][0].clone())
            };
            assert_eq!(status, StatusCode::OK, "{block}");
            let request = json!({"model":MODEL,"max_tokens":100,"messages":[
                {"role":"assistant","content":[block]}, {"role":"user","content":"new question"}
            ]});
            use crate::format::{ClientFormat as _, RouteContext, anthropic::messages::Format};
            let route =
                RouteContext::default().with_anthropic_betas(vec!["compact-2026-09-04".into()]);
            let parsed = Format::parse_request(request.to_string().as_bytes(), &route)
                .expect("block replay");
            let active = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
                parsed.compaction.as_ref().expect("boundary"),
            )
            .expect("authenticate")
            .routing_preparation(&parsed)
            .expect("active projection");
            for evidence in [
                crate::affinity::Evidence {
                    history: active.projector.evidence().history,
                    ..crate::affinity::Evidence::default()
                },
                crate::affinity::Evidence {
                    artifact: active.artifact,
                    ..crate::affinity::Evidence::default()
                },
            ] {
                let crate::affinity::Decision::Bound(binding) = scenario.affinity_cache.acquire(
                    crate::affinity::Namespace {
                        api_key_id: scenario.api_key_id,
                        pool_id,
                    },
                    &evidence,
                ) else {
                    panic!(
                        "actual Messages block must bind: native={is_native}, \
                         stream={is_streaming}, has_artifact={}",
                        evidence.artifact.is_some()
                    );
                };
                assert_eq!(binding.provider_id(), second.id);
            }
            crate::storage::quota_blocks::reset(database, first.provider.id)
                .await
                .expect("first recovers");
            crate::storage::quota_blocks::record_rejection(
                database,
                second.id,
                0,
                &["selected restriction".into()],
                &crate::provider::quota::QuotaApplicability::AllModels,
                crate::unix_time::unix_now_micros(),
            )
            .await
            .expect("second exhausts");
            let (status, body) =
                post_messages(&scenario, Some("compact-2026-09-04"), request).await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
            assert_dispatches(&scenario, 0);
            assert_eq!(second_recorded.lock().expect("recording").len(), 1);
        }
    }
}

/// Continued threshold answers establish the returned checkpoint window on the selected account,
/// independently of covered history and the invisible summary request.
///
/// # Panics
///
/// Panics if actual JSON/SSE output cannot be replayed or loses its account binding.
#[tokio::test]
async fn checkpoint_continuations_bind_actual_returned_windows() {
    use crate::format::{ClientFormat as _, RouteContext};
    for is_native in [false, true] {
        for is_messages in [false, true] {
            for is_streaming in [false, true] {
                let completed = json!({"id":"resp_answer","status":"completed",
                "output":[responses_message_item("answer λ")],
                "usage":{"input_tokens":8,"output_tokens":3}});
                let answer = if is_streaming {
                    let stream = format!(
                        "event: response.completed\ndata: {}\n\n",
                        json!({"type":"response.completed","response":completed})
                    );
                    MockAction::Respond(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{stream}",
                        stream.len()
                    ))
                } else {
                    MockAction::respond_json(completed.to_string())
                };
                let scenario = scenario(
                    UpstreamFormat::OpenAiResponse,
                    Some(if is_native {
                        standalone_section(vec![])
                    } else {
                        summarize_only_section()
                    }),
                    None,
                    None,
                    false,
                    vec![answer.clone()],
                )
                .await;
                let PoolAccounts {
                    first,
                    second,
                    second_mock: _second_mock,
                    second_recorded,
                    pool_id,
                } = add_second_pool_account(
                    &scenario,
                    vec![
                        if is_native {
                            compact_response(window_output(), 20, 5)
                        } else {
                            responses_completed(
                                json!([responses_message_item("invisible summary")]),
                                20,
                                5,
                            )
                        },
                        answer,
                    ],
                )
                .await;
                let database = &scenario._database.0;
                crate::storage::quota_blocks::record_rejection(
                    database,
                    first.provider.id,
                    0,
                    &["earlier restriction".into()],
                    &crate::provider::quota::QuotaApplicability::AllModels,
                    crate::unix_time::unix_now_micros(),
                )
                .await
                .expect("block earlier account");
                let request = if is_messages {
                    json!({"model":MODEL,"max_tokens":100,"stream":is_streaming,
                    "messages":[{"role":"user","content":"old ".repeat(150_001)},
                        {"role":"user","content":"pending question"}],
                    "context_management":{"edits":[{"type":"compact_20260112",
                        "trigger":{"type":"input_tokens","value":50_000},
                        "pause_after_compaction":false}]}})
                } else {
                    json!({"model":MODEL,"stream":is_streaming,
                    "input":[user_item("old history"),user_item("pending question")],
                    "context_management":[{"type":"compaction","compact_threshold":1}]})
                };
                let path = if is_messages {
                    "/api/anthropic-v1/messages"
                } else {
                    "/api/openai-v1/responses"
                };
                let headers = if is_messages {
                    messages_headers(Some("compact-2026-01-12"))
                } else {
                    openai_headers()
                };
                let (status, wire) = post(&scenario, path, &headers, request.to_string()).await;
                assert_eq!(status, StatusCode::OK, "{wire}");
                let returned = if is_streaming {
                    let frames = sse_payloads(&wire);
                    if is_messages {
                        let mut block = frames
                            .iter()
                            .find(|event| {
                                event["type"] == "content_block_start"
                                    && event["content_block"]["type"] == "compaction"
                            })
                            .expect("checkpoint start")["content_block"]
                            .clone();
                        let summary: String = frames
                            .iter()
                            .filter_map(|event| {
                                (event["delta"]["type"] == "compaction_delta")
                                    .then(|| event["delta"]["content"].as_str())
                                    .flatten()
                            })
                            .collect();
                        if !summary.is_empty() {
                            block["content"] = json!(summary);
                        }
                        let text: String = frames
                            .iter()
                            .filter_map(|event| event["delta"]["text"].as_str())
                            .collect();
                        assert_eq!(text, "answer λ");
                        json!([block,{"type":"text","text":text}])
                    } else {
                        frames
                            .iter()
                            .find(|event| event["type"] == "response.completed")
                            .expect("terminal actual output")["response"]["output"]
                            .clone()
                    }
                } else {
                    let body: Value = serde_json::from_str(&wire).expect("actual body");
                    body[if is_messages { "content" } else { "output" }].clone()
                };
                let replay = if is_messages {
                    json!({"model":MODEL,"max_tokens":100,"messages":[
                    {"role":"assistant","content":returned},
                    {"role":"user","content":"new question"}]})
                } else {
                    let mut items = returned.as_array().expect("returned window").clone();
                    items.push(user_item("new question"));
                    json!({"model":MODEL,"input":items})
                };
                let route = RouteContext::default().with_anthropic_betas(vec![
                    "compact-2026-01-12".into(),
                    "compact-2026-09-04".into(),
                ]);
                let parsed = if is_messages {
                    crate::format::anthropic::messages::Format::parse_request(
                        replay.to_string().as_bytes(),
                        &route,
                    )
                } else {
                    crate::format::openai::response::Format::parse_request(
                        replay.to_string().as_bytes(),
                        &route,
                    )
                }
                .expect("actual output replay");
                let active = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
                    parsed.compaction.as_ref().expect("checkpoint"),
                )
                .expect("authenticate")
                .routing_preparation(&parsed)
                .expect("active window");
                for evidence in [
                    crate::affinity::Evidence {
                        history: active.projector.evidence().history,
                        ..crate::affinity::Evidence::default()
                    },
                    crate::affinity::Evidence {
                        artifact: active.artifact,
                        ..crate::affinity::Evidence::default()
                    },
                ] {
                    let crate::affinity::Decision::Bound(binding) =
                        scenario.affinity_cache.acquire(
                            crate::affinity::Namespace {
                                api_key_id: scenario.api_key_id,
                                pool_id,
                            },
                            &evidence,
                        )
                    else {
                        panic!("checkpoint continuation must bind its actual output");
                    };
                    assert_eq!(binding.provider_id(), second.id);
                }
                crate::storage::quota_blocks::reset(database, first.provider.id)
                    .await
                    .expect("earlier account recovers");
                crate::storage::quota_blocks::record_rejection(
                    database,
                    second.id,
                    0,
                    &["bound restriction".into()],
                    &crate::provider::quota::QuotaApplicability::AllModels,
                    crate::unix_time::unix_now_micros(),
                )
                .await
                .expect("bound account exhausted");
                let replay_headers = if is_messages {
                    messages_headers(Some("compact-2026-01-12,compact-2026-09-04"))
                } else {
                    openai_headers()
                };
                let (status, error) =
                    post(&scenario, path, &replay_headers, replay.to_string()).await;
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
                if is_messages {
                    assert!(error.contains("rate_limit_error"), "{error}");
                    assert!(error.contains("required account"), "{error}");
                } else {
                    assert!(error.contains("bound_account_quota_exhausted"), "{error}");
                }
                assert_eq!(second_recorded.lock().expect("recording").len(), 2);
                assert_dispatches(&scenario, 0);
            }
        }
    }
}

/// Native no-summary output preserves the original history while establishing only its actual
/// replayable answer, including normalized signed/redacted blocks and tool relationships.
///
/// # Panics
///
/// Panics if no-op state creates an artifact or the completed answer loses its account binding.
#[tokio::test]
async fn messages_native_no_summary_binds_only_actual_answer_history() {
    use crate::format::{ClientFormat as _, RouteContext, anthropic::messages::Format};
    for has_no_op in [false, true] {
        for is_streaming in [false, true] {
            let mut blocks = Vec::new();
            if has_no_op {
                blocks.push(json!({"type":"compaction","content":null,
                    "signature":"native no-op","encrypted_content":null,"tool_changes":null}));
            }
            blocks.extend([
                json!({"type":"thinking","thinking":"reason λ","signature":"signed",
                }),
                json!({"type":"redacted_thinking",
                        "data":"{\"z\":[2,{\"z\":false,\"a\":null}],\"a\":\"encrypted\"}"}),
            ]);
            blocks.extend([
                json!({"type":"text","text":"answer λ"}),
                json!({"type":"tool_use","id":"call-native","name":"lookup",
                    "input":{"z":[2,{"z":false,"a":null}],"a":"query"}}),
            ]);
            let scenario = scenario(
                UpstreamFormat::AnthropicMessages,
                Some(messages_mechanisms_section("supported", "unsupported")),
                None,
                None,
                false,
                vec![anthropic_message(json!([]), "end_turn", 1, 1)],
            )
            .await;
            let PoolAccounts {
                first,
                second,
                second_mock: _second_mock,
                second_recorded,
                pool_id,
            } = add_second_pool_account(
                &scenario,
                vec![anthropic_message(json!(blocks), "tool_use", 8, 2)],
            )
            .await;
            let database = &scenario._database.0;
            crate::storage::quota_blocks::record_rejection(
                database,
                first.provider.id,
                0,
                &["earlier restriction".into()],
                &crate::provider::quota::QuotaApplicability::AllModels,
                crate::unix_time::unix_now_micros(),
            )
            .await
            .expect("earlier account blocked");
            let request = json!({"model":MODEL,"max_tokens":100,"stream":is_streaming,
                "messages":[{"role":"user","content":"original question"}],
                "compaction":{"type":"summarize"}});
            let (status, wire) = post(
                &scenario,
                "/api/anthropic-v1/messages",
                &messages_headers(Some("compact-2026-09-04")),
                request.to_string(),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{wire}");
            let returned = if is_streaming {
                let frames = sse_payloads(&wire);
                let mut blocks: Vec<Value> = Vec::new();
                for event in &frames {
                    if event["type"] == "content_block_start" {
                        blocks.push(event["content_block"].clone());
                    } else if event["type"] == "content_block_delta" {
                        let block = blocks.last_mut().expect("opened actual block");
                        if let Some(text) = event["delta"]["text"].as_str() {
                            let previous = block["text"].as_str().expect("text start");
                            block["text"] = json!(format!("{previous}{text}"));
                        } else if let Some(text) = event["delta"]["thinking"].as_str() {
                            let previous = block["thinking"].as_str().expect("thinking start");
                            block["thinking"] = json!(format!("{previous}{text}"));
                        } else if let Some(signature) = event["delta"]["signature"].as_str() {
                            let previous = block["signature"].as_str().unwrap_or_default();
                            block["signature"] = json!(format!("{previous}{signature}"));
                        } else if let Some(arguments) = event["delta"]["partial_json"].as_str() {
                            block["input"] =
                                serde_json::from_str(arguments).expect("complete arguments");
                        }
                    }
                }
                json!(blocks)
            } else {
                let body: Value = serde_json::from_str(&wire).expect("actual JSON");
                body["content"].clone()
            };
            let thinking = returned
                .as_array()
                .expect("actual blocks")
                .iter()
                .find(|block| block["type"] == "thinking")
                .expect("actual signed reasoning");
            assert_eq!(thinking["thinking"], "reason λ");
            assert_eq!(thinking["signature"], "signed");
            let replay = json!({"model":MODEL,"max_tokens":100,"messages":[
                {"role":"user","content":"original question"},
                {"role":"assistant","content":returned},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-native",
                    "content":"result"},{"type":"text","text":"new question"}]}]});
            let route = RouteContext::default().with_anthropic_betas(vec![
                "compact-2026-09-04".into(),
                "compact-2026-01-12".into(),
            ]);
            let parsed = Format::parse_request(replay.to_string().as_bytes(), &route)
                .expect("actual output replays");
            let preparation = parsed
                .compaction
                .as_ref()
                .map_or_else(
                    crate::compaction_model::boundary::BoundaryPreparation::default,
                    |compaction| {
                        crate::compaction_model::boundary::BoundaryPreparation::authenticate(
                            compaction,
                        )
                        .expect("no-op")
                    },
                )
                .routing_preparation(&parsed)
                .expect("original active history");
            assert!(preparation.artifact.is_none(), "a no-op is not an artifact");
            let namespace = crate::affinity::Namespace {
                api_key_id: scenario.api_key_id,
                pool_id,
            };
            let evidence = crate::affinity::Evidence {
                history: preparation.projector.evidence().history,
                ..crate::affinity::Evidence::default()
            };
            let crate::affinity::Decision::Bound(binding) =
                scenario.affinity_cache.acquire(namespace, &evidence)
            else {
                panic!("actual native no-summary answer must bind");
            };
            assert_eq!(binding.provider_id(), second.id);
            if let Some(state) = parsed.ir.state_items.first() {
                let evidence = crate::affinity::Evidence {
                    artifact: Some(preparation.projector.native_artifact(&state.item)),
                    ..crate::affinity::Evidence::default()
                };
                assert!(
                    matches!(
                        scenario.affinity_cache.acquire(namespace, &evidence),
                        crate::affinity::Decision::Fresh(_)
                    ),
                    "no-op cannot create an artifact hint"
                );
            }
            crate::storage::quota_blocks::reset(database, first.provider.id)
                .await
                .expect("earlier account recovers");
            crate::storage::quota_blocks::record_rejection(
                database,
                second.id,
                0,
                &["bound restriction".into()],
                &crate::provider::quota::QuotaApplicability::AllModels,
                crate::unix_time::unix_now_micros(),
            )
            .await
            .expect("bound account exhausted");
            let (status, error) = post_messages(
                &scenario,
                Some("compact-2026-09-04,compact-2026-01-12"),
                replay,
            )
            .await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
            assert_bound_quota_error(&error, true);
            assert_dispatches(&scenario, 0);
            assert_eq!(second_recorded.lock().expect("recording").len(), 1);
        }
    }
}

/// Native threshold output binds only fully completed outward checkpoints; a later failure
/// retains their anchors while an unfinished native block establishes none.
///
/// # Panics
///
/// Panics if delivery registers incomplete state or loses a completed native checkpoint.
#[tokio::test]
async fn messages_native_threshold_registration_follows_completed_blocks() {
    use crate::format::{ClientFormat as _, RouteContext, anthropic::messages::Format};
    let block = json!({"type":"compaction","content":"native summary λ","signature":"native sig",
        "encrypted_content":null,"tool_changes":[{"z":[{"z":2,"a":true}],"a":"tools"}]});
    for delivery in [
        "json",
        "stream",
        "stream_thinking",
        "stream_after_text",
        "json_pause",
        "stream_pause",
        "later_failure",
        "partial",
    ] {
        let has_thinking = delivery == "stream_thinking";
        let is_streaming = !delivery.starts_with("json");
        let is_paused = delivery.ends_with("pause");
        let has_failure = matches!(delivery, "later_failure" | "partial");
        let is_partial = delivery == "partial";
        let is_after_text = delivery == "stream_after_text";
        let compaction_index = if is_after_text { 1 } else { 0 };
        let answer_index = compaction_index + 1;
        let action = if is_streaming {
            let mut stream = String::new();
            for (name, payload) in [
                (
                    "message_start",
                    json!({"type":"message_start","message":{
                    "id":"msg_native","model":MODEL,
                    "usage":{"input_tokens":20,"output_tokens":0}}}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":compaction_index,
                    "content_block":{"type":"compaction","content":null,"signature":"native sig",
                        "encrypted_content":null,"tool_changes":block["tool_changes"]}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":compaction_index,
                    "delta":{"type":"compaction_delta","content":"native summary λ"}}),
                ),
            ] {
                stream.push_str(&format!("event: {name}\ndata: {payload}\n\n"));
                if name == "message_start" && is_after_text {
                    for (name, payload) in [
                        (
                            "content_block_start",
                            json!({"type":"content_block_start","index":0,
                            "content_block":{"type":"text","text":""}}),
                        ),
                        (
                            "content_block_delta",
                            json!({"type":"content_block_delta","index":0,
                            "delta":{"type":"text_delta","text":"covered answer"}}),
                        ),
                        (
                            "content_block_stop",
                            json!({"type":"content_block_stop","index":0}),
                        ),
                    ] {
                        stream.push_str(&format!("event: {name}\ndata: {payload}\n\n"));
                    }
                }
            }
            if !is_partial {
                stream.push_str(&format!(
                    "event: content_block_stop\ndata: {}\n\n",
                    json!({"type":"content_block_stop","index":compaction_index})
                ));
                if !is_paused {
                    if has_thinking {
                        for (name, payload) in [
                            (
                                "content_block_start",
                                json!({"type":"content_block_start",
                                "index":answer_index,"content_block":{"type":"thinking",
                                    "thinking":""}}),
                            ),
                            (
                                "content_block_delta",
                                json!({"type":"content_block_delta",
                                "index":answer_index,"delta":{"type":"thinking_delta",
                                    "thinking":"plan "}}),
                            ),
                            (
                                "content_block_delta",
                                json!({"type":"content_block_delta",
                                "index":answer_index,"delta":{"type":"thinking_delta",
                                    "thinking":"λ"}}),
                            ),
                            (
                                "content_block_delta",
                                json!({"type":"content_block_delta",
                                "index":answer_index,"delta":{"type":"signature_delta",
                                    "signature":"signed"}}),
                            ),
                            (
                                "content_block_stop",
                                json!({"type":"content_block_stop",
                                "index":answer_index}),
                            ),
                        ] {
                            stream.push_str(&format!("event: {name}\ndata: {payload}\n\n"));
                        }
                    }
                    let text_index = answer_index + i32::from(has_thinking);
                    for (name, payload) in [
                        (
                            "content_block_start",
                            json!({"type":"content_block_start","index":text_index,
                            "content_block":{"type":"text","text":""}}),
                        ),
                        (
                            "content_block_delta",
                            json!({"type":"content_block_delta","index":text_index,
                            "delta":{"type":"text_delta","text":"answer λ"}}),
                        ),
                        (
                            "content_block_stop",
                            json!({"type":"content_block_stop","index":text_index}),
                        ),
                    ] {
                        stream.push_str(&format!("event: {name}\ndata: {payload}\n\n"));
                    }
                }
                if has_failure {
                    stream.push_str(&format!(
                        "event: error\ndata: {}\n\n",
                        json!({"type":"error","error":{"message":"later failure"}})
                    ));
                } else {
                    let payload = json!({"type":"message_delta","delta":{"stop_reason":
                        if is_paused {"compaction"} else {"end_turn"}},
                        "usage":{"input_tokens":20,"output_tokens":5}});
                    stream.push_str(&format!("event: message_delta\ndata: {payload}\n\n"));
                    stream.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
                }
            }
            MockAction::Respond(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                Content-Length: {}\r\nConnection: close\r\n\r\n{stream}",
                stream.len()
            ))
        } else {
            let mut blocks = vec![block.clone()];
            if !is_paused {
                blocks.push(json!({"type":"text","text":"answer λ"}));
            }
            anthropic_message(
                json!(blocks),
                if is_paused { "compaction" } else { "end_turn" },
                20,
                5,
            )
        };
        let scenario = scenario(
            UpstreamFormat::AnthropicMessages,
            Some(messages_mechanisms_section("supported", "supported")),
            None,
            None,
            false,
            vec![anthropic_message(json!([]), "end_turn", 1, 1)],
        )
        .await;
        let PoolAccounts {
            first,
            second,
            second_mock: _second_mock,
            second_recorded,
            pool_id,
        } = add_second_pool_account(&scenario, vec![action]).await;
        crate::storage::quota_blocks::record_rejection(
            &scenario._database.0,
            first.provider.id,
            0,
            &["earlier restriction".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("first blocked");
        let request = json!({"model":MODEL,"max_tokens":100,"stream":is_streaming,
            "messages":[{"role":"user","content":"covered question"}],
            "context_management":{"edits":[{"type":"compact_20260112",
                "pause_after_compaction":is_paused}]}});
        let (status, wire) = post(
            &scenario,
            "/api/anthropic-v1/messages",
            &messages_headers(Some("compact-2026-01-12")),
            request.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{wire}");
        let (returned, text) = if is_streaming {
            let frames = sse_payloads(&wire);
            let actual = frames.iter().find(|event| {
                event["type"] == "content_block_start"
                    && event["content_block"]["type"] == "compaction"
            });
            if is_partial {
                assert!(actual.is_none(), "incomplete block must not escape: {wire}");
            }
            let mut returned =
                actual.map_or_else(|| block.clone(), |event| event["content_block"].clone());
            let content: String = frames
                .iter()
                .filter_map(|event| event["delta"]["content"].as_str())
                .collect();
            if !content.is_empty() {
                returned["content"] = json!(content);
            }
            let state_index = actual
                .and_then(|event| event["index"].as_u64())
                .unwrap_or(0);
            let text: String = frames
                .iter()
                .filter(|event| {
                    event["index"]
                        .as_u64()
                        .is_some_and(|index| index > state_index)
                })
                .filter_map(|event| event["delta"]["text"].as_str())
                .collect();
            if is_after_text {
                assert_eq!(text, "answer λ", "post-state text needs its own block");
            }
            (returned, text)
        } else {
            let body: Value = serde_json::from_str(&wire).expect("JSON body");
            (
                body["content"][0].clone(),
                body["content"][1]["text"].as_str().unwrap_or("").to_owned(),
            )
        };
        let mut content = vec![returned];
        if has_thinking {
            let frames = sse_payloads(&wire);
            let start = frames
                .iter()
                .find(|event| {
                    event["type"] == "content_block_start"
                        && event["content_block"]["type"] == "thinking"
                })
                .expect("actual thinking start");
            let mut thinking = start["content_block"].clone();
            let index = start["index"].as_u64().expect("thinking index");
            let deltas: Vec<_> = frames
                .iter()
                .filter(|event| event["index"].as_u64() == Some(index))
                .collect();
            thinking["thinking"] = json!(
                deltas
                    .iter()
                    .filter_map(|event| { event["delta"]["thinking"].as_str() })
                    .collect::<String>()
            );
            thinking["signature"] = json!(
                deltas
                    .iter()
                    .filter_map(|event| { event["delta"]["signature"].as_str() })
                    .collect::<String>()
            );
            content.push(thinking);
        }
        if !text.is_empty() {
            content.push(json!({"type":"text","text":text}));
        }
        let replay = json!({"model":MODEL,"max_tokens":100,"messages":[
            {"role":"assistant","content":content},{"role":"user","content":"next question"}]});
        let route = RouteContext::default().with_anthropic_betas(vec!["compact-2026-09-04".into()]);
        let parsed = Format::parse_request(replay.to_string().as_bytes(), &route)
            .expect("actual native replay");
        let active = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
            parsed.compaction.as_ref().expect("native boundary"),
        )
        .expect("native preparation")
        .routing_preparation(&parsed)
        .expect("active output window");
        let namespace = crate::affinity::Namespace {
            api_key_id: scenario.api_key_id,
            pool_id,
        };
        let evidence = crate::affinity::Evidence {
            artifact: active.artifact,
            ..crate::affinity::Evidence::default()
        };
        match scenario.affinity_cache.acquire(namespace, &evidence) {
            crate::affinity::Decision::Bound(binding) => {
                assert!(!is_partial, "incomplete native state has no artifact hint");
                assert_eq!(binding.provider_id(), second.id);
            }
            crate::affinity::Decision::Fresh(_) => {
                assert!(is_partial, "complete checkpoint must bind: {delivery}")
            }
            _ => panic!("unexpected native artifact binding"),
        }
        let completed_units = if text.is_empty() { 1 } else { 2 };
        let evidence = crate::affinity::Evidence {
            history: active
                .projector
                .evidence()
                .history
                .into_iter()
                .filter(|prefix| prefix.units.get() == completed_units)
                .collect(),
            ..crate::affinity::Evidence::default()
        };
        match scenario.affinity_cache.acquire(namespace, &evidence) {
            crate::affinity::Decision::Bound(binding) => {
                assert!(
                    !has_failure,
                    "unfinished answer cannot establish a full window"
                );
                assert_eq!(binding.provider_id(), second.id);
            }
            crate::affinity::Decision::Fresh(_) => {
                assert!(has_failure, "complete window must bind: {delivery}")
            }
            _ => panic!("unexpected native full-window binding"),
        }
        assert_dispatches(&scenario, 0);
        assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    }
}

/// Native automatic Responses checkpoints establish actual active output windows and survive
/// later failure, while added-only state never becomes an artifact or completed history.
///
/// # Panics
///
/// Panics if output ordering, artifact completion, or selected-account affinity changes.
#[tokio::test]
async fn responses_native_automatic_registration_follows_actual_output() {
    use crate::format::{ClientFormat as _, RouteContext, openai::response::Format};
    for delivery in [
        "json",
        "stream",
        "stream_after_text",
        "stream_covered_call",
        "json_pause",
        "stream_pause",
        "later_failure",
        "partial",
    ] {
        let is_streaming = !delivery.starts_with("json");
        let is_paused = delivery.ends_with("pause");
        let is_after_text = delivery == "stream_after_text";
        let is_partial = delivery == "partial";
        let is_covered_call = delivery == "stream_covered_call";
        let has_failure = matches!(delivery, "later_failure" | "partial");
        let block = json!({"type":"compaction","id":"cmp_live","encrypted_content":"native λ",
            "created_by":null});
        let compaction_index = if is_after_text || is_covered_call {
            1
        } else {
            0
        };
        let answer_index = compaction_index + 1;
        let old_call = json!({"type":"function_call","id":"fc_before","call_id":"call_before",
            "name":"old_lookup","arguments":"{\"covered\":true}","status":"completed"});
        let new_call = json!({"type":"function_call","id":"fc_after","call_id":"call_after",
            "name":"lookup","arguments":"{\"z\":[2,{\"z\":false,\"a\":null}],\"a\":\"query\"}",
            "status":"completed"});
        let mut output = Vec::new();
        if is_after_text {
            let mut message = responses_message_item("covered answer");
            message["id"] = json!("msg_before");
            output.push(message);
        }
        if is_covered_call {
            output.push(old_call.clone());
        }
        output.push(block.clone());
        if is_covered_call {
            output.push(new_call.clone());
        } else if !is_paused {
            let mut message = responses_message_item("answer λ");
            message["id"] = json!("msg_after");
            output.push(message);
        }
        let completed = json!({"id":"resp_native","status":"completed","output":output,
            "usage":{"input_tokens":20,"output_tokens":5}});
        let action = if is_streaming {
            let mut events = vec![(
                "response.created",
                json!({"type":"response.created",
                "response":{"id":"resp_native","status":"in_progress","output":[]}}),
            )];
            if is_after_text {
                events.push((
                    "response.output_text.delta",
                    json!({"type":"response.output_text.delta",
                    "item_id":"msg_before","output_index":0,
                    "content_index":0,"delta":"covered answer"}),
                ));
            }
            if is_covered_call {
                events.push((
                    "response.output_item.added",
                    json!({"type":"response.output_item.added",
                    "output_index":0,"item":old_call}),
                ));
            }
            events.push((
                "response.output_item.added",
                json!({"type":"response.output_item.added",
                "output_index":compaction_index,"item":block}),
            ));
            if !is_partial {
                events.push((
                    "response.output_item.done",
                    json!({"type":"response.output_item.done",
                    "output_index":compaction_index,"item":block}),
                ));
                if is_covered_call {
                    for (name, index, item) in [
                        ("response.output_item.added", answer_index, &new_call),
                        ("response.output_item.done", answer_index, &new_call),
                        // The old position completes last, after the checkpoint and the new call.
                        ("response.output_item.done", 0, &old_call),
                    ] {
                        events.push((name, json!({"type":name,"output_index":index,"item":item})));
                    }
                } else if !is_paused {
                    events.push((
                        "response.output_text.delta",
                        json!({
                        "type":"response.output_text.delta", "item_id":"msg_after",
                        "output_index":answer_index,"content_index":0,"delta":"answer λ"}),
                    ));
                }
                if has_failure {
                    events.push((
                        "response.failed",
                        json!({"type":"response.failed",
                        "response":{"id":"resp_native","status":"failed",
                            "error":{"message":"later failure"},"output":[]}}),
                    ));
                } else {
                    events.push((
                        "response.completed",
                        json!({"type":"response.completed","response":completed}),
                    ));
                }
            }
            let stream: String = events
                .iter()
                .map(|(name, payload)| format!("event: {name}\ndata: {payload}\n\n"))
                .collect();
            MockAction::Respond(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                Content-Length: {}\r\nConnection: close\r\n\r\n{stream}",
                stream.len()
            ))
        } else {
            MockAction::respond_json(completed.to_string())
        };
        let scenario = scenario(
            UpstreamFormat::OpenAiResponse,
            Some(automatic_section("supported")),
            None,
            None,
            false,
            vec![responses_completed(
                json!([responses_message_item("earlier answer")]),
                1,
                1,
            )],
        )
        .await;
        let PoolAccounts {
            first,
            second,
            second_mock: _second_mock,
            second_recorded,
            pool_id,
        } = add_second_pool_account(&scenario, vec![action]).await;
        crate::storage::quota_blocks::record_rejection(
            &scenario._database.0,
            first.provider.id,
            0,
            &["earlier restriction".into()],
            &crate::provider::quota::QuotaApplicability::AllModels,
            crate::unix_time::unix_now_micros(),
        )
        .await
        .expect("first blocked");
        let request = json!({"model":MODEL,"stream":is_streaming,
            "input":[user_item("covered question")],
            "context_management":[{"type":"compaction","compact_threshold":1}]});
        let (status, wire) = post(
            &scenario,
            "/api/openai-v1/responses",
            &openai_headers(),
            request.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{wire}");
        let returned = if is_streaming {
            let frames = sse_payloads(&wire);
            let actual = frames.iter().find(|event| {
                event["type"] == "response.output_item.done"
                    && event["item"]["type"] == "compaction"
            });
            if is_partial {
                assert!(actual.is_none(), "added-only state must not escape: {wire}");
            }
            if is_covered_call {
                let output = frames
                    .iter()
                    .find(|event| event["type"] == "response.completed")
                    .expect("actual completed tool output")["response"]["output"]
                    .as_array()
                    .expect("actual ordered output");
                let boundary = output
                    .iter()
                    .rposition(|item| item["type"] == "compaction")
                    .expect("actual native checkpoint");
                output.iter().skip(boundary).cloned().collect()
            } else {
                let actual_index = actual
                    .and_then(|event| event["output_index"].as_u64())
                    .unwrap_or(0);
                let state = actual.map_or_else(|| block.clone(), |event| event["item"].clone());
                let text: String = frames
                    .iter()
                    .filter(|event| {
                        event["type"] == "response.output_text.delta"
                            && event["output_index"]
                                .as_u64()
                                .is_some_and(|index| index > actual_index)
                    })
                    .filter_map(|event| event["delta"].as_str())
                    .collect();
                if is_after_text {
                    assert_eq!(text, "answer λ", "post-state text occupies a new item");
                }
                let mut items = vec![state];
                if !text.is_empty() {
                    items.push(responses_message_item(&text));
                }
                items
            }
        } else {
            let body: Value = serde_json::from_str(&wire).expect("actual JSON");
            body["output"].as_array().expect("actual output").clone()
        };
        let completed_units = if returned.len() == 1 { 1 } else { 2 };
        let mut replay = returned;
        if is_covered_call {
            replay.push(
                json!({"type":"function_call_output","call_id":"call_after","output":"result"}),
            );
        }
        replay.push(user_item("next question"));
        let replay = json!({"model":MODEL,"input":replay});
        let parsed = Format::parse_request(replay.to_string().as_bytes(), &RouteContext::default())
            .expect("actual output replay");
        let active = crate::compaction_model::boundary::BoundaryPreparation::authenticate(
            parsed.compaction.as_ref().expect("native state"),
        )
        .expect("native preparation")
        .routing_preparation(&parsed)
        .expect("active window");
        let namespace = crate::affinity::Namespace {
            api_key_id: scenario.api_key_id,
            pool_id,
        };
        let evidence = crate::affinity::Evidence {
            artifact: active.artifact,
            ..crate::affinity::Evidence::default()
        };
        match scenario.affinity_cache.acquire(namespace, &evidence) {
            crate::affinity::Decision::Bound(binding) => {
                assert!(!is_partial, "unfinished state creates no artifact");
                assert_eq!(binding.provider_id(), second.id);
            }
            crate::affinity::Decision::Fresh(_) => {
                assert!(is_partial, "completed state must bind: {delivery}")
            }
            _ => panic!("unexpected native Responses artifact binding"),
        }
        let evidence = crate::affinity::Evidence {
            history: active
                .projector
                .evidence()
                .history
                .into_iter()
                .filter(|prefix| prefix.units.get() == completed_units)
                .collect(),
            ..crate::affinity::Evidence::default()
        };
        match scenario.affinity_cache.acquire(namespace, &evidence) {
            crate::affinity::Decision::Bound(binding) => {
                assert!(
                    !has_failure,
                    "unfinished answer cannot establish its full window"
                );
                assert_eq!(binding.provider_id(), second.id);
            }
            crate::affinity::Decision::Fresh(_) => {
                assert!(has_failure, "complete active window must bind: {delivery}")
            }
            _ => panic!("unexpected native Responses window binding"),
        }
        if !is_partial {
            crate::storage::quota_blocks::reset(&scenario._database.0, first.provider.id)
                .await
                .expect("earlier account recovers");
            crate::storage::quota_blocks::record_rejection(
                &scenario._database.0,
                second.id,
                0,
                &["bound restriction".into()],
                &crate::provider::quota::QuotaApplicability::AllModels,
                crate::unix_time::unix_now_micros(),
            )
            .await
            .expect("bound account exhausted");
            let (status, error) = post_openai(&scenario, "/api/openai-v1/responses", replay).await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
            assert!(
                error.to_string().contains("bound_account_quota_exhausted"),
                "{error}"
            );
        }
        assert_dispatches(&scenario, 0);
        assert_eq!(second_recorded.lock().expect("recording").len(), 1);
    }
}
