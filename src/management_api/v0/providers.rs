//! Management routes listing, creating, rewriting, and deleting providers.

use super::{EmptyResponse, Error, ProviderEnvelope, ProviderResponse, absent_keeps, route};
use crate::{
    context::Context,
    format::UpstreamFormat,
    provider::{Credentials, NewProvider, ProviderType, storage},
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use serde::{Deserialize, Serialize};

route! {
    pub impl Route for ListProviders {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/providers:list";
        type Request = ListProvidersRequest;
        type Response = ListProvidersResponse;
    }
}

route! {
    pub impl Route for CreateProvider {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/providers:create";
        type Request = CreateProviderRequest;
        type Response = ProviderEnvelope;
    }
}

route! {
    pub impl Route for UpdateProvider {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/providers:update";
        type Request = UpdateProviderRequest;
        type Response = EmptyResponse;
    }
}

route! {
    pub impl Route for DeleteProvider {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/providers:delete";
        type Request = DeleteProviderRequest;
        type Response = EmptyResponse;
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

/// Strictly parsed `providers:create` request (README §7.1): the complete state of one
/// `openai_compatible` provider, every field required — the request is the row's desired state, so
/// creation has no defaults and no optional fields (rule 3). OAuth account kinds are not creatable
/// here; `oauth:complete` is the only path that mints their credentials.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct CreateProviderRequest {
    /// Unique provider name; must be non-empty.
    pub name: String,

    /// Public model name prefix; may be empty.
    pub model_prefix: String,

    /// Serving state.
    pub is_enabled: bool,

    /// Upstream base URL.
    pub base_url: String,

    /// Upstream wire format.
    pub protocol: UpstreamFormat,

    /// Static API key.
    pub api_key: String,
}

/// Strictly parsed `providers:update` request (README §7.1): a union discriminated by the restated
/// `type`, one variant per stored account kind, each stating the provider's complete desired
/// readable state (rule 3). The restated `type` must equal the stored, immutable one. The
/// write-only credential field is the one absent-keeps exception (rule 2): absent keeps the stored
/// secret, present replaces it; it is never `null`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum UpdateProviderRequest {
    /// Static API key account speaking a configured protocol.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible {
        /// Addressed provider row.
        provider_id: i64,

        /// Unique provider name; must be non-empty. Renames the provider when it differs from the
        /// stored name.
        name: String,

        /// Public model name prefix; may be empty.
        model_prefix: String,

        /// Serving state.
        is_enabled: bool,

        /// Upstream base URL.
        base_url: String,

        /// Upstream wire format.
        protocol: UpstreamFormat,

        /// Replacement static API key; absent keeps the stored one.
        #[serde(
            default,
            deserialize_with = "absent_keeps::deserialize",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        api_key: Option<String>,
    },

    /// ChatGPT account speaking the Codex Responses protocol.
    #[serde(rename = "openai_oauth")]
    OpenAiOauth {
        /// Addressed provider row.
        provider_id: i64,

        /// Unique provider name; must be non-empty. Renames the provider when it differs from the
        /// stored name.
        name: String,

        /// Public model name prefix; may be empty.
        model_prefix: String,

        /// Serving state.
        is_enabled: bool,

        /// Replacement credentials; absent keeps the stored ones.
        #[serde(
            default,
            deserialize_with = "absent_keeps::deserialize",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        credentials: Option<OpenAiOauthCredentials>,
    },

    /// Google account speaking Gemini GenerateContent via Antigravity.
    #[serde(rename = "antigravity_oauth")]
    AntigravityOauth {
        /// Addressed provider row.
        provider_id: i64,

        /// Unique provider name; must be non-empty. Renames the provider when it differs from the
        /// stored name.
        name: String,

        /// Public model name prefix; may be empty.
        model_prefix: String,

        /// Serving state.
        is_enabled: bool,

        /// Replacement credentials; absent keeps the stored ones.
        #[serde(
            default,
            deserialize_with = "absent_keeps::deserialize",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        credentials: Option<AntigravityOauthCredentials>,
    },
}

