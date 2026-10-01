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
        RouteContext,
        openai::{
            chat_completion::Format as ChatCompletionFormat, error,
            response::Format as ResponseFormat,
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
        error::render(error)
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
struct ModelList {
    /// Always `list`.
    object: &'static str,

    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject>,
}

/// One served model in the official OpenAI model shape, extended with the effective capability
/// declarations (README §6.5): `max_input_tokens`, `max_output_tokens`, and the gateway-extension
/// `reasoning_levels` each render only when their effective value is known — an unknown
/// capability is omitted, never fabricated.
#[derive(Debug, Serialize)]
struct ModelObject {
    /// Public routing ID (README §3).
    id: String,

    /// Always `model`.
    object: &'static str,

    /// The official shape requires `created`; the gateway serves no model creation timestamps, so
    /// it renders the epoch.
    created: u64,

    /// Name of the provider serving the model.
    owned_by: String,

    /// Effective maximum input tokens; omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_input_tokens: Option<i64>,

    /// Effective maximum output tokens; omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i64>,

    /// Effective reasoning levels in the vocabulary spellings — a gateway extension (README
    /// §6.5); omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_levels: Option<Vec<&'static str>>,
}

/// Builds one listing entry from an effectively enabled model: the public routing ID beside
/// its effective capability declarations, each omitted when unknown (README §6.5).
fn model_object(entry: crate::storage::models::EnabledModel) -> ModelObject {
    ModelObject {
        id: entry.public_id,
        object: "model",
        created: 0,
        owned_by: entry.provider_name,
        max_input_tokens: entry.max_input_tokens,
        max_output_tokens: entry.max_output_tokens,
        reasoning_levels: entry
            .thinking_levels
            .map(|levels| levels.iter().map(|level| level.as_str()).collect()),
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

/// Handles `GET /models`.
async fn models(State(context): State<Context>) -> Response {
    serving::model_list_response::<Surface, _, _>(&context.database, |listed| ModelList {
        object: "list",
        data: listed.into_iter().map(model_object).collect(),
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::{ModelList, model_object};
    use crate::{ir::IrReasoningLevel, storage::models::EnabledModel};

    /// One listing-entry fixture: the public/provider identity with every effective capability
    /// declaration set by the caller.
    fn entry(
        public_id: &str,
        thinking_levels: Option<Vec<IrReasoningLevel>>,
        max_input_tokens: Option<i64>,
        max_output_tokens: Option<i64>,
    ) -> EnabledModel {
        EnabledModel {
            public_id: public_id.to_string(),
            provider_name: "acme".to_string(),
            thinking_levels,
            max_input_tokens,
            max_output_tokens,
        }
    }

    /// Pins the model-listing entry shape (README §6.5): the official members stay verbatim
    /// (`created` fixed `0`, `owned_by` the provider name), and the three capability members —
    /// `max_input_tokens`, `max_output_tokens`, and the gateway-extension `reasoning_levels` in
    /// the vocabulary spellings — ride along when known and stay wholly absent when unknown.
    ///
    /// # Panics
    ///
    /// Panics if the rendered shape diverges from the contract.
    #[test]
    fn renders_the_model_entry_with_omitted_unknown_capabilities() {
        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let unknown = serde_json::to_value(ModelList {
            object: "list",
            data: vec![model_object(entry("m", None, None, None))],
        })
        .expect("the listing serializes");

        assert_eq!(
            unknown,
            serde_json::json!({
                "object": "list",
                "data": [
                    {
                        "id": "m",
                        "object": "model",
                        "created": 0,
                        "owned_by": "acme",
                    }
                ],
            })
        );

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let known = serde_json::to_value(ModelList {
            object: "list",
            data: vec![model_object(entry(
                "m",
                Some(vec![IrReasoningLevel::Low, IrReasoningLevel::XHigh]),
                Some(200_000),
                Some(32_768),
            ))],
        })
        .expect("the listing serializes");

        assert_eq!(
            known,
            serde_json::json!({
                "object": "list",
                "data": [
                    {
                        "id": "m",
                        "object": "model",
                        "created": 0,
                        "owned_by": "acme",
                        "max_input_tokens": 200000,
                        "max_output_tokens": 32768,
                        "reasoning_levels": ["low", "xhigh"],
                    }
                ],
            })
        );
    }
}
