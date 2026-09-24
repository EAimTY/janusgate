//! Management routes listing, discovering, creating, updating, and deleting models.

use super::{EmptyResponse, Error, Nullable, route};
use crate::{
    context::Context,
    provider::{DiscoveryState, discovery, oauth},
    storage::{
        self,
        models::{ModelCreate, ModelEntry, ModelUpdate},
    },
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};

route! {
    pub ListModels: {
        path: "/models:list",
        method: http::Method::POST,
        request: ListModelsRequest,
        response: ListModelsResponse,
    }
}

route! {
    pub DiscoverModels: {
        path: "/models:discover",
        method: http::Method::POST,
        request: DiscoverModelsRequest,
        response: DiscoverModelsResponse,
    }
}

route! {
    pub CreateModel: {
        path: "/models:create",
        method: http::Method::POST,
        request: CreateModelRequest,
        response: ModelEnvelope,
    }
}

route! {
    pub UpdateModel: {
        path: "/models:update",
        method: http::Method::POST,
        request: UpdateModelRequest,
        response: ModelEnvelope,
    }
}

route! {
    pub DeleteModel: {
        path: "/models:delete",
        method: http::Method::POST,
        request: DeleteModelRequest,
        response: EmptyResponse,
    }
}

/// One model listing entry in its management wire shape (README §7.1). `id` is the row identifier
/// (`AUTOINCREMENT`, never recycled) that every row-addressed write goes through, while
/// `public_id` is the routing string clients address. The row carries only its own inputs —
/// `is_enabled`, `discovery_state` — and never the three-input effective verdict: the caller
/// derives that from these plus the provider's `is_enabled`, which `providers:list` already
/// returns (README §7.0 rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ModelResponse {
    /// Row identifier; the address of `models:update` and `models:delete`.
    pub id: i64,

    /// Owning provider row identifier.
    pub provider_id: i64,

    /// Owning provider's current name.
    pub provider: String,

    /// Exact upstream model identifier.
    pub upstream_id: String,

    /// Public routing ID.
    pub public_id: String,

    /// Explicit public ID override; `null` when the ID is composed from the provider's prefix —
    /// the nullable mirror column (rule 2).
    pub id_override: Option<String>,

    /// Configured serving switch.
    pub is_enabled: bool,

    /// Discovery lifecycle state of the row.
    pub discovery_state: DiscoveryState,
}

impl ModelResponse {
    /// Renders one stored entry.
    fn from_entry(entry: &ModelEntry) -> Self {
        Self {
            id: entry.model.id,
            provider_id: entry.provider_id,
            provider: entry.provider_name.clone(),
            upstream_id: entry.model.upstream_id.clone(),
            public_id: entry.model.public_id.clone(),
            id_override: entry.model.id_override.clone(),
            is_enabled: entry.model.is_enabled,
            discovery_state: entry.model.discovery_state,
        }
    }
}

/// Strictly parsed `models:list` request: the empty request lists every provider's rows; the filter
/// narrows the listing to one provider and is never `null` (README §7.0 rule 2) — an unknown
/// `provider_id` yields an empty list, because a filter addresses no row.
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

/// The `models:list` success body (README §7.1): every row, `vanished` ones included; the caller
/// derives the three-input effective status from the row's inputs and the provider's switch
/// (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ListModelsResponse {
    /// Every stored model, ordered by public ID.
    pub models: Vec<ModelResponse>,
}

/// The `models:discover` success body (README §7.1): the provider's complete fresh rows — which
/// `upstream_id`s appeared, how the public IDs composed, and which rows vanished are
/// transaction-recomputed facts the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DiscoverModelsResponse {
    /// The provider's model rows as stored by the refresh, all states included.
    pub models: Vec<ModelResponse>,
}

/// Strictly parsed `models:discover` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DiscoverModelsRequest {
    /// Provider whose model rows the refresh reapplies.
    pub provider_id: i64,
}

/// Strictly parsed `models:create` request (README §7.1): the complete desired initial state of
/// one manually added row, every field required (README §7.0 rule 3). `id_override` is the
/// required-nullable mirror column (rule 2) — `null` means no override, exactly like the storage
/// column. Addressed by the natural key `(provider_id, upstream_id)` — the row does not exist yet.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct CreateModelRequest {
    /// Owning provider row identifier.
    pub provider_id: i64,

    /// Exact upstream model identifier; routing-safe: non-empty printable ASCII without
    /// whitespace.
    pub upstream_id: String,

    /// Public ID override; `null` composes the ID from the provider's prefix.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "string | null")
    )]
    pub id_override: Nullable<String>,

    /// Serving state.
    pub is_enabled: bool,
}

