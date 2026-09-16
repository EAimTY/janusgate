//! Management endpoints listing, saving, and deleting providers.

use super::{
    error::{self, ManagementApiError},
    route,
};
use crate::{
    context::Context,
    database::Database,
    format::UpstreamFormat,
    provider::{Credentials, NewProvider, ParseError, Provider, ProviderType, storage},
};
use axum::{body::Bytes, extract::State, http::StatusCode, response::Response};
use log::info;
use serde::{Deserialize, Serialize};

route! {
    pub impl Route for ListProviders {
        const METHOD: http::Method = http::Method::GET;
        const PATH: &'static str = "/list-providers";
        type Request = ListProvidersRequest;
        type Response = ListProvidersResponse;
    }
}

route! {
    pub impl Route for SaveProvider {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/save-provider";
        type Request = SaveProviderRequest;
        type Response = super::ProviderEnvelope;
    }
}

route! {
    pub impl Route for DeleteProvider {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/delete-provider";
        type Request = DeleteProviderRequest;
        type Response = DeleteProviderResponse;
    }
}

/// The `list-providers` request body (README §7.1); the route carries no
/// body, so the payload is the empty request type.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListProvidersRequest {}

/// The `list-providers` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListProvidersResponse {
    /// Every stored provider, ordered by row identifier.
    pub providers: Vec<super::ProviderResponse>,
}

/// The `delete-provider` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DeleteProviderResponse {
    /// Always `true`; a missing provider renders as a 404 instead.
    pub deleted: bool,
}

/// Strictly parsed `save-provider` request. `id` selects the provider to
/// update — its absence creates one. `name` and `model_prefix` are required
/// non-nullable strings: every request states the naming, and their content
/// is the caller's choice (README §7.1); supplying a different `name` for an
/// existing provider renames it. The remaining fields are plain optional:
/// absent keeps the stored value.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct SaveProviderRequest {
    /// Provider row identifier; absent creates a new provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub id: Option<i64>,

    /// Unique provider name; must be non-empty. Renames the provider when it
    /// differs from the stored name.
    pub name: String,

    /// Public model name prefix; may be empty.
    pub model_prefix: String,

    /// Account kind; required for creation.
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub provider_type: Option<String>,

    /// Upstream protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub protocol: Option<String>,

    /// Upstream base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub base_url: Option<String>,

    /// Plaintext credentials payload. Which of its fields applies is pinned
    /// by the sibling `type` — or the stored row on update — and validated
    /// per kind by `SaveProviderCredentials::to_credentials`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub credentials: Option<SaveProviderCredentials>,

    /// Serving state.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub is_enabled: Option<bool>,
}

/// The `save-provider` `credentials` payload (README §3): the concrete,
/// key-typed capture of the three per-provider-type credential shapes. Which
/// subset applies is pinned by the request's `type` — or the stored row on
/// update — and enforced by `SaveProviderCredentials::to_credentials`,
/// which renders the same verdicts a stored-credentials parse renders.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct SaveProviderCredentials {
    /// Static API key (`openai_compatible`).
    #[serde(rename = "apiKey")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub api_key: Option<String>,

    /// OAuth access token (`openai_oauth`, `antigravity_oauth`).
    #[serde(rename = "accessToken")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub access_token: Option<String>,

    /// OAuth refresh token (`openai_oauth`, `antigravity_oauth`).
    #[serde(rename = "refreshToken")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub refresh_token: Option<String>,

    /// ChatGPT account ID (`openai_oauth`).
    #[serde(rename = "accountId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub account_id: Option<String>,

    /// Cloud Code Assist project ID (`antigravity_oauth`).
    #[serde(rename = "projectId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub project_id: Option<String>,
}

impl SaveProviderCredentials {
    /// Validates the captured fields against the provider type's exact field
    /// set: a captured field outside the set is rejected naming it, a missing
    /// required field is rejected naming it.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the captured fields do not match the shape
    /// required by the provider type.
    fn to_credentials(&self, provider_type: ProviderType) -> Result<Credentials, ParseError> {
        match provider_type {
            ProviderType::OpenAiCompatible => {
                self.reject_fields_outside(&["apiKey"])?;

                Ok(Credentials::ApiKey(self.api_key.clone().ok_or_else(
                    || ParseError::MissingField("apiKey".to_string()),
                )?))
            }

            ProviderType::OpenAiOauth => {
                self.reject_fields_outside(&["accessToken", "refreshToken", "accountId"])?;

                Ok(Credentials::OpenAiOauth {
                    access_token: required_field(&self.access_token, "accessToken")?,
                    refresh_token: required_field(&self.refresh_token, "refreshToken")?,
                    account_id: required_field(&self.account_id, "accountId")?,
                })
            }

            ProviderType::AntigravityOauth => {
                self.reject_fields_outside(&["accessToken", "refreshToken", "projectId"])?;

                Ok(Credentials::AntigravityOauth {
                    access_token: required_field(&self.access_token, "accessToken")?,
                    refresh_token: required_field(&self.refresh_token, "refreshToken")?,
                    project_id: required_field(&self.project_id, "projectId")?,
                })
            }
        }
    }

