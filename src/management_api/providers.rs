//! Management endpoints listing, saving, and deleting providers.

use super::error::{self, ManagementApiError};
use crate::{
    context::Context,
    database::Database,
    format::UpstreamFormat,
    provider::{Credentials, NewProvider, ProviderType, storage},
};
use axum::{body::Bytes, extract::State, http::StatusCode, response::Response};
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
pub(super) async fn list(State(context): State<Context>) -> Response {
    match storage::list_providers(&context.database).await {
        Ok(providers) => {
            let rendered = providers
                .iter()
                .map(super::provider_json)
                .collect::<Vec<_>>();

            super::json_response(StatusCode::OK, serde_json::json!({ "providers": rendered }))
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
        Ok(rendered) => {
            super::json_response(StatusCode::OK, serde_json::json!({ "provider": rendered }))
        }
        Err(error) => error.into_response(),
    }
}

/// Handles `POST /delete-provider`.
pub(super) async fn delete(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_delete(&context.database, &body).await;

    match outcome {
        Ok(id) => {
            info!("deleted provider {id}");

            super::json_response(StatusCode::OK, serde_json::json!({"deleted": true}))
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
    let request: DeleteProviderBody = super::parse_body(body, "delete-provider")?;

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
/// OAuth creation attempts, type mismatches, taken names, and public name
/// collisions.
async fn handle_save(
    database: &Database,
    body: &Bytes,
) -> Result<serde_json::Value, ManagementApiError> {
    let request: SaveProviderBody = super::parse_body(body, "save-provider")?;

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

            (Some(value), Some(kind)) => Some(Credentials::parse(kind, &value.to_string())?),

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

            return Ok(super::provider_json(&stored));
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

        Ok(super::provider_json(&stored))
    }
    .await;

    if let Err(error) = &outcome {
        error::log_error(&format!("save-provider `{}`", request.name), error);
    }

    outcome
}
