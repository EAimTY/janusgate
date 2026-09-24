//! Management routes beginning and completing provider OAuth flows, across both grants
//! (README §5.6, §6).
//!
//! `oauth:start` begins a creation flow; `oauth:reauthorize` begins a reauthorization flow; the
//! two targets are disjoint because device flows complete without a second user step.
//! `oauth:complete` finishes an authorization-code flow by exchanging the pasted code, and reads
//! a device flow's relational outcome — pending, succeeded, or failed — consuming a terminal row
//! exactly once.

use super::{Error, ProviderResponse, route};
use crate::{
    context::Context,
    provider::{
        CredentialKind, Provider, StoredCredentials,
        definition::{
            credential::{CredentialAcquisition, Grant, OAuthRecipe, Pkce},
            eval::ValueEnv,
        },
        oauth,
        oauth::Error as OAuthError,
        provisioning::{self, WorkingState},
    },
    storage,
    storage::oauth_flows::{
        FlowGrant, FlowOutcome, FlowPurpose as FlowPurposeStorage, NewFlow, OngoingFlow,
    },
    unix_time,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, num::NonZeroI64};

route! {
    pub StartProviderOauth: {
        path: "/oauth:start",
        method: http::Method::POST,
        request: StartProviderOauthRequest,
        response: StartProviderOauthResponse,
    }
}

route! {
    pub CompleteProviderOauth: {
        path: "/oauth:complete",
        method: http::Method::POST,
        request: CompleteProviderOauthRequest,
        response: CompleteProviderOauthResponse,
    }
}

route! {
    pub ReauthorizeProvider: {
        path: "/oauth:reauthorize",
        method: http::Method::POST,
        request: ReauthorizeProviderRequest,
        response: StartProviderOauthResponse,
    }
}

/// Strictly parsed `oauth:start` request: the creation flow's definition reference and the new
/// provider's naming, stated in full (rule 3).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct StartProviderOauthRequest {
    /// The exact definition version the flow authorizes.
    pub provider_definition_name: String,

    /// The exact definition version the flow authorizes.
    pub provider_definition_version: i64,

    /// Name of the provider being created; must be non-empty.
    pub name: String,

    /// Public model name prefix of the provider being created; may be empty, otherwise
    /// routing-safe.
    pub model_prefix: String,

    /// The instance's initial base URL; required when the definition addresses the base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub base_url: Option<String>,
}

/// Strictly parsed `oauth:reauthorize` request: the flow replaces the credentials of one
/// existing provider, optionally against a different exact definition version.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ReauthorizeProviderRequest {
    /// Provider whose credentials the flow replaces.
    pub provider_id: i64,

    /// The exact target definition version; absent reauthorizes against the provider's current
    /// pair. The target must be an OAuth definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_definition_name: Option<String>,

    /// The exact target definition version; absent reauthorizes against the provider's current
    /// pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_definition_version: Option<i64>,
}

/// Strictly parsed `oauth:complete` request: a union discriminated by the flow's grant (rule 4),
/// because an authorization-code completion pastes its code and a device completion polls its
/// stored outcome.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "grant", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum CompleteProviderOauthRequest {
    /// Completes an authorization-code flow with the pasted code.
    AuthorizationCode {
        /// State token returned by `oauth:start` or `oauth:reauthorize`.
        state: String,

        /// Authorization code from the provider callback.
        code: String,
    },

    /// Reads a device flow's relational outcome, consuming a terminal row once.
    Device {
        /// State token returned by `oauth:start` or `oauth:reauthorize`.
        state: String,
    },
}

/// The flow-begin routes' success body: a union discriminated by the flow's grant (rule 4), because
/// an authorization-code flow answers with the URL to open and a device flow answers with the
/// verification facts to approve through.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "grant", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum StartProviderOauthResponse {
    /// An authorization-code flow: the administrator opens the URL and pastes the redirected
    /// code back through `oauth:complete`.
    AuthorizationCode {
        /// URL the administrator opens to authorize.
        authorize_url: String,

        /// Opaque state token identifying the flow; the management API names it `state`.
        state: String,

        /// Unix timestamp in microseconds when the flow expires.
        expires_at: i64,
    },

    /// A device flow: the administrator approves at the verification URI while the gateway
    /// polls.
    Device {
        /// Opaque state token identifying the flow.
        state: String,

        /// Unix timestamp in microseconds when the flow expires.
        expires_at: i64,

        /// The human-readable code the administrator enters at the verification URI.
        user_code: String,

        /// The URI the administrator opens, when the response carried one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        verification_uri: Option<String>,

        /// The verification URI carrying the user code, when the response carried one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        verification_uri_complete: Option<String>,
    },
}

