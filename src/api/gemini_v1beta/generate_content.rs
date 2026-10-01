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
//! `/models/{*rest}`, plus manual parsing in [`parse_model_action`]; malformed paths return
//! the same Gemini 404 the prefix fallback renders, so the observable behavior matches a
//! native partial-segment router.
//!
//! Shared concerns (authentication, 404/405 bodies, body limit, JSON helper, accounting row
//! id, completion pipeline) live in [`crate::api::serving`] and the prefix module [`super`];
//! this module owns only the three endpoint handlers, request parsing through
//! [`crate::format::gemini::generate_content`], and the native model-listing shape.

use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, log_error, serving},
    context::Context,
    format::{RouteContext, gemini::generate_content::Format as InboundFormat},
};
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::Method,
    response::Response,
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
/// `outputTokenLimit` — and omitting each when unknown, never fabricating a value.
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
}

/// Handles `GET /models`: the native `{"models": [...]}` shape over the same
/// effective-model set every surface lists, ignoring pagination parameters and returning
/// the full list in one page with no token.
async fn models(State(context): State<Context>) -> Response {
    serving::model_list_response::<super::Surface, _, _>(&context.database, |listed| ModelList {
        models: listed
            .into_iter()
            .map(|model| ModelEntry {
                name: format!("models/{}", model.public_id),
                input_token_limit: model.max_input_tokens,
                output_token_limit: model.max_output_tokens,
            })
            .collect(),
    })
    .await
}

/// Handles both completion actions behind the wildcard route: parses the `rest` capture
/// into its model plus action, enforces the streaming query convention, then runs the
/// shared completion pipeline.
async fn dispatch(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    OriginalUri(uri): OriginalUri,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<super::Surface>>,
) -> Response {
    let path = uri.path();

    let rest = path
        .strip_prefix("/api/gemini-v1beta/models/")
        .or_else(|| path.strip_prefix("/models/"))
        .unwrap_or("");

    // The wildcard capture is everything after `/models/`; it is re-derived from the
    // request path rather than through a typed extractor so malformed paths stay in
    // this handler's 404 domain instead of the router's.
    let action = match parse_model_action(rest) {
        Ok(action) => action,

        Err(error) => {
            log_error("request", &error);

            return serving::error_response::<super::Surface>(error);
        }
    };

    if action.is_streaming {
        // Streaming requires the `alt=sse` query form; the query is parsed manually from the
        // request URI ([`super::has_alt_sse`]), so no axum `query` extractor is declared.
        let is_sse = super::has_alt_sse(uri.query());

        if !is_sse {
            let error = Error::MalformedRequest {
                param: Some("alt".to_string()),
                message: "streamGenerateContent requires `?alt=sse`".to_string(),
            };

            log_error("request", &error);

            return serving::error_response::<super::Surface>(error);
        }
    }

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

/// Parses the wildcard capture after `/models/` into its model plus action.
///
/// The capture must be `<model>:generateContent` or `<model>:streamGenerateContent`
/// with a non-empty model containing no `/` (the URL path cannot carry such a public
/// ID; README §6.5). The model is the substring before the first `:`, so a public ID
/// carrying `:` is not separately rejected — its remainder fails the action match and
/// lands in the same surface 404, the URL path simply cannot address it. Anything
/// outside the two action shapes is an unmapped path and returns the surface 404, so
/// malformed paths observe the same body the prefix fallback renders.
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] for captures outside the two action shapes.
fn parse_model_action(rest: &str) -> Result<ModelAction, Error> {
    let invalid_url = || Error::InvalidUrl {
        method: Method::POST.as_str().to_string(),
        path: format!("/api/gemini-v1beta/models/{rest}"),
    };

    // A `/` inside the capture means the model segment itself contained a slash: the
    // URL path cannot carry such a public ID, so the path is unmapped.
    if rest.contains('/') {
        return Err(invalid_url());
    }

    let (model, action) = rest.split_once(':').ok_or_else(invalid_url)?;

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
        for (rest, model, is_streaming) in [
            ("gemini-2.0:generateContent", "gemini-2.0", false),
            ("gemini-2.0:streamGenerateContent", "gemini-2.0", true),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each valid capture is constructed to parse"
            )]
            let action = parse_model_action(rest).expect("a valid capture must parse");

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
            let error = parse_model_action(rest).expect_err("an invalid capture must 404");

            assert!(
                matches!(error, Error::InvalidUrl { .. }),
                "unexpected error for `{rest}`: {error}"
            );
            assert_eq!(error.http_status(), StatusCode::NOT_FOUND);
        }
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
    /// stay absent when unknown (README §6.5).
    ///
    /// # Panics
    ///
    /// Panics if the rendered shape diverges from the native contract.
    #[test]
    fn renders_native_model_listing_shape() {
        let models = vec![
            ModelEntry {
                name: "models/b-model".to_string(),
                input_token_limit: None,
                output_token_limit: None,
            },
            ModelEntry {
                name: "models/a-model".to_string(),
                input_token_limit: Some(1_048_576),
                output_token_limit: Some(65_535),
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
                    },
                ],
            })
        );
        assert!(value.get("nextPageToken").is_none());
    }
}
