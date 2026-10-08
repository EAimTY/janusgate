//! Management routes listing, discovering, creating, updating, and deleting models.

use super::{EmptyResponse, Error, Nullable, route};
use crate::{
    compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    context::Context,
    provider::{DiscoveredModel, DiscoveryState, discovery},
    storage::{
        self,
        models::{ModelCreate, ModelEntry, ModelUpdate},
    },
    vocabulary::ReasoningLevel,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

route! {
    pub(super) ListModels: {
        path: "/models:list",
        method: http::Method::POST,
        request: ListModelsRequest,
        response: ListModelsResponse,
    }
}

route! {
    pub(super) DiscoverModels: {
        path: "/models:discover",
        method: http::Method::POST,
        request: DiscoverModelsRequest,
        response: DiscoverModelsResponse,
    }
}

route! {
    pub(super) CreateModel: {
        path: "/models:create",
        method: http::Method::POST,
        request: CreateModelRequest,
        response: ModelEnvelope,
    }
}

route! {
    pub(super) UpdateModel: {
        path: "/models:update",
        method: http::Method::POST,
        request: UpdateModelRequest,
        response: ModelEnvelope,
    }
}

route! {
    pub(super) DeleteModel: {
        path: "/models:delete",
        method: http::Method::POST,
        request: DeleteModelRequest,
        response: EmptyResponse,
    }
}

/// One model listing entry in its management wire shape (README §7.1). `id` is the row identifier
/// (`AUTOINCREMENT`, never recycled) that every row-addressed write goes through, while
/// the physical account-local model carries its own inputs —
/// `is_enabled`, `discovery_state` — and never the three-input effective verdict: the caller
/// derives that from these plus the provider's `is_enabled`, which `providers:list` already
/// returns (README §7.0 rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct ModelResponse {
    /// Row identifier; the address of `models:update` and `models:delete`.
    id: i64,

    /// Owning provider row identifier.
    provider_id: i64,

    /// Owning provider's current name.
    provider: String,

    /// Exact upstream model identifier.
    upstream_id: String,

    /// Configured serving switch.
    is_enabled: bool,

    /// Discovery lifecycle state of the row.
    discovery_state: DiscoveryState,

    /// Discovered thinking levels in the vocabulary spellings, canonicalized; `null` is unknown
    /// — the latest upstream knowledge, never "none" (README §3).
    thinking_levels: Option<Vec<String>>,

    /// Discovered maximum input tokens; `null` is unknown.
    max_input_tokens: Option<i64>,

    /// Discovered maximum output tokens; `null` is unknown.
    max_output_tokens: Option<i64>,

    /// Administrator's thinking-level override in the vocabulary spellings, canonicalized;
    /// `null` keeps the discovered value (rule 2).
    thinking_levels_override: Option<Vec<String>>,

    /// Administrator's maximum-input-tokens override; `null` keeps the discovered value.
    max_input_tokens_override: Option<i64>,

    /// Administrator's maximum-output-tokens override; `null` keeps the discovered value.
    max_output_tokens_override: Option<i64>,

    /// Discovered compaction support facts — the per-mechanism known facts of the latest
    /// refresh, keyed by the closed mechanism spellings; `null` is unknown (README §3).
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "{ [key: string]: \"supported\" | \"unsupported\" } | null")
    )]
    compaction_support: Option<CompactionSupportFacts>,

    /// Administrator's compaction facts override; `null` keeps the discovered facts (rule 2).
    /// An explicit `"unknown"` member is legal here and suppresses the lower layers for that
    /// mechanism.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "{ [key: string]: \"supported\" | \"unsupported\" | \"unknown\" } | null")
    )]
    compaction_support_override: Option<CompactionSupportFacts>,
}

impl ModelResponse {
    /// Renders one stored entry, consuming it.
    fn from_entry(entry: ModelEntry) -> Self {
        let ModelEntry {
            provider_id,
            provider_name,
            model,
        } = entry;

        let crate::provider::Model {
            id,
            upstream_id,
            is_enabled,
            discovery_state,
            thinking_levels,
            max_input_tokens,
            max_output_tokens,
            thinking_levels_override,
            max_input_tokens_override,
            max_output_tokens_override,
            compaction_support,
            compaction_support_override,
        } = model;

        Self {
            id,
            provider_id,
            provider: provider_name,
            upstream_id,
            is_enabled,
            discovery_state,
            thinking_levels: thinking_levels.map(|levels| {
                levels
                    .into_iter()
                    .map(|level| level.as_str().to_string())
                    .collect()
            }),
            max_input_tokens,
            max_output_tokens,
            thinking_levels_override: thinking_levels_override.map(|levels| {
                levels
                    .into_iter()
                    .map(|level| level.as_str().to_string())
                    .collect()
            }),
            max_input_tokens_override,
            max_output_tokens_override,
            compaction_support,
            compaction_support_override,
        }
    }
}

