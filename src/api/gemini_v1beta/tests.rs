//! The Gemini prefix-surface tests, driven end to end through the real `/api` router against
//! the mock upstream: the `x-goog-api-key` convention and its rejection envelope, the
//! `alt=sse` streaming predicate, the wildcard action paths, the GenerateContent and
//! Interactions dispatches with their pinned upstream requests, the native SSE streaming of
//! both endpoints, the transport-failure mapping onto the native unavailable envelope, the
//! stateful-continuation rejection, the native model-list shape, and the unsupported-control
//! rejection.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section would \
              document nothing its name does not"
)]

use crate::{
    api::tests_support::{
        KEY_SECRET, TestGateway, UPSTREAM_SECRET, body_json, body_text, goog_key, lock_recordings,
        recorded_json, seed_allowing_key, seed_definition, seed_model, seed_provider, seed_route,
        upstream_gemini_json,
    },
    routing::{ModelRouteTarget, ProviderPoolMember},
    test_support::mock_upstream::{MockAction, MockUpstream},
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::error::Error as StdError;

/// The GenerateContent action path of `public_id`.
fn generate_path(public_id: &str) -> String {
    format!("/gemini-v1beta/models/{public_id}:generateContent")
}

/// The streaming action path of `public_id`.
fn stream_path(public_id: &str) -> String {
    format!("/gemini-v1beta/models/{public_id}:streamGenerateContent?alt=sse")
}

/// The model-listing endpoint path of this prefix.
const MODELS_PATH: &str = "/gemini-v1beta/models";

/// The Interactions endpoint path of this prefix.
const INTERACTIONS_PATH: &str = "/gemini-v1beta/interactions";

/// A GenerateContent request body for the URL-routed model.
fn generate_request(text: &str) -> Value {
    json!({
        "contents": [{"role": "user", "parts": [{"text": text}]}]
    })
}

/// Seeds one provider exposing `upstream_id` through the direct public route `public_id`,
/// upstreaming to `base_url` over the given Gemini definition payload.
async fn seed_gemini_model(
    gateway: &TestGateway,
    definition: Value,
    base_url: &str,
    public_id: &str,
    upstream_id: &str,
) -> Result<(), Box<dyn StdError>> {
    let stored = seed_definition(gateway.database(), "fixture-definition", definition).await?;

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

/// The GenerateContent upstream definition: the model rides the URL expression, the credential
/// rides the `x-goog-api-key` header, and the upstream matches the client's streaming mode.
fn gemini_generate_definition() -> Value {
    json!({
        "format": "gemini_generate_content",
        "inference": {
            "url": [
                {"type": "base"},
                {"type": "literal", "value": "/upstream/v1beta/models/"},
                {"type": "model"},
                {"type": "literal", "value": ":generateContent"}
            ],
            "headers": [
                {"name": "x-goog-api-key", "value": [{"type": "secret"}]}
            ],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    })
}

/// Asserts the Gemini error envelope of a rejected response.
async fn assert_error_envelope(
    response: axum::response::Response,
    expected_status: StatusCode,
    expected_code: u16,
    expected_name: &str,
) -> Result<Value, Box<dyn StdError>> {
    assert_eq!(response.status(), expected_status);

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/code"),
        &expected_code,
        "{body}"
    );

    assert_eq!(
        crate::test_support::at(&body, "/error/status"),
        expected_name,
        "{body}"
    );

    Ok(body)
}

/// Missing, unknown, and query-carried credentials all reject through the Gemini envelope; the
/// header is the only credential convention this prefix reads.
#[tokio::test]
async fn credentials_authenticate_only_through_the_goog_header() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    // A missing header rejects.
    let missing = gateway
        .post_json(&generate_path("m"), &[], generate_request("Hi"))
        .await;

    let body =
        assert_error_envelope(missing, StatusCode::UNAUTHORIZED, 401, "UNAUTHENTICATED").await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "Incorrect API key provided."
    );

    // An unknown header value rejects.
    let unknown = gateway
        .post_json(
            &generate_path("m"),
            &[goog_key("unknown-key")],
            generate_request("Hi"),
        )
        .await;

    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

    // The `?key=` query form is not a credential: the header-only convention rejects it.
    let query = gateway
        .post_json(
            &format!("{}?key={KEY_SECRET}", generate_path("m")),
            &[],
            generate_request("Hi"),
        )
        .await;

    assert_eq!(query.status(), StatusCode::UNAUTHORIZED);

    Ok(())
}