/// Strictly parsed `models:update` request (README §7.1): the complete desired state of the row's
/// two writable columns, every field required (README §7.0 rule 3). `id_override` is the
/// required-nullable mirror column (rule 2). Addressed by the row identifier `model_id`, like
/// `api-keys:update`. The request cannot state `discovery_state`: this route writes only
/// `id_override` and `is_enabled`, so it can never restore a vanished row — recovery is an
/// upstream reappearance or a `models:create` claim.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct UpdateModelRequest {
    /// Addressed model row.
    pub model_id: i64,

    /// Public ID override; `null` composes the ID from the provider's prefix.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "string | null")
    )]
    pub id_override: Nullable<String>,

    /// Serving state.
    pub is_enabled: bool,
}

/// Strictly parsed `models:delete` request (README §7.1): addressed by the row identifier.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct DeleteModelRequest {
    /// Addressed model row.
    pub model_id: i64,
}

/// The `models:create` and `models:update` success body (README §7.1): the composed `public_id`
/// derives from transaction-time provider state — the current prefix under the collision check —
/// so it is not client-computable (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ModelEnvelope {
    /// The applied model row.
    pub model: ModelResponse,
}

/// Handles `POST /models:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ListModelsRequest>, Error>,
) -> Result<Json<ListModelsResponse>, Error> {
    let entries =
        storage::models::list_model_entries(&context.database, request.provider_id).await?;

    Ok(Json(ListModelsResponse {
        models: entries.iter().map(ModelResponse::from_entry).collect(),
    }))
}

/// Handles `POST /models:discover`.
pub(super) async fn discover(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DiscoverModelsRequest>, Error>,
) -> Result<Json<DiscoverModelsResponse>, Error> {
    let entries = handle_discover(&context.database, &context.control_plane, request).await?;

    Ok(Json(DiscoverModelsResponse {
        models: entries.iter().map(ModelResponse::from_entry).collect(),
    }))
}

/// Handles `POST /models:create`.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateModelRequest>, Error>,
) -> Result<Json<ModelEnvelope>, Error> {
    let entry = handle_create(&context.database, request).await?;

    Ok(Json(ModelEnvelope {
        model: ModelResponse::from_entry(&entry),
    }))
}

/// Handles `POST /models:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateModelRequest>, Error>,
) -> Result<Json<ModelEnvelope>, Error> {
    let entry = handle_update(&context.database, request).await?;

    Ok(Json(ModelEnvelope {
        model: ModelResponse::from_entry(&entry),
    }))
}

/// Handles `POST /models:delete`.
pub(super) async fn delete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DeleteModelRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    storage::models::delete_model_checked(&context.database, request.model_id).await?;

    info!("deleted model {}", request.model_id);

    Ok(Json(EmptyResponse {}))
}

/// Applies the management surface's discovery guard to a raw upstream identifier list: duplicates
/// keep their first occurrence, and an empty set — an upstream anomaly — is refused with the 502
/// upstream-error category before it can reach the storage layer, where an empty set would mark
/// the provider's whole model list vanished (README §7.1).
fn unique_upstream_ids(upstream_ids: Vec<String>) -> Result<Vec<String>, Error> {
    let mut unique: Vec<String> = Vec::with_capacity(upstream_ids.len());

    for upstream_id in upstream_ids {
        if unique.iter().any(|existing| existing == &upstream_id) {
            continue;
        }

        unique.push(upstream_id);
    }

    if unique.is_empty() {
        return Err(Error::UpstreamReportedNoModels);
    }

    Ok(unique)
}

/// Refreshes one provider's model rows from upstream: mark, flip, and insert (README §7.1),
/// answering with the provider's complete fresh rows.
///
/// `control_plane` is the bounded client used for the token refresh and the upstream discovery
/// calls.
///
/// # Errors
///
/// Returns [`Error`] for unknown providers, upstream failures (including an upstream that reports
/// no models), and public ID collisions.
async fn handle_discover(
    database: &crate::database::Database,
    control_plane: &Client,
    request: DiscoverModelsRequest,
) -> Result<Vec<ModelEntry>, Error> {
    let provider = storage::providers::find_provider_by_id(database, request.provider_id)
        .await?
        .ok_or(Error::ProviderNotFound {
            id: request.provider_id,
        })?;

    let fresh = oauth::ensure_fresh_token(database, control_plane, &provider).await?;

    let unique_ids =
        unique_upstream_ids(discovery::discover_upstream_models(control_plane, &fresh).await?)?;

    // An upstream reporting an identifier the gateway cannot route fails the refresh before any
    // row is written: the composed public ID would carry the same characters, and the schema
    // would reject the write as a constraint violation — a storage conflict instead of the
    // upstream failure it is (README §7.1).
    for upstream_id in &unique_ids {
        if super::validate_routing_token("upstream_id", upstream_id).is_err() {
            return Err(Error::UpstreamReportedInvalidModelId {
                upstream_id: upstream_id.clone(),
            });
        }
    }

    // The apply re-reads the provider row inside its transaction, so the public IDs are composed
    // from the provider's current prefix and checked for collisions atomically with the write
    // (README §7.1).
    let entries =
        storage::models::apply_discovered_models(database, request.provider_id, &unique_ids)
            .await?;

    info!(
        "discovered {} models for provider {}",
        entries.len(),
        request.provider_id
    );

    Ok(entries)
}

