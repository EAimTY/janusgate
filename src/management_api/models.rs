//! Management endpoints discovering, listing, and overriding models.

use super::{
    error::{self, ManagementApiError},
    route,
};
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
use serde::{Deserialize, Serialize};

route! {
    pub impl Route for DiscoverModels {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/discover-models";
        type Request = DiscoverModelsRequest;
        type Response = DiscoverModelsResponse;
    }
}

route! {
    pub impl Route for ListModels {
        const METHOD: http::Method = http::Method::GET;
        const PATH: &'static str = "/list-models";
        type Request = ListModelsRequest;
        type Response = ListModelsResponse;
    }
}

route! {
    pub impl Route for SaveModel {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/save-model";
        type Request = SaveModelRequest;
        type Response = ModelEnvelope;
    }
}

/// One model listing entry in its management wire shape (README §7.1).
/// `is_enabled` is the model's configured switch while
/// `is_effectively_enabled` is the canonical effective status
/// (`provider.is_enabled && model.is_enabled`).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ModelResponse {
    /// Owning provider row identifier.
    pub provider_id: i64,

    /// Owning provider's current name.
    pub provider: String,

    /// Exact upstream model identifier.
    pub upstream_id: String,

    /// Public routing ID.
    pub id: String,

    /// Explicit public ID override; `null` when the ID is composed from the
    /// provider's prefix.
    pub id_override: Option<String>,

    /// Configured serving switch.
    pub is_enabled: bool,

    /// Whether the model currently serves traffic.
    pub is_effectively_enabled: bool,

    /// Canonical thinking level override; `null` when no filtering is
    /// configured.
    pub thinking_levels_override: Option<Vec<ir::IrReasoningEffort>>,
}

impl ModelResponse {
    /// Renders one stored entry, revalidating its stored thinking-levels
    /// override through the shared stored-override parser.
    ///
    /// # Errors
    ///
    /// Returns [`ManagementApiError`] when the stored override is corrupt.
    fn from_entry(entry: &ModelEntry) -> Result<Self, ManagementApiError> {
        Ok(Self {
            provider_id: entry.provider_id,
            provider: entry.provider_name.clone(),
            upstream_id: entry.model.upstream_id.clone(),
            id: entry.model.id.clone(),
            id_override: entry.model.id_override.clone(),
            is_enabled: entry.model.is_enabled,
            is_effectively_enabled: entry
                .model
                .is_effectively_enabled(entry.is_provider_enabled),
            thinking_levels_override: thinking_levels_override(
                &entry.model.thinking_levels_override,
            )?,
        })
    }
}

/// The `list-models` request body (README §7.1); the route carries no body,
/// so the payload is the empty request type.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListModelsRequest {}

/// The `list-models` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListModelsResponse {
    /// Every stored model, ordered by public ID.
    pub models: Vec<ModelResponse>,
}

/// The `save-model` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ModelEnvelope {
    /// The saved model row.
    pub model: ModelResponse,
}

/// The `discover-models` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DiscoverModelsResponse {
    /// Number of model rows the refresh stored.
    pub discovered: usize,
}

/// Strictly parsed `discover-models` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DiscoverModelsRequest {
    /// Provider row identifier.
    pub id: i64,
}

/// Strictly parsed `save-model` request; each override field is tri-state:
/// absent leaves it unchanged, `null` clears it, a value sets it. The
/// `double_option` adapter is what makes the mapping explicit, since plain
/// serde maps JSON `null` and field absence to the same outer `None`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct SaveModelRequest {
    /// Owning provider row identifier.
    pub provider_id: i64,

    /// Exact upstream model identifier; must be non-empty.
    pub upstream_id: String,

    /// Public ID override.
    #[serde(
        default,
        with = "::serde_with::rust::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(optional = nullable, as = "Option<String>")
    )]
    pub id_override: Option<Option<String>>,

    /// Serving state.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional = nullable))]
    pub is_enabled: Option<bool>,

    /// Canonical thinking levels override.
    #[serde(
        default,
        with = "::serde_with::rust::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(optional = nullable, as = "Option<Vec<String>>")
    )]
    pub thinking_levels_override: Option<Option<Vec<String>>>,
}

