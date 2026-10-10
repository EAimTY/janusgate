//! Outward OpenAI-compatible format sub-router (`/api/openai-v1`).
//!
//! The shared outward serving pipeline (completion handling, responses, authentication
//! middleware, fallbacks) lives in [`crate::api::serving`]; this module contributes the OpenAI
//! surface identity — bearer credentials and the OpenAI error envelope — plus the router, the two
//! completion endpoints, the standalone compact endpoint, and the native model-list shape.

use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, serving},
    compaction_model::support_facts::CompactionSupportFacts,
    context::Context,
    database::Database,
    format::{
        RouteContext,
        openai::{
            chat_completion::Format as ChatCompletionFormat, response::Format as ResponseFormat,
        },
    },
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{StatusCode, header},
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
        super::error::openai::render(error)
    }

    fn extract_credential(request: &Request) -> Result<String, serving::AuthRejection> {
        request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(bearer_token)
            .map(str::to_owned)
            .ok_or(serving::AuthRejection::MissingCredential)
    }
}

/// The `GET /models` success body (the official OpenAI list shape).
#[derive(Debug, Serialize)]
struct ModelList {
    /// Always `list`.
    object: &'static str,

    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject>,
}

/// One served model in the official OpenAI model shape, extended with the effective capability
/// declarations (`docs/design/routing.md`): `max_input_tokens`, `max_output_tokens`, the
/// gateway-extension `reasoning_levels`, and the gateway-extension `compaction` facts each render
/// only when their effective value is known — an unknown capability is omitted, never fabricated.
#[derive(Debug, Serialize)]
struct ModelObject {
    /// Public routing ID (`docs/design/storage.md`).
    id: String,

    /// Always `model`.
    object: &'static str,

    /// The official shape requires `created`; the gateway serves no model creation timestamps, so
    /// it renders the epoch.
    created: u64,

    /// Name of the provider or pool serving the model.
    owned_by: String,

    /// Effective maximum input tokens; omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_input_tokens: Option<i64>,

    /// Effective maximum output tokens; omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i64>,

    /// Effective reasoning levels in the vocabulary spellings — a gateway extension
    /// (`docs/design/routing.md`); omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_levels: Option<Vec<&'static str>>,

    /// Effective compaction support facts keyed by the mechanism spellings — a gateway
    /// extension rendering only the known mechanisms; omitted when no mechanism is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionSupportFacts>,
}

/// Builds one listing entry from an effectively enabled model: the public routing ID beside
/// its effective capability declarations, each omitted when unknown (`docs/design/routing.md`).
fn model_object(entry: crate::storage::models::EnabledModel) -> ModelObject {
    ModelObject {
        id: entry.public_id,
        object: "model",
        created: 0,
        owned_by: entry.owner_name,
        max_input_tokens: entry.max_input_tokens,
        max_output_tokens: entry.max_output_tokens,
        reasoning_levels: entry
            .thinking_levels
            .map(|levels| levels.iter().map(|level| level.as_str()).collect()),
        compaction: entry.compaction,
    }
}

/// Constructs the format sub-router with OpenAI bearer authentication and its own 404/405
/// responses.
pub(super) fn router(database: Database) -> Router<Context> {
    serving::surface_router::<Surface>(
        database,
        Router::new()
            .route("/chat/completions", post(chat_completions))
            .route("/responses", post(responses))
            .route("/responses/compact", post(responses_compact))
            .route("/models", get(models)),
    )
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

/// Handles `POST /responses/compact`, the Responses standalone compact
/// endpoint (`docs/design/compaction.md`): its body carries no generation-only controls
/// and its success body is always a non-streaming `response.compaction`
/// object, so the handler skips the streaming machinery of `responses`
/// entirely.
async fn responses_compact(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<Surface>>,
) -> Response {
    serving::outcome_response::<Surface>(serving::run_compact(&context, &body, api_key.0).await)
}

/// Handles `GET /models`.
async fn models(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
) -> Response {
    serving::model_list_response::<Surface, _, _>(&context.database, api_key.0, |listed| {
        ModelList {
            object: "list",
            data: listed.into_iter().map(model_object).collect(),
        }
    })
    .await
}