/// The streaming action without its `?alt=sse` query is the streaming convention's 400, and a
/// capture outside the two action shapes is the prefix's unmapped-path 404.
#[tokio::test]
async fn url_actions_enforce_their_conventions() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let missing_query = gateway
        .post_json(
            "/gemini-v1beta/models/m:streamGenerateContent",
            &[goog_key(KEY_SECRET)],
            generate_request("Hi"),
        )
        .await;

    let body = assert_error_envelope(
        missing_query,
        StatusCode::BAD_REQUEST,
        400,
        "INVALID_ARGUMENT",
    )
    .await?;

    assert!(
        crate::test_support::at(&body, "/error/message")
            .as_str()
            .is_some_and(|message| message.contains("alt"))
    );

    let unknown_action = gateway
        .post_json(
            "/gemini-v1beta/models/m:countTokens",
            &[goog_key(KEY_SECRET)],
            json!({}),
        )
        .await;

    let body =
        assert_error_envelope(unknown_action, StatusCode::NOT_FOUND, 404, "NOT_FOUND").await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "Invalid URL (POST /gemini-v1beta/models/m:countTokens)"
    );

    Ok(())
}

/// An unknown or unaddressable URL model is the native model-not-found 404.
#[tokio::test]
async fn an_unknown_url_model_is_model_not_found() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let unknown = gateway
        .post_json(
            &generate_path("missing.model"),
            &[goog_key(KEY_SECRET)],
            generate_request("Hi"),
        )
        .await;

    let body = assert_error_envelope(unknown, StatusCode::NOT_FOUND, 404, "NOT_FOUND").await?;

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("missing.model"));

    Ok(())
}

/// The GenerateContent dispatch renders the pinned upstream request — the URL expression
/// carrying the physical upstream id, the secret header — and renders the native candidate
/// envelope under the public id.
#[tokio::test]
async fn generate_content_dispatches_the_pinned_upstream_request() -> Result<(), Box<dyn StdError>>
{
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_gemini_json("Gemini says hi").to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_gemini_model(
        &gateway,
        gemini_generate_definition(),
        &mock.base(),
        "public.gemini",
        "physical-gemini",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            &generate_path("public.gemini"),
            &[goog_key(KEY_SECRET)],
            generate_request("Hello"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/modelVersion"),
        "public.gemini"
    );

    assert_eq!(
        crate::test_support::at(&body, "/candidates/0/content/parts/0/text"),
        "Gemini says hi"
    );

    assert_eq!(
        crate::test_support::at(&body, "/candidates/0/finishReason"),
        "STOP"
    );

    assert_eq!(
        crate::test_support::at(&body, "/usageMetadata/totalTokenCount"),
        &json!(7)
    );

    let recorded = lock_recordings(&requests);

    assert_eq!(recorded.len(), 1);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    // The URL expression substituted the physical upstream id.
    assert_eq!(
        dispatched.target,
        "/upstream/v1beta/models/physical-gemini:generateContent"
    );

    assert_eq!(dispatched.header("x-goog-api-key"), Some(UPSTREAM_SECRET));

    let rendered: Value = recorded_json(&dispatched.body);

    // The GenerateContent body carries no model member; the URL addressed it.
    assert!(rendered.get("model").is_none());

    assert_eq!(
        crate::test_support::at(&rendered, "/contents"),
        &json!([{"role": "user", "parts": [{"text": "Hello"}]}])
    );

    Ok(())
}

/// The streaming action streams the native candidate chunks as `text/event-stream`.
#[tokio::test]
async fn stream_generate_content_streams_native_chunks() -> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let chunk = json!({
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": "streamed"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 1, "totalTokenCount": 3}
    });

    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "{head}data: {chunk}\n\n"
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_gemini_model(
        &gateway,
        gemini_generate_definition(),
        &mock.base(),
        "public.gemini",
        "physical-gemini",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            &stream_path("public.gemini"),
            &[goog_key(KEY_SECRET)],
            generate_request("Hello"),
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

    assert!(body.contains("streamed"));

    assert!(body.contains("STOP"));

    Ok(())
}

/// A known unsupported semantic control rejects with the 400 the request boundary pins.
#[tokio::test]
async fn unsupported_controls_are_rejected_before_routing() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let rejected = gateway
        .post_json(
            &generate_path("any.model"),
            &[goog_key(KEY_SECRET)],
            json!({
                "contents": [{"role": "user", "parts": [{"text": "Hi"}]}],
                "generationConfig": {"topK": 3}
            }),
        )
        .await;

    let body =
        assert_error_envelope(rejected, StatusCode::BAD_REQUEST, 400, "INVALID_ARGUMENT").await?;

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("topK"));

    Ok(())
}