    /// Rejects captured fields outside the provider type's exact field set,
    /// naming the first offender.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::UnknownField`] naming the first captured field
    /// outside `allowed`.
    fn reject_fields_outside(&self, allowed: &[&str]) -> Result<(), ParseError> {
        let captured = [
            ("apiKey", &self.api_key),
            ("accessToken", &self.access_token),
            ("refreshToken", &self.refresh_token),
            ("accountId", &self.account_id),
            ("projectId", &self.project_id),
        ];

        for (name, value) in captured {
            if value.is_some() && !allowed.contains(&name) {
                return Err(ParseError::UnknownField(
                    name.to_string(),
                    allowed.join("`, `"),
                ));
            }
        }

        Ok(())
    }
}

/// Requires one captured credential field's value.
///
/// # Errors
///
/// Returns [`ParseError::MissingField`] naming the field when it is absent.
fn required_field(field: &Option<String>, name: &str) -> Result<String, ParseError> {
    field
        .clone()
        .ok_or_else(|| ParseError::MissingField(name.to_string()))
}

/// Strictly parsed `delete-provider` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DeleteProviderRequest {
    /// Provider row identifier.
    pub id: i64,
}

/// Handles `GET /list-providers`.
pub(super) async fn list(State(context): State<Context>) -> Response {
    match storage::list_providers(&context.database).await {
        Ok(providers) => {
            let rendered = providers
                .iter()
                .map(super::ProviderResponse::from_provider)
                .collect::<Vec<_>>();

            super::json_response(
                StatusCode::OK,
                &ListProvidersResponse {
                    providers: rendered,
                },
            )
        }

        Err(error) => {
            let error = ManagementApiError::from(error);

            error::log_error("list-providers", &error);

            error.into_response()
        }
    }
}

/// Handles `POST /save-provider`.
pub(super) async fn save(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_save(&context.database, &body).await;

    match outcome {
        Ok(provider) => super::json_response(
            StatusCode::OK,
            &super::ProviderEnvelope {
                provider: super::ProviderResponse::from_provider(&provider),
            },
        ),
        Err(error) => error.into_response(),
    }
}

/// Handles `POST /delete-provider`.
pub(super) async fn delete(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_delete(&context.database, &body).await;

    match outcome {
        Ok(id) => {
            info!("deleted provider {id}");

            super::json_response(StatusCode::OK, &DeleteProviderResponse { deleted: true })
        }
        Err(error) => error.into_response(),
    }
}

/// Deletes one provider by row identifier. `parse_body` runs before the
/// logged work block and logs a malformed body itself, so its early `?`
/// return bypasses the work-block logging tail; every other failure — a
/// storage failure or an unknown id — logs exactly once at that tail, and
/// the wrapper renders the response without logging (README §8 logs every
/// rendered API error exactly once).
///
/// # Errors
///
/// Returns [`ManagementApiError`] for a malformed body, storage failures,
/// and an unknown id.
async fn handle_delete(database: &Database, body: &Bytes) -> Result<i64, ManagementApiError> {
    let request: DeleteProviderRequest = super::parse_body(body, "delete-provider")?;

    let outcome = async {
        if !storage::delete_provider(database, request.id).await? {
            return Err(ManagementApiError::not_found(
                storage::provider_not_found_message(request.id),
            ));
        }

        Ok(request.id)
    }
    .await;

    if let Err(error) = &outcome {
        error::log_error("delete-provider", error);
    }

    outcome
}

