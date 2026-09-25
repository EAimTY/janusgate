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
use std::num::NonZeroI64;

route! {
    pub ListProviders: {
        path: "/providers:list",
        method: http::Method::POST,
        request: ListProvidersRequest,
        response: ListProvidersResponse,
    }
}

route! {
    pub CreateProvider: {
        path: "/providers:create",
        method: http::Method::POST,
        request: CreateProviderRequest,
        response: ProviderEnvelope,
    }
}

route! {
    pub UpdateProvider: {
        path: "/providers:update",
        method: http::Method::POST,
        request: UpdateProviderRequest,
        response: EmptyResponse,
    }
}

route! {
    pub DeleteProvider: {
        path: "/providers:delete",
        method: http::Method::POST,
        request: DeleteProviderRequest,
        response: EmptyResponse,
    }
}

/// The `providers:list` request body (README §7.1): the empty request of a no-argument route.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListProvidersRequest {}

/// The `providers:list` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListProvidersResponse {
    /// Every stored provider, ordered by name.
    pub providers: Vec<ProviderResponse>,
}

/// Strictly parsed `providers:create` request: the complete state of one static-key provider
/// (rule 3). The request references one exact definition version, whose credential recipe must
/// be the static-key one — OAuth credentials are minted by `oauth:complete` alone.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct CreateProviderRequest {
    /// The definition name the provider binds to.
    pub provider_definition_name: String,

    /// The exact definition version the provider binds to.
    pub provider_definition_version: i64,

    /// Unique provider name; must be non-empty.
    pub name: String,

    /// Public model name prefix; may be empty, otherwise routing-safe.
    pub model_prefix: String,

    /// Serving state.
    pub is_enabled: bool,

    /// Upstream base URL; required when the definition's URLs reference the base, `null` when
    /// they do not.
    pub base_url: Option<String>,

    /// Static API key; non-empty printable ASCII without whitespace.
    pub api_key: String,
}

/// Strictly parsed `providers:update` request (rule 3): the complete desired readable state of
/// one provider row. The write-only `api_key` is the absent-keeps exception — absent keeps the
/// stored key, present replaces it. An explicit definition rebind names both parts of the
/// target reference; the write validates the target version's existence and the credential-kind
/// agreement atomically, and a different credential kind requires the replacement `api_key` in
/// the same request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct UpdateProviderRequest {
    /// Addressed provider row.
    pub provider_id: i64,

    /// Unique provider name; a differing name renames the provider.
    pub name: String,

    /// Public model name prefix; may be empty, otherwise routing-safe.
    pub model_prefix: String,

    /// Serving state.
    pub is_enabled: bool,

    /// Upstream base URL; required when the definition's URLs reference the base, `null` when
    /// they do not.
    pub base_url: Option<String>,

    /// The definition name to rebind to; absent keeps the stored reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_definition_name: Option<String>,

    /// The definition version to rebind to; absent keeps the stored reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_definition_version: Option<i64>,

    /// Replacement static API key; absent keeps the stored one, present must be non-empty
    /// printable ASCII without whitespace.
    #[serde(
        default,
        deserialize_with = "absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub api_key: Option<String>,
}

/// Strictly parsed `providers:delete` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DeleteProviderRequest {
    /// Addressed provider row.
    pub provider_id: i64,
}

