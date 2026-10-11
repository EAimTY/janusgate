//! The outward error-domain tests: the HTTP status each contract row pins, the three wire
//! renders' envelope shapes, the accounting classification, and the one-place domain
//! conversions from the gateway, storage, and provider failure vocabularies.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::Error;
use crate::{
    api::error::{anthropic, gemini, openai},
    gateway::Error as GatewayError,
    ir::InvalidRequest,
    provider::Error as ProviderError,
    storage::Error as StorageError,
};
use axum::http::StatusCode;
use std::error::Error as StdError;

/// A malformed-request rejection naming `temperature`.
fn malformed() -> Error {
    Error::MalformedRequest {
        param: Some("temperature".to_owned()),
        message: "unsupported control".to_owned(),
    }
}

/// A mirrored upstream rejection carrying `status` and a bounded body excerpt.
fn upstream(status: StatusCode) -> Error {
    Error::Upstream {
        status,
        message: "upstream said no".to_owned(),
    }
}

/// Every variant pins the HTTP status its contract row specifies.
#[test]
fn http_status_pins_the_contract_rows() {
    let cases: Vec<(Error, StatusCode)> = vec![
        (Error::InvalidApiKey, StatusCode::UNAUTHORIZED),
        (
            Error::InvalidUrl {
                method: "POST".to_owned(),
                path: "/nope".to_owned(),
            },
            StatusCode::NOT_FOUND,
        ),
        (Error::MethodNotAllowed, StatusCode::METHOD_NOT_ALLOWED),
        (Error::PayloadTooLarge, StatusCode::PAYLOAD_TOO_LARGE),
        (malformed(), StatusCode::BAD_REQUEST),
        (
            Error::ModelNotFound {
                model: "m".to_owned(),
            },
            StatusCode::NOT_FOUND,
        ),
        (
            Error::PoolUnavailable {
                model: "m".to_owned(),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            Error::PoolQuotaExhausted {
                model: "m".to_owned(),
                retry_after: None,
            },
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            Error::BoundAccountUnavailable {
                model: "m".to_owned(),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            Error::BoundAccountQuotaExhausted {
                model: "m".to_owned(),
                retry_after: None,
            },
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            Error::RoutingStateConflict {
                model: "m".to_owned(),
            },
            StatusCode::BAD_REQUEST,
        ),
        (
            upstream(StatusCode::UNPROCESSABLE_ENTITY),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            Error::UpstreamUnavailable {
                message: "down".to_owned(),
            },
            StatusCode::BAD_GATEWAY,
        ),
        (Error::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ];

    for (error, expected) in cases {
        let (status, _) = openai::render(&error);

        assert_eq!(status, expected, "{error:?} pins the wrong status");
    }
}

/// The OpenAI render keeps the `{error: {message, type, param, code}}` envelope with the stable
/// type/code vocabulary and the param passthrough.
#[test]
fn openai_render_uses_the_stable_code_vocabulary() {
    let envelope_of = |error: &Error| {
        let (_, envelope) = openai::render(error);

        #[expect(
            clippy::expect_used,
            reason = "an envelope that does not serialize is the test's failure report"
        )]
        serde_json::to_value(&envelope).expect("the openai envelope serializes")
    };

    let invalid_key = envelope_of(&Error::InvalidApiKey);

    assert_eq!(
        crate::test_support::at(&invalid_key, "/error/type"),
        "invalid_request_error"
    );

    assert_eq!(
        crate::test_support::at(&invalid_key, "/error/code"),
        "invalid_api_key"
    );

    assert_eq!(
        crate::test_support::at(&invalid_key, "/error/message"),
        "Incorrect API key provided."
    );

    let not_found = envelope_of(&Error::ModelNotFound {
        model: "nope".to_owned(),
    });

    assert_eq!(
        crate::test_support::at(&not_found, "/error/code"),
        "model_not_found"
    );

    assert_eq!(crate::test_support::at(&not_found, "/error/param"), "model");

    let pool_quota = envelope_of(&Error::PoolQuotaExhausted {
        model: "m".to_owned(),
        retry_after: None,
    });

    assert_eq!(
        crate::test_support::at(&pool_quota, "/error/type"),
        "rate_limit_error"
    );

    assert_eq!(
        crate::test_support::at(&pool_quota, "/error/code"),
        "pool_quota_exhausted"
    );

    let mirrored = envelope_of(&upstream(StatusCode::TOO_MANY_REQUESTS));

    assert_eq!(
        crate::test_support::at(&mirrored, "/error/type"),
        "api_error"
    );

    assert_eq!(
        crate::test_support::at(&mirrored, "/error/code"),
        "upstream_error"
    );

    let internal = envelope_of(&Error::Internal);

    assert_eq!(
        crate::test_support::at(&internal, "/error/code"),
        "internal"
    );

    assert_eq!(
        crate::test_support::at(&internal, "/error/message"),
        "Internal server error."
    );

    let named = envelope_of(&malformed());

    assert_eq!(
        crate::test_support::at(&named, "/error/param"),
        "temperature"
    );
}

/// The Gemini render maps the gateway's own failures onto the canonical code names and every
/// mirrored upstream status onto its canonical or closest-semantic name.
#[test]
fn gemini_render_maps_statuses_onto_canonical_names() {
    let body_of = |error: &Error| {
        let (_, envelope) = gemini::render(error);

        #[expect(
            clippy::expect_used,
            reason = "an envelope that does not serialize is the test's failure report"
        )]
        serde_json::to_value(&envelope).expect("the gemini envelope serializes")
    };

    let invalid_key = body_of(&Error::InvalidApiKey);

    assert_eq!(
        crate::test_support::at(&invalid_key, "/error/code"),
        &serde_json::json!(401)
    );

    assert_eq!(
        crate::test_support::at(&invalid_key, "/error/status"),
        "UNAUTHENTICATED"
    );

    let not_found = body_of(&Error::ModelNotFound {
        model: "m".to_owned(),
    });

    assert_eq!(
        crate::test_support::at(&not_found, "/error/code"),
        &serde_json::json!(404)
    );

    assert_eq!(
        crate::test_support::at(&not_found, "/error/status"),
        "NOT_FOUND"
    );

    // Method, body limit, and malformed requests share INVALID_ARGUMENT; the numeric code
    // distinguishes their HTTP statuses.
    for error in [Error::MethodNotAllowed, Error::PayloadTooLarge, malformed()] {
        let body = body_of(&error);

        assert_eq!(
            crate::test_support::at(&body, "/error/status"),
            "INVALID_ARGUMENT",
            "{error:?}"
        );
    }

    let pool_quota = body_of(&Error::PoolQuotaExhausted {
        model: "m".to_owned(),
        retry_after: None,
    });

    assert_eq!(
        crate::test_support::at(&pool_quota, "/error/code"),
        &serde_json::json!(429)
    );

    assert_eq!(
        crate::test_support::at(&pool_quota, "/error/status"),
        "RESOURCE_EXHAUSTED"
    );

    let pool_unavailable = body_of(&Error::PoolUnavailable {
        model: "m".to_owned(),
    });

    assert_eq!(
        crate::test_support::at(&pool_unavailable, "/error/status"),
        "UNAVAILABLE"
    );

    // Mirrored statuses take their canonical rows where the mapping defines one.
    for (status, name) in [
        (StatusCode::BAD_REQUEST, "INVALID_ARGUMENT"),
        (StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"),
        (StatusCode::NOT_FOUND, "NOT_FOUND"),
        (StatusCode::CONFLICT, "ABORTED"),
        (StatusCode::TOO_MANY_REQUESTS, "RESOURCE_EXHAUSTED"),
        (StatusCode::NOT_IMPLEMENTED, "UNIMPLEMENTED"),
        (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE"),
        (StatusCode::GATEWAY_TIMEOUT, "DEADLINE_EXCEEDED"),
    ] {
        let body = body_of(&upstream(status));

        assert_eq!(
            crate::test_support::at(&body, "/error/status"),
            name,
            "mirrored {status}"
        );

        assert_eq!(
            &serde_json::json!(u16::from(status)),
            crate::test_support::at(&body, "/error/code"),
            "mirrored {status}"
        );
    }

    // The extension rows: the closest semantic member where the canonical mapping has none.
    for (status, name) in [
        (StatusCode::REQUEST_TIMEOUT, "DEADLINE_EXCEEDED"),
        (StatusCode::RANGE_NOT_SATISFIABLE, "OUT_OF_RANGE"),
        (StatusCode::BAD_GATEWAY, "UNAVAILABLE"),
        // And UNKNOWN where the vocabulary has no member.
        (StatusCode::IM_A_TEAPOT, "UNKNOWN"),
    ] {
        let body = body_of(&upstream(status));

        assert_eq!(
            crate::test_support::at(&body, "/error/status"),
            name,
            "mirrored {status}"
        );
    }

    let internal = body_of(&Error::Internal);

    assert_eq!(
        crate::test_support::at(&internal, "/error/status"),
        "INTERNAL"
    );

    let unavailable = body_of(&Error::UpstreamUnavailable {
        message: "down".to_owned(),
    });

    assert_eq!(
        crate::test_support::at(&unavailable, "/error/code"),
        &serde_json::json!(502)
    );

    assert_eq!(
        crate::test_support::at(&unavailable, "/error/status"),
        "UNAVAILABLE"
    );
}

/// The Anthropic render maps each status family onto the official error-type table, including
/// the mirrored rows and the overloaded extension.
#[test]
fn anthropic_render_maps_statuses_onto_the_official_type_table() {
    let body_of = |error: &Error| {
        let (_, envelope) = anthropic::render(error);

        #[expect(
            clippy::expect_used,
            reason = "an envelope that does not serialize is the test's failure report"
        )]
        let value = serde_json::to_value(&envelope).expect("the anthropic envelope serializes");

        assert_eq!(crate::test_support::at(&value, "/type"), "error");

        crate::test_support::at(&value, "/error").clone()
    };

    assert_eq!(
        crate::test_support::at(&body_of(&Error::InvalidApiKey), "/type"),
        "authentication_error"
    );

    assert_eq!(
        crate::test_support::at(
            &body_of(&Error::ModelNotFound {
                model: "m".to_owned()
            }),
            "/type"
        ),
        "not_found_error"
    );

    assert_eq!(
        crate::test_support::at(
            &body_of(&Error::InvalidUrl {
                method: "GET".to_owned(),
                path: "/x".to_owned(),
            }),
            "/type"
        ),
        "not_found_error"
    );

    assert_eq!(
        crate::test_support::at(&body_of(&Error::MethodNotAllowed), "/type"),
        "invalid_request_error"
    );

    assert_eq!(
        crate::test_support::at(&body_of(&malformed()), "/type"),
        "invalid_request_error"
    );

    assert_eq!(
        crate::test_support::at(
            &body_of(&Error::RoutingStateConflict {
                model: "m".to_owned(),
            }),
            "/type"
        ),
        "invalid_request_error"
    );

    assert_eq!(
        crate::test_support::at(&body_of(&Error::PayloadTooLarge), "/type"),
        "request_too_large"
    );

    assert_eq!(
        crate::test_support::at(
            &body_of(&Error::PoolQuotaExhausted {
                model: "m".to_owned(),
                retry_after: None,
            }),
            "/type"
        ),
        "rate_limit_error"
    );

    // Mirrored statuses take their own rows.
    for (status, kind) in [
        (StatusCode::BAD_REQUEST, "invalid_request_error"),
        (StatusCode::METHOD_NOT_ALLOWED, "invalid_request_error"),
        (StatusCode::UNAUTHORIZED, "authentication_error"),
        (StatusCode::NOT_FOUND, "not_found_error"),
        (StatusCode::CONFLICT, "conflict_error"),
        (StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error"),
        (StatusCode::GATEWAY_TIMEOUT, "timeout_error"),
    ] {
        assert_eq!(
            crate::test_support::at(&body_of(&upstream(status)), "/type"),
            kind,
            "mirrored {status}"
        );
    }

    // The extension row: mirrored 503 renders overloaded_error where the table has no row.
    assert_eq!(
        crate::test_support::at(
            &body_of(&upstream(StatusCode::SERVICE_UNAVAILABLE)),
            "/type"
        ),
        "overloaded_error"
    );

    // The degradation rows: statuses with neither a row nor a closer member, the pool
    // unavailability rows, and the gateway's own failures render api_error.
    for error in [
        upstream(StatusCode::BAD_GATEWAY),
        upstream(StatusCode::IM_A_TEAPOT),
        Error::PoolUnavailable {
            model: "m".to_owned(),
        },
        Error::BoundAccountUnavailable {
            model: "m".to_owned(),
        },
        Error::UpstreamUnavailable {
            message: "down".to_owned(),
        },
        Error::Internal,
    ] {
        assert_eq!(
            crate::test_support::at(&body_of(&error), "/type"),
            "api_error",
            "{error:?}"
        );
    }

    assert_eq!(
        crate::test_support::at(&body_of(&Error::Internal), "/message"),
        "Internal server error."
    );
}

