//! Management routes listing, discovering, and overriding models.

use super::{Error, IrReasoningEffort, Nullable, route, validate_identifier};
use crate::{
    context::Context,
    ir,
    provider::{
        discovery, oauth,
        storage::{self, ModelEntry},
    },
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};

route! {
    pub impl Route for ListModels {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/models:list";
        type Request = ListModelsRequest;
        type Response = ListModelsResponse;
    }
}

route! {
    pub impl Route for DiscoverModels {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/models:discover";
        type Request = DiscoverModelsRequest;
        type Response = DiscoverModelsResponse;
    }
}

route! {
    pub impl Route for UpdateModel {
        const METHOD: http::Method = http::Method::POST;
        const PATH: &'static str = "/models:update";
        type Request = UpdateModelRequest;
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
    /// provider's prefix. One of the two nullable mirror columns.
    pub id_override: Option<String>,

    /// Configured serving switch.
    pub is_enabled: bool,

    /// Whether the model currently serves traffic.
    pub is_effectively_enabled: bool,

    /// Canonical thinking level override; `null` when no filtering is
    /// configured. The other nullable mirror column.
    pub thinking_levels_override: Option<Vec<IrReasoningEffort>>,
}

impl ModelResponse {
    /// Renders one stored entry, revalidating its stored thinking-levels
    /// override through the shared stored-override parser.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the stored override is corrupt.
    fn from_entry(entry: &ModelEntry) -> Result<Self, Error> {
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

/// Strictly parsed `models:list` request: the empty request lists every
/// provider's rows; the filter narrows the listing to one provider and is
/// never `null` (README §7.0 rule 2) — an unknown `provider_id` yields an
/// empty list, because a filter addresses no row.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListModelsRequest {
    /// Owning provider row identifier filter.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub provider_id: Option<i64>,
}

/// The `models:list` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListModelsResponse {
    /// Every stored model, ordered by public ID.
    pub models: Vec<ModelResponse>,
}

/// The `models:discover` success body (README §7.1): the provider's
/// complete fresh rows — which `upstream_id`s appeared, how the public IDs
/// composed, and which overrides carried over are transaction-recomputed
/// facts the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DiscoverModelsResponse {
    /// The provider's model rows as stored by the refresh.
    pub models: Vec<ModelResponse>,
}

/// Strictly parsed `models:discover` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DiscoverModelsRequest {
    /// Provider whose model rows the refresh replaces.
    pub provider_id: i64,
}

/// Strictly parsed `models:update` request (README §7.1): the complete
/// desired override state of the row, every field required (README §7.0
/// rule 3). The two override fields are the required-nullable mirror
/// columns (rule 2) — `null` means no override, exactly like the storage
/// columns — so the request is the row's mirror: a client renders the row,
/// edits it, and sends the same shape back. Model rows are never created or
/// deleted here; discovery is their only source.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct UpdateModelRequest {
    /// Owning provider row identifier.
    pub provider_id: i64,

    /// Exact upstream model identifier; must be non-empty.
    pub upstream_id: String,

    /// Public ID override; `null` composes the ID from the provider's
    /// prefix.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "string | null")
    )]
    pub id_override: Nullable<String>,

    /// Serving state.
    pub is_enabled: bool,

    /// Canonical thinking level override; `null` disables filtering.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<IrReasoningEffort> | null")
    )]
    pub thinking_levels_override: Nullable<Vec<IrReasoningEffort>>,
}

/// The `models:update` success body (README §7.1): the recomposed `id` and
/// `is_effectively_enabled` derive from transaction-time provider state, so
/// they are not client-computable.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ModelEnvelope {
    /// The rewritten model row.
    pub model: ModelResponse,
}

/// Handles `POST /models:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ListModelsRequest>, Error>,
) -> Result<Json<ListModelsResponse>, Error> {
    let entries = storage::list_model_entries(&context.database, request.provider_id).await?;

    let models = entries
        .iter()
        .map(ModelResponse::from_entry)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(ListModelsResponse { models }))
}

/// Handles `POST /models:discover`.
pub(super) async fn discover(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DiscoverModelsRequest>, Error>,
) -> Result<Json<DiscoverModelsResponse>, Error> {
    let entries = handle_discover(&context.database, &context.control_plane, request).await?;

    let models = entries
        .iter()
        .map(ModelResponse::from_entry)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(DiscoverModelsResponse { models }))
}

/// Handles `POST /models:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateModelRequest>, Error>,
) -> Result<Json<ModelEnvelope>, Error> {
    let entry = handle_update(&context.database, request).await?;

    Ok(Json(ModelEnvelope {
        model: ModelResponse::from_entry(&entry)?,
    }))
}

