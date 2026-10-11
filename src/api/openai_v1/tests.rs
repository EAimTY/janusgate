//! The OpenAI surface tests, driven end to end through the real `/api` router against the mock
//! upstream: bearer authentication and its rejection envelope, the 404/405 fallbacks, request
//! rejections, the model list's policy filtering and shape, the unavailable-pool outcome, the
//! key's access-control denial, disabled routes and models, the exhausted-pool 429, the exact
//! rendered upstream request, the Responses endpoint's cross-format aggregation, the compact
//! endpoint's request contract, upstream-error mirroring and transport-failure mapping, the
//! malformed-payload 502, the SSE passthrough, the aggregated always-streaming upstream, the
//! foreign response envelope, the mid-stream failure's native error event, and the mid-stream
//! client departure.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section would \
              document nothing its name does not"
)]

use crate::{
    access_control::{AccessDecision, ApiKeyAccessControl, ApiKeyRouteOverride},
    api::tests_support::{
        KEY_SECRET, TestGateway, UPSTREAM_SECRET, bearer, body_json, body_text, chat_definition,
        goog_key, lock_recordings, recorded_json, seed_allowing_key, seed_definition, seed_key,
        seed_model, seed_pool, seed_provider, seed_quota_block, seed_route, upstream_chat_json,
        upstream_chat_sse_frames,
    },
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{
        api_keys::{ApiKeyFields, update_api_key},
        models::ModelUpdate,
    },
    test_support::mock_upstream::{MockAction, MockUpstream},
    vocabulary::ReasoningLevel,
};
use axum::response::Response;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::{error::Error as StdError, time::Duration};

/// The chat-completions endpoint path of this surface, relative to the `/api` parent router.
const CHAT_PATH: &str = "/openai-v1/chat/completions";

/// The model-listing endpoint path of this surface.
const MODELS_PATH: &str = "/openai-v1/models";

/// A one-turn chat-completions request body for `public_id`.
fn chat_request(public_id: &str) -> Value {
    json!({
        "model": public_id,
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 50
    })
}

/// A chat-completions definition whose upstream URL rides the provider base URL plus the
/// literal `/upstream/v1/chat/completions` path, with optional inference overrides.
fn chat_definition_at(overrides: Value) -> Value {
    chat_definition(
        json!([
            {"type": "base"},
            {"type": "literal", "value": "/upstream/v1/chat/completions"}
        ]),
        overrides,
    )
}

/// Seeds one provider exposing `upstream_id` through the direct public route `public_id`,
/// upstreaming to `base_url` over the given definition payload.
async fn seed_direct_model(
    gateway: &TestGateway,
    base_url: &str,
    public_id: &str,
    upstream_id: &str,
) -> Result<(), Box<dyn StdError>> {
    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({})),
    )
    .await?;

    let provider_id =
        seed_provider(gateway.database(), "fixture-account", "", base_url, &stored).await?;

    let model_id = seed_model(gateway.database(), provider_id, upstream_id).await?;

    let _ = seed_route(
        gateway.database(),
        public_id,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    Ok(())
}

/// Asserts the OpenAI error envelope of a rejected response and returns the parsed body.
async fn assert_error_envelope(
    response: Response,
    expected_status: StatusCode,
    expected_code: Value,
) -> Result<Value, Box<dyn StdError>> {
    assert_eq!(response.status(), expected_status);

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/type"),
        "invalid_request_error",
        "{body}"
    );

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        &expected_code,
        "{body}"
    );

    Ok(body)
}

/// A missing credential is the invalid-key rejection in the OpenAI envelope.
#[tokio::test]
async fn a_missing_bearer_key_is_rejected() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let response = gateway
        .post_json(CHAT_PATH, &[], json!({"model": "m", "messages": []}))
        .await;

    let body =
        assert_error_envelope(response, StatusCode::UNAUTHORIZED, "invalid_api_key".into()).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "Incorrect API key provided."
    );

    Ok(())
}