/// Strictly parsed `models:list` request: the empty request lists every provider's rows; the filter
/// narrows the listing to one provider and is never `null` (README §7.0 rule 2) — an unknown
/// `provider_id` yields an empty list, because a filter addresses no row.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListModelsRequest {
    /// Owning provider row identifier filter.
    #[serde(
        default,
        deserialize_with = "super::absent_keeps::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    provider_id: Option<i64>,
}

/// The `models:list` success body (README §7.1): every row, `vanished` ones included; the caller
/// derives the three-input effective status from the row's inputs and the provider's switch
/// (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListModelsResponse {
    /// Every stored model, ordered by provider ID and upstream ID.
    models: Vec<ModelResponse>,
}

/// The `models:discover` success body (README §7.1): the provider's complete fresh rows — which
/// `upstream_id`s appeared, which rows vanished are
/// transaction-recomputed facts the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DiscoverModelsResponse {
    /// The provider's model rows as stored by the refresh, all states included.
    models: Vec<ModelResponse>,
}

/// Strictly parsed `models:discover` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DiscoverModelsRequest {
    /// Provider whose model rows the refresh reapplies.
    provider_id: i64,
}

/// Strictly parsed `models:create` request (README §7.1): the complete desired initial state of
/// one manually added row, every field required (README §7.0 rule 3). The
/// capability overrides are the required-nullable mirror columns (rule 2) — `null` means no
/// override, exactly like the storage columns. Addressed by the natural key
/// `(provider_id, upstream_id)` — the row does not exist yet.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateModelRequest {
    /// Owning provider row identifier.
    provider_id: i64,

    /// Exact upstream model identifier; routing-safe: nonempty printable ASCII without
    /// whitespace, `/`, or `:`.
    upstream_id: String,

    /// Serving state.
    is_enabled: bool,

    /// Thinking-level override; `null` keeps the discovered value, an array replaces the
    /// declaration wholesale — the empty array is the positive declaration of no levels (README
    /// §3).
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<string> | null")
    )]
    thinking_levels_override: Nullable<Vec<String>>,

    /// Maximum-input-tokens override; `null` keeps the discovered value.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "number | null")
    )]
    max_input_tokens_override: Nullable<i64>,

    /// Maximum-output-tokens override; `null` keeps the discovered value.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "number | null")
    )]
    max_output_tokens_override: Nullable<i64>,

    /// Compaction support facts override; `null` keeps the discovered facts, an object replaces
    /// them wholesale — the mechanism spellings are the closed keys, `"unknown"` is a legal
    /// explicit member value, and the base facts are never the request's to state (README §3).
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "{ [key: string]: \"supported\" | \"unsupported\" | \"unknown\" } | null")
    )]
    compaction_support_override: Nullable<BTreeMap<String, String>>,
}

/// Strictly parsed `models:update` request (README §7.1): the complete state of the row's
/// writable columns, every field required (README §7.0 rule 3). The capability
/// overrides are the required-nullable mirror columns (rule 2). Addressed by the row identifier
/// `model_id`, like `api-keys:update`. The request cannot state `discovery_state` or a
/// capability base column: this route writes only `is_enabled` and the
/// capability overrides, so it can never restore a vanished row — recovery is an upstream
/// reappearance or a `models:create` claim.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateModelRequest {
    /// Addressed model row.
    model_id: i64,

    /// Serving state.
    is_enabled: bool,

    /// Thinking-level override; `null` keeps the discovered value, an array replaces the
    /// declaration wholesale — the empty array is the positive declaration of no levels (README
    /// §3).
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "Array<string> | null")
    )]
    thinking_levels_override: Nullable<Vec<String>>,

    /// Maximum-input-tokens override; `null` keeps the discovered value.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "number | null")
    )]
    max_input_tokens_override: Nullable<i64>,

    /// Maximum-output-tokens override; `null` keeps the discovered value.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "number | null")
    )]
    max_output_tokens_override: Nullable<i64>,

    /// Compaction support facts override; `null` keeps the discovered facts, an object replaces
    /// them wholesale.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(type = "{ [key: string]: \"supported\" | \"unsupported\" | \"unknown\" } | null")
    )]
    compaction_support_override: Nullable<BTreeMap<String, String>>,
}