/// The accounting classification derives from the same variant table: client faults record
/// `completed` under their pinned status, upstream failures `upstream_error`, and internal
/// failures `internal`.
#[test]
fn accounting_classifies_each_failure_family() {
    let client_faults: Vec<(Error, i64)> = vec![
        (Error::InvalidApiKey, 401),
        (
            Error::InvalidUrl {
                method: "POST".to_owned(),
                path: "/x".to_owned(),
            },
            404,
        ),
        (Error::MethodNotAllowed, 405),
        (Error::PayloadTooLarge, 413),
        (malformed(), 400),
        (
            Error::ModelNotFound {
                model: "m".to_owned(),
            },
            404,
        ),
        (
            Error::RoutingStateConflict {
                model: "m".to_owned(),
            },
            400,
        ),
    ];

    for (error, status) in client_faults {
        let (outcome, recorded) = error.accounting();

        assert_eq!(outcome, crate::accounting::ApiLogOutcome::Completed);

        assert_eq!(recorded, status);
    }

    let upstream_failures: Vec<Error> = vec![
        upstream(StatusCode::UNPROCESSABLE_ENTITY),
        Error::QuotaRejected {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "exhausted".to_owned(),
        },
        Error::UpstreamUnavailable {
            message: "down".to_owned(),
        },
        Error::PoolUnavailable {
            model: "m".to_owned(),
        },
        Error::PoolQuotaExhausted {
            model: "m".to_owned(),
            retry_after: None,
        },
        Error::BoundAccountUnavailable {
            model: "m".to_owned(),
        },
        Error::BoundAccountQuotaExhausted {
            model: "m".to_owned(),
            retry_after: None,
        },
    ];

    for error in upstream_failures {
        let (outcome, _) = error.accounting();

        assert_eq!(
            outcome,
            crate::accounting::ApiLogOutcome::UpstreamError,
            "{error:?}"
        );
    }

    let (outcome, status) = Error::Internal.accounting();

    assert_eq!(outcome, crate::accounting::ApiLogOutcome::Internal);

    assert_eq!(status, 500);
}

