//! Gemini Interactions endpoint: `POST /interactions`.
//!
//! The Interactions surface addresses its model in the request body (unlike GenerateContent,
//! whose model rides the URL path), so parsing takes the public model id from the body; only the
//! streaming mode may arrive from the route — the `?alt=sse` query ([`super::has_alt_sse`])
//! forces streaming, otherwise the body's `stream` member decides. Request parsing and rendering
//! go through [`crate::format::gemini::interaction`]; errors render through the shared
//! [`crate::api::serving::error_response`] over the prefix's [`super::Surface`] marker (the
//! Gemini envelope); accounting records the endpoint as
//! [`ApiLogEndpointType::GeminiInteractions`](crate::accounting::ApiLogEndpointType).
//!
//! Shared concerns (authentication, 404/405 bodies, body limit, JSON helper, accounting row id,
//! completion pipeline) live in [`crate::api::serving`] and the prefix module [`super`]; this
//! module owns only the endpoint handler and its route.

use crate::{
    accounting::ApiLogEndpointType,
    api::serving,
    context::Context,
    format::{RouteContext, gemini::interaction::Format as InteractionsFormat},
};
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    response::Response,
    routing::post,
};
use axum_extra::extract::WithRejection;

/// Builds the endpoint routes; the prefix router mounts them.
pub(super) fn router() -> Router<Context> {
    Router::new().route("/interactions", post(interactions))
}

/// Handles `POST /interactions`.
async fn interactions(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    OriginalUri(uri): OriginalUri,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<super::Surface>>,
) -> Response {
    // The `?alt=sse` query forces streaming; otherwise the body's `stream` member decides. The
    // model always rides the body on this surface, so the route context carries no public id.
    let is_alt_sse = super::has_alt_sse(uri.query());

    serving::outcome_response::<super::Surface>(
        serving::run_completion::<InteractionsFormat>(
            &context,
            &body,
            api_key.0,
            RouteContext {
                public_id: None,
                is_streaming: is_alt_sse.then_some(true),
            },
            serving::OperationKind::Interaction,
            ApiLogEndpointType::GeminiInteractions,
        )
        .await,
    )
}
