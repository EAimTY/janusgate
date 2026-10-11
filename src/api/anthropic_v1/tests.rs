//! The Anthropic surface tests, driven end to end through the real `/api` router against the
//! mock upstream: the `x-api-key` plus required-present `anthropic-version` convention and its
//! rejection envelope, the Messages dispatch with its pinned upstream request, the streamed
//! message lifecycle, the native model-list page, the unsupported-control and missing-cap
//! rejections, and the upstream rejection mirrored through the official type table.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section would \
              document nothing its name does not"
)]

use crate::{
    api::tests_support::{
        KEY_SECRET, TestGateway, UPSTREAM_SECRET, anthropic_headers, body_json, body_text,
        lock_recordings, recorded_json, seed_allowing_key, seed_definition, seed_model,
        seed_provider, seed_route, upstream_anthropic_json,
    },
    routing::{ModelRouteTarget, ProviderPoolMember},
    test_support::mock_upstream::{MockAction, MockUpstream},
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::error::Error as StdError;

/// The Messages endpoint path of this surface.
const MESSAGES_PATH: &str = "/anthropic-v1/messages";

/// The model-listing endpoint path of this surface.
const MODELS_PATH: &str = "/anthropic-v1/models";

/// A one-turn Messages request body for `public_id`.
fn messages_request(public_id: &str) -> Value {
    json!({
        "model": public_id,
        "max_tokens": 100,
        "messages": [{"role": "user", "content": "Hello"}]
    })
}

/// The Messages upstream definition: the `x-api-key` credential header, a vendor beta row, and
/// the match-client streaming mode over the base URL.
fn anthropic_definition() -> Value {
    json!({
        "format": "anthropic_messages",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/upstream/v1/messages"}],
            "headers": [
                {"name": "x-api-key", "value": [{"type": "secret"}]},
                {"name": "anthropic-version", "value": [{"type": "literal", "value": "2023-06-01"}]}
            ],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    })
}

/// Seeds one provider exposing `upstream_id` through the direct public route `public_id`,
/// upstreaming to `base_url` over the Messages definition.
async fn seed_anthropic_model(
    gateway: &TestGateway,
    base_url: &str,
    public_id: &str,
    upstream_id: &str,
) -> Result<(), Box<dyn StdError>> {
    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        anthropic_definition(),
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

/// Asserts the Anthropic error envelope of a rejected response and returns the parsed body.
async fn assert_error_envelope(
    response: axum::response::Response,
    expected_status: StatusCode,
    expected_type: &str,
) -> Result<Value, Box<dyn StdError>> {
    assert_eq!(response.status(), expected_status);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/type"), "error", "{body}");

    assert_eq!(
        crate::test_support::at(&body, "/error/type"),
        expected_type,
        "{body}"
    );

    Ok(body)
}

/// The credential convention reads `x-api-key` only and requires the `anthropic-version`
/// header present — its value never validated — before any key comparison.
#[tokio::test]
async fn credentials_require_the_version_header_but_never_its_value()
-> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    // A missing key is the invalid-key rejection in the Anthropic envelope.
    let missing = gateway
        .post_json(
            MESSAGES_PATH,
            &[("anthropic-version", "2023-06-01".to_owned())],
            messages_request("m"),
        )
        .await;

    let body =
        assert_error_envelope(missing, StatusCode::UNAUTHORIZED, "authentication_error").await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "Incorrect API key provided."
    );

    // An unknown key rejects.
    let unknown = gateway
        .post_json(
            MESSAGES_PATH,
            &anthropic_headers("sk-unknown"),
            messages_request("m"),
        )
        .await;

    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

    // A key without the version header is the convention's own 400 before the key comparison.
    let no_version = gateway
        .post_json(
            MESSAGES_PATH,
            &[("x-api-key", KEY_SECRET.to_owned())],
            messages_request("m"),
        )
        .await;

    let body =
        assert_error_envelope(no_version, StatusCode::BAD_REQUEST, "invalid_request_error").await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "The anthropic-version header is required"
    );

    // Any version value is accepted: the header is required present, never validated.
    let odd_version = gateway
        .get(
            MODELS_PATH,
            &[
                ("x-api-key", KEY_SECRET.to_owned()),
                ("anthropic-version", "not-a-real-version".to_owned()),
            ],
        )
        .await;

    assert_eq!(odd_version.status(), StatusCode::OK);

    Ok(())
}

