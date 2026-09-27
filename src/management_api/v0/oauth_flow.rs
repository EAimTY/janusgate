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
        definition::{
            credential::{CredentialAcquisition, Grant},
            eval::ValueEnv,
        },
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
    /// The definition name the flow authorizes against.
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
pub(super) struct ReauthorizeProviderRequest {
    /// Provider whose credentials the flow replaces.
    pub provider_id: i64,

    /// The definition name to reauthorize against; absent keeps the stored name. The target must
    /// be an OAuth definition.
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

/// The definition-reference rejection this surface renders for a malformed reference, wrapping
/// the message in the version's invalid-request error.
fn invalid_definition(message: impl Into<String>) -> Error {
    Error::InvalidDefinition(crate::provider::definition::ValidationError::new(message))
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
            return Err(invalid_definition(
                "a definition rebind names both `provider_definition_name` and \
                 `provider_definition_version`",
            ));
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
        provider: ProviderResponse::from_provider(&provider),
    })
}

/// Completes one consumed authorization-code flow against its definition: the exchange runs and
/// its product classifies, the shared completion pipeline folds the outcome through provisioning
/// into the stored credentials, and the provider write commits — creating the provider, or
/// atomically replacing the target's credentials, definition reference, and base URL.
///
/// # Errors
///
/// Returns [`Error`] for exchange failures, provisioning failures, a vanished reauthorization
/// target, and storage failures.
async fn complete_consumed_flow(
    database: &crate::database::Database,
    control_plane: &Client,
    definition: &crate::provider::ProviderDefinition,
    flow: &OngoingFlow,
    code: &str,
) -> Result<Provider, Error> {
    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return Err(Error::Internal {
            message: "the consumed flow's definition carries no OAuth credential".to_string(),
        });
    };

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
        let target = flow.target_provider_id.unwrap_or_default();

        return Err(Error::ProviderVanishedDuringReauthorization { id: target });
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
            let result_provider_id = flow.result_provider_id.unwrap_or_default();

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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{
        Error, ReauthorizeProviderRequest, StartProviderOauthRequest, StartProviderOauthResponse,
        complete_authorization_code, complete_device, handle_reauthorize_start, handle_start,
    };
    use crate::{
        format::UpstreamFormat,
        provider::StoredCredentials,
        storage::oauth_flows::{
            FlowGrant, FlowOutcome, FlowPurpose, NewFlow, claim_due_device_flow,
            commit_device_success_in_transaction, find_flow_by_state, insert_flow,
        },
        test_support,
    };
    use std::collections::BTreeMap;

    /// A reauthorization without an explicit target is valid only against an OAuth provider
    /// (README §7.1): against a static-key provider it is the same client fault the explicit
    /// target produces, not a gateway failure surfaced from slot initialization.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn reauthorizing_a_static_key_provider_defaults_to_the_kind_mismatch() {
        let (database, _guard) = test_support::create_test_database().await;

        let static_definition = test_support::insert_static_key_definition(
            &database,
            "static-def",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        let provider = test_support::insert_provider(
            &database,
            &static_definition,
            "static-provider",
            "s-",
            None,
            StoredCredentials::ApiKey {
                api_key: "sk".to_string(),
            },
        )
        .await;

        let error = handle_reauthorize_start(
            &database,
            &crate::http_client::control_plane(),
            ReauthorizeProviderRequest {
                provider_id: provider.id,
                provider_definition_name: None,
                provider_definition_version: None,
            },
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::CredentialKindMismatch { message } = error else {
            panic!("a static-key default target must render the credential-kind mismatch");
        };

        assert!(message.contains("static-def"), "{message}");
    }

    /// A reauthorization without an explicit target against an OAuth provider begins the flow
    /// from the provider's own definition pair.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the flow does not begin.
    #[tokio::test]
    async fn reauthorizing_an_oauth_provider_against_its_own_definition_begins() {
        let (database, _guard) = test_support::create_test_database().await;

        let oauth_definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        // The fixture definition addresses its base URL, so the provider row carries one.
        let provider = test_support::insert_provider(
            &database,
            &oauth_definition,
            "oauth-provider",
            "o-",
            Some("https://upstream.example"),
            StoredCredentials::OAuth {
                access_token: "old".to_string(),
                refresh_token: None,
                slots: std::collections::BTreeMap::new(),
                expires_at: None,
                refresh_started_at: None,
            },
        )
        .await;

        let response = handle_reauthorize_start(
            &database,
            &crate::http_client::control_plane(),
            ReauthorizeProviderRequest {
                provider_id: provider.id,
                provider_definition_name: None,
                provider_definition_version: None,
            },
        )
        .await
        .unwrap();

        #[expect(
            clippy::panic,
            reason = "a divergent response variant is the failure report"
        )]
        let StartProviderOauthResponse::AuthorizationCode { state, .. } = response else {
            panic!("an authorization-code definition begins an authorization-code flow");
        };

        assert!(!state.is_empty());
    }

    /// `oauth:start` rejects a provider name another row already holds with the same 409 the
    /// creation route renders, before any upstream round-trip: the flow that would lose the
    /// commit-time race never starts (README §7.2).
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn starting_a_flow_with_a_taken_provider_name_is_a_conflict() {
        let (database, _guard) = test_support::create_test_database().await;

        let static_definition = test_support::insert_static_key_definition(
            &database,
            "static-def",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        test_support::insert_provider(
            &database,
            &static_definition,
            "taken",
            "t-",
            None,
            StoredCredentials::ApiKey {
                api_key: "sk".to_string(),
            },
        )
        .await;

        let oauth_definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        // The fixture definition addresses its base URL, so the request carries the base URL the
        // flow needs to reach the name-taken check.
        let error = handle_start(
            &database,
            &crate::http_client::control_plane(),
            StartProviderOauthRequest {
                provider_definition_name: oauth_definition.name.clone(),
                provider_definition_version: oauth_definition.version.get(),
                name: "taken".to_string(),
                model_prefix: String::new(),
                base_url: Some("https://upstream.example".to_string()),
            },
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::ProviderNameTaken { name } = error else {
            panic!("a taken provider name must render the name-taken conflict");
        };

        assert_eq!(name, "taken");
    }

    /// `oauth:start` rejects a definition whose URLs reference the base when the request carries
    /// no `base_url` (README §7.1): the flow would mint a provider whose every dispatch evaluates
    /// those references against a missing base URL.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn starting_a_flow_against_a_base_addressing_definition_requires_the_base_url() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        let error = handle_start(
            &database,
            &crate::http_client::control_plane(),
            StartProviderOauthRequest {
                provider_definition_name: definition.name.clone(),
                provider_definition_version: definition.version.get(),
                name: "created".to_string(),
                model_prefix: String::new(),
                base_url: None,
            },
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::Validation { param, message } = error else {
            panic!("a missing base URL renders the base_url validation rejection");
        };

        assert_eq!(param, "base_url");
        assert!(message.contains("oauth-def"), "{message}");
    }

    /// `oauth:reauthorize` rejects an effective target definition whose URLs reference the base
    /// when the provider row carries no base URL (README §7.1): the flow runs from the stored
    /// base, so such a target — the provider's own pair, or an explicitly named version — is
    /// unsatisfiable without one.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn reauthorizing_without_a_stored_base_url_against_a_base_addressing_target_is_rejected()
    {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        let provider = test_support::insert_provider(
            &database,
            &definition,
            "oauth-provider",
            "o-",
            None,
            StoredCredentials::OAuth {
                access_token: "old".to_string(),
                refresh_token: None,
                slots: std::collections::BTreeMap::new(),
                expires_at: None,
                refresh_started_at: None,
            },
        )
        .await;

        let error = handle_reauthorize_start(
            &database,
            &crate::http_client::control_plane(),
            ReauthorizeProviderRequest {
                provider_id: provider.id,
                provider_definition_name: None,
                provider_definition_version: None,
            },
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::Validation { param, message } = error else {
            panic!("a missing stored base URL renders the base_url validation rejection");
        };

        assert_eq!(param, "base_url");
        assert!(message.contains("oauth-def"), "{message}");
    }

    /// A succeeded device flow whose result provider row no longer exists answers the 404
    /// without consuming the terminal row (README §7.2): the single-consume retrieval is spent
    /// only by a read that succeeds, so the flow's "succeeded" fact survives the failed
    /// retrieval until the expiry sweep removes it.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the outcome diverges.
    #[tokio::test]
    async fn completing_a_succeeded_device_flow_whose_provider_vanished_keeps_the_row() {
        let (database, _guard) = test_support::create_test_database().await;

        // The flow row references one exact definition version through a foreign key, so the
        // fixture inserts the definition it names.
        let definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
                model_prefix: Some(String::new()),
                base_url: None,
                target_provider_id: None,
                grant_type: FlowGrant::Device,
                slots: BTreeMap::new(),
                code_verifier: None,
                device_code: Some("device-code".to_string()),
                device_user_code: Some("user-code".to_string()),
                device_verification_uri: Some("https://verify.example".to_string()),
                device_verification_uri_complete: None,
                device_poll_interval_seconds: Some(5),
                device_next_poll_at: Some(now - 1),
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        // The claim spends just enough to record the terminal success against a provider id no
        // row carries — `result_provider_id` is a bare integer without a foreign key (README
        // §3), which is exactly the deleted-provider state a real flow can reach.
        #[expect(clippy::expect_used, reason = "the due flow is claimed")]
        let claimed = claim_due_device_flow(&database, now, now - 1)
            .await
            .unwrap()
            .expect("the due flow is claimed");

        let claim = claimed.device_poll_started_at.unwrap();

        let mut transaction = database.transaction().await.unwrap();

        let is_committed =
            commit_device_success_in_transaction(&mut transaction, claimed.id, claim, 987_654)
                .await
                .unwrap();

        assert!(is_committed, "the claim is held");

        transaction.commit().await.unwrap();

        let error = complete_device(&database, "state-token").await.unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::ProviderNotFound { id } = error else {
            panic!("a vanished result provider renders the provider 404");
        };

        assert_eq!(id, 987_654);

        // The terminal row was not consumed: the succeeded outcome stays retrievable.
        #[expect(clippy::expect_used, reason = "the terminal row survives")]
        let settled = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the terminal row survives the failed retrieval");

        assert_eq!(settled.outcome, Some(FlowOutcome::Succeeded));
    }

    /// A device-form completion naming an authorization-code flow's state is a grant mismatch
    /// rejected before any consumption (README §7.1, §7.2): the flow stays pending with no
    /// outcome — the schema makes a terminal outcome unrepresentable on an authorization-code
    /// row — so the flow's own completion form still observes it afterwards.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn completing_an_authorization_code_flow_through_the_device_form_is_a_grant_mismatch() {
        let (database, _guard) = test_support::create_test_database().await;

        // The flow row references one exact definition version through a foreign key, so the
        // fixture inserts the definition it names.
        let definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        let now = crate::unix_time::unix_now_micros();

        insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "state-token".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("created".to_string()),
                model_prefix: Some(String::new()),
                base_url: None,
                target_provider_id: None,
                grant_type: FlowGrant::AuthorizationCode,
                slots: BTreeMap::new(),
                code_verifier: None,
                device_code: None,
                device_user_code: None,
                device_verification_uri: None,
                device_verification_uri_complete: None,
                device_poll_interval_seconds: None,
                device_next_poll_at: None,
                expires_at: now + 600 * crate::unix_time::MICROS_PER_SEC,
            },
        )
        .await
        .unwrap();

        let error = complete_device(&database, "state-token").await.unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::OauthGrantMismatch { state_grant } = error else {
            panic!(
                "a device-form completion of an authorization-code state renders the grant \
                 mismatch"
            );
        };

        assert_eq!(state_grant, FlowGrant::AuthorizationCode.as_str());

        // The mismatch is rejected before consumption: the row survives unconsumed.
        #[expect(clippy::expect_used, reason = "the unconsumed row survives")]
        let surviving = find_flow_by_state(&database, "state-token", now)
            .await
            .unwrap()
            .expect("the mismatched flow is not consumed");

        assert_eq!(surviving.grant_type, FlowGrant::AuthorizationCode);
        assert_eq!(surviving.outcome, None);

        // The flow's own completion form still observes the row: its pre-consume checks pass and
        // the exchange runs — failing against the fixture's unreachable token endpoint as a
        // gateway-side upstream failure, never as an unknown-state or grant rejection.
        let retry = complete_authorization_code(
            &database,
            &crate::http_client::control_plane(),
            "state-token",
            "code",
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::OAuth(crate::provider::Error::Unavailable { .. }) = retry else {
            panic!(
                "the retrying authorization-code completion reaches its exchange, not a state or \
                 grant rejection"
            );
        };
    }
}
