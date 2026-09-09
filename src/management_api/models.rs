//! Management endpoints discovering, listing, and overriding models.

use super::{error_response, json_response, parse_body};
use crate::{
    database::Database,
    error::ApiError,
    ir::IrReasoningEffort,
    provider::{
        Model, discovery, oauth,
        storage::{self, ModelEntry},
    },
};
use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `discover-models` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverModelsBody {
    /// Provider identifier.
    provider: String,
}

/// Strictly parsed `save-model` request; each override field is tri-state:
/// absent leaves it unchanged, `null` clears it, a value sets it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveModelBody {
    /// Provider identifier.
    provider: String,

    /// Exact upstream model identifier.
    upstream_name: String,

    /// Public name override.
    #[serde(default)]
    name_override: Option<Option<String>>,

    /// Serving state override.
    #[serde(default)]
    enabled_override: Option<Option<bool>>,

    /// Canonical thinking levels override.
    #[serde(default)]
    thinking_levels_override: Option<Option<Vec<String>>>,
}

/// Handles `POST /discover-models`.
pub(crate) async fn discover(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_discover(&database, &body).await;

    match outcome {
        Ok(discovered) => json_response(200, serde_json::json!({ "discovered": discovered })),
        Err(error) => error_response(error),
    }
}

/// Handles `GET /list-models`.
pub(crate) async fn list(State(database): State<Database>) -> Response {
    match storage::list_model_entries(&database).await {
        Ok(entries) => {
            let rendered = entries.iter().map(model_entry_json).collect::<Vec<_>>();

            json_response(200, serde_json::json!({ "models": rendered }))
        }

        Err(error) => error_response(error),
    }
}

/// Handles `POST /save-model`.
pub(crate) async fn save(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_save(&database, &body).await;

    match outcome {
        Ok(rendered) => json_response(200, serde_json::json!({ "model": rendered })),
        Err(error) => error_response(error),
    }
}

/// Refreshes one provider's model rows from upstream, carrying overrides over
/// by `upstream_name`.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown providers, upstream failures, and public
/// name collisions.
async fn handle_discover(database: &Database, body: &Bytes) -> Result<usize, ApiError> {
    let request: DiscoverModelsBody = parse_body(body)?;

    let provider = storage::find_provider(database, &request.provider)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!("Provider `{}` does not exist", request.provider))
        })?;

    let fresh = oauth::ensure_fresh_token(database, crate::http(), &provider).await?;

    let upstream_names = discovery::discover_upstream_models(crate::http(), &fresh).await?;

    let previous = storage::find_models_of_provider(database, provider.id).await?;

    let mut models: Vec<Model> = Vec::with_capacity(upstream_names.len());

    for upstream_name in upstream_names {
        if models
            .iter()
            .any(|model| model.upstream_name == upstream_name)
        {
            continue;
        }

        let previous = previous
            .iter()
            .find(|model| model.upstream_name == upstream_name);

        let name_override = previous.and_then(|model| model.name_override.clone());

        let name = Model::public_name(
            &provider.model_prefix,
            &upstream_name,
            name_override.as_deref(),
        );

        models.push(Model {
            provider_id: provider.id,
            upstream_name,
            name,
            name_override,
            enabled_override: previous.and_then(|model| model.enabled_override),
            thinking_levels_override: previous
                .and_then(|model| model.thinking_levels_override.clone()),
        });
    }

    let mut transaction = database.transaction().await?;

    storage::replace_provider_models(&mut transaction, provider.id, &models).await?;

    transaction.commit().await.map_err(|error| {
        ApiError::internal(format!("failed to commit discovered models: {error}"))
    })?;

    Ok(models.len())
}

