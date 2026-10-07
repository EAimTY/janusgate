//! Gemini GenerateContent endpoints (`/api/gemini-v1beta/models/...`).
//!
//! Implements `GET /models`, `POST /models/{model}:generateContent` (non-streaming), and
//! `POST /models/{model}:streamGenerateContent?alt=sse` (streaming — only the `alt=sse` form
//! is supported, else 400) per README §§6.3–6.6.
//!
//! Routing notes: axum 0.8 does not support partial-segment parameters such as
//! `/models/{model}:generateContent` — constructing such a route panics with "Only one
//! parameter is allowed per path segment" (verified with a build-time probe against axum
//! 0.8.9; see [`router`] tests). The endpoints therefore share one wildcard route,
//! `/models/{*rest}`, whose capture [`WildcardCapture`] extractor resolves every
//! URL-determined request fact — the capture parsed by [`parse_model_action`], and the
//! streaming action's `alt=sse` query convention — ahead of the body extractor, so a
//! URL-determined rejection never uploads the body first; malformed paths return the same
//! Gemini 404 the prefix fallback renders, so the observable behavior matches a native
//! partial-segment router.
//!
//! Shared concerns (authentication, 404/405 bodies, body limit, JSON helper, accounting row
//! id, completion pipeline) live in [`crate::api::serving`] and the prefix module [`super`];
//! this module owns only the three endpoint handlers, request parsing through
//! [`crate::format::gemini::generate_content`], and the native model-listing shape.

