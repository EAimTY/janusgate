//! Anthropic inbound surface (`/api/anthropic-v1`).
//!
//! `POST /messages` (stream via body `stream: true`, `max_tokens` required per spec) and
//! `GET /models` per README §6.3. Authentication is `x-api-key` only with the
//! `anthropic-version` header required present (value not validated). Request parsing and
//! rendering go through [`crate::format::anthropic::messages`]; errors render through
//! [`crate::format::anthropic::error`]; the model stays in the body for this surface, so parsing
//! uses [`crate::format::RouteContext::empty`]; the accounting endpoint is
//! [`crate::accounting::ApiLogEndpointType::AnthropicMessages`].
//!
//! The shared outward serving pipeline (completion handling, responses, authentication
//! middleware, fallbacks) lives in [`crate::api::serving`]; this module contributes the Anthropic
//! surface identity — `x-api-key` credentials with the required `anthropic-version` header and
//! the Anthropic error envelope — plus the router, the completion endpoint, and the native
//! model-list shape.

use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, log_error, serving},
    context::Context,
    database::Database,
    format::{
        RouteContext,
        anthropic::{error, messages::Format as MessagesFormat},
    },
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Request, State},
    http::StatusCode,
    middleware,
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;

/// The Anthropic surface's identity for the shared pipeline: `x-api-key` credentials with a
/// required-present `anthropic-version` header, and the Anthropic error envelope.
struct Surface;

impl serving::Surface for Surface {
    fn render_error(error: &Error) -> (StatusCode, impl Serialize) {
        error::render(error)
    }

    fn extract_credential(request: &Request) -> Result<String, serving::AuthRejection> {
        let Some(presented) = request
            .headers()
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
        else {
            log_error("request", &Error::InvalidApiKey);

            return Err(serving::AuthRejection::MissingCredential);
        };

        if !request.headers().contains_key("anthropic-version") {
            let error = Error::MalformedRequest {
                param: Some("anthropic-version".to_string()),
                message: "The anthropic-version header is required".to_string(),
            };

            log_error("request", &error);

            return Err(serving::AuthRejection::Malformed(error));
        }

        Ok(presented.to_owned())
    }
}

/// The `GET /models` success body in the native Anthropic shape: the full effective-model set in
/// one page (`has_more: false`), ignoring pagination parameters. The page bookends
/// (`first_id`/`last_id`) and the per-entry members (`display_name`, `created_at`) that the
/// official shape and its SDKs require are always rendered.
#[derive(Debug, Serialize)]
struct ModelList {
    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject>,

    /// The first entry's public ID; `null` when the page is empty.
    first_id: Option<String>,

    /// Always `false`: the gateway returns the full list in one page.
    has_more: bool,

    /// The last entry's public ID; `null` when the page is empty.
    last_id: Option<String>,
}

/// One served model in the native Anthropic model shape.
#[derive(Debug, Serialize)]
struct ModelObject {
    /// Always `model`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// The public routing ID, which doubles as the display name: the gateway serves no separate
    /// display name.
    display_name: String,

    /// Public routing ID (README §3).
    id: String,

    /// The official shape requires `created_at`; the gateway serves no model creation timestamps,
    /// so it renders the epoch — the same convention the OpenAI surface documents for `created`.
    created_at: u64,
}

/// Builds the native model-list page from one effective-model listing: every entry renders its
/// public routing ID as both `id` and `display_name` at the epoch `created_at`, and the page
/// bookends are the page's own first and last entry ids. Shared by the `GET /models` handler
/// and the shape test, so the test pins the handler's actual mapping.
fn native_model_page(listed: Vec<(String, String)>) -> ModelList {
    let data: Vec<ModelObject> = listed
        .into_iter()
        .map(|(public_id, _provider_name)| ModelObject {
            kind: "model",
            display_name: public_id.clone(),
            id: public_id,
            created_at: 0,
        })
        .collect();

    ModelList {
        // The page bookends are the page's own first and last entry ids.
        first_id: data.first().map(|entry| entry.id.clone()),
        last_id: data.last().map(|entry| entry.id.clone()),
        data,
        has_more: false,
    }
}

/// Constructs the surface router with Anthropic-key authentication and its own 404/405 responses.
///
/// `database` feeds the authentication middleware its own state, so every request checks the
/// presented key against the live database rather than the router state.
pub(super) fn router(database: Database) -> Router<Context> {
    Router::new()
        .route("/messages", post(messages))
        .route("/models", get(models))
        .fallback(serving::fallback::<Surface>)
        .method_not_allowed_fallback(serving::method_not_allowed::<Surface>)
        .layer(middleware::from_fn_with_state(
            database,
            serving::authenticate::<Surface>,
        ))
        .layer(DefaultBodyLimit::max(serving::MAX_BODY_BYTES))
}

/// Handles `POST /messages`.
async fn messages(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<Surface>>,
) -> Response {
    serving::outcome_response::<Surface>(
        serving::run_completion::<MessagesFormat>(
            &context,
            &body,
            api_key.0,
            RouteContext::empty(),
            serving::OperationKind::Completion,
            ApiLogEndpointType::AnthropicMessages,
        )
        .await,
    )
}

/// Handles `GET /models` in the native Anthropic shape over the same effective-model set every
/// surface lists, ignoring pagination parameters and returning the full list in one page.
async fn models(State(context): State<Context>) -> Response {
    serving::model_list_response::<Surface, _, _>(&context.database, native_model_page).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the native model-listing shape: every entry carries the doc-pinned members
    /// (`type`, `id`, `display_name`, `created_at`), the page carries `first_id`/`last_id`
    /// bookending its `data` (`null` when the page is empty), and `has_more` is always `false`.
    ///
    /// # Panics
    ///
    /// Panics if the rendered shape diverges from the native contract.
    #[test]
    fn renders_native_model_listing_shape() {
        let render = native_model_page;

        let listed = vec![
            ("b-model".to_string(), "b-provider".to_string()),
            ("a-model".to_string(), "a-provider".to_string()),
        ];

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let value = serde_json::to_value(render(listed)).expect("the listing serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "data": [
                    {"type": "model", "display_name": "b-model", "id": "b-model", "created_at": 0},
                    {"type": "model", "display_name": "a-model", "id": "a-model", "created_at": 0},
                ],
                "first_id": "b-model",
                "has_more": false,
                "last_id": "a-model",
            })
        );

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let value = serde_json::to_value(render(Vec::new())).expect("the listing serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "data": [],
                "first_id": null,
                "has_more": false,
                "last_id": null,
            })
        );
    }
}