/// Strictly parsed `models:delete` request (README §7.1): addressed by the row identifier.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DeleteModelRequest {
    /// Addressed model row.
    model_id: i64,
}

/// The applied physical model returned by creation and administrator override updates.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ModelEnvelope {
    /// The applied model row.
    model: ModelResponse,
}

/// Handles `POST /models:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ListModelsRequest>, Error>,
) -> Result<Json<ListModelsResponse>, Error> {
    let entries =
        storage::models::list_model_entries(&context.database, request.provider_id).await?;

    Ok(Json(ListModelsResponse {
        models: entries.into_iter().map(ModelResponse::from_entry).collect(),
    }))
}

/// Handles `POST /models:discover`.
pub(super) async fn discover(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DiscoverModelsRequest>, Error>,
) -> Result<Json<DiscoverModelsResponse>, Error> {
    let entries = handle_discover(&context.database, &context.control_plane, request).await?;

    Ok(Json(DiscoverModelsResponse {
        models: entries.into_iter().map(ModelResponse::from_entry).collect(),
    }))
}

/// Handles `POST /models:create`.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateModelRequest>, Error>,
) -> Result<Json<ModelEnvelope>, Error> {
    let entry = handle_create(&context.database, request).await?;

    Ok(Json(ModelEnvelope {
        model: ModelResponse::from_entry(entry),
    }))
}