/// Handles `POST /discover-models`.
pub(super) async fn discover(State(context): State<Context>, body: Bytes) -> Response {
    let outcome = handle_discover(&context.database, &context.control_plane, &body).await;

    match outcome {
        Ok(discovered) => {
            super::json_response(StatusCode::OK, &DiscoverModelsResponse { discovered })
        }
        Err(error) => error.into_response(),
    }
}

/// Handles `GET /list-models`.
pub(super) async fn list(State(context): State<Context>) -> Response {
    let outcome = async {
        let entries = storage::list_model_entries(&context.database).await?;

        let rendered = entries
            .iter()
            .map(ModelResponse::from_entry)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ListModelsResponse { models: rendered })
    }
    .await;

    match outcome {
        Ok(rendered) => super::json_response(StatusCode::OK, &rendered),
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
        Ok(rendered) => super::json_response(StatusCode::OK, &ModelEnvelope { model: rendered }),
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
    let request: DiscoverModelsRequest = super::parse_body(body, "discover-models")?;

    let outcome = async {
        let provider = storage::find_provider_by_id(database, request.id)
            .await?
            .ok_or_else(|| {
                ManagementApiError::not_found(storage::provider_not_found_message(request.id))
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

/// Writes one model's overrides and recomputes its public ID. The outcome is
/// logged: the saved overrides at info level, every failure at the
/// model's response boundary.
///
/// # Errors
///
/// Returns [`ManagementApiError`] for empty identifiers, unknown rows, invalid
/// thinking levels, and public ID collisions.
async fn handle_save(
    database: &Database,
    body: &Bytes,
) -> Result<ModelResponse, ManagementApiError> {
    let request: SaveModelRequest = super::parse_body(body, "save-model")?;

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

                Some(Some(ir::render_thinking_levels(&parsed)))
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
    .and_then(|entry| ModelResponse::from_entry(&entry));

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

/// Renders the stored thinking-level override for listing as canonical
/// levels, validating it through the shared stored-override parser: a corrupt
/// stored value is drift inside the database — an invariant violation,
/// symmetric with the gateway's fail-fast — and is never masked as an absent
/// override.
///
/// # Errors
///
/// Returns [`ManagementApiError`] when the stored override is corrupt.
fn thinking_levels_override(
    stored: &Option<String>,
) -> Result<Option<Vec<ir::IrReasoningEffort>>, ManagementApiError> {
    let Some(json) = stored else {
        return Ok(None);
    };

    ir::parse_stored_thinking_levels_override(json)
        .map(Some)
        .map_err(ManagementApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that a stored `thinking_levels_override` renders as canonical
    /// level names, that an absent override stays `null`, and that drift fails
    /// instead of rendering as absent. The render is the listing contract
    /// README §7.1 documents, and the drift rejection is what keeps a corrupt
    /// stored value from silently relaxing a model's filtering.
    ///
    /// # Panics
    ///
    /// Panics if the rendered override diverges from the documented shape or
    /// the drift rejection stops firing.
    #[test]
    fn renders_stored_thinking_levels_override() {
        #[expect(clippy::expect_used, reason = "an absent override renders")]
        let absent = thinking_levels_override(&None)
            .expect("an absent override renders")
            .is_none();

        assert!(absent);

        #[expect(
            clippy::expect_used,
            reason = "the stored override is constructed to be valid"
        )]
        let valid = thinking_levels_override(&Some(r#"["low","off"]"#.to_string()))
            .expect("a valid stored override parses");

        assert_eq!(
            valid,
            Some(vec![ir::IrReasoningEffort::Low, ir::IrReasoningEffort::Off])
        );

        assert!(thinking_levels_override(&Some(r#"["bogus"]"#.to_string())).is_err());
    }
}