/// Creates or updates one provider per the management contract. A request
/// carrying `id` updates (and may rename) that provider; a request without
/// `id` creates one. The outcome is logged: every failure at the provider's
/// response boundary, the successful creation or update at info level.
///
/// # Errors
///
/// Returns [`ManagementApiError`] for empty names, unknown ids, unknown types,
/// OAuth creation attempts, type mismatches, taken names, malformed
/// credentials, and public name collisions.
async fn handle_save(database: &Database, body: &Bytes) -> Result<Provider, ManagementApiError> {
    let request: SaveProviderRequest = super::parse_body(body, "save-provider")?;

    let outcome = async {
        super::validate_identifier("name", &request.name)?;

        // The outer read only pins request validation: the stored type is
        // immutable, so it cannot drift before the transactional write. Absent
        // fields are merged from the row re-read inside `update_provider`'s
        // transaction, never from this snapshot (README §7.1).
        let existing = match request.id {
            Some(id) => Some(
                storage::find_provider_by_id(database, id)
                    .await?
                    .ok_or_else(|| {
                        ManagementApiError::not_found(storage::provider_not_found_message(id))
                    })?,
            ),

            // Creation checks name availability here for a clean 409; the
            // database's UNIQUE constraint is the concurrent backstop.
            None => {
                if storage::find_provider(database, &request.name)
                    .await?
                    .is_some()
                {
                    return Err(ManagementApiError::conflict(
                        storage::provider_name_taken_message(&request.name),
                    ));
                }

                None
            }
        };

        // The stored row pins the account kind: an omitted `type` defaults to
        // it on update, and an explicit mismatching `type` is an
        // invalid request rather than a conflict, because no concurrent
        // state is overwritten.
        let provider_type = match request.provider_type.as_deref() {
            Some(raw) => {
                let provider_type = ProviderType::parse(raw).ok_or_else(|| {
                    ManagementApiError::invalid_request(format!("Unknown provider type `{raw}`"))
                })?;

                if let Some(existing) = &existing
                    && provider_type != existing.provider_type
                {
                    return Err(ManagementApiError::invalid_request(format!(
                        "Provider `{}` has type `{}` and cannot become `{}`",
                        existing.name,
                        existing.provider_type.as_str(),
                        provider_type.as_str()
                    )));
                }

                Some(provider_type)
            }

            None => existing.as_ref().map(|provider| provider.provider_type),
        };

        let protocol = match request.protocol.as_deref() {
            None => None,
            Some(raw) => Some(UpstreamFormat::parse(raw).ok_or_else(|| {
                ManagementApiError::invalid_request(format!("Unknown protocol `{raw}`"))
            })?),
        };

        let credentials = match (&request.credentials, provider_type) {
            (None, _) => None,

            (Some(credentials), Some(kind)) => Some(credentials.to_credentials(kind)?),

            (Some(_), None) => {
                return Err(ManagementApiError::invalid_request(
                    "credentials require a `type` in the same request",
                ));
            }
        };

        if let Some(id) = request.id {
            // One transactional write owns the provider row — the supplied
            // `name` included, so a different name renames the provider —
            // any credentials replacement, and the recomputed public model
            // names; a taken name or a prefix change that would collide is
            // rejected with 409 before anything is written.
            let stored = storage::update_provider(
                database,
                id,
                storage::ProviderUpdate {
                    name: &request.name,
                    model_prefix: &request.model_prefix,
                    protocol,
                    base_url: request.base_url.as_deref(),
                    is_enabled: request.is_enabled,
                    credentials: credentials.as_ref(),
                },
            )
            .await?;

            info!("updated provider `{}`", request.name);

            return Ok(stored);
        }

        let Some(provider_type) = provider_type else {
            return Err(ManagementApiError::invalid_request(
                "type is required when creating a provider",
            ));
        };

        if provider_type != ProviderType::OpenAiCompatible {
            return Err(ManagementApiError::invalid_request(format!(
                "`{}` providers are created by completing an OAuth flow",
                provider_type.as_str()
            )));
        }

        let protocol = protocol.ok_or_else(|| {
            ManagementApiError::invalid_request(
                "protocol is required for openai_compatible providers",
            )
        })?;

        let base_url = request.base_url.as_deref().ok_or_else(|| {
            ManagementApiError::invalid_request(
                "base_url is required for openai_compatible providers",
            )
        })?;

        let credentials = credentials.ok_or_else(|| {
            ManagementApiError::invalid_request("credentials are required when creating a provider")
        })?;

        let stored = storage::insert_provider(
            database,
            &NewProvider {
                name: &request.name,
                model_prefix: &request.model_prefix,
                provider_type,
                protocol: Some(protocol),
                base_url: Some(base_url),
                credentials: &credentials,
                expires_at: None,
                is_enabled: request.is_enabled.unwrap_or(true),
            },
        )
        .await?;

        info!("created provider `{}`", request.name);

        Ok(stored)
    }
    .await;

    if let Err(error) = &outcome {
        error::log_error(&format!("save-provider `{}`", request.name), error);
    }

    outcome
}