/// The `oauth:complete` success body: a union over the flow's observable state (rule 4).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum CompleteProviderOauthResponse {
    /// The flow's provider write committed; the row is the exchange's product.
    Succeeded {
        /// The created or reauthorized provider row.
        provider: ProviderResponse,
    },

    /// The device flow is still pending upstream.
    Pending {},

    /// The device flow failed with its gateway-owned failure pair.
    Failed {
        /// The gateway-owned failure code.
        failure_code: String,

        /// The gateway-owned failure message.
        failure_message: String,
    },
}

/// Handles `POST /oauth:start`.
pub(super) async fn start(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<StartProviderOauthRequest>, Error>,
) -> Result<Json<StartProviderOauthResponse>, Error> {
    let response = handle_start(&context.database, &context.control_plane, request).await?;

    Ok(Json(response))
}

/// Handles `POST /oauth:reauthorize`.
pub(super) async fn reauthorize(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ReauthorizeProviderRequest>, Error>,
) -> Result<Json<StartProviderOauthResponse>, Error> {
    let response =
        handle_reauthorize_start(&context.database, &context.control_plane, request).await?;

    Ok(Json(response))
}

/// Handles `POST /oauth:complete`.
pub(super) async fn complete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CompleteProviderOauthRequest>, Error>,
) -> Result<Json<CompleteProviderOauthResponse>, Error> {
    let response = handle_complete(&context.database, &context.control_plane, request).await?;

    Ok(Json(response))
}

/// Trims the pasted authorization code and rejects paste mistakes: an empty code, or a code
/// carrying callback-URL structure (`?`, `#`, `&`) or whitespace, is a value the provider never
/// issued. Runs before the state is consumed, so the pending flow survives for the corrected
/// paste instead of burning on a doomed exchange.
///
/// # Errors
///
/// Returns [`Error`] when the pasted value is empty or carries URL structure or whitespace.
fn pasted_authorization_code(code: &str) -> Result<String, Error> {
    let code = code.trim();

    if code.is_empty() {
        return Err(Error::EmptyIdentifier { param: "code" });
    }

    if code
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '?' | '#' | '&'))
    {
        return Err(Error::MalformedAuthorizationCode);
    }

    Ok(code.to_string())
}

/// Resolves the OAuth recipe a flow names: the exact definition version, which must carry an
/// OAuth credential.
async fn oauth_definition(
    database: &crate::database::Database,
    name: &str,
    version: i64,
) -> Result<crate::provider::ProviderDefinition, Error> {
    let Some(version) = NonZeroI64::new(version) else {
        return Err(Error::InvalidDefinition(
            crate::provider::definition::ValidationError::new(
                "`provider_definition_version` must be a positive version number",
            ),
        ));
    };

    let Some(definition) =
        storage::provider_definitions::find_definition(database, name, version).await?
    else {
        return Err(Error::DefinitionNotFound {
            name: name.to_string(),
            version: version.get(),
        });
    };

    if definition.credential_kind() != CredentialKind::OAuth {
        return Err(Error::CredentialKindMismatch {
            message: format!(
                "Provider definition `{name}` version {} carries `{}` credentials and \
                 authorizes no flow",
                definition.version.get(),
                definition.credential_kind().as_str(),
            ),
        });
    }

    Ok(definition)
}

/// Initializes the working slots of a new flow: the definition's initial slots run once, over
/// the target provider's stored slots for a reauthorization, so only the missing names are
/// initialized.
fn initialize_slots(
    definition: &crate::provider::ProviderDefinition,
    stored: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, Error> {
    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return Err(Error::Internal {
            message: "the flow's definition carries no OAuth credential".to_string(),
        });
    };

    let mut slots = stored.clone();

    for initializer in &recipe.initial_slots {
        if slots.contains_key(&initializer.name) {
            continue;
        }

        let value = match initializer.generator {
            crate::provider::definition::credential::SlotGenerator::Uuid => crate::ids::new_uuid(),

            crate::provider::definition::credential::SlotGenerator::UuidHex => {
                crate::ids::new_uuid().replace('-', "")
            }
        };

        slots.insert(initializer.name.clone(), value);
    }

    Ok(slots)
}