/// Gateway failures convert once, here: typed quota variants keep their identity, the mirrored
/// upstream body truncates to the bounded excerpt, and internal drift stays internal.
#[test]
fn gateway_failures_convert_into_the_contract_taxonomy() -> Result<(), Box<dyn StdError>> {
    assert!(matches!(
        Error::from(GatewayError::ModelNotFound {
            model: "m".to_owned(),
        }),
        Error::ModelNotFound { model } if model == "m"
    ));

    #[expect(clippy::expect_used, reason = "the fixture delay is a nonzero literal")]
    let delay = std::num::NonZeroU64::new(30).expect("nonzero literal");

    assert!(matches!(
        Error::from(GatewayError::PoolQuotaExhausted {
            model: "m".to_owned(),
            retry_after: Some(delay),
        }),
        Error::PoolQuotaExhausted { retry_after: Some(seconds), .. } if seconds.get() == 30
    ));

    let rejected = Error::from(GatewayError::InvalidRequest(InvalidRequest::field(
        "stream", "nope",
    )));

    let Error::MalformedRequest { param, message } = rejected else {
        return Err("an invalid request converts into MalformedRequest".into());
    };

    assert_eq!(param.as_deref(), Some("stream"));

    assert!(message.contains("nope"));

    // The mirrored body truncates to the bounded excerpt the outward wire carries.
    let detail = "x".repeat(5_000);
    let converted = Error::from(GatewayError::Upstream {
        status: StatusCode::BAD_GATEWAY,
        detail: detail.clone(),
    });

    let Error::Upstream { status, message } = converted else {
        return Err("an upstream failure converts into Upstream".into());
    };

    assert_eq!(status, StatusCode::BAD_GATEWAY);

    assert!(message.len() < detail.len());

    let Some(prefix) = detail.get(..100) else {
        return Err("a 5000-character body has a 100-character prefix".into());
    };

    assert!(message.starts_with(prefix));

    assert!(matches!(
        Error::from(GatewayError::Unavailable {
            detail: "down".to_owned(),
        }),
        Error::UpstreamUnavailable { .. }
    ));

    assert!(matches!(
        Error::from(GatewayError::Internal {
            message: "drift".to_owned(),
        }),
        Error::Internal
    ));

    Ok(())
}