/// The bearer convention authenticates case-insensitively by scheme and byte-exactly by
/// secret; an unknown secret, a foreign scheme, and a key disabled since its last request all
/// reject.
#[tokio::test]
async fn credentials_authenticate_per_the_bearer_convention() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let key_id = seed_allowing_key(gateway.database()).await?;

    // An unknown secret rejects.
    let unknown = gateway.get(MODELS_PATH, &[bearer("sk-unknown")]).await;

    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

    // A foreign scheme rejects.
    let foreign = gateway
        .get(
            MODELS_PATH,
            &[("authorization", format!("Basic {KEY_SECRET}"))],
        )
        .await;

    assert_eq!(foreign.status(), StatusCode::UNAUTHORIZED);

    // The scheme comparison is case-insensitive: a lowercase `bearer` of the enabled secret
    // authenticates through to the handler.
    let lowercase = gateway
        .get(
            MODELS_PATH,
            &[("authorization", format!("bearer {KEY_SECRET}"))],
        )
        .await;

    assert_eq!(lowercase.status(), StatusCode::OK);

    // A key disabled since authentication rejects on the next request.
    update_api_key(
        gateway.database(),
        key_id,
        ApiKeyFields {
            name: "allowing",
            key: KEY_SECRET,
            is_enabled: false,
            access_control: &ApiKeyAccessControl {
                default_access: AccessDecision::Allow,
                owner_defaults: Vec::new(),
                route_overrides: Vec::new(),
            },
        },
    )
    .await?;

    let disabled = gateway.get(MODELS_PATH, &[bearer(KEY_SECRET)]).await;

    assert_eq!(disabled.status(), StatusCode::UNAUTHORIZED);

    Ok(())
}

/// Unmapped paths render the surface's own 404 and unregistered methods its 405, both through
/// the OpenAI error envelope.
#[tokio::test]
async fn unmapped_paths_and_methods_render_the_surface_envelope() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let unknown_path = gateway
        .post_json("/openai-v1/nope", &[bearer(KEY_SECRET)], json!({}))
        .await;

    assert_eq!(unknown_path.status(), StatusCode::NOT_FOUND);

    let body = body_json(unknown_path).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/type"),
        "invalid_request_error"
    );

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "Invalid URL (POST /openai-v1/nope)"
    );

    let wrong_method = gateway.get(CHAT_PATH, &[bearer(KEY_SECRET)]).await;

    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);

    let body = body_json(wrong_method).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "Method not allowed."
    );

    Ok(())
}

/// Unknown members, malformed JSON, and conflicting semantic controls reject with the 400 the
/// request boundary pins, before any routing or dispatch.
#[tokio::test]
async fn malformed_and_unsupported_bodies_are_rejected() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let unknown_member = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            json!({"model": "m", "messages": [], "frequency_penalty": 0.5}),
        )
        .await;

    assert_error_envelope(unknown_member, StatusCode::BAD_REQUEST, Value::Null).await?;

    let malformed = gateway
        .post_raw(CHAT_PATH, &[bearer(KEY_SECRET)], "{not json")
        .await;

    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let conflicting = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            json!({
                "model": "m",
                "messages": [],
                "max_tokens": 10,
                "max_completion_tokens": 20
            }),
        )
        .await;

    assert_error_envelope(conflicting, StatusCode::BAD_REQUEST, Value::Null).await?;

    Ok(())
}