/// Begins a creation flow and persists its row.
///
/// # Errors
///
/// Returns [`Error`] for empty names, unknown definitions, a non-OAuth definition, upstream
/// device-authorization failures, and storage failures.
async fn handle_start(
    database: &crate::database::Database,
    control_plane: &Client,
    request: StartProviderOauthRequest,
) -> Result<StartProviderOauthResponse, Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_model_prefix(&request.model_prefix)?;

    if let Some(base_url) = &request.base_url {
        super::validate_base_url(base_url)?;
    }

    let definition = oauth_definition(
        database,
        &request.provider_definition_name,
        request.provider_definition_version,
    )
    .await?;

    let slots = initialize_slots(&definition, &BTreeMap::new())?;

    begin_flow(
        database,
        control_plane,
        definition,
        FlowPurposeStorage::Create,
        Some(request.name.clone()),
        Some(request.model_prefix.clone()),
        None,
        request.base_url.clone(),
        slots,
    )
    .await
}

/// Begins a reauthorization flow and persists its row.
///
/// # Errors
///
/// Returns [`Error`] for unknown providers, unknown target definitions, a non-OAuth target, and
/// upstream or storage failures.
async fn handle_reauthorize_start(
    database: &crate::database::Database,
    control_plane: &Client,
    request: ReauthorizeProviderRequest,
) -> Result<StartProviderOauthResponse, Error> {
    let provider = storage::providers::find_provider_by_id(database, request.provider_id)
        .await?
        .ok_or(Error::ProviderNotFound {
            id: request.provider_id,
        })?;

    let definition = match (
        request.provider_definition_name.as_deref(),
        request.provider_definition_version,
    ) {
        (Some(name), Some(version)) => oauth_definition(database, name, version).await?,

        (None, None) => provider.definition.clone(),

        _ => {
            return Err(Error::InvalidDefinition(
                crate::provider::definition::ValidationError::new(
                    "a definition rebind names both `provider_definition_name` and \
                     `provider_definition_version`",
                ),
            ));
        }
    };

    let slots = initialize_slots(&definition, provider.credentials.slots())?;

    begin_flow(
        database,
        control_plane,
        definition,
        FlowPurposeStorage::Reauthorize,
        None,
        None,
        Some(provider.id),
        provider.base_url.clone(),
        slots,
    )
    .await
}

