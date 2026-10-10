//! Management routes listing, creating, rewriting, and deleting providers.

use super::{EmptyResponse, Error, ProviderEnvelope, ProviderResponse, absent_keeps, route};
use crate::{
    context::Context,
    provider::{CredentialKind, StoredCredentials},
    storage,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use serde::{Deserialize, Serialize};

route! {
    pub(super) ListProviders: {
        path: "/providers:list",
        method: http::Method::POST,
        request: ListProvidersRequest,
        response: ListProvidersResponse,
    }
}

route! {
    pub(super) CreateProvider: {
        path: "/providers:create",
        method: http::Method::POST,
        request: CreateProviderRequest,
        response: ProviderEnvelope,
    }
}

route! {
    pub(super) UpdateProvider: {
        path: "/providers:update",
        method: http::Method::POST,
        request: UpdateProviderRequest,
        response: EmptyResponse,
    }
}

route! {
    pub(super) DeleteProvider: {
        path: "/providers:delete",
        method: http::Method::POST,
        request: DeleteProviderRequest,
        response: EmptyResponse,
    }
}

/// The `providers:list` request body (`docs/reference/management-api.md`): the empty request of a
/// no-argument route.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListProvidersRequest {}

/// The `providers:list` success body (`docs/reference/management-api.md`).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListProvidersResponse {
    /// Every stored provider, ordered by name.
    providers: Vec<ProviderResponse>,
}

/// Strictly parsed `providers:create` request: the complete state of one static-key provider
/// (full-state writes). The request references one exact definition version, whose credential
/// recipe must be the static-key one — OAuth credentials are minted by `oauth:complete` alone.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateProviderRequest {
    /// Prefix prepended to each automatically exposed upstream model ID; empty is allowed.
    model_prefix: String,
    /// The definition name the provider binds to.
    provider_definition_name: String,

    /// The exact definition version the provider binds to.
    provider_definition_version: i64,

    /// Unique provider name; must be non-empty.
    name: String,

    /// Serving state.
    is_enabled: bool,

    /// Upstream base URL; required when the definition's URLs reference the base, `null` when
    /// they do not.
    base_url: Option<String>,

    /// Static API key; non-empty printable ASCII without whitespace.
    api_key: String,
}

/// Strictly parsed `providers:update` request (full-state writes): the complete desired readable
/// state of one provider row. The write-only `api_key` is the absent-keeps exception — absent keeps
/// the stored key, present replaces it. An explicit definition rebind names both parts of the
/// target reference; the write validates the target version's existence and the credential-kind
/// agreement atomically. A rebind onto an api_key-target definition requires the replacement
/// `api_key` in the same request, whatever the row's current kind, while an OAuth-target
/// definition is reachable only through `oauth:reauthorize`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateProviderRequest {
    /// Prefix prepended to each automatically exposed upstream model ID; empty is allowed.
    model_prefix: String,
    /// Addressed provider row.
    provider_id: i64,

    /// Unique provider name; a differing name renames the provider.
    name: String,

    /// Serving state.
    is_enabled: bool,

    /// Upstream base URL; required when the definition's URLs reference the base, `null` when
    /// they do not.
    base_url: Option<String>,

    /// The definition name to rebind to; absent keeps the stored reference.
    #[serde(
        default,
        deserialize_with = "absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    provider_definition_name: Option<String>,

    /// The definition version to rebind to; absent keeps the stored reference.
    #[serde(
        default,
        deserialize_with = "absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    provider_definition_version: Option<i64>,

    /// Replacement static API key; absent keeps the stored one, present must be non-empty
    /// printable ASCII without whitespace.
    #[serde(
        default,
        deserialize_with = "absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    api_key: Option<String>,
}

/// Strictly parsed `providers:delete` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DeleteProviderRequest {
    /// Addressed provider row.
    provider_id: i64,
}

/// Handles `POST /providers:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(_request), _): WithRejection<Json<ListProvidersRequest>, Error>,
) -> Result<Json<ListProvidersResponse>, Error> {
    let providers = storage::providers::list_providers(&context.database).await?;

    Ok(Json(ListProvidersResponse {
        providers: providers
            .into_iter()
            .map(ProviderResponse::from_provider)
            .collect(),
    }))
}

/// Handles `POST /providers:create`.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateProviderRequest>, Error>,
) -> Result<Json<ProviderEnvelope>, Error> {
    let provider = handle_create(&context.database, request).await?;

    Ok(Json(ProviderEnvelope {
        provider: ProviderResponse::from_provider(provider),
    }))
}