/// The model list renders the official OpenAI list shape over the key's admitted, structurally
/// available routes in public-id order, omitting unknown capabilities and hiding denied routes.
#[tokio::test]
async fn the_model_list_renders_policy_filtered_ordered_entries() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({})),
    )
    .await?;

    let direct_provider = seed_provider(
        gateway.database(),
        "zeta-account",
        "zeta.",
        "http://unused.test",
        &stored,
    )
    .await?;

    let direct_model = seed_model(gateway.database(), direct_provider, "up-b").await?;

    crate::storage::models::update_model_overrides_checked(
        gateway.database(),
        ModelUpdate {
            model_id: direct_model,
            is_enabled: true,
            thinking_levels_override: Some(&[ReasoningLevel::Low, ReasoningLevel::High]),
            max_input_tokens_override: Some(4096),
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    let direct_route = seed_route(
        gateway.database(),
        "zeta.model-b",
        &ModelRouteTarget::Direct {
            model_id: direct_model,
        },
    )
    .await?;

    let pooled_provider = seed_provider(
        gateway.database(),
        "alpha-account",
        "",
        "http://unused.test",
        &stored,
    )
    .await?;

    // The pool exists before the member's inventory: the discovery then creates the pool's own
    // automatic public entry `alpha.up-a`, which needs no authored route.
    let _pool_id = seed_pool(
        gateway.database(),
        "alpha-pool",
        "alpha.",
        &[ProviderPoolMember {
            provider_id: pooled_provider,
            priority: 1,
        }],
    )
    .await?;

    let _ = seed_model(gateway.database(), pooled_provider, "up-a").await?;

    let key_id = seed_allowing_key(gateway.database()).await?;

    let response = gateway.get(MODELS_PATH, &[bearer(KEY_SECRET)]).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/object"), "list");

    let entries = crate::test_support::at(&body, "/data")
        .as_array()
        .ok_or_else(|| format!("expected a data array, got {body}"))?;

    let ids: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();

    // The pool's automatic entry, the authored alias, and the direct provider's standalone
    // automatic entry all list, ordered by public id.
    assert_eq!(ids, vec!["alpha.up-a", "zeta.model-b", "zeta.up-b"]);

    let direct_entry = entries
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some("zeta.model-b"))
        .ok_or_else(|| format!("expected the direct entry, got {body}"))?;

    assert_eq!(crate::test_support::at(direct_entry, "/object"), "model");

    assert_eq!(crate::test_support::at(direct_entry, "/created"), &json!(0));

    assert_eq!(
        crate::test_support::at(direct_entry, "/owned_by"),
        "zeta-account"
    );

    assert_eq!(
        crate::test_support::at(direct_entry, "/max_input_tokens"),
        &json!(4096)
    );

    assert_eq!(
        crate::test_support::at(direct_entry, "/reasoning_levels"),
        &json!(["low", "high"])
    );

    // Unknown capabilities are omitted, never fabricated.
    assert!(direct_entry.get("max_output_tokens").is_none());

    assert!(direct_entry.get("compaction").is_none());

    // A route the key denies disappears from the list on the next request.
    update_api_key(
        gateway.database(),
        key_id,
        ApiKeyFields {
            name: "allowing",
            key: KEY_SECRET,
            is_enabled: true,
            access_control: &ApiKeyAccessControl {
                default_access: AccessDecision::Allow,
                owner_defaults: Vec::new(),
                route_overrides: vec![ApiKeyRouteOverride {
                    model_route_id: direct_route,
                    access: AccessDecision::Deny,
                }],
            },
        },
    )
    .await?;

    let filtered = gateway.get(MODELS_PATH, &[bearer(KEY_SECRET)]).await;

    let body = body_json(filtered).await?;

    let ids: Vec<&str> = crate::test_support::at(&body, "/data")
        .as_array()
        .ok_or_else(|| format!("expected a data array, got {body}"))?
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();

    assert_eq!(ids, vec!["alpha.up-a", "zeta.up-b"]);

    Ok(())
}

/// An unknown public model is the native model-not-found 404, and an enabled pool route with
/// no structural candidate is the pool-unavailable 503.
#[tokio::test]
async fn routing_gates_render_their_native_outcomes() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let unknown = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("missing.model"),
        )
        .await;

    let body =
        assert_error_envelope(unknown, StatusCode::NOT_FOUND, "model_not_found".into()).await?;

    assert_eq!(crate::test_support::at(&body, "/error/param"), "model");

    // An enabled pool route whose pool has no members is structurally unavailable.
    let pool_id = seed_pool(gateway.database(), "empty-pool", "", &[]).await?;

    let _ = seed_route(
        gateway.database(),
        "empty.model",
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "up".to_owned(),
        },
    )
    .await?;

    let unavailable = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("empty.model"),
        )
        .await;

    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = body_json(unavailable).await?;

    assert_eq!(crate::test_support::at(&body, "/error/type"), "api_error");

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        "pool_unavailable"
    );

    Ok(())
}

