//! Management routes beginning and completing provider OAuth flows, across both grants
//! (README §7.1, §7.2).
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
        CredentialKind, Provider,
        definition::{credential::Grant, eval::ValueEnv},
        oauth,
    },
    storage,
    storage::oauth_flows::{
        FlowGrant, FlowOutcome, FlowPurpose as FlowPurposeStorage, OngoingFlow,
    },
    unix_time,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

route! {
    pub(super) StartProviderOauth: {
        path: "/oauth:start",
        method: http::Method::POST,
        request: StartProviderOauthRequest,
        response: StartProviderOauthResponse,
    }
}

route! {
    pub(super) CompleteProviderOauth: {
        path: "/oauth:complete",
        method: http::Method::POST,
        request: CompleteProviderOauthRequest,
        response: CompleteProviderOauthResponse,
    }
}

route! {
    pub(super) ReauthorizeProvider: {
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
pub(super) struct StartProviderOauthRequest {
    /// Prefix prepended to each automatically exposed upstream model ID; empty is allowed.
    model_prefix: String,
    /// The definition name the flow authorizes against.
    provider_definition_name: String,

    /// The exact definition version the flow authorizes.
    provider_definition_version: i64,

    /// Name of the provider being created; must be non-empty.
    name: String,

    /// The instance's initial base URL; required when the definition addresses the base.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    base_url: Option<String>,
}

/// Strictly parsed `oauth:reauthorize` request: the flow replaces the credentials of one
/// existing provider, optionally against a different exact definition version.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ReauthorizeProviderRequest {
    /// Provider whose credentials the flow replaces.
    provider_id: i64,

    /// The definition name to reauthorize against; absent keeps the stored name. The target must
    /// be an OAuth definition.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    provider_definition_name: Option<String>,

    /// The exact target definition version; absent reauthorizes against the provider's current
    /// pair.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    provider_definition_version: Option<i64>,
}

/// Strictly parsed `oauth:complete` request: a union discriminated by the flow's grant (rule 4),
/// because an authorization-code completion pastes its code and a device completion polls its
/// stored outcome.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "grant", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) enum CompleteProviderOauthRequest {
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
pub(super) enum StartProviderOauthResponse {
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
pub(super) enum CompleteProviderOauthResponse {
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

/// The credential-kind rejection both flow-begin paths render: a definition a flow would run
/// against must carry OAuth credentials (README §7.1).
fn credential_kind_mismatch(definition: &crate::provider::ProviderDefinition) -> Error {
    Error::CredentialKindMismatch {
        message: format!(
            "Provider definition `{}` version {} carries `{}` credentials and \
             authorizes no flow",
            definition.name,
            definition.version.get(),
            definition.credential_kind().as_str(),
        ),
    }
}

/// Resolves the OAuth recipe a flow names: the exact definition version, which must carry an
/// OAuth credential.
async fn oauth_definition(
    database: &crate::database::Database,
    name: &str,
    version: i64,
) -> Result<crate::provider::ProviderDefinition, Error> {
    let definition = super::resolve_definition(database, name, version).await?;

    if definition.credential_kind() != CredentialKind::OAuth {
        return Err(credential_kind_mismatch(&definition));
    }

    Ok(definition)
}

/// Begins a creation flow and persists its row.
///
/// # Errors
///
/// Returns [`Error`] for empty names, unknown definitions, a non-OAuth definition, a definition
/// whose base-URL requirement the request cannot satisfy, upstream device-authorization
/// failures, and storage failures.
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

    // A definition whose URLs reference the base requires the instance base URL: the flow stores
    // it for the exchange and provisioning, and the created provider's every dispatch evaluates
    // those references against it (README §3, §7.1).
    if definition.payload.references_base() && request.base_url.is_none() {
        return Err(super::base_url_required(&definition));
    }

    // The pre-insert check renders a clean 409 before any upstream round-trip; the database's
    // UNIQUE constraint stays the concurrent backstop, whose loss the device poller classifies
    // as the flow's terminal `name_conflict` outcome (README §7.2).
    if storage::providers::is_provider_name_taken(database, &request.name).await? {
        return Err(Error::ProviderNameTaken {
            name: request.name.clone(),
        });
    }

    let slots = oauth::initialize_slots(&definition, &BTreeMap::new())?;

    begin(
        database,
        control_plane,
        definition,
        oauth::BeginPurpose::Create {
            provider_name: request.name,
            model_prefix: request.model_prefix,
        },
        request.base_url,
        slots,
    )
    .await
}

/// Begins a reauthorization flow and persists its row.
///
/// # Errors
///
/// Returns [`Error`] for unknown providers, unknown target definitions, a non-OAuth target, an
/// effective target whose base-URL requirement the provider row cannot satisfy, and upstream or
/// storage failures.
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

        (None, None) => {
            // The default target is the provider's own definition pair, and the request
            // validity condition is the explicit target's: the target must carry OAuth
            // credentials (README §7.1). A static-key provider reauthorized without an explicit
            // target is the same client fault, so it renders the same rejection instead of
            // failing deep in slot initialization as a gateway failure.
            let definition = provider.definition.clone();

            if definition.credential_kind() != CredentialKind::OAuth {
                return Err(credential_kind_mismatch(&definition));
            }

            definition
        }

        _ => {
            return Err(super::definition_rebind_invalid());
        }
    };

    // The flow runs from the provider's stored base URL, so an effective target definition — the
    // explicit target, or the provider's own pair — whose URLs reference the base is satisfiable
    // only while the row carries one (README §3, §7.1).
    if definition.payload.references_base() && provider.base_url.is_none() {
        return Err(super::stored_base_url_required(&definition));
    }

    let slots = oauth::initialize_slots(&definition, provider.credentials.slots())?;

    begin(
        database,
        control_plane,
        definition,
        oauth::BeginPurpose::Reauthorize {
            target_provider_id: provider.id,
        },
        provider.base_url,
        slots,
    )
    .await
}

