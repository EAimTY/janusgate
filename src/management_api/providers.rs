//! Management endpoints listing, saving, and deleting providers.

use super::{error_response, parse_body, provider_json, validate_identifier};
use crate::{
    context::Context,
    database::Database,
    error::{self, ApiError, log_api_error},
    format::UpstreamFormat,
    provider::{Credentials, NewProvider, ProviderType, storage},
};
use axum::{body::Bytes, extract::State, response::Response};
use log::info;
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `save-provider` request. `id` selects the provider to
/// update — its absence creates one. `name` and `model_prefix` are required
/// non-nullable strings: every request states the naming, and their content
/// is the caller's choice (README §7.1); supplying a different `name` for an
/// existing provider renames it. The remaining fields are plain optional:
/// absent keeps the stored value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveProviderBody {
    /// Provider row identifier; absent creates a new provider.
    id: Option<i64>,

    /// Unique provider name; must be non-empty. Renames the provider when it
    /// differs from the stored name.
    name: String,

    /// Public model name prefix; may be empty.
    model_prefix: String,

    /// Account kind; required for creation.
    #[serde(rename = "type")]
    provider_type: Option<String>,

    /// Upstream protocol.
    protocol: Option<String>,

    /// Upstream base URL.
    base_url: Option<String>,

    /// Plaintext credentials payload.
    credentials: Option<Value>,

    /// Serving state.
    is_enabled: Option<bool>,
}

/// Strictly parsed `delete-provider` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteProviderBody {
    /// Provider row identifier.
    id: i64,
}

/// Handles `GET /list-providers`.
pub(crate) async fn list(State(context): State<Context>) -> Response {
    match storage::list_providers(&context.database).await {
        Ok(providers) => {
            let rendered = providers.iter().map(provider_json).collect::<Vec<_>>();

            error::json_response(200, serde_json::json!({ "providers": rendered }))
        }

        Err(error) => {
            log_api_error("list-providers", &error);

            error_response(error)
        }
    }
}

/// Handles `POST /save-provider`.
pub(crate) async fn save(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_save(&context.database, &body).await;

    match outcome {
        Ok(rendered) => error::json_response(200, serde_json::json!({ "provider": rendered })),
        Err(error) => error_response(error),
    }
}

/// Handles `POST /delete-provider`.
pub(crate) async fn delete(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = async {
        let request: DeleteProviderBody = parse_body(&body, "delete-provider")?;

        if !storage::delete_provider(&context.database, request.id).await? {
            return Err(ApiError::not_found(format!(
                "Provider {} does not exist",
                request.id
            )));
        }

        Ok(request.id)
    }
    .await;

    match outcome {
        Ok(id) => {
            info!("deleted provider {id}");

            error::json_response(200, serde_json::json!({"deleted": true}))
        }
        Err(error) => {
            log_api_error("delete-provider", &error);

            error_response(error)
        }
    }
}

/// Creates or updates one provider per the management contract. A request
/// carrying `id` updates (and may rename) that provider; a request without
/// `id` creates one. The outcome is logged: every failure at the provider's
/// response boundary, the successful creation or update at info level.
///
/// # Errors
///
/// Returns [`ApiError`] for empty names, unknown ids, unknown types, OAuth
/// creation attempts, type mismatches, taken names, and public name
/// collisions.
async fn handle_save(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: SaveProviderBody = parse_body(body, "save-provider")?;

    let outcome = async {
        validate_identifier("name", &request.name)?;

        // The outer read only pins request validation: the stored type is
        // immutable, so it cannot drift before the transactional write. Absent
        // fields are merged from the row re-read inside `update_provider`'s
        // transaction, never from this snapshot (README §7.1).
        let existing = match request.id {
            Some(id) => Some(
                storage::find_provider_by_id(database, id)
                    .await?
                    .ok_or_else(|| ApiError::not_found(format!("Provider {id} does not exist")))?,
            ),

            // Creation checks name availability here for a clean 409; the
            // database's UNIQUE constraint is the concurrent backstop.
            None => {
                if storage::find_provider(database, &request.name)
                    .await?
                    .is_some()
                {
                    return Err(ApiError::conflict(format!(
                        "Provider name `{}` is already taken",
                        request.name
                    )));
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
                    ApiError::invalid_parameter("type", format!("Unknown provider type `{raw}`"))
                })?;

                if let Some(existing) = &existing
                    && provider_type != existing.provider_type
                {
                    return Err(ApiError::invalid_parameter(
                        "type",
                        format!(
                            "Provider `{}` has type `{}` and cannot become `{}`",
                            existing.name,
                            existing.provider_type.as_str(),
                            provider_type.as_str()
                        ),
                    ));
                }

                Some(provider_type)
            }

            None => existing.as_ref().map(|provider| provider.provider_type),
        };

        let protocol = match request.protocol.as_deref() {
            None => None,
            Some(raw) => Some(UpstreamFormat::parse(raw).ok_or_else(|| {
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

            return Ok(provider_json(&stored));
        }

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

        let base_url = request.base_url.as_deref().ok_or_else(|| {
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

        let stored = storage::find_provider(database, &request.name)
            .await?
            .ok_or_else(|| ApiError::internal("provider vanished after save"))?;

        info!("created provider `{}`", request.name);

        Ok(provider_json(&stored))
    }
    .await;

    if let Err(error) = &outcome {
        log_api_error(&format!("save-provider `{}`", request.name), error);
    }

    outcome
}