/// The model list renders the native Gemini shape with camelCase limits and no pagination
/// token.
#[tokio::test]
async fn the_model_list_renders_the_native_gemini_shape() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        gemini_generate_definition(),
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

    let model_id = seed_model(gateway.database(), provider_id, "up-g").await?;

    let _ = seed_route(
        gateway.database(),
        "a.gemini-model",
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway.get(MODELS_PATH, &[goog_key(KEY_SECRET)]).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    let models = crate::test_support::at(&body, "/models")
        .as_array()
        .ok_or_else(|| format!("expected a models array, got {body}"))?;

    // The authored alias and the provider's standalone automatic entry both list, ordered by
    // public id.
    let names: Vec<&str> = models
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .collect();

    assert_eq!(names, vec!["models/a.gemini-model", "models/up-g"]);

    // Unknown capability values are omitted, never fabricated.
    let Some(authored) = models.first() else {
        return Err(format!("expected entries, got {body}").into());
    };

    assert!(authored.get("inputTokenLimit").is_none());

    assert!(body.get("nextPageToken").is_none());

    Ok(())
}

/// The Interactions endpoint addresses its model in the body, dispatches the pinned upstream
/// request, and renders the native steps envelope.
#[tokio::test]
async fn interactions_dispatch_by_body_model_and_render_steps() -> Result<(), Box<dyn StdError>> {
    let upstream_response = json!({
        "id": "up-1",
        "model": "physical-interactions",
        "status": "completed",
        "steps": [{"type": "model_output", "content": [{"type": "text", "text": "Hi"}]}],
        "usage": {"total_input_tokens": 2, "total_output_tokens": 1}
    });

    let (mock, requests) = MockUpstream::start_json(vec![upstream_response.to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_gemini_model(
        &gateway,
        interactions_definition(),
        &mock.base(),
        "public.interactions",
        "physical-interactions",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            INTERACTIONS_PATH,
            &[goog_key(KEY_SECRET)],
            json!({"model": "public.interactions", "input": "Hello"}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(
        crate::test_support::at(&body, "/model"),
        "public.interactions"
    );

    assert_eq!(crate::test_support::at(&body, "/status"), "completed");

    assert_eq!(
        crate::test_support::at(&body, "/steps/0/content/0/text"),
        "Hi"
    );

    let recorded = lock_recordings(&requests);

    assert_eq!(recorded.len(), 1);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    assert_eq!(dispatched.target, "/upstream/v1/interactions");

    let rendered: Value = recorded_json(&dispatched.body);

    assert_eq!(
        crate::test_support::at(&rendered, "/model"),
        "physical-interactions"
    );

    Ok(())
}

/// The Interactions upstream definition: the body-routed model with the `x-goog-api-key`
/// credential header and the match-client streaming mode.
fn interactions_definition() -> Value {
    json!({
        "format": "gemini_interaction",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/upstream/v1/interactions"}],
            "headers": [{"name": "x-goog-api-key", "value": [{"type": "secret"}]}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    })
}

/// A transport failure without a usable HTTP response maps onto the upstream-unavailable 502 in
/// the Gemini envelope's canonical `UNAVAILABLE` name.
#[tokio::test]
async fn a_transport_failure_maps_to_the_unavailable_envelope() -> Result<(), Box<dyn StdError>> {
    let (mock, _requests) = MockUpstream::start(vec![MockAction::Close]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_gemini_model(
        &gateway,
        gemini_generate_definition(),
        &mock.base(),
        "public.gemini",
        "physical-gemini",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            &generate_path("public.gemini"),
            &[goog_key(KEY_SECRET)],
            generate_request("Hello"),
        )
        .await;

    assert_error_envelope(response, StatusCode::BAD_GATEWAY, 502, "UNAVAILABLE").await?;

    Ok(())
}

/// A rejected stateful-continuation control is the request boundary's 400, naming the member,
/// before any routing: the gateway never chains interactions.
#[tokio::test]
async fn interactions_reject_stateful_continuation() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let rejected = gateway
        .post_json(
            INTERACTIONS_PATH,
            &[goog_key(KEY_SECRET)],
            json!({
                "model": "any.model",
                "input": "Hello",
                "previous_interaction_id": "up-0"
            }),
        )
        .await;

    let body =
        assert_error_envelope(rejected, StatusCode::BAD_REQUEST, 400, "INVALID_ARGUMENT").await?;

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("previous_interaction_id"));

    Ok(())
}

/// A streaming Interactions request translates the upstream lifecycle into the native event
/// stream under `text/event-stream`: the created shell, the live step deltas, and the terminal
/// completed event, with the upstream addressed as a streaming request.
#[tokio::test]
async fn interactions_stream_the_native_lifecycle() -> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let created = json!({"interaction": {"id": "up-1", "model": "physical-interactions"}});

    let step_start = json!({"index": 0, "step": {"type": "model_output"}});

    let delta = json!({"index": 0, "delta": {"type": "text", "text": "streamed"}});

    let completed = json!({
        "interaction": {
            "id": "up-1",
            "status": "completed",
            "usage": {"total_input_tokens": 2, "total_output_tokens": 1}
        }
    });

    let (mock, requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "{head}event: interaction.created\ndata: {created}\n\nevent: step.start\ndata: \
         {step_start}\n\nevent: step.delta\ndata: {delta}\n\nevent: interaction.completed\n\
         data: {completed}\n\n"
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_gemini_model(
        &gateway,
        interactions_definition(),
        &mock.base(),
        "public.interactions",
        "physical-interactions",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            INTERACTIONS_PATH,
            &[goog_key(KEY_SECRET)],
            json!({"model": "public.interactions", "input": "Hello", "stream": true}),
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

    assert!(body.contains("event: interaction.created"));

    assert!(body.contains("streamed"));

    assert!(body.contains("event: interaction.completed"));

    // The outward identity stays the public id the client addressed.
    assert!(body.contains(r#""model":"public.interactions""#));

    // The upstream was addressed as a streaming request.
    let recorded = lock_recordings(&requests);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    let rendered: Value = recorded_json(&dispatched.body);

    assert_eq!(crate::test_support::at(&rendered, "/stream"), &json!(true));

    Ok(())
}

/// A pooled entry appears in the native list alongside direct declarations, ordered by public
/// id: the union composition the pool prefix owns.
#[tokio::test]
async fn pooled_entries_join_the_native_model_list() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        gemini_generate_definition(),
    )
    .await?;

    let member = seed_provider(
        gateway.database(),
        "member-account",
        "",
        "http://unused.test",
        &stored,
    )
    .await?;

    // The pool exists before the member's inventory: the discovery creates the pool's own
    // automatic public entry `beta.up-p`.
    let _pool_id = crate::api::tests_support::seed_pool(
        gateway.database(),
        "fixture-pool",
        "beta.",
        &[ProviderPoolMember {
            provider_id: member,
            priority: 1,
        }],
    )
    .await?;

    let _ = seed_model(gateway.database(), member, "up-p").await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway.get(MODELS_PATH, &[goog_key(KEY_SECRET)]).await;

    let body = body_json(response).await?;

    let names: Vec<&str> = crate::test_support::at(&body, "/models")
        .as_array()
        .ok_or_else(|| format!("expected a models array, got {body}"))?
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .collect();

    assert_eq!(names, vec!["models/beta.up-p"]);

    Ok(())
}