/// Runs one resolved flow begin through the engine and renders its v0 response: the shared tail
/// of `oauth:start` and `oauth:reauthorize`. The engine owns the orchestration — state minting,
/// the authorize URL or the device authorization, the expiry resolution, and the storage insert
/// — and this surface owns the log and the wire mapping.
///
/// # Errors
///
/// Returns [`Error`] for the engine's begin failures: an authorize-URL evaluation failure, an
/// upstream device-authorization failure, and storage failures.
async fn begin(
    database: &crate::database::Database,
    control_plane: &Client,
    definition: crate::provider::ProviderDefinition,
    purpose: oauth::BeginPurpose,
    base_url: Option<String>,
    slots: BTreeMap<String, String>,
) -> Result<StartProviderOauthResponse, Error> {
    let began = oauth::begin_flow(
        database,
        control_plane,
        &definition,
        purpose,
        base_url,
        slots,
    )
    .await?;

    info!(
        "started a `{}` OAuth flow against definition `{}` version {}",
        began.flow.grant_type.as_str(),
        definition.name,
        definition.version.get()
    );

    Ok(begin_response(began))
}

/// Maps one began flow's engine outcome onto the v0 wire response: the state token and the
/// expiry read from the persisted row — the response's source of truth — and the grant's begin
/// facts mapped per variant.
fn begin_response(began: oauth::BeganFlow) -> StartProviderOauthResponse {
    match began.facts {
        oauth::BeginFacts::AuthorizationCode { authorize_url } => {
            StartProviderOauthResponse::AuthorizationCode {
                authorize_url,
                state: began.flow.state_token,
                expires_at: began.flow.expires_at,
            }
        }

        oauth::BeginFacts::Device {
            user_code,
            verification_uri,
            verification_uri_complete,
        } => StartProviderOauthResponse::Device {
            state: began.flow.state_token,
            expires_at: began.flow.expires_at,
            user_code,
            verification_uri,
            verification_uri_complete,
        },
    }
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

    let provider =
        complete_consumed_flow(database, control_plane, &definition, &flow, code).await?;

    Ok(CompleteProviderOauthResponse::Succeeded {
        provider: ProviderResponse::from_provider(provider),
    })
}