/// Refreshes one provider's model rows from upstream, carrying overrides
/// over by `upstream_id`.
///
/// `control_plane` is the bounded client used for the token refresh and the
/// upstream discovery calls.
///
/// # Errors
///
/// Returns [`Error`] for unknown providers, upstream failures (including
/// an upstream that reports no models), and public ID collisions.
async fn handle_discover(
    database: &crate::database::Database,
    control_plane: &Client,
    request: DiscoverModelsRequest,
) -> Result<Vec<ModelEntry>, Error> {
    let provider = storage::find_provider_by_id(database, request.provider_id)
        .await?
        .ok_or(Error::ProviderNotFound {
            id: request.provider_id,
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
        return Err(Error::UpstreamReportedNoModels);
    }

    // The replace re-reads the provider row inside its transaction, so the
    // public IDs are composed from the provider's current prefix and
    // checked for collisions atomically with the write (README §7.1).
    let entries =
        storage::replace_discovered_models(database, request.provider_id, &unique_ids).await?;

    info!(
        "discovered {} models for provider {}",
        entries.len(),
        request.provider_id
    );

    Ok(entries)
}

/// Rewrites one model's full override state and recomputes its public ID.
///
/// # Errors
///
/// Returns [`Error`] for empty identifiers, unknown rows, invalid
/// thinking levels, and public ID collisions.
async fn handle_update(
    database: &crate::database::Database,
    request: UpdateModelRequest,
) -> Result<ModelEntry, Error> {
    validate_identifier("upstream_id", &request.upstream_id)?;

    if let Some(id_override) = &request.id_override.0 {
        validate_identifier("id_override", id_override)?;
    }

    let thinking_levels_override = match &request.thinking_levels_override.0 {
        // The request field is already the canonical level set; only the
        // documented non-emptiness needs enforcing here.
        None => None,

        Some(levels) => {
            if levels.is_empty() {
                return Err(Error::from(ir::InvalidRequest::field(
                    "thinking_levels_override",
                    "must be a non-empty array",
                )));
            }

            Some(ir::render_thinking_levels(levels))
        }
    };

    // The checked update re-reads the provider and model rows inside its
    // transaction, so the recomputed public ID uses the provider's current
    // prefix and the collision check applies atomically with the write
    // (README §7.1).
    let entry = storage::update_model_overrides_checked(
        database,
        storage::ModelOverrideUpdate {
            provider_id: request.provider_id,
            upstream_id: &request.upstream_id,
            id_override: request.id_override.0.as_deref(),
            is_enabled: request.is_enabled,
            thinking_levels_override: thinking_levels_override.as_deref(),
        },
    )
    .await?;

    info!(
        "updated the overrides of model `{}`/`{}`",
        request.provider_id, request.upstream_id
    );

    Ok(entry)
}

/// Renders the stored thinking-level override for listing as canonical
/// levels, validating it through the shared stored-override parser: a corrupt
/// stored value is drift inside the database — an invariant violation,
/// symmetric with the gateway's fail-fast — and is never masked as an absent
/// override.
///
/// # Errors
///
/// Returns [`Error`] when the stored override is corrupt.
fn thinking_levels_override(
    stored: &Option<String>,
) -> Result<Option<Vec<ir::IrReasoningEffort>>, Error> {
    let Some(json) = stored else {
        return Ok(None);
    };

    ir::parse_stored_thinking_levels_override(json)
        .map(Some)
        .map_err(|detail| Error::StoredOverrideDrift { detail })
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

    /// Verifies the `models:update` request's two-state poles (README §7.0
    /// rule 2): both override fields must be present and accept `null`, and
    /// an unknown field is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the request parses or rejects outside its contract.
    #[test]
    fn parses_the_full_state_update_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: UpdateModelRequest = serde_json::from_str(
            r#"{"provider_id":7,"upstream_id":"gpt-4o","id_override":null,
                "is_enabled":true,"thinking_levels_override":["low","high"]}"#,
        )
        .unwrap();

        assert_eq!(parsed.id_override, Nullable(None));
        assert_eq!(
            parsed.thinking_levels_override,
            Nullable(Some(vec![IrReasoningEffort::Low, IrReasoningEffort::High]))
        );

        // An omitted override key reads as `null` — the documented
        // tolerance of rule 2: under full-state semantics the two spellings
        // share the one meaning, and the bindings keep well-typed clients
        // honest by requiring the key.
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let omitted: UpdateModelRequest = serde_json::from_str(
            r#"{"provider_id":7,"upstream_id":"gpt-4o","is_enabled":true,
                "thinking_levels_override":null}"#,
        )
        .unwrap();

        assert_eq!(omitted.id_override, Nullable(None));

        assert!(
            serde_json::from_str::<UpdateModelRequest>(
                r#"{"provider_id":7,"upstream_id":"gpt-4o","id_override":null,
                    "is_enabled":true,"thinking_levels_override":null,"bogus":1}"#
            )
            .is_err()
        );
    }
}