/// The `openai_oauth` replacement credentials (README §3): the exact stored field set, camelCase
/// like the storage form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct OpenAiOauthCredentials {
    /// OAuth access token.
    pub access_token: String,

    /// OAuth refresh token.
    pub refresh_token: String,

    /// ChatGPT account ID.
    pub account_id: String,
}

/// The `antigravity_oauth` replacement credentials (README §3): the exact stored field set,
/// camelCase like the storage form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct AntigravityOauthCredentials {
    /// OAuth access token.
    pub access_token: String,

    /// OAuth refresh token.
    pub refresh_token: String,

    /// Antigravity project ID.
    pub project_id: String,
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
    let providers = storage::list_providers(&context.database).await?;

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
    super::validate_identifier("name", &request.name)?;

    // The pre-insert check renders a clean 409; the database's UNIQUE constraint is the concurrent
    // backstop.
    if storage::find_provider(&context.database, &request.name)
        .await?
        .is_some()
    {
        return Err(Error::ProviderNameTaken {
            name: request.name.clone(),
        });
    }

    let credentials = Credentials::ApiKey(request.api_key);

    let provider = storage::insert_provider(
        &context.database,
        &NewProvider {
            name: &request.name,
            model_prefix: &request.model_prefix,
            provider_type: ProviderType::OpenAiCompatible,
            protocol: Some(request.protocol),
            base_url: Some(&request.base_url),
            credentials: &credentials,
            expires_at: None,
            is_enabled: request.is_enabled,
        },
    )
    .await?;

    info!("created provider `{}`", request.name);

    Ok(Json(ProviderEnvelope {
        provider: ProviderResponse::from_provider(&provider),
    }))
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
    if !storage::delete_provider(&context.database, request.provider_id).await? {
        return Err(Error::ProviderNotFound {
            id: request.provider_id,
        });
    }

    info!("deleted provider {}", request.provider_id);

    Ok(Json(EmptyResponse {}))
}

