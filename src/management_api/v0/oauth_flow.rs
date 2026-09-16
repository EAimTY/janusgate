//! Management routes beginning and completing provider OAuth flows.

use super::{
    Error, OAuthProviderType, ProviderEnvelope, ProviderResponse, route, validate_identifier,
};
use crate::{
    context::Context,
    provider::{NewProvider, Provider, oauth, storage},
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
        const PATH: &'static str = "/oauth:start";
        type Request = StartProviderOauthRequest;
        type Response = StartProviderOauthResponse;
    }
}

route! {
    pub impl Route for CompleteProviderOauth {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/oauth:complete";
        type Request = CompleteProviderOauthRequest;
        type Response = super::ProviderEnvelope;
    }
}

route! {
    pub impl Route for ReauthorizeProvider {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/oauth:reauthorize";
        type Request = ReauthorizeProviderRequest;
        type Response = ReauthorizeProviderResponse;
    }
}

/// Strictly parsed `oauth:start` request: the account kind the flow
/// authorizes. The field's own parsing rejects values outside the two OAuth
/// account kinds — `openai_compatible` authorizes nothing.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct StartProviderOauthRequest {
    /// OAuth account kind to authorize.
    #[serde(rename = "type")]
    pub provider_type: OAuthProviderType,
}

/// The `oauth:start` success body (README §7.1): three server-derived
/// facts.
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

/// Strictly parsed `oauth:complete` request (README §7.1): the flow creates
/// a provider, so the naming is stated in full — every field required, no
/// defaults. Re-authorizing an existing provider is the separate
/// `oauth:reauthorize` route; the two completion forms cannot be mixed
/// because they are two routes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct CompleteProviderOauthRequest {
    /// State token returned by `oauth:start`.
    pub state: String,

    /// Authorization code from the provider callback.
    pub code: String,

    /// Name of the provider being created; must be non-empty.
    pub name: String,

    /// Public model name prefix of the provider being created; may be
    /// empty.
    pub model_prefix: String,
}

/// Strictly parsed `oauth:reauthorize` request (README §7.1): the flow
/// replaces the credentials of one existing provider.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ReauthorizeProviderRequest {
    /// State token returned by `oauth:start`.
    pub state: String,

    /// Authorization code from the provider callback.
    pub code: String,

    /// Provider whose credentials the flow replaces.
    pub provider_id: i64,
}

/// The `oauth:reauthorize` success body (README §7.1): the new expiry is
/// the only readable fact the exchange produced — the credentials
/// themselves are write-only.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ReauthorizeProviderResponse {
    /// Unix timestamp when the new OAuth access token expires.
    pub expires_at: i64,
}

/// Handles `POST /oauth:start`.
pub(super) async fn start(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<StartProviderOauthRequest>, Error>,
) -> Result<Json<StartProviderOauthResponse>, Error> {
    let flow = handle_start(&context.database, request.provider_type).await?;

    Ok(Json(StartProviderOauthResponse {
        authorize_url: flow.authorize_url,
        state: flow.state,
        expires_at: flow.expires_at,
    }))
}

/// Handles `POST /oauth:complete`.
pub(super) async fn complete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CompleteProviderOauthRequest>, Error>,
) -> Result<Json<ProviderEnvelope>, Error> {
    let provider = handle_complete(&context.database, &context.control_plane, request).await?;

    Ok(Json(ProviderEnvelope {
        provider: ProviderResponse::from_provider(&provider),
    }))
}

/// Handles `POST /oauth:reauthorize`.
pub(super) async fn reauthorize(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ReauthorizeProviderRequest>, Error>,
) -> Result<Json<ReauthorizeProviderResponse>, Error> {
    let expires_at = handle_reauthorize(&context.database, &context.control_plane, request).await?;

    Ok(Json(ReauthorizeProviderResponse { expires_at }))
}

/// Begins an OAuth flow and persists its state row.
///
/// # Errors
///
/// Returns [`Error`] for storage failures.
async fn handle_start(
    database: &crate::database::Database,
    provider_type: OAuthProviderType,
) -> Result<oauth::OAuthFlow, Error> {
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

    info!("started a `{}` OAuth flow", provider_type.as_str());

    Ok(flow)
}

/// Exchanges the authorization code and creates the provider (README §7.1).
///
/// `control_plane` is the bounded client for the token exchange and the
/// project resolution.
///
/// # Errors
///
/// Returns [`Error`] for empty or taken names, unknown, expired, or
/// already-consumed states, exchange failures, and a name taken
/// concurrently by another creation.
async fn handle_complete(
    database: &crate::database::Database,
    control_plane: &Client,
    request: CompleteProviderOauthRequest,
) -> Result<Provider, Error> {
    // Pre-consume validations, computed from the still-unconsumed state so
    // the pending flow survives every rejection.
    validate_identifier("name", &request.name)?;

    // The pre-insert check renders a clean 409; the database's UNIQUE
    // constraint is the concurrent backstop.
    if storage::find_provider(database, &request.name)
        .await?
        .is_some()
    {
        return Err(Error::ProviderNameTaken {
            name: request.name.clone(),
        });
    }

    let now = unix_time::unix_now();

    // An unknown or expired state is left to the consume, which yields the
    // 404.
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

    let stored = storage::insert_provider(
        database,
        &NewProvider {
            name: &request.name,
            model_prefix: &request.model_prefix,
            provider_type: state.state_type.as_provider_type(),
            protocol: None,
            base_url: None,
            credentials: &exchanged.credentials,
            expires_at: Some(exchanged.expires_at),
            is_enabled: true,
        },
    )
    .await?;

    info!(
        "created provider `{}` from an OAuth completion",
        request.name
    );

    Ok(stored)
}

/// Exchanges the authorization code and replaces one provider's credentials
/// (README §7.1).
///
/// `control_plane` is the bounded client for the token exchange and the
/// project resolution.
///
/// # Errors
///
/// Returns [`Error`] for unknown, expired, or already-consumed states,
/// unknown providers, exchange failures, type mismatches, and a provider
/// deleted concurrently after the completion's lookup.
async fn handle_reauthorize(
    database: &crate::database::Database,
    control_plane: &Client,
    request: ReauthorizeProviderRequest,
) -> Result<i64, Error> {
    // Pre-consume validations, computed from the still-unconsumed state so
    // the pending flow survives every rejection.
    let provider = storage::find_provider_by_id(database, request.provider_id)
        .await?
        .ok_or(Error::ProviderNotFound {
            id: request.provider_id,
        })?;

    let now = unix_time::unix_now();

    // State rows are immutable, so a provider-type mismatch is computable
    // before the atomic consume: the rejection must not burn the pending
    // flow or spend the token exchange. An unknown or expired state is left
    // to the consume below, which yields the 404.
    if let Some(state_type) = storage::find_oauth_state_type(database, &request.state, now).await?
        && state_type.as_provider_type() != provider.provider_type
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

    let is_updated = storage::update_credentials(
        database,
        request.provider_id,
        &exchanged.credentials,
        Some(exchanged.expires_at),
    )
    .await?;

    // A concurrent `providers:delete` between the outer lookup and this
    // write affects zero rows; like the refresh path, the vanished provider
    // is a 404, not a storage-drift 500.
    if !is_updated {
        return Err(Error::ProviderVanishedDuringReauthorization {
            id: request.provider_id,
        });
    }

    info!(
        "refreshed the credentials of provider {}",
        request.provider_id
    );

    Ok(exchanged.expires_at)
}