/// The completion dispatches exactly the pinned upstream request — base-URL address, bearer
/// credential, engine-owned Accept, the physical upstream model id — and renders the
/// aggregated Chat Completions body under the public id.
#[tokio::test]
async fn completion_dispatches_the_pinned_upstream_request() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_chat_json("Hi there", 4, 3).to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/object"), "chat.completion");

    assert_eq!(crate::test_support::at(&body, "/model"), "public.model");

    assert_eq!(
        crate::test_support::at(&body, "/choices/0/message/content"),
        "Hi there"
    );

    assert_eq!(
        crate::test_support::at(&body, "/choices/0/finish_reason"),
        "stop"
    );

    assert_eq!(
        crate::test_support::at(&body, "/usage/prompt_tokens"),
        &json!(4)
    );

    assert_eq!(
        crate::test_support::at(&body, "/usage/completion_tokens"),
        &json!(3)
    );

    let recorded = lock_recordings(&requests);

    assert_eq!(recorded.len(), 1);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    assert_eq!(dispatched.method, "POST");

    assert_eq!(dispatched.target, "/upstream/v1/chat/completions");

    assert_eq!(
        dispatched.header("authorization"),
        Some(format!("Bearer {UPSTREAM_SECRET}").as_str())
    );

    assert_eq!(dispatched.header("accept"), Some("application/json"));

    let rendered: Value = recorded_json(&dispatched.body);

    // The upstream receives the physical id, never the public alias.
    assert_eq!(
        crate::test_support::at(&rendered, "/model"),
        "physical-model"
    );

    assert_eq!(crate::test_support::at(&rendered, "/stream"), &json!(false));

    assert_eq!(
        crate::test_support::at(&rendered, "/max_completion_tokens"),
        &json!(50)
    );

    assert_eq!(
        crate::test_support::at(&rendered, "/messages"),
        &json!([{"role": "user", "content": [{"type": "text", "text": "Hello"}]}])
    );

    Ok(())
}

/// A rejected upstream request mirrors its non-2xx status and carries the bounded body
/// excerpt, in the OpenAI envelope's upstream-error row.
#[tokio::test]
async fn upstream_rejections_mirror_status_and_excerpt() -> Result<(), Box<dyn StdError>> {
    let rejection = json!({"error": {"message": "context length exceeded"}});

    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{}",
        rejection.to_string().len(),
        rejection
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/error/type"), "api_error");

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        "upstream_error"
    );

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("context length exceeded"));

    Ok(())
}

/// A transport failure without a usable HTTP response maps onto the upstream-unavailable 502.
#[tokio::test]
async fn transport_close_maps_to_unavailable_502() -> Result<(), Box<dyn StdError>> {
    let (mock, _requests) = MockUpstream::start(vec![MockAction::Close]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        "upstream_error"
    );

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("upstream unreachable"));

    Ok(())
}

