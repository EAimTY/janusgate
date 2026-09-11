//! Management endpoints beginning and completing OAuth authorization flows.

use super::{error_response, parse_body, provider_json, validate_identifier};
use crate::{
    database::Database,
    error::{self, ApiError, log_api_error},
    provider::{NewProvider, ProviderType, oauth, storage},
    unix_time,
};
use axum::{body::Bytes, extract::State, response::Response};
use log::info;
use serde::Deserialize;

/// Strictly parsed `start-provider-oauth` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartOAuthBody {
    /// Account kind to authorize.
    #[serde(rename = "type")]
    provider_type: String,
}

/// Strictly parsed `complete-provider-oauth` request. A completion takes
/// exactly one form: `id` re-authorizes an existing provider, while `name`
/// plus `model_prefix` create a new one; mixing the forms is an invalid
/// request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteOAuthBody {
    /// State token returned by `start-provider-oauth`.
    state: String,

    /// Authorization code from the provider callback.
    code: String,

    /// Provider row identifier to re-authorize; absent creates a provider.
    id: Option<i64>,

    /// Provider name for a provider being created; must be non-empty.
    name: Option<String>,

    /// Public model name prefix for a provider being created; may be empty.
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

/// Begins an OAuth flow and persists its state row. The outcome is logged:
/// the started flow at info level, every failure at the flow's response
/// boundary.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown provider types and storage failures.
async fn handle_start(database: &Database, body: &Bytes) -> Result<oauth::OAuthFlow, ApiError> {
    let request: StartOAuthBody = parse_body(body, "start-provider-oauth")?;

    let outcome = async {
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
    .await;

    match &outcome {
        Ok(_) => info!("started a `{}` OAuth flow", request.provider_type),

        Err(error) => log_api_error("start-provider-oauth", error),
    }

    outcome
}

/// The form a completion takes: re-authorizing an existing provider by id,
/// or creating a new one carrying a name and model prefix.
enum Completion<'a> {
    /// Replaces the credentials of an existing provider identified by its
    /// integer primary key.
    Reauthorization(i64),

    /// Creates a new provider with the given name and model prefix.
    Creation {
        /// Unique, non-empty label for the new provider.
        name: &'a str,

        /// String prefixed to model names.
        model_prefix: &'a str,
    },
}

/// Exchanges the authorization code and creates or re-authorizes the
/// provider. The outcome is logged: the created or re-authorized provider at
/// info level, every failure at the completion's response boundary.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown, expired, or already-consumed states,
/// mixed or incomplete completion forms, empty names, taken names, unknown
/// ids, exchange failures, type mismatches, and a provider deleted
/// concurrently after the completion's lookup.
async fn handle_complete(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: CompleteOAuthBody = parse_body(body, "complete-provider-oauth")?;

    // The log context rides on whichever form the request names, computed
    // from the raw request so it survives the moved fields below.
    let log_context = match (request.id, request.name.as_deref()) {
        (Some(provider_id), _) => format!("provider {provider_id}"),

        (None, Some(name)) => format!("provider `{name}`"),

        (None, None) => "provider creation".to_string(),
    };

    let outcome = async {
        // A completion takes exactly one form. Every form rejection fires
        // before the state is consumed, so a malformed request cannot burn
        // the pending flow.
        let completion = match (request.id, &request.name, &request.model_prefix) {
            (Some(provider_id), None, None) => Completion::Reauthorization(provider_id),

            (Some(_), Some(_), _) => {
                return Err(ApiError::invalid_parameter(
                    "name",
                    "name cannot be supplied together with id",
                ));
            }

            (Some(_), None, Some(_)) => {
                return Err(ApiError::invalid_parameter(
                    "model_prefix",
                    "model_prefix cannot be supplied together with id",
                ));
            }

            (None, None, _) => {
                return Err(ApiError::invalid_parameter(
                    "name",
                    "name is required when creating a provider",
                ));
            }

            (None, Some(_), None) => {
                return Err(ApiError::invalid_parameter(
                    "model_prefix",
                    "model_prefix is required when creating a provider",
                ));
            }

            (None, Some(name), Some(model_prefix)) => Completion::Creation { name, model_prefix },
        };

        let now = unix_time::unix_now();

        // Pre-consume validations, computed from the still-unconsumed state
        // so the pending flow survives every rejection.
        let reauthorized = match &completion {
            Completion::Reauthorization(provider_id) => {
                let provider = storage::find_provider_by_id(database, *provider_id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::not_found(format!("Provider {provider_id} does not exist"))
                    })?;

                Some(provider)
            }

            Completion::Creation { name, .. } => {
                validate_identifier("name", name)?;

                // The database's UNIQUE constraint is the concurrent backstop
                // for a name taken after this check.
                if storage::find_provider(database, name).await?.is_some() {
                    return Err(ApiError::conflict(format!(
                        "Provider name `{name}` is already taken"
                    )));
                }

                None
            }
        };

        // State rows are immutable, so a provider-type mismatch is computable
        // before the atomic consume: the rejection must not burn the pending
        // flow or spend the token exchange. An unknown or expired state
        // is left to the consume below, which yields the 404.
        if let (Some(provider), Some(state_type)) = (
            &reauthorized,
            storage::find_oauth_state_type(database, &request.state, now).await?,
        ) && state_type != provider.provider_type
        {
            return Err(ApiError::conflict(format!(
                "Provider `{}` has type `{}` and cannot be authorized as `{}`",
                provider.name,
                provider.provider_type.as_str(),
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

        match completion {
            Completion::Reauthorization(provider_id) => {
                let is_updated = storage::update_credentials(
                    database,
                    provider_id,
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

                info!("refreshed the credentials of provider {provider_id}");
            }

            Completion::Creation { name, model_prefix } => {
                storage::insert_provider(
                    database,
                    &NewProvider {
                        name,
                        model_prefix,
                        provider_type: state.state_type,
                        protocol: None,
                        base_url: None,
                        credentials: &exchanged.credentials,
                        expires_at: Some(exchanged.expires_at),
                        is_enabled: true,
                    },
                )
                .await?;

                info!("created provider `{name}` from an OAuth completion");
            }
        }

        let stored = match completion {
            Completion::Reauthorization(provider_id) => {
                storage::reload_provider(database, provider_id, "OAuth completion").await?
            }

            Completion::Creation { name, .. } => storage::find_provider(database, name)
                .await?
                .ok_or_else(|| ApiError::internal("provider vanished after OAuth completion"))?,
        };

        Ok(provider_json(&stored))
    }
    .await;

    if let Err(error) = &outcome {
        log_api_error(&format!("complete-provider-oauth for {log_context}"), error);
    }

    outcome
}