use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, log_error, serving},
    compaction_model::support_facts::CompactionSupportFacts,
    context::Context,
    format::{RouteContext, gemini::generate_content::Format as InboundFormat},
};
use axum::{
    Router,
    body::Bytes,
    extract::{FromRequestParts, OriginalUri, Path, State},
    http::{Method, request::Parts as RequestParts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;

/// Builds the endpoint routes; the prefix router mounts them.
pub(super) fn router() -> Router<Context> {
    Router::new()
        .route("/models", get(models))
        .route("/models/{*rest}", post(dispatch))
}

/// The `GET /models` success body: the native Gemini list shape over the effective-model
/// set, with no pagination token.
#[derive(Debug, Serialize)]
struct ModelList {
    /// One entry per served model, ordered by public ID.
    models: Vec<ModelEntry>,
}

/// One served model in the native Gemini shape, carrying the official token-limit members
/// when their effective values are known (README §6.5) — `inputTokenLimit` and
/// `outputTokenLimit` — and omitting each when unknown, never fabricating a value, with the
/// gateway-extension `compaction` facts beside them rendering only the known mechanisms.
#[derive(Debug, Serialize)]
struct ModelEntry {
    /// Resource name in the official `models/<id>` form.
    name: String,

    /// Effective maximum input tokens in the official member's camelCase spelling; omitted when
    /// unknown.
    #[serde(rename = "inputTokenLimit", skip_serializing_if = "Option::is_none")]
    input_token_limit: Option<i64>,

    /// Effective maximum output tokens in the official member's camelCase spelling; omitted when
    /// unknown.
    #[serde(rename = "outputTokenLimit", skip_serializing_if = "Option::is_none")]
    output_token_limit: Option<i64>,

    /// Effective compaction support facts keyed by the mechanism spellings — a gateway
    /// extension; omitted when no mechanism is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionSupportFacts>,
}

/// Handles `GET /models`: the native `{"models": [...]}` shape over the same
/// effective-model set every surface lists, ignoring pagination parameters and returning
/// the full list in one page with no token.
async fn models(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
) -> Response {
    serving::model_list_response::<super::Surface, _, _>(&context.database, api_key.0, |listed| {
        ModelList {
            models: listed
                .into_iter()
                .map(|model| ModelEntry {
                    name: format!("models/{}", model.public_id),
                    input_token_limit: model.max_input_tokens,
                    output_token_limit: model.max_output_tokens,
                    compaction: model.compaction,
                })
                .collect(),
        }
    })
    .await
}

/// The router's own `{*rest}` capture, resolved into the URL-determined request facts: the
/// public model plus its action's streaming mode. The extractor runs ahead of the body
/// extractor, so every URL-determined rejection — an unrouteable capture, or a streaming
/// action missing its `alt=sse` query — is decided before any body byte is buffered (the
/// outward body cap is 64 MiB, README §4). A matched single-capture wildcard route always
/// carries its capture, so the capture arm of the rejection — the surface's unmapped-path
/// 404 over the request's own path — is unrepresentable in practice and exists only to keep
/// the surface's error wire should that contract change.
struct WildcardCapture(ModelAction);

impl<S> FromRequestParts<S> for WildcardCapture
where
    S: Send + Sync,
{
    type Rejection = WildcardRejection;

    async fn from_request_parts(
        parts: &mut RequestParts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        // The original URI, not the router-stripped one: the invalid-URL
        // classification names the request's own full path.
        let OriginalUri(uri) = OriginalUri::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|error| match error {});

        match Path::<String>::from_request_parts(parts, state).await {
            // The rejection is built only here: a matched single-capture wildcard route always
            // carries its capture, so every successful dispatch would otherwise pay a method
            // clone and a path allocation for a rejection that never renders.
            Err(_) => Err(WildcardRejection(Error::InvalidUrl {
                method: parts.method.as_str().to_string(),
                path: uri.path().to_string(),
            })),

            Ok(Path(capture)) => resolve_url_facts(Some(&capture), uri.query(), uri.path())
                .map(Self)
                .map_err(WildcardRejection),
        }
    }
}

/// Why the URL-determined facts could not extract: the router failed to hand
/// the handler its own route's capture, the capture parsed to no model action,
/// or a streaming action lacked its `alt=sse` query. Rendered through the
/// surface's error envelope — the capture and action arms as the unmapped-path
/// 404 the prefix fallback also renders, the query arm as the streaming
/// convention's 400.
#[derive(Debug)]
struct WildcardRejection(Error);

impl IntoResponse for WildcardRejection {
    fn into_response(self) -> Response {
        log_error("request", &self.0);

        serving::error_response::<super::Surface>(self.0)
    }
}

/// Handles both completion actions behind the wildcard route: every
/// URL-determined fact was already resolved by the [`WildcardCapture`]
/// extractor ahead of the body, so the handler only runs the shared
/// completion pipeline over the buffered body.
async fn dispatch(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    WildcardCapture(action): WildcardCapture,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<super::Surface>>,
) -> Response {
    serving::outcome_response::<super::Surface>(
        serving::run_completion::<InboundFormat>(
            &context,
            &body,
            api_key.0,
            RouteContext::with_model(action.model, Some(action.is_streaming)),
            serving::OperationKind::Completion,
            ApiLogEndpointType::GeminiGenerateContent,
        )
        .await,
    )
}

/// One parsed wildcard action: the URL-routed model plus the streaming mode from the URL
/// action, not the body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelAction {
    /// Public model id from the URL path.
    model: String,

    /// Streaming mode from the URL action.
    is_streaming: bool,
}

/// Parses the router's wildcard capture into its model plus action.
///
/// The capture must be `<model>:generateContent` or `<model>:streamGenerateContent`
/// with a non-empty model containing no `/` (the URL path cannot carry such a public
/// ID; README §6.5). The model is the substring before the first `:`, so a public ID
/// carrying `:` is not separately rejected — its remainder fails the action match and
/// lands in the same surface 404, the URL path simply cannot address it. Anything
/// outside the two action shapes is an unmapped path and returns the surface 404, so
/// malformed paths observe the same body the prefix fallback renders.
///
/// `request_path` is the request's raw path, carried verbatim into the invalid-URL
/// classification.
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] for captures outside the two action shapes.
fn parse_model_action(capture: &str, request_path: &str) -> Result<ModelAction, Error> {
    let invalid_url = || Error::InvalidUrl {
        method: Method::POST.as_str().to_string(),
        path: request_path.to_string(),
    };

    // A `/` inside the capture means the model segment itself contained a slash: the
    // URL path cannot carry such a public ID, so the path is unmapped.
    if capture.contains('/') {
        return Err(invalid_url());
    }

    let (model, action) = capture.split_once(':').ok_or_else(invalid_url)?;

    if model.is_empty() {
        return Err(invalid_url());
    }

    match action {
        "generateContent" => Ok(ModelAction {
            model: model.to_string(),
            is_streaming: false,
        }),

        "streamGenerateContent" => Ok(ModelAction {
            model: model.to_string(),
            is_streaming: true,
        }),

        _ => Err(invalid_url()),
    }
}

