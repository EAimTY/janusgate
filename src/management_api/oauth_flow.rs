//! Management endpoints beginning and completing OAuth authorization flows.

use super::{error_response, parse_body, provider_json};
use crate::{
    database::Database,
    error::{self, ApiError},
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

    /// Public model name prefix, only accepted for a provider being created.
    #[serde(default)]
    model_prefix: Option<String>,
}

/// Handles `POST /start-provider-oauth`.
pub(crate) async fn start(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_start(&database, &body).await;

    match outcome {
        Ok(flow) => error::json_response(
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
        Ok(rendered) => error::json_response(200, serde_json::json!({ "provider": rendered })),
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
/// Returns [`ApiError`] for unknown, expired, or already-consumed states,
/// exchange failures, type mismatches, and a provider deleted concurrently
/// after the completion's lookup.
async fn handle_complete(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: CompleteOAuthBody = parse_body(body)?;

    crate::provider::validate_provider_name(&request.name)?;

    let existing = storage::find_provider(database, &request.name).await?;

    // A `model_prefix` only names a provider being created; applying one to an
    // existing provider would silently rename its models. The rejection fires
    // before the state is consumed, so a malformed request cannot burn the
    // pending flow.
    if existing.is_some() && request.model_prefix.is_some() {
        return Err(ApiError::invalid_parameter(
            "model_prefix",
            format!(
                "Provider `{}` already exists; model_prefix cannot be supplied for it",
                request.name
            ),
        ));
    }

    let now = unix_time::unix_now();

    // State rows are immutable, so a provider-type mismatch is computable
    // before the atomic consume: the rejection must not burn the pending flow
    // or spend the token exchange. An unknown or expired state is left to the
    // consume below, which yields the 404.
    if let (Some(existing), Some(state_type)) = (
        &existing,
        storage::find_oauth_state_type(database, &request.state, now).await?,
    ) && state_type != existing.provider_type
    {
        return Err(ApiError::conflict(format!(
            "Provider `{}` has type `{}` and cannot be authorized as `{}`",
            existing.name,
            existing.provider_type.as_str(),
            state_type.as_str()
        )));
    }

    let state = storage::consume_oauth_state(database, &request.state, now)
        .await?
        .ok_or_else(|| {
            ApiError::not_found("OAuth state is unknown, expired, or already consumed")
        })?;

    let exchanged = oauth::exchange_code(
        crate::http(),
        state.state_type,
        &request.code,
        &state.verifier,
    )
    .await?;

    match existing {
        Some(existing) => {
            let is_updated = storage::update_credentials(
                database,
                existing.id,
                &exchanged.credentials,
                Some(exchanged.expires_at),
            )
            .await?;

            // A concurrent `delete-provider` between the outer lookup and
            // this write affects zero rows; like the refresh path, the
            // vanished provider is a 404, not a storage-drift 500 from the
            // reload below.
            if !is_updated {
                return Err(ApiError::not_found(
                    "provider vanished during OAuth completion",
                ));
            }
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

    let stored = storage::reload_provider(database, &request.name, "OAuth completion").await?;

    Ok(provider_json(&stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use std::{error::Error as StdError, fs, time::Duration};

    /// Verifies supplying `model_prefix` for an existing provider is rejected
    /// with 400 naming the field, and that the rejection fires before the
    /// state is consumed.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn complete_rejects_model_prefix_for_existing_provider() -> Result<(), Box<dyn StdError>>
    {
        let (database, path) = test_support::open_test_database(2, Duration::from_secs(5)).await?;

        test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        let body = test_support::body(
            r#"{"state":"unused-state","code":"unused-code","name":"alpha","model_prefix":"p-"}"#,
        );

        let error = handle_complete(&database, &body)
            .await
            .err()
            .ok_or("model_prefix on an existing provider must be rejected")?;

        assert_eq!(error.status, 400);
        assert_eq!(error.param.as_deref(), Some("model_prefix"));

        // The rejection fired before state consumption: an unknown state is
        // the only reason a retry without model_prefix can still fail.
        let retry =
            test_support::body(r#"{"state":"unused-state","code":"unused-code","name":"alpha"}"#);

        let error = handle_complete(&database, &retry)
            .await
            .err()
            .ok_or("an unknown state must be rejected")?;

        assert_eq!(error.status, 404);
        assert!(error.message.contains("already consumed"));

        let _ = fs::remove_file(path);

        Ok(())
    }

    /// Verifies completing a state whose provider type mismatches the existing
    /// provider is rejected with 409 before the state is consumed, so the
    /// pending flow stays available to a matching completion.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn complete_rejects_type_mismatch_without_burning_the_state()
    -> Result<(), Box<dyn StdError>> {
        let (database, path) = test_support::open_test_database(2, Duration::from_secs(5)).await?;

        test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        let now = crate::unix_time::unix_now();

        storage::insert_oauth_state(
            &database,
            ProviderType::OpenAiOauth,
            "state-mismatch",
            "verifier",
            now + 60,
        )
        .await?;

        let body =
            test_support::body(r#"{"state":"state-mismatch","code":"unused-code","name":"alpha"}"#);

        let error = handle_complete(&database, &body)
            .await
            .err()
            .ok_or("a type mismatch must be rejected")?;

        assert_eq!(error.status, 409);

        assert!(
            error.message.contains("cannot be authorized"),
            "the error must state the authorization conflict: {error}"
        );

        // The rejection fired before consumption: the state is still there for
        // a matching completion, which is observable by consuming it now.
        let consumed = storage::consume_oauth_state(&database, "state-mismatch", now).await?;

        let consumed = consumed.ok_or("the mismatched completion must not burn the state")?;

        assert_eq!(consumed.state_type, ProviderType::OpenAiOauth);

        let _ = fs::remove_file(path);

        Ok(())
    }
}