/// Writes one model's overrides and recomputes its public name.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown rows, invalid thinking levels, and public
/// name collisions.
async fn handle_save(database: &Database, body: &Bytes) -> Result<Value, ApiError> {
    let request: SaveModelBody = parse_body(body)?;

    let provider = storage::find_provider(database, &request.provider)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!("Provider `{}` does not exist", request.provider))
        })?;

    let model = storage::find_model(database, provider.id, &request.upstream_name)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "Model `{}` does not exist on provider `{}`",
                request.upstream_name, provider.name
            ))
        })?;

    let thinking_levels_override = match &request.thinking_levels_override {
        None => None,

        Some(None) => Some(None),

        Some(Some(levels)) => {
            let mut parsed = Vec::with_capacity(levels.len());

            for level in levels {
                let effort = IrReasoningEffort::parse_canonical(level).ok_or_else(|| {
                    ApiError::invalid_parameter(
                        "thinking_levels_override",
                        format!(
                            "Unknown thinking level `{level}`; \
                             expected one of off, low, medium, high"
                        ),
                    )
                })?;

                if parsed.contains(&effort) {
                    return Err(ApiError::invalid_parameter(
                        "thinking_levels_override",
                        format!("Duplicate thinking level `{level}`"),
                    ));
                }

                parsed.push(effort);
            }

            if parsed.is_empty() {
                return Err(ApiError::invalid_parameter(
                    "thinking_levels_override",
                    "thinking_levels_override must be a non-empty array",
                ));
            }

            let canonical: Vec<&str> = parsed
                .iter()
                .map(|effort| match effort {
                    IrReasoningEffort::Off => "off",
                    IrReasoningEffort::Low => "low",
                    IrReasoningEffort::Medium => "medium",
                    IrReasoningEffort::High => "high",
                })
                .collect();

            Some(Some(serde_json::to_string(&canonical).map_err(
                |error| ApiError::internal(format!("failed to encode thinking levels: {error}")),
            )?))
        }
    };

    let name_override = request.name_override.clone();

    let new_name_override = request
        .name_override
        .clone()
        .unwrap_or_else(|| model.name_override.clone());

    let new_name = Model::public_name(
        &provider.model_prefix,
        &request.upstream_name,
        new_name_override.as_deref(),
    );

    if let Some(routing) = storage::find_routing(database, &new_name).await?
        && !(routing.provider.id == provider.id
            && routing.model.upstream_name == request.upstream_name)
    {
        return Err(ApiError::conflict(format!(
            "Public model name `{new_name}` is already routed by provider `{}`",
            routing.provider.name
        )));
    }

    storage::update_model_overrides(
        database,
        provider.id,
        &request.upstream_name,
        &new_name,
        name_override,
        request.enabled_override,
        thinking_levels_override,
    )
    .await?;

    let updated = storage::find_model(database, provider.id, &request.upstream_name)
        .await?
        .ok_or_else(|| ApiError::internal("model vanished after save"))?;

    Ok(serde_json::json!({
        "provider": provider.name,
        "upstream_name": updated.upstream_name,
        "name": updated.name,
        "name_override": updated.name_override,
        "enabled": updated.enabled_override.unwrap_or(provider.enabled),
        "enabled_override": updated.enabled_override,
        "thinking_levels_override": thinking_levels_json(&updated.thinking_levels_override),
    }))
}

/// Renders one model listing entry.
fn model_entry_json(entry: &ModelEntry) -> Value {
    serde_json::json!({
        "provider": entry.provider_name,
        "upstream_name": entry.model.upstream_name,
        "name": entry.model.name,
        "name_override": entry.model.name_override,
        "enabled": entry.model.enabled_override.unwrap_or(entry.is_provider_enabled),
        "enabled_override": entry.model.enabled_override,
        "thinking_levels_override": thinking_levels_json(&entry.model.thinking_levels_override),
    })
}

/// Parses the stored canonical thinking-level array for rendering.
fn thinking_levels_json(stored: &Option<String>) -> Value {
    stored
        .as_deref()
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
        .unwrap_or(Value::Null)
}
