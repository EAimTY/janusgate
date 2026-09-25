//! Outward OpenAI-compatible format sub-router (`/api/openai-v1`).
//!
//! The shared outward serving pipeline (completion handling, responses, authentication
//! middleware, fallbacks) lives in [`crate::api::serving`]; this module contributes the OpenAI
//! surface identity — bearer credentials and the OpenAI error envelope — plus the router, the
//! two completion endpoints, and the native model-list shape.

use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, log_error, serving},
    context::Context,
    database::Database,
    format::{
        RouteContext, error_openai, openai_chat_completion::Format as ChatCompletionFormat,
        openai_response::Format as ResponseFormat,
    },
    storage,
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    middleware,
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;

/// The OpenAI surface's identity for the shared pipeline: bearer credentials and the OpenAI
/// error envelope.
struct Surface;

impl serving::Surface for Surface {
    fn render_error(error: &Error) -> (StatusCode, impl Serialize) {
        error_openai::render(error)
    }

    fn extract_credential(request: &Request) -> Result<String, serving::AuthRejection> {
        request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(bearer_token)
            .map(str::to_owned)
            .ok_or_else(|| {
                log_error("request", &Error::InvalidApiKey);

                serving::AuthRejection::MissingCredential
            })
    }
}

/// The `GET /models` success body (the official OpenAI list shape).
#[derive(Debug, Serialize)]
struct ModelList<'a> {
    /// Always `list`.
    object: &'static str,

    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject<'a>>,
}

/// One served model in the official OpenAI model shape.
#[derive(Debug, Serialize)]
struct ModelObject<'a> {
    /// Public routing ID (README §3).
    id: &'a str,

    /// Always `model`.
    object: &'static str,

    /// The official shape requires `created`; the gateway serves no model creation timestamps, so
    /// it renders the epoch.
    created: u64,

    /// Name of the provider serving the model.
    owned_by: &'a str,
}

/// Constructs the format sub-router with OpenAI bearer authentication and its own 404/405
/// responses.
///
/// `database` feeds the authentication middleware its own state, so every request checks the
/// presented key against the live database rather than the router state.
pub(super) fn router(database: Database) -> Router<Context> {
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/responses", post(responses))
        .route("/models", get(models))
        .fallback(serving::fallback::<Surface>)
        .method_not_allowed_fallback(serving::method_not_allowed::<Surface>)
        .layer(middleware::from_fn_with_state(
            database,
            serving::authenticate::<Surface>,
        ))
        .layer(DefaultBodyLimit::max(serving::MAX_BODY_BYTES))
}

/// Extracts the bearer token from an `Authorization` header value.
///
/// The scheme is compared case-insensitively per RFC 9110 §11.1; the token is taken verbatim after
/// the first space, so malformed forms — no space, tab separator, glued scheme, leading whitespace,
/// trailing content — are rejected by the scheme mismatch or by the credential comparison.
fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.split_once(' ')?;

    if scheme.eq_ignore_ascii_case("Bearer") {
        Some(token)
    } else {
        None
    }
}

/// Handles `POST /chat/completions`.
async fn chat_completions(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<Surface>>,
) -> Response {
    serving::outcome_response::<Surface>(
        serving::run_completion::<ChatCompletionFormat>(
            &context,
            &body,
            api_key.0,
            RouteContext::empty(),
            serving::OperationKind::Completion,
            ApiLogEndpointType::OpenaiChatCompletions,
        )
        .await,
    )
}

/// Handles `POST /responses`.
async fn responses(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<Surface>>,
) -> Response {
    serving::outcome_response::<Surface>(
        serving::run_completion::<ResponseFormat>(
            &context,
            &body,
            api_key.0,
            RouteContext::empty(),
            serving::OperationKind::Completion,
            ApiLogEndpointType::OpenaiResponses,
        )
        .await,
    )
}

/// Handles `GET /models`.
async fn models(State(context): State<Context>) -> Response {
    match storage::models::list_enabled_models_with_owner(&context.database).await {
        Ok(listed) => {
            let data = listed
                .iter()
                .map(|(public_id, provider_name)| ModelObject {
                    id: public_id,
                    object: "model",
                    created: 0,
                    owned_by: provider_name,
                })
                .collect();

            serving::json_response(
                StatusCode::OK,
                &ModelList {
                    object: "list",
                    data,
                },
            )
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            serving::error_response::<Surface>(error)
        }
    }
}