/// A streaming client receives the translated `text/event-stream` passthrough: data chunks in
/// order, the usage chunk once immediately before the terminal `[DONE]`.
#[tokio::test]
async fn streaming_clients_receive_translated_sse() -> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let frames: Vec<String> = upstream_chat_sse_frames();

    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "{head}{}",
        frames.concat()
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            json!({
                "model": "public.model",
                "messages": [{"role": "user", "content": "Hello"}],
                "stream": true,
                "stream_options": {"include_usage": true}
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = body_text(response).await;

    // The content deltas arrive as data chunks, the usage chunk rides the terminal frame, and
    // the sentinel closes the stream.
    assert!(body.contains(r#""content":"Hello ""#));

    assert!(body.contains(r#""content":"stream""#));

    assert!(body.contains(r#""prompt_tokens":7"#));

    assert!(body.trim_end().ends_with("data: [DONE]"));

    Ok(())
}

/// A non-streaming caller of an always-streaming upstream receives the aggregated native
/// envelope: the definition's framing decides, never header sniffing.
#[tokio::test]
async fn non_streaming_clients_aggregate_always_streaming_upstreams()
-> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let frames: Vec<String> = upstream_chat_sse_frames();

    let (mock, requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "{head}{}",
        frames.concat()
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({"upstream_streaming": "always"})),
    )
    .await?;

    let provider_id = seed_provider(
        gateway.database(),
        "fixture-account",
        "",
        &mock.base(),
        &stored,
    )
    .await?;

    let model_id = seed_model(gateway.database(), provider_id, "physical-model").await?;

    let _ = seed_route(
        gateway.database(),
        "public.model",
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/object"), "chat.completion");

    assert_eq!(
        crate::test_support::at(&body, "/choices/0/message/content"),
        "Hello stream"
    );

    assert_eq!(
        crate::test_support::at(&body, "/usage/prompt_tokens"),
        &json!(7)
    );

    // The upstream was addressed as a streaming request even though the caller is not.
    let recorded = lock_recordings(&requests);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    let rendered: Value = recorded_json(&dispatched.body);

    assert_eq!(crate::test_support::at(&rendered, "/stream"), &json!(true));

    Ok(())
}

/// A configured foreign response envelope is selected before the codec ingests the response.
#[tokio::test]
async fn a_foreign_response_envelope_is_unwrapped() -> Result<(), Box<dyn StdError>> {
    let wrapped = json!({
        "trace_id": "abc",
        "result": upstream_chat_json("unwrapped", 2, 2)
    });

    let (mock, _requests) = MockUpstream::start_json(vec![wrapped.to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({"response_unwrap": "result"})),
    )
    .await?;

    let provider_id = seed_provider(
        gateway.database(),
        "fixture-account",
        "",
        &mock.base(),
        &stored,
    )
    .await?;

    let model_id = seed_model(gateway.database(), provider_id, "physical-model").await?;

    let _ = seed_route(
        gateway.database(),
        "public.model",
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/choices/0/message/content"),
        "unwrapped"
    );

    Ok(())
}

/// Dropping the streamed body mid-flight, while the upstream is still streaming, settles
/// promptly instead of hanging the response task or the runtime.
#[tokio::test]
async fn a_mid_stream_client_departure_finalizes_without_hanging() -> Result<(), Box<dyn StdError>>
{
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let frame = "data: {\"choices\": [{\"index\": 0, \"delta\": {\"content\": \"tick\"}}]}\n\n";

    let (mock, requests) = MockUpstream::start(vec![MockAction::StreamForever {
        head: head.to_owned(),
        frame: frame.to_owned(),
        interval: Duration::from_millis(20),
    }])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            json!({
                "model": "public.model",
                "messages": [{"role": "user", "content": "Hello"}],
                "stream": true
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    // The client departs without reading the body: the render task observes the closed channel
    // on its next frame and finalizes the row on its own.
    drop(response);

    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::time::sleep(Duration::from_millis(100)),
    )
    .await
    .map_err(|_| "the departure path does not settle within the budget")?;

    let recorded = lock_recordings(&requests);

    assert_eq!(recorded.len(), 1);

    Ok(())
}

/// Departing before the upstream produced even its first frame settles the same way: the
/// response task observes the closed body channel and finalizes without hanging.
#[tokio::test]
async fn a_departure_before_the_first_frame_finalizes_without_hanging()
-> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let frame = "data: {\"choices\": [{\"index\": 0, \"delta\": {\"content\": \"late\"}}]}\n\n";

    let (mock, requests) = MockUpstream::start(vec![MockAction::StreamAfterDelay {
        head: head.to_owned(),
        frame: frame.to_owned(),
        delay: Duration::from_millis(100),
    }])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            json!({
                "model": "public.model",
                "messages": [{"role": "user", "content": "Hello"}],
                "stream": true
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    // The client leaves before the upstream's delayed frame ever existed.
    drop(response);

    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::time::sleep(Duration::from_millis(200)),
    )
    .await
    .map_err(|_| "the departure path does not settle within the budget")?;

    let recorded = lock_recordings(&requests);

    assert_eq!(recorded.len(), 1);

    Ok(())
}

/// A Gemini credential header never authenticates the OpenAI surface: the conventions are not
/// shared, and an unreadable credential is the invalid-key rejection.
#[tokio::test]
async fn a_gemini_credential_does_not_authenticate_the_openai_surface()
-> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[goog_key(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    Ok(())
}

/// A route the key's access policy denies resolves as the same model-not-found 404 as an
/// unknown model: access control filters route admission, it never renders an authorization
/// error naming the model's existence.
#[tokio::test]
async fn a_route_denied_to_the_key_is_model_not_found() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({})),
    )
    .await?;

    let provider_id = seed_provider(
        gateway.database(),
        "fixture-account",
        "",
        "http://unused.test",
        &stored,
    )
    .await?;

    let model_id = seed_model(gateway.database(), provider_id, "up-a").await?;

    let denied_route = seed_route(
        gateway.database(),
        "secret.model",
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    seed_key(
        gateway.database(),
        "denying",
        KEY_SECRET,
        &ApiKeyAccessControl {
            default_access: AccessDecision::Allow,
            owner_defaults: Vec::new(),
            route_overrides: vec![ApiKeyRouteOverride {
                model_route_id: denied_route,
                access: AccessDecision::Deny,
            }],
        },
    )
    .await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("secret.model"),
        )
        .await;

    let body =
        assert_error_envelope(response, StatusCode::NOT_FOUND, "model_not_found".into()).await?;

    assert_eq!(crate::test_support::at(&body, "/error/param"), "model");

    Ok(())
}

/// A disabled route and a route whose target model was disabled both resolve as unknown:
/// completion admission requires effective enablement on the route and its physical target.
#[tokio::test]
async fn a_disabled_route_or_model_is_model_not_found() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({})),
    )
    .await?;

    let provider_id = seed_provider(
        gateway.database(),
        "fixture-account",
        "",
        "http://unused.test",
        &stored,
    )
    .await?;

    let route_model = seed_model(gateway.database(), provider_id, "up-route").await?;

    crate::storage::routing::create_route(
        gateway.database(),
        "disabled.route",
        false,
        &ModelRouteTarget::Direct {
            model_id: route_model,
        },
    )
    .await?;

    let disabled_model = seed_model(gateway.database(), provider_id, "up-model").await?;

    let _ = seed_route(
        gateway.database(),
        "disabled.model",
        &ModelRouteTarget::Direct {
            model_id: disabled_model,
        },
    )
    .await?;

    crate::storage::models::update_model_overrides_checked(
        gateway.database(),
        ModelUpdate {
            model_id: disabled_model,
            is_enabled: false,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let disabled_route = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("disabled.route"),
        )
        .await;

    assert_error_envelope(
        disabled_route,
        StatusCode::NOT_FOUND,
        "model_not_found".into(),
    )
    .await?;

    let disabled_model = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("disabled.model"),
        )
        .await;

    assert_error_envelope(
        disabled_model,
        StatusCode::NOT_FOUND,
        "model_not_found".into(),
    )
    .await?;

    Ok(())
}

/// A pool whose every member carries persisted quota evidence is the pool-quota exhaustion 429
/// in the OpenAI envelope, advertising no `Retry-After` while no candidate's reset is proven.
#[tokio::test]
async fn an_exhausted_pool_is_the_pool_quota_exhaustion_429() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition_at(json!({})),
    )
    .await?;

    let provider_id = seed_provider(
        gateway.database(),
        "blocked-account",
        "",
        "http://unused.test",
        &stored,
    )
    .await?;

    let _ = seed_model(gateway.database(), provider_id, "up-blocked").await?;

    let pool_id = seed_pool(
        gateway.database(),
        "blocked-pool",
        "",
        &[ProviderPoolMember {
            provider_id,
            priority: 1,
        }],
    )
    .await?;

    let _ = seed_route(
        gateway.database(),
        "blocked.model",
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "up-blocked".to_owned(),
        },
    )
    .await?;

    seed_quota_block(gateway.database(), provider_id).await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("blocked.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    assert!(
        !response
            .headers()
            .contains_key(axum::http::header::RETRY_AFTER)
    );

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/type"),
        "rate_limit_error"
    );

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        "pool_quota_exhausted"
    );

    Ok(())
}