/// Resolves the URL-determined request facts of the wildcard route: the
/// capture parsed into its model plus action ([`parse_model_action`]), then
/// the streaming action's query convention — streaming requires the `?alt=sse`
/// form, parsed manually from the request URI ([`super::has_alt_sse`]), so no
/// axum `query` extractor is declared. The capture is `None` when the router
/// failed to hand it over; the convention's 400 and every capture 404 here are
/// the rejections the [`WildcardCapture`] extractor renders ahead of body
/// extraction.
///
/// `request_path` is the request's raw path, carried verbatim into the
/// invalid-URL classification.
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] for a missing capture or captures outside the
/// two action shapes, and [`Error::MalformedRequest`] naming `alt` for a
/// streaming action without its `alt=sse` query.
fn resolve_url_facts(
    capture: Option<&str>,
    query: Option<&str>,
    request_path: &str,
) -> Result<ModelAction, Error> {
    let action = parse_model_action(
        capture.ok_or_else(|| Error::InvalidUrl {
            method: Method::POST.as_str().to_string(),
            path: request_path.to_string(),
        })?,
        request_path,
    )?;

    if action.is_streaming && !super::has_alt_sse(query) {
        return Err(Error::MalformedRequest {
            param: Some(String::from("alt")),
            message: String::from("streamGenerateContent requires `?alt=sse`"),
        });
    }

    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// Verifies the wildcard capture parsing: the two action shapes route with their
    /// streaming modes, while every other shape — missing or empty model, unknown
    /// action, extra segments, or a model carrying routing-unsafe characters — is an
    /// unmapped path returning the surface 404.
    ///
    /// # Panics
    ///
    /// Panics if a capture parses outside its documented outcome.
    #[test]
    fn parses_wildcard_captures_into_model_actions() {
        let request_path = |rest: &str| format!("/api/gemini-v1beta/models/{rest}");

        for (rest, model, is_streaming) in [
            ("gemini-2.0:generateContent", "gemini-2.0", false),
            ("gemini-2.0:streamGenerateContent", "gemini-2.0", true),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each valid capture is constructed to parse"
            )]
            let action =
                parse_model_action(rest, &request_path(rest)).expect("a valid capture must parse");

            assert_eq!(
                action,
                ModelAction {
                    model: model.to_string(),
                    is_streaming,
                }
            );
        }

        for rest in [
            "",
            ":generateContent",
            "model:unknownAction",
            "modelgenerateContent",
            "a/b:generateContent",
            "a:b:generateContent",
            "model:",
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each invalid capture is constructed to fail"
            )]
            let error = parse_model_action(rest, &request_path(rest))
                .expect_err("an invalid capture must 404");

            assert!(
                matches!(error, Error::InvalidUrl { .. }),
                "unexpected error for `{rest}`: {error}"
            );
            assert_eq!(error.http_status(), StatusCode::NOT_FOUND);
        }
    }

    /// Verifies the URL-facts resolution the capture extractor performs ahead
    /// of the body extractor: both action shapes resolve with their streaming
    /// modes whatever the query, a non-streaming action needs no query, a
    /// streaming action demands exactly the `alt=sse` form — the convention's
    /// 400 — and a capture the router never handed over is the unmapped-path
    /// 404.
    ///
    /// # Panics
    ///
    /// Panics if a resolution outcome diverges from the documented shapes.
    #[test]
    fn resolves_url_facts_and_enforces_the_streaming_query_convention() {
        let request_path = |rest: &str| format!("/api/gemini-v1beta/models/{rest}");

        for (rest, query, model, is_streaming) in [
            ("gemini-2.0:generateContent", None, "gemini-2.0", false),
            (
                "gemini-2.0:generateContent",
                Some("other=1"),
                "gemini-2.0",
                false,
            ),
            (
                "gemini-2.0:streamGenerateContent",
                Some("alt=sse"),
                "gemini-2.0",
                true,
            ),
            (
                "gemini-2.0:streamGenerateContent",
                Some("other=1&alt=sse"),
                "gemini-2.0",
                true,
            ),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each accepted capture is constructed to resolve"
            )]
            let action = resolve_url_facts(Some(rest), query, &request_path(rest))
                .expect("an accepted capture resolves");

            assert_eq!(
                action,
                ModelAction {
                    model: model.to_string(),
                    is_streaming,
                },
                "unexpected action resolved from `{rest}`",
            );
        }

        for (rest, query) in [
            ("gemini-2.0:streamGenerateContent", None),
            ("gemini-2.0:streamGenerateContent", Some("alt=stream")),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each queryless stream capture is constructed to reject"
            )]
            let error = resolve_url_facts(Some(rest), query, &request_path(rest))
                .expect_err("a streaming capture without alt=sse rejects");

            assert!(
                matches!(error, Error::MalformedRequest { .. }),
                "unexpected error for `{rest}`: {error}"
            );
            assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);
        }

        #[expect(
            clippy::expect_used,
            reason = "the routerless capture is constructed to reject"
        )]
        let error = resolve_url_facts(None, None, &request_path("gemini-2.0:generateContent"))
            .expect_err("a capture the router never handed over rejects");

        assert!(
            matches!(error, Error::InvalidUrl { .. }),
            "unexpected error for the missing capture: {error}"
        );
        assert_eq!(error.http_status(), StatusCode::NOT_FOUND);
    }

    /// Verifies the router builds without panicking: axum 0.8 rejects partial-segment
    /// parameters (`/models/{model}:generateContent` panics with "Only one parameter is
    /// allowed per path segment"), so the wildcard route above is the supported shape
    /// and this test pins that it stays buildable.
    ///
    /// # Panics
    ///
    /// Panics if the router fails to build.
    #[test]
    fn router_builds_with_wildcard_routes() {
        let _router: Router<Context> = router();
    }

    /// Verifies the native model-listing shape renders `models/<id>` names with no
    /// pagination token, and that the official token-limit members ride along when known and
    /// stay absent when unknown — the gateway-extension `compaction` facts beside them,
    /// keyed by the mechanism spellings and carrying only known mechanisms (README §6.5).
    ///
    /// # Panics
    ///
    /// Panics if the rendered shape diverges from the native contract.
    #[test]
    fn renders_native_model_listing_shape() {
        use crate::compaction_model::{
            mechanism::{Mechanism, MechanismSupport},
            support_facts::CompactionSupportFacts as Facts,
        };

        #[expect(clippy::expect_used, reason = "the fixture fact pairs are non-empty")]
        let compaction = Facts::from_pairs([(
            Mechanism::MessagesThresholdCompaction,
            MechanismSupport::Unsupported,
        )])
        .expect("fixture fact pairs are non-empty");

        let models = vec![
            ModelEntry {
                name: "models/b-model".to_string(),
                input_token_limit: None,
                output_token_limit: None,
                compaction: None,
            },
            ModelEntry {
                name: "models/a-model".to_string(),
                input_token_limit: Some(1_048_576),
                output_token_limit: Some(65_535),
                compaction: Some(compaction),
            },
        ];

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let value = serde_json::to_value(&ModelList { models }).expect("the listing serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "models": [
                    {"name": "models/b-model"},
                    {
                        "name": "models/a-model",
                        "inputTokenLimit": 1048576,
                        "outputTokenLimit": 65535,
                        "compaction": {"messages_threshold_compaction": "unsupported"},
                    },
                ],
            })
        );
        assert!(value.get("nextPageToken").is_none());
    }
}