/// Completes one consumed authorization-code flow against its definition: a create flow whose
/// provider name another row claimed between the begin pre-check and this completion renders the
/// name-taken conflict before the exchange's round-trip, the exchange runs and its product
/// classifies, the shared completion pipeline folds the outcome through provisioning into the
/// stored credentials, and the provider write commits — creating the provider, or atomically
/// replacing the target's credentials, definition reference, and base URL.
///
/// # Errors
///
/// Returns [`Error`] for a taken create-flow provider name, exchange failures, provisioning
/// failures, a vanished reauthorization target, and storage failures.
async fn complete_consumed_flow(
    database: &crate::database::Database,
    control_plane: &Client,
    definition: &crate::provider::ProviderDefinition,
    flow: &OngoingFlow,
    code: &str,
) -> Result<Provider, Error> {
    // A create flow whose provider name another row already holds can never succeed. The
    // conflict renders the same clean 409 the begin path does, before the exchange's upstream
    // round-trip (README §7.1); the write's UNIQUE constraint stays the concurrent backstop,
    // exactly like the device poller's terminal `name_conflict` classification (README §7.2).
    // The flow row is already consumed either way — this classifies the loss, it does not
    // un-spend the flow.
    if flow.purpose == FlowPurposeStorage::Create {
        let name = flow.create_provider_name();

        if storage::providers::is_provider_name_taken(database, name).await? {
            return Err(Error::ProviderNameTaken {
                name: name.to_string(),
            });
        }
    }

    let recipe = definition.oauth_recipe().ok_or_else(|| Error::Internal {
        message: "the consumed flow's definition carries no OAuth credential".to_string(),
    })?;

    // The flow row's grant was checked before the consume; a definition whose grant disagrees
    // with the flow it issued is stored drift.
    let Grant::AuthorizationCode { redirect_uri, .. } = &recipe.grant else {
        return Err(Error::Internal {
            message: "the consumed flow's definition carries no authorization-code grant"
                .to_string(),
        });
    };

    let env = ValueEnv::for_exchange(
        &flow.slots,
        flow.base_url.as_deref(),
        recipe.client_id.as_str(),
        recipe.client_secret.as_deref(),
        code,
        flow.code_verifier.as_deref(),
        redirect_uri.as_str(),
    );

    let outcome = oauth::completed_outcome(
        oauth::execute_exchange(control_plane, &recipe.exchange, &env).await?,
    )?;

    let product = oauth::complete_exchange(control_plane, definition, outcome, flow).await?;

    let mut transaction = database
        .transaction()
        .await
        .map_err(|error| Error::Storage(error.into()))?;

    let Some(provider_id) =
        oauth::write_completion_provider(&mut transaction, flow, definition, &product).await?
    else {
        // Only a reauthorize flow's target can vanish mid-write; a create flow always inserts.
        return Err(Error::ProviderVanishedDuringReauthorization {
            id: flow.reauthorize_target_id(),
        });
    };

    transaction
        .commit()
        .await
        .map_err(|error| Error::Storage(error.into()))?;

    let provider = storage::providers::find_provider_by_id(database, provider_id)
        .await?
        .ok_or(Error::ProviderVanishedDuringReauthorization { id: provider_id })?;

    match flow.purpose {
        FlowPurposeStorage::Create => {
            info!(
                "created provider `{}` from an OAuth completion",
                provider.name
            );
        }

        FlowPurposeStorage::Reauthorize => {
            info!("refreshed the credentials of provider {provider_id}");
        }
    }

    Ok(provider)
}

/// Reads a device flow's relational outcome after the pre-consume grant check: pending stays
/// unconsumed, a terminal row is consumed exactly once and its facts returned.
async fn complete_device(
    database: &crate::database::Database,
    state: &str,
) -> Result<CompleteProviderOauthResponse, Error> {
    let now = unix_time::unix_now_micros();

    let Some(flow) = storage::oauth_flows::find_flow_by_state(database, state, now).await? else {
        return Err(Error::UnknownOauthState);
    };

    // The pre-consume grant check mirrors the authorization-code form's: a device-form completion
    // of an authorization-code state is a client fault, not a permanently pending flow — the
    // schema makes a terminal outcome unrepresentable on an authorization-code row, so the
    // mismatched form would answer `pending` until the flow's expiry without this rejection.
    if flow.grant_type != FlowGrant::Device {
        return Err(Error::OauthGrantMismatch {
            state_grant: flow.grant_type.as_str().to_string(),
        });
    }

    match flow.outcome {
        None => Ok(CompleteProviderOauthResponse::Pending {}),

        Some(FlowOutcome::Succeeded) => {
            let result_provider_id = flow.result_provider_id();

            // The result provider resolves before the consume, so a provider row deleted
            // between the poller's commit and this retrieval never spends the one permitted
            // retrieval on a 404 — the succeeded fact stays retrievable until the expiry sweep
            // removes the row. Two racing successful reads may both resolve; the consume below
            // still answers exactly one of them.
            let provider = storage::providers::find_provider_by_id(database, result_provider_id)
                .await?
                .ok_or(Error::ProviderNotFound {
                    id: result_provider_id,
                })?;

            storage::oauth_flows::consume_terminal_flow(database, &flow.state_token, now)
                .await?
                .ok_or(Error::UnknownOauthState)?;

            Ok(CompleteProviderOauthResponse::Succeeded {
                provider: ProviderResponse::from_provider(provider),
            })
        }

        Some(FlowOutcome::Failed) => {
            let (failure_code, failure_message) = flow.failure_pair();

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