/// Begins one flow against its definition: mints the state token and optional PKCE verifier,
/// builds the authorize URL or executes the device authorization, and persists the flow row.
#[allow(clippy::too_many_arguments)]
async fn begin_flow(
    database: &crate::database::Database,
    control_plane: &Client,
    definition: crate::provider::ProviderDefinition,
    purpose: FlowPurposeStorage,
    provider_name: Option<String>,
    model_prefix: Option<String>,
    target_provider_id: Option<i64>,
    base_url: Option<String>,
    slots: BTreeMap<String, String>,
) -> Result<StartProviderOauthResponse, Error> {
    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return Err(Error::Internal {
            message: "the flow's definition carries no OAuth credential".to_string(),
        });
    };

    let OAuthRecipe {
        client_id,
        client_secret,
        grant,
        ..
    } = &**recipe;

    let state_token = oauth::random_url_safe(32);

    let (flow_grant, new_flow, response) = match grant {
        Grant::AuthorizationCode { pkce, .. } => {
            let code_verifier = match pkce {
                Pkce::S256 => Some(oauth::random_url_safe(64)),
                Pkce::None => None,
            };

            let env = ValueEnv {
                base_url: base_url.as_deref(),
                secret: None,
                slots: &slots,
                client_id: Some(client_id.as_str()),
                client_secret: client_secret.as_deref(),
                authorization_code: None,
                code_verifier: code_verifier.as_deref(),
                redirect_uri: None,
                oauth_state: Some(&state_token),
                device_code: None,
                refresh_token: None,
                request_uuid: None,
                model: None,
            };

            let authorize_url =
                oauth::build_authorize_url(grant, &env, &state_token, code_verifier.as_deref())?;

            let response = StartProviderOauthResponse::AuthorizationCode {
                authorize_url: authorize_url.clone(),
                state: state_token.clone(),
                expires_at: unix_time::unix_now_micros() + oauth::AUTHORIZATION_CODE_TTL,
            };

            let new_flow = NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: state_token.clone(),
                purpose,
                provider_name,
                model_prefix,
                base_url,
                target_provider_id,
                grant_type: FlowGrant::AuthorizationCode,
                slots,
                code_verifier,
                device_code: None,
                device_user_code: None,
                device_verification_uri: None,
                device_verification_uri_complete: None,
                device_poll_interval_seconds: None,
                device_next_poll_at: None,
                expires_at: unix_time::unix_now_micros() + oauth::AUTHORIZATION_CODE_TTL,
            };

            (FlowGrant::AuthorizationCode, new_flow, response)
        }

        Grant::Device {
            authorization: _,
            expires_in_fallback_seconds,
        } => {
            let env = ValueEnv {
                base_url: base_url.as_deref(),
                secret: None,
                slots: &slots,
                client_id: Some(client_id.as_str()),
                client_secret: client_secret.as_deref(),
                authorization_code: None,
                code_verifier: None,
                redirect_uri: None,
                oauth_state: None,
                device_code: None,
                refresh_token: None,
                request_uuid: None,
                model: None,
            };

            let device = oauth::execute_device_authorization(control_plane, grant, &env).await?;

            let now = unix_time::unix_now_micros();

            // The expiry comes from the response's lifetime, else the definition's explicit
            // fallback — the write-time validation guarantees one of the two exists.
            let expires_in = device
                .expires_in_seconds
                .or_else(|| {
                    expires_in_fallback_seconds
                        .map(|fallback| i64::try_from(fallback.get()).unwrap_or(i64::MAX))
                })
                .unwrap_or(600);

            let interval = device
                .interval_seconds
                .unwrap_or(oauth::DEFAULT_DEVICE_INTERVAL_SECONDS);

            let expires_at = now + expires_in * unix_time::MICROS_PER_SEC;

            let response = StartProviderOauthResponse::Device {
                state: state_token.clone(),
                expires_at,
                user_code: device.user_code.clone(),
                verification_uri: device.verification_uri.clone(),
                verification_uri_complete: device.verification_uri_complete.clone(),
            };

            let new_flow = NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: state_token.clone(),
                purpose,
                provider_name,
                model_prefix,
                base_url,
                target_provider_id,
                grant_type: FlowGrant::Device,
                slots,
                code_verifier: None,
                device_code: Some(device.device_code),
                device_user_code: Some(device.user_code),
                device_verification_uri: device.verification_uri,
                device_verification_uri_complete: device.verification_uri_complete,
                device_poll_interval_seconds: Some(interval),
                device_next_poll_at: Some(now + interval * unix_time::MICROS_PER_SEC),
                expires_at,
            };

            (FlowGrant::Device, new_flow, response)
        }
    };

    debug_assert_eq!(flow_grant.as_str(), new_flow.grant_type.as_str());

    // The stored row is the response's source of truth: the device facts read back from the row
    // the insert returns, never from the caller's local copy.
    let stored = storage::oauth_flows::insert_flow(database, new_flow).await?;

    info!(
        "started a `{}` OAuth flow against definition `{}` version {}",
        flow_grant.as_str(),
        definition.name,
        definition.version.get()
    );

    Ok(match response {
        StartProviderOauthResponse::AuthorizationCode {
            authorize_url,
            state,
            expires_at,
        } => StartProviderOauthResponse::AuthorizationCode {
            authorize_url,
            state,
            expires_at,
        },

        StartProviderOauthResponse::Device { state, .. } => StartProviderOauthResponse::Device {
            state,
            expires_at: stored.expires_at,
            user_code: stored.device_user_code.clone().unwrap_or_default(),
            verification_uri: stored.device_verification_uri.clone(),
            verification_uri_complete: stored.device_verification_uri_complete.clone(),
        },
    })
}

/// Completes one flow: an authorization-code flow is exchanged and its provider written
/// synchronously; a device flow reads its relational outcome and consumes a terminal row once.
///
/// # Errors
///
/// Returns [`Error`] for paste mistakes, unknown or expired states, grant mismatches, exchange
/// failures, and storage failures.
async fn handle_complete(
    database: &crate::database::Database,
    control_plane: &Client,
    request: CompleteProviderOauthRequest,
) -> Result<CompleteProviderOauthResponse, Error> {
    match request {
        CompleteProviderOauthRequest::AuthorizationCode { state, code } => {
            let code = pasted_authorization_code(&code)?;

            complete_authorization_code(database, control_plane, &state, &code).await
        }

        CompleteProviderOauthRequest::Device { state } => complete_device(database, &state).await,
    }
}