/// Handles `POST /models:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateModelRequest>, Error>,
) -> Result<Json<ModelEnvelope>, Error> {
    let entry = handle_update(&context.database, request).await?;

    Ok(Json(ModelEnvelope {
        model: ModelResponse::from_entry(entry),
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

/// Applies the management surface's discovery guard to a raw upstream model list: duplicates
/// keep their first occurrence, and an empty set — an upstream anomaly — is refused with the 502
/// upstream-error category before it can reach the storage layer, where an empty set would mark
/// the provider's whole model list vanished (README §7.1).
fn unique_discovered_models(
    discovered: Vec<DiscoveredModel>,
) -> Result<Vec<DiscoveredModel>, Error> {
    let mut unique: Vec<DiscoveredModel> = Vec::with_capacity(discovered.len());
    let mut seen: HashSet<String> = HashSet::with_capacity(discovered.len());

    for model in discovered {
        if !seen.contains(&model.upstream_id) {
            seen.insert(model.upstream_id.clone());
            unique.push(model);
        }
    }

    if unique.is_empty() {
        return Err(Error::UpstreamReportedNoModels);
    }

    Ok(unique)
}

/// The parsed, validated form of one request's capability overrides (README §7.1): level
/// values validated against the closed vocabulary and canonicalized — deduplicated,
/// vocabulary-ordered, the empty set preserved as the positive declaration of no levels — token
/// caps checked positive, and the compaction facts map's mechanism and support spellings
/// validated against their closed vocabularies, the map canonicalized into the mechanism
/// order.
#[derive(Debug)]
struct CapabilityOverrides {
    /// Validated, canonicalized thinking-level override; `None` keeps the discovered value.
    thinking_levels: Option<Vec<ReasoningLevel>>,

    /// Validated maximum-input-tokens override.
    max_input_tokens: Option<i64>,

    /// Validated maximum-output-tokens override.
    max_output_tokens: Option<i64>,

    /// Validated, canonicalized compaction support facts override; `None` keeps the discovered
    /// facts.
    compaction_support: Option<CompactionSupportFacts>,
}

/// Validates one request's capability overrides: every level string parses against the closed
/// vocabulary (README §6.4), the set canonicalizes (deduplicated, vocabulary-ordered), every
/// present token cap is positive, and every compaction-facts key and value parses against the
/// closed mechanism and support vocabularies — the map then canonicalizes into the mechanism
/// order, an empty object normalizing to the no-override `None`.
///
/// # Errors
///
/// Returns [`Error::UnknownReasoningLevel`] naming the field for a foreign level string,
/// [`Error::NonPositiveTokenCap`] for a non-positive cap, and
/// [`Error::UnknownCompactionSupport`] naming the field for a foreign mechanism or support
/// spelling.
fn parse_capability_overrides(
    thinking_levels: Nullable<Vec<String>>,
    max_input_tokens: Nullable<i64>,
    max_output_tokens: Nullable<i64>,
    compaction_support: Nullable<BTreeMap<String, String>>,
) -> Result<CapabilityOverrides, Error> {
    let thinking_levels = thinking_levels
        .0
        .map(|levels| {
            levels
                .into_iter()
                .map(|level| {
                    ReasoningLevel::from_vocabulary_str(&level).ok_or_else(|| {
                        Error::UnknownReasoningLevel {
                            param: "thinking_levels_override",
                            value: level,
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(ReasoningLevel::canonicalize_levels)
        })
        .transpose()?;

    for (param, cap) in [
        ("max_input_tokens_override", max_input_tokens.0),
        ("max_output_tokens_override", max_output_tokens.0),
    ] {
        if cap.is_some_and(|value| value <= 0) {
            return Err(Error::NonPositiveTokenCap { param });
        }
    }

    let compaction_support = compaction_support
        .0
        .map(|map| {
            map.into_iter()
                .map(|(mechanism, support)| {
                    let mechanism = Mechanism::from_str(&mechanism).ok_or_else(|| {
                        Error::UnknownCompactionSupport {
                            param: "compaction_support_override",
                            value: mechanism,
                        }
                    })?;

                    let support = MechanismSupport::from_str(&support).ok_or_else(|| {
                        Error::UnknownCompactionSupport {
                            param: "compaction_support_override",
                            value: support,
                        }
                    })?;

                    Ok((mechanism, support))
                })
                .collect::<Result<BTreeMap<_, _>, Error>>()
        })
        .transpose()?
        .and_then(CompactionSupportFacts::from_pairs);

    Ok(CapabilityOverrides {
        thinking_levels,
        max_input_tokens: max_input_tokens.0,
        max_output_tokens: max_output_tokens.0,
        compaction_support,
    })
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
/// no models), or failed storage.
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

    // The discovery engine refreshes the access token itself when a known expiry requires it.
    let unique_models = unique_discovered_models(
        discovery::discover_upstream_models(database, control_plane, &provider).await?,
    )?;

    // Validate the complete report before any lifecycle or capability write can apply. The
    // upstream-reported ids are not request fields, so the boundary checks the shared token
    // predicate directly and maps a failure to the report rejection — there is no request
    // parameter for a client-facing identifier rejection to name.
    for model in &unique_models {
        if !crate::routing_text::is_routing_safe_token(&model.upstream_id) {
            return Err(Error::UpstreamReportedInvalidModelId {
                upstream_id: model.upstream_id.clone(),
            });
        }
    }

    // Apply the complete discovery report atomically against the current account.
    let entries =
        storage::models::apply_discovered_models(database, request.provider_id, &unique_models)
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
    super::validate_routing_token("upstream_id", &request.upstream_id)?;

    let overrides = parse_capability_overrides(
        request.thinking_levels_override,
        request.max_input_tokens_override,
        request.max_output_tokens_override,
        request.compaction_support_override,
    )?;

    // Natural-key creation and vanished-row claims share one atomic write.
    let entry = storage::models::insert_model_checked(
        database,
        ModelCreate {
            provider_id: request.provider_id,
            upstream_id: &request.upstream_id,
            is_enabled: request.is_enabled,
            thinking_levels_override: overrides.thinking_levels.as_deref(),
            max_input_tokens_override: overrides.max_input_tokens,
            max_output_tokens_override: overrides.max_output_tokens,
            compaction_support_override: overrides.compaction_support.as_ref(),
        },
    )
    .await?;

    info!(
        "created model `{}`/`{}`",
        request.provider_id, request.upstream_id
    );

    Ok(entry)
}

/// Handles one `models:update`: addressed by row id, writing the serving switch and capability
/// overrides.
///
/// # Errors
///
/// Returns [`Error`] for invalid capability overrides, an unknown row, or failed storage.
async fn handle_update(
    database: &crate::database::Database,
    request: UpdateModelRequest,
) -> Result<ModelEntry, Error> {
    let overrides = parse_capability_overrides(
        request.thinking_levels_override,
        request.max_input_tokens_override,
        request.max_output_tokens_override,
        request.compaction_support_override,
    )?;

    // Row-addressed curation preserves discovery-owned state and base facts.
    let entry = storage::models::update_model_overrides_checked(
        database,
        ModelUpdate {
            model_id: request.model_id,
            is_enabled: request.is_enabled,
            thinking_levels_override: overrides.thinking_levels.as_deref(),
            max_input_tokens_override: overrides.max_input_tokens,
            max_output_tokens_override: overrides.max_output_tokens,
            compaction_support_override: overrides.compaction_support.as_ref(),
        },
    )
    .await?;

    info!("updated model {}", request.model_id);

    Ok(entry)
}