/// Handles one `models:create`: an absent row becomes a manual one, a vanished row is claimed
/// (the manual recovery path), an already-listed row conflicts.
///
/// # Errors
///
/// Returns [`Error`] for empty identifiers, unknown providers, already-stored models, and public
/// ID collisions.
async fn handle_create(
    database: &crate::database::Database,
    request: CreateModelRequest,
) -> Result<ModelEntry, Error> {
    validate_model_identifiers(&request.upstream_id, &request.id_override)?;

    // The checked insert re-reads the provider row inside its transaction, so the composed
    // public ID uses the provider's current prefix and the collision check applies atomically
    // with the write (README §7.1).
    let entry = storage::models::insert_model_checked(
        database,
        ModelCreate {
            provider_id: request.provider_id,
            upstream_id: &request.upstream_id,
            id_override: request.id_override.0.as_deref(),
            is_enabled: request.is_enabled,
        },
    )
    .await?;

    info!(
        "created model `{}`/`{}`",
        request.provider_id, request.upstream_id
    );

    Ok(entry)
}

/// Handles one `models:update`: addressed by row id, writing only `id_override` and `is_enabled`.
///
/// # Errors
///
/// Returns [`Error`] for an empty override, an unknown row, and public ID collisions.
async fn handle_update(
    database: &crate::database::Database,
    request: UpdateModelRequest,
) -> Result<ModelEntry, Error> {
    if let Some(id_override) = &request.id_override.0 {
        super::validate_routing_token("id_override", id_override)?;
    }

    // The checked update re-reads the row and its provider inside its transaction, so the
    // recomposed public ID uses the provider's current prefix and the collision check applies
    // atomically with the write (README §7.1).
    let entry = storage::models::update_model_overrides_checked(
        database,
        ModelUpdate {
            model_id: request.model_id,
            id_override: request.id_override.0.as_deref(),
            is_enabled: request.is_enabled,
        },
    )
    .await?;

    info!("updated model {}", request.model_id);

    Ok(entry)
}

