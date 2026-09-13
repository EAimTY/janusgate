//! Management endpoints discovering, listing, and overriding models.

use super::error::{self, ManagementApiError};
use crate::{
    context::Context,
    database::Database,
    ir,
    provider::{
        discovery, oauth,
        storage::{self, ModelEntry},
    },
};
use axum::{body::Bytes, extract::State, http::StatusCode, response::Response};
use log::info;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `discover-models` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverModelsBody {
    /// Provider row identifier.
    id: i64,
}

/// Strictly parsed `save-model` request; each override field is tri-state:
/// absent leaves it unchanged, `null` clears it, a value sets it. The
/// `double_option` adapter is what makes the mapping explicit, since plain
/// serde maps JSON `null` and field absence to the same outer `None`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveModelBody {
    /// Owning provider row identifier.
    provider_id: i64,

    /// Exact upstream model identifier; must be non-empty.
    upstream_id: String,

    /// Public ID override.
    #[serde(default, with = "::serde_with::rust::double_option")]
    id_override: Option<Option<String>>,

    /// Serving state.
    is_enabled: Option<bool>,

    /// Canonical thinking levels override.
    #[serde(default, with = "::serde_with::rust::double_option")]
    thinking_levels_override: Option<Option<Vec<String>>>,
}

/// Handles `POST /discover-models`.
pub(super) async fn discover(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_discover(&context.database, &context.control_plane, &body).await;

    match outcome {
        Ok(discovered) => super::json_response(
            StatusCode::OK,
            serde_json::json!({ "discovered": discovered }),
        ),
        Err(error) => error.into_response(),
    }
}

/// Handles `GET /list-models`.
pub(super) async fn list(State(context): State<Context>) -> Response {
    let outcome = async {
        let entries = storage::list_model_entries(&context.database).await?;

        let rendered = entries
            .iter()
            .map(model_entry_json)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(serde_json::json!({ "models": rendered }))
    }
    .await;

    match outcome {
        Ok(rendered) => super::json_response(StatusCode::OK, rendered),
        Err(error) => {
            error::log_error("list-models", &error);

            error.into_response()
        }
    }
}

/// Handles `POST /save-model`.
pub(super) async fn save(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_save(&context.database, &body).await;

    match outcome {
        Ok(rendered) => {
            super::json_response(StatusCode::OK, serde_json::json!({ "model": rendered }))
        }
        Err(error) => error.into_response(),
    }
}

/// Refreshes one provider's model rows from upstream, carrying overrides over
/// by `upstream_id`. The outcome is logged: the discovered count at info
/// level, every failure at the provider's response boundary.
///
/// `control_plane` is the bounded client used for the token refresh and the
/// upstream discovery calls.
///
/// # Errors
///
/// Returns [`ManagementApiError`] for an empty provider identifier, unknown
/// providers, upstream failures (including an upstream that reports no
/// models), and public ID collisions.
async fn handle_discover(
    database: &Database,
    control_plane: &Client,
    body: &Bytes,
) -> Result<usize, ManagementApiError> {
    let request: DiscoverModelsBody = super::parse_body(body, "discover-models")?;

    let outcome = async {
        let provider = storage::find_provider_by_id(database, request.id)
            .await?
            .ok_or_else(|| {
                ManagementApiError::not_found(format!("Provider {} does not exist", request.id))
            })?;

        let fresh = oauth::ensure_fresh_token(database, control_plane, &provider).await?;

        let upstream_ids = discovery::discover_upstream_models(control_plane, &fresh).await?;

        // Discovery payloads can repeat an upstream identifier; keep the first
        // occurrence.
        let mut unique_ids: Vec<String> = Vec::with_capacity(upstream_ids.len());

        for upstream_id in upstream_ids {
            if unique_ids.iter().any(|existing| existing == &upstream_id) {
                continue;
            }

            unique_ids.push(upstream_id);
        }

        // An upstream reporting no models must not wipe curated rows, so the
        // empty replacement is refused before the replace is attempted
        // (README §7.1).
        if unique_ids.is_empty() {
            return Err(ManagementApiError::upstream(concat!(
                "Upstream request failed: upstream reported no models; ",
                "refusing to replace the provider's model rows",
            )));
        }

        // The replace re-reads the provider row inside its transaction, so the
        // public IDs are composed from the provider's current prefix and
        // checked for collisions atomically with the write (README
        // §7.1).
        Ok(storage::replace_discovered_models(database, request.id, &unique_ids).await?)
    }
    .await;

    match &outcome {
        Ok(discovered) => {
            info!("discovered {discovered} models for provider {}", request.id)
        }

        Err(error) => {
            error::log_error(
                &format!("model discovery for provider {}", request.id),
                error,
            );
        }
    }

    outcome
}

