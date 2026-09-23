//! Anthropic inbound surface (`/api/anthropic-v1`).
//!
//! `POST /messages` (stream via body `stream: true`, `max_tokens` required per spec) and
//! `GET /models` per README §6.3. Authentication is `x-api-key` only with the
//! `anthropic-version` header required present (value not validated). Request parsing and
//! rendering go through [`crate::format::anthropic_messages`]; errors render through
//! [`crate::format::error_anthropic`]; the model stays in the body for this surface, so parsing
//! uses [`crate::format::RouteContext::empty`]; the accounting endpoint is
//! [`crate::storage::api_logs::ApiLogEndpointType::AnthropicMessages`].
//!
//! The shared outward serving pipeline (completion handling, responses, authentication
//! middleware, fallbacks) lives in [`crate::api::serving`]; this module contributes the Anthropic
//! surface identity — `x-api-key` credentials with the required `anthropic-version` header and
//! the Anthropic error envelope — plus the router, the completion endpoint, and the native
//! model-list shape.

use crate::{
    api::{Error, log_error, serving},
    context::Context,
    database::Database,
    format::{RouteContext, anthropic_messages::Format as MessagesFormat, error_anthropic},
    storage,
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
    fn render_error(error: Error) -> Response {
        error_response(error)
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

    fn log_invalid_key() {
        log_error("request", &Error::InvalidApiKey);
    }
}

/// The `GET /models` success body in the native Anthropic shape: the full effective-model set in
/// one page (`has_more: false`), ignoring pagination parameters.
#[derive(Debug, Serialize)]
struct ModelList<'a> {
    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject<'a>>,

    /// Always `false`: the gateway returns the full list in one page.
    has_more: bool,
}

/// One served model in the native Anthropic model shape.
#[derive(Debug, Serialize)]
struct ModelObject<'a> {
    /// Always `model`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Public routing ID (README §3).
    id: &'a str,
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

/// Renders one API failure as an Anthropic JSON error response through the format's render
/// module.
fn error_response(error: Error) -> Response {
    let (status, envelope) = error_anthropic::render(&error);

    serving::json_response(status, &envelope)
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
            "completion",
        )
        .await,
    )
}

/// Handles `GET /models` in the native Anthropic shape over the same effective-model set every
/// surface lists, ignoring pagination parameters and returning the full list in one page.
async fn models(State(context): State<Context>) -> Response {
    match storage::list_enabled_models_with_owner(&context.database).await {
        Ok(listed) => {
            let data = listed
                .iter()
                .map(|(public_id, _provider_name)| ModelObject {
                    kind: "model",
                    id: public_id,
                })
                .collect();

            serving::json_response(
                StatusCode::OK,
                &ModelList {
                    data,
                    has_more: false,
                },
            )
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            error_response(error)
        }
    }
}
