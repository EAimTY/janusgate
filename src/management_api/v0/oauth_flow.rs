//! Management endpoints starting and completing provider OAuth flows.

use super::{Error, route, validate_identifier};
use crate::{
    context::Context,
    database::Database,
    provider::{NewProvider, OAuthProviderType, Provider, oauth, storage},
    unix_time,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};

route! {
    pub impl Route for StartProviderOauth {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/start-provider-oauth";
        type Request = StartProviderOauthRequest;
        type Response = StartProviderOauthResponse;
    }
}

route! {
    pub impl Route for CompleteProviderOauth {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/complete-provider-oauth";
        type Request = CompleteProviderOauthRequest;
        type Response = super::ProviderEnvelope;
    }
}

/// The `start-provider-oauth` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct StartProviderOauthResponse {
    /// URL the administrator opens to authorize.
    pub authorize_url: String,

    /// Opaque state token identifying the flow.
    pub state: String,

    /// Unix timestamp when the flow expires.
    pub expires_at: i64,
}

/// Strictly parsed `start-provider-oauth` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct StartProviderOauthRequest {
    /// Account kind to authorize.
    #[serde(rename = "type")]
    pub provider_type: String,
}

/// Strictly parsed `complete-provider-oauth` request. A completion takes
/// exactly one form: `id` re-authorizes an existing provider, while `name`
/// plus `model_prefix` create a new one; mixing the forms is an invalid
/// request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct CompleteProviderOauthRequest {
    /// State token returned by `start-provider-oauth`.
    pub state: String,

    /// Authorization code from the provider callback.
    pub code: String,

    /// Provider row identifier to re-authorize; absent creates a provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub id: Option<i64>,

    /// Provider name for a provider being created; must be non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub name: Option<String>,

    /// Public model name prefix for a provider being created; may be empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub model_prefix: Option<String>,
}

/// Handles `POST /start-provider-oauth`.
pub(super) async fn start(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<StartProviderOauthRequest>, Error>,
) -> Result<Json<StartProviderOauthResponse>, Error> {
    let flow = handle_start(&context.database, request).await?;

    Ok(Json(StartProviderOauthResponse {
        authorize_url: flow.authorize_url,
        state: flow.state,
        expires_at: flow.expires_at,
    }))
}

/// Handles `POST /complete-provider-oauth`.
pub(super) async fn complete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CompleteProviderOauthRequest>, Error>,
) -> Result<Json<super::ProviderEnvelope>, Error> {
    let provider = handle_complete(&context.database, &context.control_plane, request).await?;

    Ok(Json(super::ProviderEnvelope {
        provider: super::ProviderResponse::from_provider(&provider),
    }))
}

/// Begins an OAuth flow and persists its state row.
///
/// # Errors
///
/// Returns [`Error`] for unknown provider types and storage failures.
async fn handle_start(
    database: &Database,
    request: StartProviderOauthRequest,
) -> Result<oauth::OAuthFlow, Error> {
    let provider_type = OAuthProviderType::parse(&request.provider_type).ok_or_else(|| {
        Error::UnknownProviderType {
            value: request.provider_type.clone(),
        }
    })?;

    let flow = oauth::begin_flow(provider_type);

    storage::delete_expired_oauth_states(database, unix_time::unix_now()).await?;

    storage::insert_oauth_state(
        database,
        provider_type,
        &flow.state,
        &flow.verifier,
        flow.expires_at,
    )
    .await?;

    info!("started a `{}` OAuth flow", request.provider_type);

    Ok(flow)
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
/// provider.
///
/// `control_plane` is the bounded client for the token exchange and the
/// project resolution.
///
/// # Errors
///
/// Returns [`Error`] for unknown, expired, or already-consumed
/// states, mixed or incomplete completion forms, empty names, taken names,
/// unknown ids, exchange failures, type mismatches, and a provider deleted
/// concurrently after the completion's lookup.
async fn handle_complete(
    database: &Database,
    control_plane: &Client,
    request: CompleteProviderOauthRequest,
) -> Result<Provider, Error> {
    // A completion takes exactly one form. Every form rejection fires
    // before the state is consumed, so a malformed request cannot burn
    // the pending flow.
    let completion = match (request.id, &request.name, &request.model_prefix) {
        (Some(provider_id), None, None) => Completion::Reauthorization(provider_id),

        (Some(_), Some(_), _) => return Err(Error::CompletionNameWithId),

        (Some(_), None, Some(_)) => return Err(Error::CompletionModelPrefixWithId),

        (None, None, _) => return Err(Error::CompletionRequiresName),

        (None, Some(_), None) => return Err(Error::CompletionRequiresModelPrefix),

        (None, Some(name), Some(model_prefix)) => Completion::Creation { name, model_prefix },
    };

    let now = unix_time::unix_now();

    // Pre-consume validations, computed from the still-unconsumed state
    // so the pending flow survives every rejection.
    let reauthorized = match &completion {
        Completion::Reauthorization(provider_id) => {
            let provider = storage::find_provider_by_id(database, *provider_id)
                .await?
                .ok_or(Error::ProviderNotFound { id: *provider_id })?;

            Some(provider)
        }

        Completion::Creation { name, .. } => {
            validate_identifier("name", name)?;

            // The database's UNIQUE constraint is the concurrent backstop
            // for a name taken after this check.
            if storage::find_provider(database, name).await?.is_some() {
                return Err(Error::ProviderNameTaken {
                    name: name.to_string(),
                });
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
    ) && state_type.as_provider_type() != provider.provider_type
    {
        return Err(Error::OauthStateTypeMismatch {
            name: provider.name.clone(),
            provider_type: provider.provider_type,
            state_type,
        });
    }

    let state = storage::consume_oauth_state(database, &request.state, now)
        .await?
        .ok_or(Error::UnknownOauthState)?;

    let exchanged = oauth::exchange_code(
        control_plane,
        state.state_type,
        &request.code,
        &state.verifier,
    )
    .await?;

    let stored = match completion {
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
            // reload.
            if !is_updated {
                return Err(Error::ProviderVanishedDuringCompletion { id: provider_id });
            }

            info!("refreshed the credentials of provider {provider_id}");

            storage::reload_provider(database, provider_id, "OAuth completion").await?
        }

        Completion::Creation { name, model_prefix } => {
            let stored = storage::insert_provider(
                database,
                &NewProvider {
                    name,
                    model_prefix,
                    provider_type: state.state_type.as_provider_type(),
                    protocol: None,
                    base_url: None,
                    credentials: &exchanged.credentials,
                    expires_at: Some(exchanged.expires_at),
                    is_enabled: true,
                },
            )
            .await?;

            info!("created provider `{name}` from an OAuth completion");

            stored
        }
    };

    Ok(stored)
}