/// Storage and provider failures classify once through their own conversions: an inactive key is
/// the invalid-key rejection, every other storage failure internal, a provider rejection mirrors
/// its valid status, and an invalid mirrored status degrades to a transport failure.
#[test]
fn storage_and_provider_failures_convert_into_the_contract_taxonomy()
-> Result<(), Box<dyn StdError>> {
    assert!(matches!(
        Error::from(StorageError::InactiveApiKey { id: 7 }),
        Error::InvalidApiKey
    ));

    assert!(matches!(
        Error::from(StorageError::NotFound {
            message: "gone".to_owned(),
        }),
        Error::Internal
    ));

    let mirrored = Error::from(ProviderError::Rejected {
        status: 429,
        detail: "slow down".to_owned(),
    });

    let Error::Upstream { status, message } = mirrored else {
        return Err("a provider rejection converts into Upstream".into());
    };

    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    assert_eq!(message, "slow down");

    assert!(matches!(
        Error::from(ProviderError::Rejected {
            status: 1000,
            detail: "nonsense".to_owned(),
        }),
        Error::UpstreamUnavailable { .. }
    ));

    assert!(matches!(
        Error::from(ProviderError::Unavailable {
            detail: "unreachable".to_owned(),
        }),
        Error::UpstreamUnavailable { .. }
    ));

    assert!(matches!(
        Error::from(ProviderError::Timeout {
            detail: "timed out".to_owned(),
        }),
        Error::UpstreamUnavailable { .. }
    ));

    assert!(matches!(
        Error::from(ProviderError::NotFound {
            message: "vanished".to_owned(),
        }),
        Error::Internal
    ));

    Ok(())
}

/// The display messages name the request context the contract specifies.
#[test]
fn displays_name_the_request_context() {
    let invalid_url = Error::InvalidUrl {
        method: "PATCH".to_owned(),
        path: "/api/openai-v1/nope".to_owned(),
    };

    assert_eq!(
        invalid_url.to_string(),
        "Invalid URL (PATCH /api/openai-v1/nope)"
    );

    assert_eq!(
        Error::ModelNotFound {
            model: "m".to_owned(),
        }
        .to_string(),
        "The model `m` does not exist or you do not have access to it."
    );

    assert_eq!(Error::MethodNotAllowed.to_string(), "Method not allowed.");

    assert_eq!(
        Error::PayloadTooLarge.to_string(),
        "Request body too large."
    );
}