/// Completes an authorization-code flow: the pre-consume checks fire from the still-unconsumed
/// row, then the consume spends it, the exchange and provisioning run, and the provider write
/// commits.
async fn complete_authorization_code(
    database: &crate::database::Database,
    control_plane: &Client,
    state: &str,
    code: &str,
) -> Result<CompleteProviderOauthResponse, Error> {
    let now = unix_time::unix_now_micros();

    // The pre-consume grant check keeps a device flow alive for its poller: a pasted completion
    // of a device state is a client fault, not a consumed flow.
    let peeked = storage::oauth_flows::find_flow_by_state(database, state, now)
        .await?
        .ok_or(Error::UnknownOauthState)?;

    if peeked.grant_type != FlowGrant::AuthorizationCode {
        return Err(Error::OauthGrantMismatch {
            state_grant: peeked.grant_type.as_str().to_string(),
        });
    }

    // The consume is atomic: exactly one completion observes the row, and a failed exchange
    // burns the flow (§6).
    let flow = storage::oauth_flows::consume_flow(database, state, now)
        .await?
        .ok_or(Error::UnknownOauthState)?;

    let definition = storage::provider_definitions::find_definition(
        database,
        &flow.definition_name,
        flow.definition_version,
    )
    .await?
    .ok_or_else(|| Error::Internal {
        message: "the consumed flow's definition vanished".to_string(),
    })?;

    let outcome = exchange_and_provision(database, control_plane, &definition, &flow, code).await?;

    let provider = commit_authorization_code(database, &definition, &flow, outcome).await?;

    Ok(CompleteProviderOauthResponse::Succeeded {
        provider: ProviderResponse::from_provider(&provider),
    })
}

/// Runs the token exchange and any provisioning for one consumed authorization-code flow,
/// producing the credentials and working base URL to commit.
async fn exchange_and_provision(
    _database: &crate::database::Database,
    control_plane: &Client,
    definition: &crate::provider::ProviderDefinition,
    flow: &OngoingFlow,
    code: &str,
) -> Result<(StoredCredentials, Option<String>), Error> {
    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return Err(Error::Internal {
            message: "the consumed flow's definition carries no OAuth credential".to_string(),
        });
    };

    let OAuthRecipe {
        client_id,
        client_secret,
        exchange,
        access_token_lifetime_seconds,
        id_token_claims,
        ..
    } = &**recipe;

    let code_verifier = flow.code_verifier.as_deref();

    let redirect_uri = match &definition.payload.credential {
        CredentialAcquisition::OAuth(recipe) => match &recipe.grant {
            Grant::AuthorizationCode { redirect_uri, .. } => Some(redirect_uri.as_str()),
            Grant::Device { .. } => None,
        },
        CredentialAcquisition::ApiKey => None,
    };

    let env = ValueEnv {
        base_url: flow.base_url.as_deref(),
        secret: None,
        slots: &flow.slots,
        client_id: Some(client_id.as_str()),
        client_secret: client_secret.as_deref(),
        authorization_code: Some(code),
        code_verifier,
        redirect_uri,
        oauth_state: Some(&flow.state_token),
        device_code: None,
        refresh_token: None,
        request_uuid: None,
        model: None,
    };

    let outcome = match oauth::execute_exchange(control_plane, exchange, &env).await? {
        oauth::ExchangeProduct::Completed(outcome) => outcome,
        oauth::ExchangeProduct::Pending | oauth::ExchangeProduct::SlowDown => {
            return Err(Error::Internal {
                message: "the exchange classified as a poll outcome under an authorization-code \
                          flow"
                    .to_string(),
            });
        }

        oauth::ExchangeProduct::Terminal(code) => {
            return Err(OAuthError::Unavailable {
                detail: format!(
                    "the token endpoint rejected the authorization code with `{code}`: the code \
                     is expired, already used, or invalid — start a fresh flow, authorize its \
                     URL, and complete it promptly"
                ),
            }
            .into());
        }

        oauth::ExchangeProduct::Other(code) => {
            return Err(OAuthError::Unavailable {
                detail: format!("the token endpoint rejected the authorization code: `{code}`"),
            }
            .into());
        }
    };

    let access_token = outcome
        .access_token
        .ok_or_else(|| OAuthError::Unavailable {
            detail: "token exchange response lacks an access token".to_string(),
        })?;

    let expires_at = oauth::resolve_expiry(
        outcome.expires_in_seconds,
        Some(&access_token),
        *access_token_lifetime_seconds,
    )?;

    let mut slots = flow.slots.clone();

    if let Some(id_token) = &outcome.id_token {
        oauth::apply_id_token_claims(id_token, id_token_claims, &mut slots);
    }

    slots.extend(outcome.slots);

    let mut state = WorkingState::new(slots, flow.base_url.clone());

    state.access_token = Some(access_token);
    state.refresh_token = outcome.refresh_token;
    state.expires_at = expires_at;

    if let Some(steps) = &definition.payload.provisioning {
        provisioning::run_steps(control_plane, steps, &env, &mut state).await?;
    }

    // The access token must exist before the provider write; the definition's validation
    // guarantees the exchange or a provisioning step supplies it.
    let access_token = state.access_token.ok_or_else(|| Error::Internal {
        message: "the completed exchange produced no access token".to_string(),
    })?;

    let expires_at = state.expires_at;

    let credentials = StoredCredentials::OAuth {
        access_token,
        refresh_token: state.refresh_token,
        slots: state.slots,
        expires_at,
        refresh_started_at: None,
    };

    Ok((credentials, state.base_url))
}

