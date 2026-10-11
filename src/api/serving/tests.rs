//! The shared outward serving-pipeline tests: the response-level policy glue that has no surface
//! of its own — the stable-code Retry-After header of the pool/bound quota-exhaustion errors.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{Error, error_response};
use crate::api::error::openai;
use axum::http::StatusCode;
use serde::Serialize;

/// The OpenAI surface identity the shared-policy tests render through: the OpenAI error
/// envelope, exactly like the real surface's marker.
struct Surface;

impl super::Surface for Surface {
    fn render_error(error: &Error) -> (StatusCode, impl Serialize) {
        openai::render(error)
    }

    fn extract_credential(
        _request: &axum::extract::Request,
    ) -> Result<String, super::AuthRejection> {
        Err(super::AuthRejection::MissingCredential)
    }
}

/// A pool quota exhaustion with complete future reset evidence renders the positive whole
/// seconds it computed as its `Retry-After` header.
#[test]
fn proven_pool_resets_render_the_retry_after_header() {
    #[expect(clippy::expect_used, reason = "the fixture delay is a nonzero literal")]
    let delay = std::num::NonZeroU64::new(42).expect("nonzero literal");

    let response = error_response::<Surface>(Error::PoolQuotaExhausted {
        model: "pub.m".to_owned(),
        retry_after: Some(delay),
    });

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let header = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok());

    assert_eq!(header, Some("42"));
}

/// A bound-account quota exhaustion without complete reset evidence carries no `Retry-After`
/// header: recheck, never proven replenishment, is never advertised.
#[test]
fn unproven_bound_resets_render_no_retry_after_header() {
    let response = error_response::<Surface>(Error::BoundAccountQuotaExhausted {
        model: "pub.m".to_owned(),
        retry_after: None,
    });

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    assert!(
        !response
            .headers()
            .contains_key(axum::http::header::RETRY_AFTER)
    );
}

/// An error without a retry dimension carries no `Retry-After` header.
#[test]
fn unrelated_errors_render_no_retry_after_header() {
    let response = error_response::<Surface>(Error::ModelNotFound {
        model: "pub.m".to_owned(),
    });

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    assert!(
        !response
            .headers()
            .contains_key(axum::http::header::RETRY_AFTER)
    );
}