/// Handles `POST /providers:list`.
pub(super) async fn list(
    State(context): State<Context>,
) -> Result<Json<ListProvidersResponse>, Error> {
    let providers = storage::providers::list_providers(&context.database).await?;

    Ok(Json(ListProvidersResponse {
        providers: providers
            .iter()
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
        provider: ProviderResponse::from_provider(&provider),
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

    let Some(version) = NonZeroI64::new(request.provider_definition_version) else {
        return Err(definition_version_invalid());
    };

    let Some(definition) = storage::provider_definitions::find_definition(
        database,
        &request.provider_definition_name,
        version,
    )
    .await?
    else {
        return Err(Error::DefinitionNotFound {
            name: request.provider_definition_name.clone(),
            version: request.provider_definition_version,
        });
    };

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
    // evaluates those references against the stored row (README §3, §7.1).
    if definition.payload.references_base() && request.base_url.is_none() {
        return Err(base_url_required(&definition));
    }

    // The pre-insert check renders a clean 409; the database's UNIQUE constraint is the
    // concurrent backstop.
    if storage::providers::find_provider(database, &request.name)
        .await?
        .is_some()
    {
        return Err(Error::ProviderNameTaken {
            name: request.name.clone(),
        });
    }

    let credentials = StoredCredentials::ApiKey {
        api_key: request.api_key.clone(),
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

/// Builds the rejection for a non-positive `provider_definition_version` — the one spelling both
/// provider write paths share.
fn definition_version_invalid() -> Error {
    Error::InvalidDefinition(crate::provider::definition::ValidationError::new(
        "`provider_definition_version` must be a positive version number",
    ))
}

/// Builds the 400 the provider writes render when the effective definition's URLs reference the
/// base but no base URL is supplied (README §3, §7.1).
fn base_url_required(definition: &crate::provider::ProviderDefinition) -> Error {
    Error::Validation {
        param: "base_url",
        message: format!(
            "`base_url` is required: provider definition `{}` version {} addresses its base \
             URL, so the provider row must carry one",
            definition.name,
            definition.version.get(),
        ),
    }
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
pub(super) async fn delete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DeleteProviderRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    if !storage::providers::delete_provider(&context.database, request.provider_id).await? {
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
/// public-ID collisions, a rebind whose target version or credential kinds disagree, and a base
/// URL the effective definition requires being absent.
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
        return Err(Error::InvalidDefinition(
            crate::provider::definition::ValidationError::new(
                "a definition rebind names both `provider_definition_name` and \
                 `provider_definition_version`",
            ),
        ));
    }

    let rebind = match (
        request.provider_definition_name.as_deref(),
        request
            .provider_definition_version
            .and_then(NonZeroI64::new),
    ) {
        (Some(name), Some(version)) => Some((name, version)),
        (None, None) => None,

        _ => {
            return Err(definition_version_invalid());
        }
    };

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

    // An API key is the only hand-replaceable credential: an OAuth row's credentials are
    // minted by flows, so a replacement key on one is a client fault.
    if credentials.is_some() && existing.credential_kind() != CredentialKind::ApiKey {
        return Err(Error::CredentialKindMismatch {
            message: format!(
                "Provider `{}` carries `{}` credentials, which only an OAuth flow replaces",
                existing.name,
                existing.credential_kind().as_str(),
            ),
        });
    }

    // The rebind target, resolved for the clean rejections; an unknown target defers to the
    // transactional write's 404.
    let rebind_target = match rebind {
        Some((name, version)) => {
            storage::provider_definitions::find_definition(database, name, version).await?
        }

        None => None,
    };

    // A rebind whose target kind differs from the stored credentials requires the replacement
    // key in the same request; without a replacement the kinds must already agree.
    if let Some(target) = &rebind_target {
        let credentials_kind = credentials
            .as_ref()
            .map(crate::provider::StoredCredentials::kind)
            .unwrap_or_else(|| existing.credential_kind());

        if credentials_kind != target.credential_kind() {
            return Err(Error::CredentialKindMismatch {
                message: format!(
                    "Provider definition `{}` version {} carries `{}` credentials; the \
                     provider carries `{}` credentials, so replacement credentials are required",
                    target.name,
                    target.version.get(),
                    target.credential_kind().as_str(),
                    credentials_kind.as_str(),
                ),
            });
        }
    }

    // The base-URL requirement's pre-check mirrors the write's transactional one with the
    // cleaner 400: on the no-rebind path it holds against the stored definition, so a full-state
    // write can never null out the base URL a base-addressing provider's every dispatch
    // evaluates; on a named rebind it holds against the resolved target (README §3, §7.1). A
    // named rebind whose target does not exist defers the check entirely — the transactional
    // write's unknown-definition 404 is the real answer, and judging the stored definition here
    // would reject a request whose actual fault is the target.
    let precheck_definition = if rebind.is_some() {
        rebind_target.as_ref()
    } else {
        Some(&existing.definition)
    };

    if let Some(definition) = precheck_definition
        && definition.payload.references_base()
        && request.base_url.is_none()
    {
        return Err(base_url_required(definition));
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

#[cfg(test)]
mod tests {
    use super::{Error, UpdateProviderRequest, handle_update};
    use crate::{format::UpstreamFormat, provider::StoredCredentials, storage, test_support};

    /// Verifies the `providers:update` request's strictness: unknown fields are rejected, the
    /// write-only `api_key` rejects an explicit `null` (absence keeps the stored key), and the
    /// whole full-state shape parses back.
    ///
    /// # Panics
    ///
    /// Panics if the request parses or rejects outside its contract.
    #[test]
    fn parses_the_update_request_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: UpdateProviderRequest = serde_json::from_str(
            r#"{"provider_id":7,"name":"acme","model_prefix":"acme-","is_enabled":true,
                "base_url":"https://upstream.example","api_key":"sk"}"#,
        )
        .unwrap();

        assert_eq!(parsed.provider_id, 7);
        assert_eq!(parsed.api_key.as_deref(), Some("sk"));
        assert_eq!(parsed.provider_definition_name, None);

        // An unknown field is rejected.
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":7,"name":"acme","model_prefix":"","is_enabled":true,
                    "base_url":null,"expires_at":1}"#
            )
            .is_err()
        );

        // The write-only field never accepts `null`.
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":7,"name":"acme","model_prefix":"","is_enabled":true,
                    "base_url":null,"api_key":null}"#
            )
            .is_err()
        );

        // A partial rebind — one reference half only — parses here and is rejected by the
        // handler's validation, because the wire shape cannot express the pairing rule.
        #[expect(clippy::unwrap_used, reason = "the payload is constructed to parse")]
        let partial: UpdateProviderRequest = serde_json::from_str(
            r#"{"provider_id":7,"name":"acme","model_prefix":"","is_enabled":true,
                "base_url":null,"provider_definition_name":"d"}"#,
        )
        .unwrap();

        assert!(partial.provider_definition_version.is_none());
    }

    /// `providers:update` enforces the base-URL requirement against the effective definition even
    /// on the no-rebind path: nulling the base URL of a provider bound to a base-addressing
    /// definition is rejected instead of writing a row whose every dispatch evaluates the
    /// definition's base references against a missing URL (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn updating_without_a_rebind_still_enforces_the_base_url_requirement() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = test_support::insert_static_key_definition(
            &database,
            "static-def",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        let provider = test_support::insert_provider(
            &database,
            &definition,
            "static-provider",
            "s-",
            Some("https://upstream.example"),
            StoredCredentials::ApiKey {
                api_key: "sk".to_string(),
            },
        )
        .await;

        #[expect(
            clippy::unwrap_used,
            reason = "the rejection is the documented side of this call"
        )]
        let error = handle_update(
            &database,
            UpdateProviderRequest {
                provider_id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: true,
                base_url: None,
                provider_definition_name: None,
                provider_definition_version: None,
                api_key: None,
            },
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::Validation { param, message } = error else {
            panic!("nulling a base-addressing provider's base URL renders the base_url rejection");
        };

        assert_eq!(param, "base_url");
        assert!(message.contains("static-def"), "{message}");
    }

    /// A named rebind whose target version does not exist defers the base-URL requirement to the
    /// transactional write: the write's unknown-definition 404 answers, never a pre-checked
    /// `base_url` rejection judged against the stored definition (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the rejection diverges.
    #[tokio::test]
    async fn updating_to_an_unknown_rebind_target_defers_the_base_url_check_to_the_write() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = test_support::insert_static_key_definition(
            &database,
            "static-def",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        let provider = test_support::insert_provider(
            &database,
            &definition,
            "static-provider",
            "s-",
            Some("https://upstream.example"),
            StoredCredentials::ApiKey {
                api_key: "sk".to_string(),
            },
        )
        .await;

        #[expect(
            clippy::unwrap_used,
            reason = "the rejection is the documented side of this call"
        )]
        let error = handle_update(
            &database,
            UpdateProviderRequest {
                provider_id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: true,
                base_url: None,
                provider_definition_name: Some("absent-def".to_string()),
                provider_definition_version: Some(1),
                api_key: None,
            },
        )
        .await
        .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::Storage(storage::Error::NotFound { message }) = error else {
            panic!("an unknown rebind target renders the write's unknown-definition 404");
        };

        assert!(message.contains("absent-def"), "{message}");
    }
}
