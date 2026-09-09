//! Management endpoints beginning and completing OAuth authorization flows.

use super::{error_response, json_response, parse_body, provider_json};
use crate::{
    database::Database,
    error::ApiError,
    provider::{NewProvider, ProviderType, oauth, storage},
    unix_time,
};
use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;

/// Strictly parsed `start-provider-oauth` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartOAuthBody {
    /// Account kind to authorize.
    #[serde(rename = "type")]
    provider_type: String,
}

/// Strictly parsed `complete-provider-oauth` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteOAuthBody {
    /// State token returned by `start-provider-oauth`.
    state: String,

    /// Authorization code from the provider callback.
    code: String,

    /// Provider identifier to create or refresh.
    name: String,

    /// Public model name prefix for a created provider.
    #[serde(default)]
    model_prefix: Option<String>,
}

/// Handles `POST /start-provider-oauth`.
pub(crate) async fn start(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_start(&database, &body).await;

    match outcome {
        Ok(flow) => json_response(
            200,
            serde_json::json!({
                "authorize_url": flow.authorize_url,
                "state": flow.state,
                "expires_at": flow.expires_at,
            }),
        ),
        Err(error) => error_response(error),
    }
}

/// Handles `POST /complete-provider-oauth`.
pub(crate) async fn complete(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_complete(&database, &body).await;

    match outcome {
        Ok(rendered) => json_response(200, serde_json::json!({ "provider": rendered })),
        Err(error) => error_response(error),
    }
}

/// Begins an OAuth flow and persists its state row.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown provider types and storage failures.
async fn handle_start(database: &Database, body: &Bytes) -> Result<oauth::OAuthFlow, ApiError> {
    let request: StartOAuthBody = parse_body(body)?;

    let provider_type = ProviderType::parse(&request.provider_type).ok_or_else(|| {
        ApiError::invalid_parameter(
            "type",
            format!("Unknown provider type `{}`", request.provider_type),
        )
    })?;

    let flow = oauth::begin_flow(provider_type)?;

    storage::delete_expired_oauth_states(database, unix_time::unix_now()).await?;

    storage::insert_oauth_state(
        database,
        provider_type,
        &flow.state,
        &flow.verifier,
        flow.expires_at,
    )
    .await?;

    Ok(flow)
}

/// Exchanges the authorization code and creates or refreshes the provider.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown or expired states, exchange failures, and
/// type mismatches.
async fn handle_complete(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: CompleteOAuthBody = parse_body(body)?;

    crate::provider::validate_provider_name(&request.name)?;

    let now = unix_time::unix_now();

    let state = storage::consume_oauth_state(database, &request.state, now)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown or expired OAuth state"))?;

    let exchanged = oauth::exchange_code(
        crate::http(),
        state.state_type,
        &request.code,
        &state.verifier,
    )
    .await?;

    let existing = storage::find_provider(database, &request.name).await?;

    match existing {
        Some(existing) => {
            if existing.provider_type != state.state_type {
                return Err(ApiError::conflict(format!(
                    "Provider `{}` has type `{}` and cannot be authorized as `{}`",
                    existing.name,
                    existing.provider_type.as_str(),
                    state.state_type.as_str()
                )));
            }

            storage::update_credentials(
                database,
                existing.id,
                &exchanged.credentials,
                Some(exchanged.expires_at),
            )
            .await?;
        }

        None => {
            storage::insert_provider(
                database,
                &NewProvider {
                    name: &request.name,
                    model_prefix: request.model_prefix.as_deref().unwrap_or_default(),
                    provider_type: state.state_type,
                    protocol: None,
                    base_url: None,
                    credentials: &exchanged.credentials,
                    expires_at: Some(exchanged.expires_at),
                    enabled: true,
                },
            )
            .await?;
        }
    }

    let stored = storage::find_provider(database, &request.name)
        .await?
        .ok_or_else(|| ApiError::internal("provider vanished after OAuth completion"))?;

    Ok(provider_json(&stored))
}
