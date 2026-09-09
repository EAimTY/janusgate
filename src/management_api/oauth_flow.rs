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

    /// Public model name prefix, only accepted for a provider being created.
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
/// Returns [`ApiError`] for unknown, expired, or already-consumed states,
/// exchange failures, and type mismatches.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{format::UpstreamFormat, provider::Credentials};
    use std::{
        env,
        error::Error as StdError,
        fs,
        num::NonZero,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    /// Monotonic counter making every test database path unique within the
    /// process.
    static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Opens an isolated test database at a unique temporary path, returning
    /// the database and its file path for cleanup.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database fails.
    async fn open_test_database() -> (Database, PathBuf) {
        let unique_id = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);

        let path: PathBuf = env::temp_dir().join(format!(
            "janusgate_oauth_flow_test_{}_{unique_id}.db",
            process::id()
        ));

        #[expect(
            clippy::expect_used,
            reason = "the test pool size is a non-zero constant"
        )]
        let database = Database::open(
            &path,
            NonZero::new(2).expect("pool size is non-zero"),
            Duration::from_secs(5),
        )
        .await
        .expect("failed to open isolated test database");

        (database, path)
    }

    /// Inserts one `openai_compatible` provider row.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while inserting the row.
    async fn insert_compatible_provider(
        database: &Database,
        name: &str,
    ) -> Result<(), Box<dyn StdError>> {
        storage::insert_provider(
            database,
            &NewProvider {
                name,
                model_prefix: "",
                provider_type: ProviderType::OpenAiCompatible,
                protocol: Some(UpstreamFormat::OpenAiChatCompletion),
                base_url: Some("https://upstream.example"),
                credentials: &Credentials::ApiKey("test-key".to_string()),
                expires_at: None,
                enabled: true,
            },
        )
        .await?;

        Ok(())
    }

    /// Verifies supplying `model_prefix` for an existing provider is rejected
    /// with 400 naming the field, and that the rejection fires before the
    /// state is consumed.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn complete_rejects_model_prefix_for_existing_provider() -> Result<(), Box<dyn StdError>>
    {
        let (database, path) = open_test_database().await;

        insert_compatible_provider(&database, "alpha").await?;

        let body = Bytes::from(
            r#"{"state":"unused-state","code":"unused-code","name":"alpha","model_prefix":"p-"}"#
                .as_bytes()
                .to_vec(),
        );

        let error = handle_complete(&database, &body)
            .await
            .err()
            .ok_or("model_prefix on an existing provider must be rejected")?;

        assert_eq!(error.status, 400);
        assert_eq!(error.param.as_deref(), Some("model_prefix"));

        // The rejection fired before state consumption: an unknown state is
        // the only reason a retry without model_prefix can still fail.
        let retry = Bytes::from(
            r#"{"state":"unused-state","code":"unused-code","name":"alpha"}"#
                .as_bytes()
                .to_vec(),
        );

        let error = handle_complete(&database, &retry)
            .await
            .err()
            .ok_or("an unknown state must be rejected")?;

        assert_eq!(error.status, 404);
        assert!(error.message.contains("already consumed"));

        let _ = fs::remove_file(path);

        Ok(())
    }
}