/// Writes one model's overrides and recomputes its public ID. The outcome
/// is logged: the saved overrides at info level, every failure at the
/// model's response boundary.
///
/// # Errors
///
/// Returns [`ManagementApiError`] for empty identifiers, unknown rows, invalid
/// thinking levels, and public ID collisions.
async fn handle_save(database: &Database, body: &Bytes) -> Result<Value, ManagementApiError> {
    let request: SaveModelBody = super::parse_body(body, "save-model")?;

    let outcome = async {
        super::validate_identifier("upstream_id", &request.upstream_id)?;

        if let Some(Some(ref id_override)) = request.id_override {
            super::validate_identifier("id_override", id_override)?;
        }

        let thinking_levels_override = match &request.thinking_levels_override {
            None => None,

            Some(None) => Some(None),

            Some(Some(levels)) => {
                // The shared parser keeps save-model validation byte-identical
                // with the stored-override contract (README §7.1).
                let parsed = ir::parse_thinking_levels(levels, "thinking_levels_override")?;

                Some(Some(ir::render_thinking_levels(&parsed).to_string()))
            }
        };

        // The checked update re-reads the provider and model rows inside its
        // transaction, so the recomputed public ID uses the provider's
        // current prefix and the collision check applies atomically
        // with the write (README §7.1).
        Ok(storage::update_model_overrides_checked(
            database,
            storage::ModelOverrideUpdate {
                provider_id: request.provider_id,
                upstream_id: &request.upstream_id,
                id_override: request.id_override.clone(),
                is_enabled: request.is_enabled,
                thinking_levels_override,
            },
        )
        .await?)
    }
    .await
    .and_then(|entry| model_entry_json(&entry));

    match &outcome {
        Ok(_) => info!(
            "saved the overrides of model `{}`/`{}`",
            request.provider_id, request.upstream_id
        ),

        Err(error) => error::log_error(
            &format!(
                "save-model `{}`/`{}`",
                request.provider_id, request.upstream_id
            ),
            error,
        ),
    }

    outcome
}

/// Renders one model listing entry.
///
/// # Errors
///
/// Returns [`ManagementApiError`] when the stored thinking-level override is
/// corrupt.
fn model_entry_json(entry: &ModelEntry) -> Result<Value, ManagementApiError> {
    let thinking_levels_override = thinking_levels_json(&entry.model.thinking_levels_override)?;

    // `is_enabled` is the model's configured switch; `is_effectively_enabled`
    // is the canonical effective status (`provider.is_enabled &&
    // model.is_enabled`).
    Ok(serde_json::json!({
        "provider_id": entry.provider_id,
        "provider": entry.provider_name,
        "upstream_id": entry.model.upstream_id,
        "id": entry.model.id,
        "id_override": entry.model.id_override,
        "is_enabled": entry.model.is_enabled,
        "is_effectively_enabled": entry.model.is_effectively_enabled(entry.is_provider_enabled),
        "thinking_levels_override": thinking_levels_override,
    }))
}

/// Renders the stored thinking-level override for listing, validating it
/// through the shared stored-override parser: a corrupt stored value is
/// drift inside the database — an invariant violation, symmetric with the
/// gateway's fail-fast — and is never masked as an absent override.
///
/// # Errors
///
/// Returns [`ManagementApiError`] when the stored override is corrupt.
fn thinking_levels_json(stored: &Option<String>) -> Result<Value, ManagementApiError> {
    let Some(json) = stored else {
        return Ok(Value::Null);
    };

    let levels =
        ir::parse_stored_thinking_levels_override(json).map_err(ManagementApiError::internal)?;

    Ok(ir::render_thinking_levels(&levels))
}