/// Creates one static-key provider row against its referenced definition version.
///
/// # Errors
///
/// Returns [`Error`] for empty names, unknown definition versions, an OAuth definition whose
/// credentials only a flow mints, a base URL the definition requires being absent, and taken
/// names.
async fn handle_create(
    database: &crate::database::Database,
    request: CreateProviderRequest,
) -> Result<crate::provider::Provider, Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_model_prefix(&request.model_prefix)?;
    super::validate_composition("api_key", &request.api_key)?;

    if let Some(base_url) = &request.base_url {
        super::validate_base_url(base_url)?;
    }

    let definition = super::resolve_definition(
        database,
        &request.provider_definition_name,
        request.provider_definition_version,
    )
    .await?;

    // Only static-key definitions accept a direct write; OAuth credentials are minted by the
    // completion of a flow.
    if definition.credential_kind() != CredentialKind::ApiKey {
        return Err(Error::CredentialKindMismatch {
            message: format!(
                "Provider definition `{}` version {} carries `{}` credentials, which only an \
                 OAuth flow mints",
                definition.name,
                definition.version.get(),
                definition.credential_kind().as_str(),
            ),
        });
    }

    // A definition whose URLs reference the base requires the instance base URL: every dispatch
    // evaluates those references against the stored row (`docs/design/storage.md` and
    // `docs/reference/management-api.md`).
    if definition.payload.references_base() && request.base_url.is_none() {
        return Err(super::base_url_required(&definition));
    }

    // The pre-insert check renders a clean 409; the database's UNIQUE constraint is the
    // concurrent backstop.
    if storage::providers::is_provider_name_taken(database, &request.name).await? {
        return Err(Error::ProviderNameTaken {
            name: request.name.clone(),
        });
    }

    let credentials = StoredCredentials::ApiKey {
        api_key: request.api_key,
    };

    let provider = storage::providers::insert_provider(
        database,
        storage::providers::NewProvider {
            name: &request.name,
            model_prefix: &request.model_prefix,
            base_url: request.base_url.as_deref(),
            credentials: &credentials,
            definition: &definition,
            is_enabled: request.is_enabled,
        },
    )
    .await?;

    info!("created provider `{}`", request.name);

    Ok(provider)
}

/// Handles `POST /providers:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateProviderRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    handle_update(&context.database, request).await?;

    Ok(Json(EmptyResponse {}))
}

/// Handles `POST /providers:delete`.
/// Deletion and the quota coordinator's slot invalidation share the version's detached mutation
/// owner across caller cancellation: ids are never reused, so a committed write whose invalidation
/// is lost to a cancelled caller would leave a stale slot forever.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider and storage failures.
///
/// # Panics
///
/// Panics if the mutation task fails to hand off its result, a fail-fast task invariant.
pub(super) async fn delete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DeleteProviderRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    let database = context.database.clone();
    let coordinator = context.quota_coordinator.clone();
    let provider_id = request.provider_id;
    let is_deleted = super::detach_mutation(async move {
        let is_deleted = storage::providers::delete_provider(&database, provider_id).await?;
        if is_deleted {
            coordinator.forget(provider_id);
        }
        Ok(is_deleted)
    })
    .await?;

    if !is_deleted {
        return Err(Error::ProviderNotFound {
            id: request.provider_id,
        });
    }

    info!("deleted provider {}", request.provider_id);

    Ok(Json(EmptyResponse {}))
}