/// Validates the identifiers `models:create` carries: `upstream_id` is always a routing token,
/// and a present `id_override` composes the public routing ID directly, so both carry the same
/// composition — while human labels stay free-form (README §7).
///
/// # Errors
///
/// Returns [`Error`] when `upstream_id` or a present `id_override` is empty or carries a
/// character outside the allowed set.
fn validate_model_identifiers(
    upstream_id: &str,
    id_override: &Nullable<String>,
) -> Result<(), Error> {
    super::validate_routing_token("upstream_id", upstream_id)?;

    if let Some(id_override) = &id_override.0 {
        super::validate_routing_token("id_override", id_override)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CreateModelRequest, DeleteModelRequest, ModelResponse, UpdateModelRequest};
    use crate::{
        management_api::v0::Nullable,
        provider::{DiscoveryState, Model},
        storage::models::ModelEntry,
    };

    /// Pins the `models` wire row at the JSON level: the row identifier under `id`, the routing
    /// string under `public_id`, and the row's own inputs `is_enabled` and `discovery_state` — and,
    /// positively, that no thinking-level field and no derived effective verdict rides along: the
    /// row carries its inputs and the caller derives the three-input status (README §7.0 rule 7).
    /// The `vanished` row is pinned beside `is_enabled: true`, so the derivation has everything it
    /// needs right in the row.
    ///
    /// # Panics
    ///
    /// Panics if the rendered row diverges from the documented shape.
    #[test]
    fn renders_the_documented_model_row() {
        let entry = |discovery_state, is_enabled| ModelEntry {
            provider_id: 7,
            provider_name: "acme".to_string(),
            model: Model {
                id: 42,
                upstream_id: "gpt-4".to_string(),
                public_id: "acme-gpt-4".to_string(),
                id_override: None,
                is_enabled,
                discovery_state,
            },
        };

        #[expect(clippy::expect_used, reason = "the row body always serializes")]
        let rendered = serde_json::to_value(ModelResponse::from_entry(&entry(
            DiscoveryState::Discovered,
            true,
        )))
        .expect("the row body serializes");

        assert_eq!(
            rendered,
            serde_json::json!({
                "id": 42,
                "provider_id": 7,
                "provider": "acme",
                "upstream_id": "gpt-4",
                "public_id": "acme-gpt-4",
                "id_override": null,
                "is_enabled": true,
                "discovery_state": "discovered",
            })
        );

        #[expect(clippy::expect_used, reason = "the row body always serializes")]
        let vanished = serde_json::to_value(ModelResponse::from_entry(&entry(
            DiscoveryState::Vanished,
            true,
        )))
        .expect("the row body serializes");

        assert_eq!(
            vanished,
            serde_json::json!({
                "id": 42,
                "provider_id": 7,
                "provider": "acme",
                "upstream_id": "gpt-4",
                "public_id": "acme-gpt-4",
                "id_override": null,
                "is_enabled": true,
                "discovery_state": "vanished",
            })
        );
    }

    /// Verifies the `models:update` request (README §7.0 rules 2-3, §7.1): addressed by `model_id`,
    /// both fields required, `id_override` nullable with the omitted-key tolerance, an unknown
    /// field rejected — and no `discovery_state` or thinking-level field accepted.
    ///
    /// # Panics
    ///
    /// Panics if the request parses or rejects outside its contract.
    #[test]
    fn parses_the_row_addressed_update_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: UpdateModelRequest =
            serde_json::from_str(r#"{"model_id":7,"id_override":null,"is_enabled":true}"#).unwrap();

        assert_eq!(parsed.model_id, 7);
        assert_eq!(parsed.id_override, Nullable(None));
        assert!(parsed.is_enabled);

        assert!(
            serde_json::from_str::<UpdateModelRequest>(
                r#"{"model_id":7,"id_override":null,"is_enabled":true,"bogus":1}"#
            )
            .is_err()
        );

        assert!(
            serde_json::from_str::<UpdateModelRequest>(
                r#"{"model_id":7,"id_override":null,"is_enabled":true,"discovery_state":"manual"}"#
            )
            .is_err()
        );
    }

    /// Verifies the `models:create` request (README §7.0 rule 3, §7.1): every field required, the
    /// nullable `id_override` mirror, an unknown field rejected, and no thinking-level field
    /// accepted.
    ///
    /// # Panics
    ///
    /// Panics if the request parses or rejects outside its contract.
    #[test]
    fn parses_the_create_request_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: CreateModelRequest = serde_json::from_str(
            r#"{"provider_id":7,"upstream_id":"gpt-4o","id_override":"custom","is_enabled":false}"#,
        )
        .unwrap();

        assert_eq!(parsed.provider_id, 7);
        assert_eq!(parsed.upstream_id, "gpt-4o");
        assert_eq!(parsed.id_override, Nullable(Some("custom".to_string())));
        assert!(!parsed.is_enabled);

        assert!(
            serde_json::from_str::<CreateModelRequest>(
                r#"{"provider_id":7,"upstream_id":"gpt-4o","id_override":null,"is_enabled":true}"#
            )
            .is_ok()
        );

        assert!(
            serde_json::from_str::<CreateModelRequest>(
                r#"{"provider_id":7,"upstream_id":"gpt-4o","id_override":null,
                    "is_enabled":true,"thinking_levels_override":null}"#
            )
            .is_err()
        );
    }

    /// Verifies the `models:delete` request is the row-addressed single-field payload (README
    /// §7.1) and that the discovery guard deduplicates identifiers keeping the first occurrence
    /// while refusing an empty upstream set with the upstream-error category — the 502 that keeps
    /// an empty discovery from marking the provider's rows vanished (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if the payload parses outside its contract or the guard diverges.
    #[test]
    fn parses_the_delete_request_and_guards_discovery() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: DeleteModelRequest = serde_json::from_str(r#"{"model_id":7}"#).unwrap();

        assert_eq!(parsed.model_id, 7);

        assert!(
            serde_json::from_str::<DeleteModelRequest>(r#"{"provider_id":7}"#).is_err(),
            "models:delete addresses the row by its own identifier"
        );

        #[expect(clippy::unwrap_used, reason = "duplicates are constructed to dedupe")]
        let unique =
            super::unique_upstream_ids(vec!["a".to_string(), "b".to_string(), "a".to_string()])
                .unwrap();

        assert_eq!(unique, ["a", "b"]);

        #[expect(
            clippy::expect_used,
            reason = "the empty set is constructed to be refused"
        )]
        let empty = super::unique_upstream_ids(Vec::new())
            .expect_err("an empty upstream set must be refused");

        assert!(matches!(empty, super::Error::UpstreamReportedNoModels));
    }
}