/// The Responses endpoint translates to the definition's upstream framing and renders the
/// aggregated Responses envelope under the public id: the client family and the upstream wire
/// format are independent choices, and the upstream receives the physical id non-streaming.
#[tokio::test]
async fn the_responses_endpoint_renders_the_aggregated_envelope() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_chat_json("Hi there", 4, 3).to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            "/openai-v1/responses",
            &[bearer(KEY_SECRET)],
            json!({"model": "public.model", "input": "Hello"}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/object"), "response");

    assert_eq!(crate::test_support::at(&body, "/status"), "completed");

    assert_eq!(crate::test_support::at(&body, "/model"), "public.model");

    let text = crate::test_support::at(&body, "/output")
        .as_array()
        .ok_or_else(|| format!("expected an output array, got {body}"))?
        .iter()
        .filter_map(|item| {
            item.get("content")
                .and_then(|content| content.as_array())
                .and_then(|parts| parts.first())
                .and_then(|part| part.get("text").and_then(Value::as_str))
        })
        .collect::<Vec<_>>();

    assert_eq!(text, vec!["Hi there"]);

    assert_eq!(
        crate::test_support::at(&body, "/usage/input_tokens"),
        &json!(4)
    );

    assert_eq!(
        crate::test_support::at(&body, "/usage/output_tokens"),
        &json!(3)
    );

    let recorded = lock_recordings(&requests);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    let rendered: Value = recorded_json(&dispatched.body);

    // The upstream receives the physical id and the definition's non-streaming framing; the
    // chat-completions body carries no store member of the Responses family.
    assert_eq!(
        crate::test_support::at(&rendered, "/model"),
        "physical-model"
    );

    assert_eq!(crate::test_support::at(&rendered, "/stream"), &json!(false));

    assert!(rendered.get("store").is_none());

    Ok(())
}