/// Rewrites one provider's full state per the management contract (README §7.1). The request's
/// discriminated variant names the account kind; the stored row must carry the same one.
///
/// # Errors
///
/// Returns [`Error`] for empty names, unknown ids, type mismatches, taken
/// names, and public name collisions.
async fn handle_update(
    database: &crate::database::Database,
    request: UpdateProviderRequest,
) -> Result<(), Error> {
    // One match flattens the union into the full storage state: every readable field is restated,
    // and the write-only credential is the one absent-keeps field.
    let (provider_id, name, model_prefix, is_enabled, protocol, base_url, credentials) =
        match &request {
            UpdateProviderRequest::OpenAiCompatible {
                provider_id,
                name,
                model_prefix,
                is_enabled,
                base_url,
                protocol,
                api_key,
            } => (
                *provider_id,
                name,
                model_prefix,
                *is_enabled,
                Some(*protocol),
                Some(base_url.as_str()),
                api_key
                    .as_ref()
                    .map(|api_key| Credentials::ApiKey(api_key.clone())),
            ),

            UpdateProviderRequest::OpenAiOauth {
                provider_id,
                name,
                model_prefix,
                is_enabled,
                credentials,
            } => (
                *provider_id,
                name,
                model_prefix,
                *is_enabled,
                None,
                None,
                credentials
                    .as_ref()
                    .map(|credentials| Credentials::OpenAiOauth {
                        access_token: credentials.access_token.clone(),
                        refresh_token: credentials.refresh_token.clone(),
                        account_id: credentials.account_id.clone(),
                    }),
            ),

            UpdateProviderRequest::AntigravityOauth {
                provider_id,
                name,
                model_prefix,
                is_enabled,
                credentials,
            } => (
                *provider_id,
                name,
                model_prefix,
                *is_enabled,
                None,
                None,
                credentials
                    .as_ref()
                    .map(|credentials| Credentials::AntigravityOauth {
                        access_token: credentials.access_token.clone(),
                        refresh_token: credentials.refresh_token.clone(),
                        project_id: credentials.project_id.clone(),
                    }),
            ),
        };

    super::validate_identifier("name", name)?;

    // The outer read pins request validation only: the type is immutable, so it cannot drift before
    // the transactional write, and the update's merge and checks read the row again inside the
    // transaction.
    let Some(existing) = storage::find_provider_by_id(database, provider_id).await? else {
        return Err(Error::ProviderNotFound { id: provider_id });
    };

    // The stored row pins the account kind: the restated `type` doubles as the union's
    // discriminator and a guard against updating the wrong row, and a mismatch is an invalid
    // request rather than a conflict, because no concurrent state is overwritten.
    let requested_type = match request {
        UpdateProviderRequest::OpenAiCompatible { .. } => ProviderType::OpenAiCompatible,
        UpdateProviderRequest::OpenAiOauth { .. } => ProviderType::OpenAiOauth,
        UpdateProviderRequest::AntigravityOauth { .. } => ProviderType::AntigravityOauth,
    };

    if requested_type != existing.provider_type {
        return Err(Error::ProviderTypeMismatch {
            name: existing.name.clone(),
            stored: existing.provider_type,
            requested: requested_type,
        });
    }

    // One transactional write owns the provider row — the restated `name` included, so a different
    // name renames the provider — the optional credentials replacement, and the recomputed public
    // model names; a taken name or a prefix change that would collide is rejected with 409 before
    // anything is written.
    storage::update_provider(
        database,
        provider_id,
        storage::ProviderUpdate {
            name,
            model_prefix,
            protocol,
            base_url,
            is_enabled,
            credentials: credentials.as_ref(),
        },
    )
    .await?;

    info!("updated provider `{name}`");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::UpdateProviderRequest;

    /// Verifies the `providers:update` union's strictness (README §7.0 rules 2-4): the
    /// discriminator selects the variant, an unknown field is rejected inside it, an unknown
    /// discriminator names the closed set, a required field's absence is named, and an explicit
    /// `null` on the write-only credential field is rejected instead of reading as absence.
    ///
    /// # Panics
    ///
    /// Panics if the union parses or rejects outside its contract.
    #[test]
    fn parses_the_update_union_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: UpdateProviderRequest = serde_json::from_str(
            r#"{"provider_id":7,"type":"openai_compatible","name":"acme",
                "model_prefix":"acme/","is_enabled":true,
                "base_url":"https://upstream.example",
                "protocol":"openai_chat_completion","api_key":"sk"}"#,
        )
        .unwrap();

        assert!(matches!(
            parsed,
            UpdateProviderRequest::OpenAiCompatible { .. }
        ));

        // An unknown field is rejected inside the addressed variant, naming the variant's own
        // fields.
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":7,"type":"openai_compatible","name":"acme",
                    "model_prefix":"acme/","is_enabled":true,
                    "base_url":"https://upstream.example",
                    "protocol":"openai_chat_completion","expires_at":1}"#
            )
            .is_err()
        );

        // A field of another variant is as unknown as a field no variant carries.
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":3,"type":"openai_oauth","name":"codex",
                    "model_prefix":"codex/","is_enabled":true,"base_url":"x"}"#
            )
            .is_err()
        );

        // An unknown discriminator names the closed set.
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":3,"type":"bogus","name":"x",
                    "model_prefix":"","is_enabled":true}"#
            )
            .is_err()
        );

        // A required field's absence is named by serde's own missing-field rejection
        // (`model_prefix` is absent here).
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":3,"type":"openai_oauth","name":"codex",
                    "is_enabled":true}"#
            )
            .is_err()
        );

        // The write-only credential field never accepts `null`.
        assert!(
            serde_json::from_str::<UpdateProviderRequest>(
                r#"{"provider_id":3,"type":"openai_oauth","name":"codex",
                    "model_prefix":"codex/","is_enabled":true,"credentials":null}"#
            )
            .is_err()
        );
    }
}