/// The Messages dispatch renders the pinned upstream request — the physical model id, the
/// vendor headers, the synthesized output cap absent from the client request — and renders
/// the native message envelope under the public id.
#[tokio::test]
async fn messages_dispatches_the_pinned_upstream_request() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_anthropic_json("From upstream").to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_anthropic_model(
        &gateway,
        &mock.base(),
        "public.anthropic",
        "physical-anthropic",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            MESSAGES_PATH,
            &anthropic_headers(KEY_SECRET),
            messages_request("public.anthropic"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    assert_eq!(crate::test_support::at(&body, "/type"), "message");

    assert_eq!(crate::test_support::at(&body, "/model"), "public.anthropic");

    assert_eq!(
        crate::test_support::at(&body, "/content/0/text"),
        "From upstream"
    );

    assert_eq!(crate::test_support::at(&body, "/stop_reason"), "end_turn");

    assert_eq!(
        crate::test_support::at(&body, "/usage/input_tokens"),
        &json!(5)
    );

    let recorded = lock_recordings(&requests);

    assert_eq!(recorded.len(), 1);

    let dispatched = recorded
        .first()
        .ok_or_else(|| "the mock recorded one request".to_string())?;

    assert_eq!(dispatched.target, "/upstream/v1/messages");

    assert_eq!(dispatched.header("x-api-key"), Some(UPSTREAM_SECRET));

    let rendered: Value = recorded_json(&dispatched.body);

    // The upstream receives the physical id and the client's own output cap.
    assert_eq!(
        crate::test_support::at(&rendered, "/model"),
        "physical-anthropic"
    );

    assert_eq!(
        crate::test_support::at(&rendered, "/max_tokens"),
        &json!(100)
    );

    assert_eq!(
        crate::test_support::at(&rendered, "/messages"),
        &json!([{"role": "user", "content": [{"type": "text", "text": "Hello"}]}])
    );

    Ok(())
}

/// A streaming client receives the native message lifecycle over `text/event-stream`.
#[tokio::test]
async fn streaming_clients_receive_the_message_lifecycle() -> Result<(), Box<dyn StdError>> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    let start = json!({
        "type": "message_start",
        "message": {"id": "msg-upstream", "type": "message", "role": "assistant",
            "model": "physical-anthropic", "content": [], "usage": {"input_tokens": 3}}
    });

    let block_start = json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "text", "text": ""}
    });

    let delta = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "text_delta", "text": "streamed"}
    });

    let stop = json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 1}
    });

    let done = json!({"type": "message_stop"});

    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "{head}event: message_start\ndata: {start}\n\nevent: content_block_start\ndata: \
         {block_start}\n\nevent: content_block_delta\ndata: {delta}\n\nevent: message_delta\n\
         data: {stop}\n\nevent: message_stop\ndata: {done}\n\n"
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_anthropic_model(
        &gateway,
        &mock.base(),
        "public.anthropic",
        "physical-anthropic",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            MESSAGES_PATH,
            &anthropic_headers(KEY_SECRET),
            json!({
                "model": "public.anthropic",
                "max_tokens": 100,
                "stream": true,
                "messages": [{"role": "user", "content": "Hello"}]
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

    assert!(body.contains("event: message_start"));

    assert!(body.contains("streamed"));

    assert!(body.contains("event: message_stop"));

    // The outward model name stays the public id the client addressed.
    assert!(body.contains(r#""model":"public.anthropic""#));

    Ok(())
}

/// The model list renders the native one-page Anthropic shape with epoch timestamps and the
/// page bookends.
#[tokio::test]
async fn the_model_list_renders_the_native_anthropic_page() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        anthropic_definition(),
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

    let model_id = seed_model(gateway.database(), provider_id, "up-m").await?;

    let _ = seed_route(
        gateway.database(),
        "a.anthropic-model",
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .get(
            MODELS_PATH,
            &[
                ("x-api-key", KEY_SECRET.to_owned()),
                ("anthropic-version", "2023-06-01".to_owned()),
            ],
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await?;

    let entries = crate::test_support::at(&body, "/data")
        .as_array()
        .ok_or_else(|| format!("expected a data array, got {body}"))?;

    // The authored alias and the provider's standalone automatic entry both list, ordered by
    // public id.
    let ids: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();

    assert_eq!(ids, vec!["a.anthropic-model", "up-m"]);

    let Some(authored) = entries.first() else {
        return Err(format!("expected entries, got {body}").into());
    };

    assert_eq!(crate::test_support::at(authored, "/type"), "model");

    assert_eq!(
        crate::test_support::at(authored, "/display_name"),
        "a.anthropic-model"
    );

    assert_eq!(
        crate::test_support::at(authored, "/created_at"),
        "1970-01-01T00:00:00Z"
    );

    assert_eq!(
        crate::test_support::at(&body, "/first_id"),
        "a.anthropic-model"
    );

    assert_eq!(crate::test_support::at(&body, "/last_id"), "up-m");

    assert_eq!(crate::test_support::at(&body, "/has_more"), &json!(false));

    Ok(())
}

/// The Messages output cap is required per its protocol: its absence is the 400 the request
/// boundary pins, before routing.
#[tokio::test]
async fn a_missing_output_cap_is_rejected() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let rejected = gateway
        .post_json(
            MESSAGES_PATH,
            &anthropic_headers(KEY_SECRET),
            json!({
                "model": "any.model",
                "messages": [{"role": "user", "content": "Hello"}]
            }),
        )
        .await;

    assert_error_envelope(rejected, StatusCode::BAD_REQUEST, "invalid_request_error").await?;

    Ok(())
}

/// A known unsupported semantic control rejects with the 400 the request boundary pins, naming
/// the offending member, before any routing or dispatch.
#[tokio::test]
async fn an_unsupported_semantic_control_is_rejected() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    seed_allowing_key(gateway.database()).await?;

    let rejected = gateway
        .post_json(
            MESSAGES_PATH,
            &anthropic_headers(KEY_SECRET),
            json!({
                "model": "any.model",
                "max_tokens": 100,
                "top_k": 3,
                "messages": [{"role": "user", "content": "Hello"}]
            }),
        )
        .await;

    let body =
        assert_error_envelope(rejected, StatusCode::BAD_REQUEST, "invalid_request_error").await?;

    assert_eq!(
        crate::test_support::at(&body, "/error/message"),
        "top_k is not supported"
    );

    Ok(())
}

/// A rejected upstream request mirrors its non-2xx status through the Anthropic envelope's
/// official type table: a 429 renders the `rate_limit_error` row.
#[tokio::test]
async fn an_upstream_rejection_mirrors_through_the_anthropic_envelope()
-> Result<(), Box<dyn StdError>> {
    let rejection =
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}});

    let (mock, _requests) = MockUpstream::start(vec![MockAction::Respond(format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{}",
        rejection.to_string().len(),
        rejection
    ))])
    .await;

    let (gateway, _guard) = TestGateway::open().await;

    seed_anthropic_model(
        &gateway,
        &mock.base(),
        "public.anthropic",
        "physical-anthropic",
    )
    .await?;

    seed_allowing_key(gateway.database()).await?;

    let response = gateway
        .post_json(
            MESSAGES_PATH,
            &anthropic_headers(KEY_SECRET),
            messages_request("public.anthropic"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let body =
        assert_error_envelope(response, StatusCode::TOO_MANY_REQUESTS, "rate_limit_error").await?;

    let message = crate::test_support::at(&body, "/error/message")
        .as_str()
        .ok_or_else(|| format!("expected a message member, got {body}"))?;

    assert!(message.contains("slow down"));

    Ok(())
}

/// A pooled entry joins the native page beside direct declarations, ordered by public id.
#[tokio::test]
async fn pooled_entries_join_the_native_page() -> Result<(), Box<dyn StdError>> {
    let (gateway, _guard) = TestGateway::open().await;

    let stored = seed_definition(
        gateway.database(),
        "fixture-definition",
        anthropic_definition(),
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

    let response = gateway
        .get(
            MODELS_PATH,
            &[
                ("x-api-key", KEY_SECRET.to_owned()),
                ("anthropic-version", "2023-06-01".to_owned()),
            ],
        )
        .await;

    let body = body_json(response).await?;

    let ids: Vec<&str> = crate::test_support::at(&body, "/data")
        .as_array()
        .ok_or_else(|| format!("expected a data array, got {body}"))?
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();

    assert_eq!(ids, vec!["beta.up-p"]);

    Ok(())
}