/// Rewrites one provider's full state per the management contract.
///
/// # Errors
///
/// Returns [`Error`] for empty names, unknown ids, absent-keeps violations, taken names,
/// public-ID collisions, a rebind naming a non-positive or unknown target version or
/// disagreeing credential kinds, and a base URL the effective definition requires being absent.
async fn handle_update(
    database: &crate::database::Database,
    request: UpdateProviderRequest,
) -> Result<(), Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_model_prefix(&request.model_prefix)?;

    if let Some(base_url) = &request.base_url {
        super::validate_base_url(base_url)?;
    }

    if let Some(api_key) = &request.api_key {
        super::validate_composition("api_key", api_key)?;
    }

    if request.provider_definition_name.is_some() != request.provider_definition_version.is_some() {
        return Err(super::definition_rebind_invalid());
    }

    // A non-positive rebind version is the shared malformed-parameter rejection, judged before
    // any storage read; the pairing rule above guarantees both halves ride together.
    let rebind_version = request
        .provider_definition_version
        .map(|version| super::definition_version(version, "provider_definition_version"))
        .transpose()?;

    let rebind = request
        .provider_definition_name
        .as_deref()
        .zip(rebind_version);

    let credentials = request
        .api_key
        .map(|api_key| StoredCredentials::ApiKey { api_key });

    // The outer read pins request validation only; the rebind validation and the rename check
    // re-read the row inside the transactional write.
    let Some(existing) =
        storage::providers::find_provider_by_id(database, request.provider_id).await?
    else {
        return Err(Error::ProviderNotFound {
            id: request.provider_id,
        });
    };

    // The rebind target, resolved for the clean rejections; an unknown target defers to the
    // transactional write's 404.
    let rebind_target = match rebind {
        Some((name, version)) => {
            storage::provider_definitions::find_definition(database, name, version).await?
        }

        None => None,
    };

    // The effective definition both pre-checks judge: the named rebind target when a rebind is
    // present, the stored definition otherwise. A named rebind whose target does not exist
    // defers both entirely — the transactional write's unknown-definition 404 is the real
    // answer, and judging the stored definition here would reject a request whose actual fault
    // is the target.
    let effective_definition = if rebind.is_some() {
        rebind_target.as_ref()
    } else {
        Some(&existing.definition)
    };

    // A replacement key serves an api_key-target definition alone (`docs/design/storage.md`): an
    // OAuth definition's credentials are minted by flows, so a keyed write aimed at one — the
    // stored definition on the no-rebind path, the named target on a rebind — is a client
    // fault. A keyed write against an api_key target is accepted regardless of the row's
    // current kind — the documented static-key write that converts an OAuth provider onto a
    // static-key definition — with the storage write's transactional kind check as the
    // backstop.
    if let Some(definition) = effective_definition
        && credentials.is_some()
        && definition.credential_kind() != CredentialKind::ApiKey
    {
        return Err(Error::CredentialKindMismatch {
            message: format!(
                "Provider definition `{}` version {} carries `{}` credentials, which only an \
                 OAuth flow mints — reauthorize the provider with `oauth:reauthorize` instead \
                 of writing a replacement key",
                definition.name,
                definition.version.get(),
                definition.credential_kind().as_str(),
            ),
        });
    }

    // A rebind whose target kind differs from the effective credentials is rejected with the
    // remedy the target's credential kind admits. Only a keyless rebind arrives here: a keyed
    // write was already judged against the target's kind above. An `oauth` target's credentials
    // are only ever minted by a flow, so the message names the `oauth:reauthorize` remedy
    // against this definition, while an `api_key` target's remedy — the replacement static key
    // the request itself can carry — is satisfiable, so the message asks for it.
    if let Some(target) = &rebind_target {
        let credentials_kind = credentials
            .as_ref()
            .map(crate::provider::StoredCredentials::kind)
            .unwrap_or_else(|| existing.credential_kind());

        if credentials_kind != target.credential_kind() {
            let message = if target.credential_kind() == CredentialKind::OAuth {
                format!(
                    "Provider definition `{}` version {} carries `{}` credentials, which only \
                     an OAuth flow mints — reauthorize the provider against this definition \
                     with `oauth:reauthorize`",
                    target.name,
                    target.version.get(),
                    target.credential_kind().as_str(),
                )
            } else {
                format!(
                    "Provider definition `{}` version {} carries `{}` credentials; the \
                     provider carries `{}` credentials, so replacement credentials are required",
                    target.name,
                    target.version.get(),
                    target.credential_kind().as_str(),
                    credentials_kind.as_str(),
                )
            };

            return Err(Error::CredentialKindMismatch { message });
        }
    }

    // The base-URL requirement's pre-check mirrors the write's transactional one with the
    // cleaner 400: on the no-rebind path it holds against the stored definition, so a full-state
    // write can never null out the base URL a base-addressing provider's every dispatch
    // evaluates; on a named rebind it holds against the resolved target (`docs/design/storage.md`
    // and `docs/reference/management-api.md`).
    if let Some(definition) = effective_definition
        && definition.payload.references_base()
        && request.base_url.is_none()
    {
        return Err(super::base_url_required(definition));
    }

    storage::providers::update_provider(
        database,
        request.provider_id,
        storage::providers::ProviderUpdate {
            name: &request.name,
            model_prefix: &request.model_prefix,
            base_url: request.base_url.as_deref(),
            is_enabled: request.is_enabled,
            definition: rebind,
            credentials: credentials.as_ref(),
        },
    )
    .await?;

    info!("updated provider `{}`", request.name);

    Ok(())
}