/// The standalone compact endpoint rejects its protocol's generation-only controls and stateful
/// continuation at the request boundary, and resolves its model through the same routing
/// admission as completion.
#[tokio::test]
async fn the_compact_endpoint_rejects_out_of_contract_bodies() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let streaming = gateway
        .post_json(
            "/openai-v1/responses/compact",
            &[bearer(KEY_SECRET)],
            json!({"model": "m", "input": "Hello", "stream": true}),
        )
        .await;

    assert_error_envelope(streaming, StatusCode::BAD_REQUEST, Value::Null).await?;

    let stateful = gateway
        .post_json(
            "/openai-v1/responses/compact",
            &[bearer(KEY_SECRET)],
            json!({
                "model": "m",
                "input": "Hello",
                "previous_response_id": "resp-old"
            }),
        )
        .await;

    let body = assert_error_envelope(stateful, StatusCode::BAD_REQUEST, Value::Null).await?;

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("previous_response_id"));

    let unknown = gateway
        .post_json(
            "/openai-v1/responses/compact",
            &[bearer(KEY_SECRET)],
            json!({"model": "missing.model", "input": "Hello"}),
        )
        .await;

    let body =
        assert_error_envelope(unknown, StatusCode::NOT_FOUND, "model_not_found".into()).await?;

    assert_eq!(crate::test_support::at(&body, "/error/param"), "model");

    Ok(())
}

/// A 2xx upstream body that is not valid JSON is a malformed payload: the unavailable 502 in
/// the surface envelope, never a fabricated empty completion.
#[tokio::test]
async fn a_malformed_upstream_payload_is_the_unavailable_502() -> Result<(), Box<dyn StdError>> {
    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\nnot json"
            .to_owned(),
    )])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            chat_request("public.model"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/error/type"), "api_error");

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        "upstream_error"
    );

    Ok(())
}

/// A malformed upstream SSE frame after committed headers renders the classified failure as the
/// native mid-stream error event under the already-declared 200, keeping the frames the stream
/// delivered before the failure.
#[tokio::test]
async fn a_mid_stream_failure_renders_the_native_error_event() -> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let healthy = r#"data: {"choices": [{"index": 0, "delta": {"content": ""}}]}

data: {"choices": [{"index": 0, "delta": {"content": "partial"}}]}

"#;

    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "{head}{healthy}data: {{broken\n\n"
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_direct_model(&gateway, &mock.base(), "public.model", "physical-model").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            CHAT_PATH,
            &[bearer(KEY_SECRET)],
            json!({
                "model": "public.model",
                "messages": [{"role": "user", "content": "Hello"}],
                "stream": true
            }),
        )
        .await;

    // The status was committed before the failure, so it stays the declared 200.
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = body_text(response).await;

    assert!(body.contains(r#""content":"partial""#));

    assert!(body.contains(r#""error""#));

    assert!(body.contains(r#""code":"upstream_error""#));

    Ok(())
}
