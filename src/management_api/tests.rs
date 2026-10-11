//! The management API transport tests, driven through the real root router: the bearer-less
//! verbatim token convention, the authentication ordering against unmatched paths and methods,
//! the empty 404 outside every version, and the version's errno error envelope over every
//! transport-level rejection (`docs/reference/management-api.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions and unreadable fixtures; a \
              # Panics section on every test would restate that"
)]

use crate::{
    management_api::tests_support::{
        Authorization, TestGateway, VERSION_PREFIX, assert_error_envelope,
    },
    test_support,
};
use http::StatusCode;
use serde_json::{Value, json};
use std::error::Error as StdError;

/// The no-argument `providers:list` action path within the version prefix.
const LIST_PROVIDERS: &str = "/providers:list";

/// The complete empty request of a no-argument action.
fn empty_body() -> Value {
    json!({})
}

/// Management requests authenticate the verbatim configuration token only: a missing header, a
/// wrong token, and the `Bearer`-prefixed spelling the contract forbids all render the
/// unauthorized envelope, while the correct token's success is HTTP 200 JSON.
#[tokio::test]
async fn management_requests_authenticate_the_verbatim_token_only() -> Result<(), Box<dyn StdError>>
{
    let gateway = TestGateway::open().await;

    let path = format!("{VERSION_PREFIX}{LIST_PROVIDERS}");

    for credential in [
        Authorization::Absent,
        Authorization::WrongToken,
        Authorization::BearerPrefixed,
    ] {
        let response = gateway
            .post_management_with(&path, credential, empty_body())
            .await;

        let body = assert_error_envelope(&gateway, response, StatusCode::UNAUTHORIZED, 2).await?;

        assert_eq!(
            *test_support::at(&body, "/message"),
            "Missing or invalid management token",
            "{body}"
        );
    }

    let response = gateway.post_management(&path, empty_body()).await;

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get("content-type")
        .expect("the success content type")
        .to_str()?;

    assert_eq!(content_type, "application/json", "success is JSON");

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({"providers": []}), "an empty result answers []");

    Ok(())
}

/// Authentication precedes every other judgment inside the version: an unknown path and a wrong
/// method on a known path both render the unauthorized envelope without a token, while the
/// token-authorized forms render the version's unmapped-path 404 and method-not-allowed 405 with
/// its `Allow` header.
#[tokio::test]
async fn authentication_precedes_unmatched_paths_and_methods() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let unmapped = format!("{VERSION_PREFIX}/nothing:here");

    let known = format!("{VERSION_PREFIX}{LIST_PROVIDERS}");

    // Without a token, an unknown path is rejected before routing: 401, not 404.
    let response = gateway
        .post_management_with(&unmapped, Authorization::Absent, empty_body())
        .await;

    assert_error_envelope(&gateway, response, StatusCode::UNAUTHORIZED, 2).await?;

    // Without a token, a wrong method on a known path is rejected before routing: 401, not 405.
    let response = gateway
        .request(
            "GET",
            &known,
            &[("content-type", "application/json".to_owned())],
            None,
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::UNAUTHORIZED, 2).await?;

    // With the token, the unknown path renders the version's unmapped-path 404.
    let response = gateway
        .post_management_with(&unmapped, Authorization::Correct, empty_body())
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Invalid URL (POST /management-api/v0/nothing:here)",
        "{body}"
    );

    // With the token, the wrong method renders the version's 405, carrying the `Allow` header.
    let response = gateway
        .request(
            "GET",
            &known,
            &[("authorization", test_support::MANAGEMENT_TOKEN.to_owned())],
            None,
        )
        .await;

    let allow = response
        .headers()
        .get("allow")
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()?;

    let body = assert_error_envelope(&gateway, response, StatusCode::METHOD_NOT_ALLOWED, 8).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Invalid method (GET /management-api/v0/providers:list)",
        "{body}"
    );

    assert_eq!(
        allow.as_deref(),
        Some("POST"),
        "the registered method is advertised"
    );

    Ok(())
}

/// Paths under `/management-api` that belong to no version — the version prefixes themselves
/// included — and paths outside the management router altogether answer with the shared empty
/// 404, never an errno body.
#[tokio::test]
async fn paths_outside_every_version_render_the_empty_404() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    for path in [
        "/management-api/v1/providers:list",
        "/management-api",
        "/unrelated",
    ] {
        let response = gateway.post_management(path, empty_body()).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");

        assert!(
            gateway.body_text(response).await.is_empty(),
            "{path} answers an empty body"
        );
    }

    Ok(())
}

/// Unusable requests render the version's error envelope with the category each failure maps to:
/// a missing or non-JSON content type is 415, malformed JSON and unknown fields are 400, and a
/// body over the version's limit is 413 with the category's fixed message rather than any
/// internal cause.
#[tokio::test]
async fn unusable_requests_render_the_version_error_envelope() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let path = format!("{VERSION_PREFIX}{LIST_PROVIDERS}");

    let authorized = [("authorization", test_support::MANAGEMENT_TOKEN.to_owned())];

    // A JSON body with no Content-Type is 415.
    let response = gateway
        .request("POST", &path, &authorized, Some(empty_body().to_string()))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::UNSUPPORTED_MEDIA_TYPE, 9).await?;

    // A non-JSON Content-Type is 415 even when the body is well-formed JSON.
    let headers = [
        ("authorization", test_support::MANAGEMENT_TOKEN.to_owned()),
        ("content-type", "text/plain".to_owned()),
    ];

    let response = gateway
        .request("POST", &path, &headers, Some(empty_body().to_string()))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::UNSUPPORTED_MEDIA_TYPE, 9).await?;

    // Malformed JSON is 400.
    let response = gateway.post_management(&path, json!({})).await;

    assert_eq!(response.status(), StatusCode::OK);

    let malformed = "this is not json".to_owned();

    let headers = [
        ("authorization", test_support::MANAGEMENT_TOKEN.to_owned()),
        ("content-type", "application/json".to_owned()),
    ];

    let response = gateway
        .request("POST", &path, &headers, Some(malformed))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    // Unknown request fields are rejected: the payloads are strict.
    let response = gateway.post_management(&path, json!({"bogus": 1})).await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    // A body over the 2 MiB limit is 413 with the category's fixed message.
    let oversized = format!("{{\"fill\": \"{}\"}}", "a".repeat(3 * 1024 * 1024));

    let headers = [
        ("authorization", test_support::MANAGEMENT_TOKEN.to_owned()),
        ("content-type", "application/json".to_owned()),
    ];

    let response = gateway
        .request("POST", &path, &headers, Some(oversized))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::PAYLOAD_TOO_LARGE, 7).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "request body exceeds the management body limit",
        "{body}"
    );

    Ok(())
}
