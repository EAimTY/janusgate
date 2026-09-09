//! Management endpoints listing, saving, and deleting providers.

use super::{error_response, json_response, parse_body, provider_json};
use crate::{
    database::Database,
    error::ApiError,
    provider::{self, Credentials, NewProvider, ProviderType, UpstreamProtocol, storage},
};
use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `save-provider` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveProviderBody {
    /// Provider identifier.
    name: String,

    /// Account kind; required for creation.
    #[serde(default, rename = "type")]
    provider_type: Option<String>,

    /// Public model name prefix.
    #[serde(default)]
    model_prefix: Option<String>,

    /// Upstream protocol.
    #[serde(default)]
    protocol: Option<String>,

    /// Upstream base URL.
    #[serde(default)]
    base_url: Option<String>,

    /// Plaintext credentials payload.
    #[serde(default)]
    credentials: Option<Value>,

    /// Serving state.
    #[serde(default)]
    enabled: Option<bool>,
}

/// Strictly parsed `delete-provider` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteProviderBody {
    /// Provider identifier.
    name: String,
}

/// Handles `GET /list-providers`.
pub(crate) async fn list(State(database): State<Database>) -> Response {
    match storage::list_providers(&database).await {
        Ok(providers) => {
            let rendered = providers.iter().map(provider_json).collect::<Vec<_>>();

            json_response(200, serde_json::json!({ "providers": rendered }))
        }

        Err(error) => error_response(error),
    }
}

/// Handles `POST /save-provider`.
pub(crate) async fn save(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_save(&database, &body).await;

    match outcome {
        Ok(rendered) => json_response(200, serde_json::json!({ "provider": rendered })),
        Err(error) => error_response(error),
    }
}

/// Handles `POST /delete-provider`.
pub(crate) async fn delete(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = async {
        let request: DeleteProviderBody = parse_body(&body)?;

        if !storage::delete_provider(&database, &request.name).await? {
            return Err(ApiError::not_found(format!(
                "Provider `{}` does not exist",
                request.name
            )));
        }

        Ok(())
    }
    .await;

    match outcome {
        Ok(()) => json_response(200, serde_json::json!({"deleted": true})),
        Err(error) => error_response(error),
    }
}

/// Creates or updates one provider per the management contract.
///
/// # Errors
///
/// Returns [`ApiError`] for invalid names, unknown types, OAuth creation
/// attempts, and type mismatches.
async fn handle_save(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: SaveProviderBody = parse_body(body)?;

    provider::validate_provider_name(&request.name)?;

    let provider_type = match request.provider_type.as_deref() {
        None => None,
        Some(raw) => Some(ProviderType::parse(raw).ok_or_else(|| {
            ApiError::invalid_parameter("type", format!("Unknown provider type `{raw}`"))
        })?),
    };

    let protocol = match request.protocol.as_deref() {
        None => None,
        Some(raw) => Some(UpstreamProtocol::parse(raw).ok_or_else(|| {
            ApiError::invalid_parameter("protocol", format!("Unknown protocol `{raw}`"))
        })?),
    };

    let credentials = match (&request.credentials, provider_type) {
        (None, _) => None,
        (Some(value), Some(kind)) => Some(Credentials::parse(kind, &value.to_string())?),
        (Some(_), None) => {
            return Err(ApiError::invalid_parameter(
                "type",
                "credentials require a `type` in the same request",
            ));
        }
    };

    let existing = storage::find_provider(database, &request.name).await?;

    if let Some(existing) = existing {
        if let Some(provider_type) = provider_type
            && provider_type != existing.provider_type
        {
            return Err(ApiError::conflict(format!(
                "Provider `{}` has type `{}` and cannot become `{}`",
                existing.name,
                existing.provider_type.as_str(),
                provider_type.as_str()
            )));
        }

        let model_prefix = request
            .model_prefix
            .clone()
            .unwrap_or_else(|| existing.model_prefix.clone());

        let updated_protocol = protocol.or(existing.protocol);

        let base_url = request
            .base_url
            .clone()
            .or_else(|| existing.base_url.clone());

        let enabled = request.enabled.unwrap_or(existing.enabled);

        storage::update_provider_config(
            database,
            existing.id,
            &model_prefix,
            updated_protocol,
            base_url.as_deref(),
            enabled,
        )
        .await?;

        if let Some(credentials) = credentials {
            storage::update_credentials(database, existing.id, &credentials, None).await?;
        }
    } else {
        let Some(provider_type) = provider_type else {
            return Err(ApiError::invalid_parameter(
                "type",
                "type is required when creating a provider",
            ));
        };

        if provider_type != ProviderType::OpenAiCompatible {
            return Err(ApiError::invalid_parameter(
                "type",
                format!(
                    "`{}` providers are created by completing an OAuth flow",
                    provider_type.as_str()
                ),
            ));
        }

        let protocol = protocol.ok_or_else(|| {
            ApiError::invalid_parameter(
                "protocol",
                "protocol is required for openai_compatible providers",
            )
        })?;

        let base_url = request.base_url.clone().ok_or_else(|| {
            ApiError::invalid_parameter(
                "base_url",
                "base_url is required for openai_compatible providers",
            )
        })?;

        let credentials = credentials.ok_or_else(|| {
            ApiError::invalid_parameter(
                "credentials",
                "credentials are required when creating a provider",
            )
        })?;

        storage::insert_provider(
            database,
            &NewProvider {
                name: &request.name,
                model_prefix: request.model_prefix.as_deref().unwrap_or_default(),
                provider_type,
                protocol: Some(protocol),
                base_url: Some(&base_url),
                credentials: &credentials,
                expires_at: None,
                enabled: request.enabled.unwrap_or(true),
            },
        )
        .await?;
    }

    let stored = storage::find_provider(database, &request.name)
        .await?
        .ok_or_else(|| ApiError::internal("provider vanished after save"))?;

    Ok(provider_json(&stored))
}
