//! Gemini GenerateContent endpoints (`/api/gemini-v1beta/models/...`).
//!
//! Implements `GET /models`, `POST /models/{model}:generateContent` (non-streaming), and
//! `POST /models/{model}:streamGenerateContent?alt=sse` (streaming — only the `alt=sse` form
//! is supported, else 400) per `docs/reference/inference.md`–6.6.
//!
//! Routing notes: axum 0.8 does not support partial-segment parameters such as
//! `/models/{model}:generateContent` — constructing such a route panics with "Only one
//! parameter is allowed per path segment" (verified with a build-time probe against axum
//! 0.8.9). The endpoints therefore share one wildcard route,
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
/// when their effective values are known (`docs/design/routing.md`) — `inputTokenLimit` and
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
/// outward body cap is 64 MiB, `docs/reference/configuration.md`). A matched single-capture
/// wildcard route always carries its capture, so the capture arm of the rejection — the surface's
/// unmapped-path 404 over the request's own path — is unrepresentable in practice and exists only
/// to keep the surface's error wire should that contract change.
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
/// ID; `docs/design/routing.md`). The model is the substring before the first `:`, so a public ID
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