/// Commits one completed authorization-code flow: creates the provider, or atomically replaces
/// the target's credentials and definition reference.
async fn commit_authorization_code(
    database: &crate::database::Database,
    definition: &crate::provider::ProviderDefinition,
    flow: &OngoingFlow,
    outcome: (StoredCredentials, Option<String>),
) -> Result<Provider, Error> {
    let (credentials, base_url) = outcome;

    match flow.purpose {
        FlowPurposeStorage::Create => {
            let name = flow.provider_name.clone().unwrap_or_default();

            // The pre-insert check happened at start; the database's UNIQUE constraint is the
            // concurrent backstop.
            let provider = storage::providers::insert_provider(
                database,
                storage::providers::NewProvider {
                    name: &name,
                    model_prefix: flow.model_prefix.as_deref().unwrap_or_default(),
                    base_url: base_url.as_deref(),
                    credentials: &credentials,
                    definition,
                    is_enabled: true,
                },
            )
            .await?;

            info!("created provider `{name}` from an OAuth completion");

            Ok(provider)
        }

        FlowPurposeStorage::Reauthorize => {
            let target = flow.target_provider_id.unwrap_or_default();

            let mut transaction = database
                .transaction()
                .await
                .map_err(|error| Error::Storage(error.into()))?;

            let is_updated = storage::providers::rebind_credentials_in_transaction(
                &mut transaction,
                target,
                definition,
                &credentials,
            )
            .await?;

            if !is_updated {
                return Err(Error::ProviderVanishedDuringReauthorization { id: target });
            }

            if let Some(base_url) = base_url {
                storage::providers::set_base_url_in_transaction(
                    &mut transaction,
                    target,
                    &base_url,
                )
                .await?;
            }

            transaction
                .commit()
                .await
                .map_err(|error| Error::Storage(error.into()))?;

            let provider = storage::providers::find_provider_by_id(database, target)
                .await?
                .ok_or(Error::ProviderVanishedDuringReauthorization { id: target })?;

            info!("refreshed the credentials of provider {target}");

            Ok(provider)
        }
    }
}

/// Reads a device flow's relational outcome: pending stays unconsumed, a terminal row is
/// consumed exactly once and its facts returned.
async fn complete_device(
    database: &crate::database::Database,
    state: &str,
) -> Result<CompleteProviderOauthResponse, Error> {
    let now = unix_time::unix_now_micros();

    let Some(flow) = storage::oauth_flows::find_flow_by_state(database, state, now).await? else {
        return Err(Error::UnknownOauthState);
    };

    match flow.outcome {
        None => Ok(CompleteProviderOauthResponse::Pending {}),

        Some(FlowOutcome::Succeeded) => {
            let result_provider_id = flow.result_provider_id.unwrap_or_default();

            storage::oauth_flows::consume_terminal_flow(database, &flow.state_token, now)
                .await?
                .ok_or(Error::UnknownOauthState)?;

            let provider = storage::providers::find_provider_by_id(database, result_provider_id)
                .await?
                .ok_or(Error::ProviderNotFound {
                    id: result_provider_id,
                })?;

            Ok(CompleteProviderOauthResponse::Succeeded {
                provider: ProviderResponse::from_provider(&provider),
            })
        }

        Some(FlowOutcome::Failed) => {
            let failure_code = flow.failure_code.clone().unwrap_or_default();
            let failure_message = flow.failure_message.clone().unwrap_or_default();

            storage::oauth_flows::consume_terminal_flow(database, &flow.state_token, now)
                .await?
                .ok_or(Error::UnknownOauthState)?;

            Ok(CompleteProviderOauthResponse::Failed {
                failure_code,
                failure_message,
            })
        }
    }
}
