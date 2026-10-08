//! Management routes listing, creating, and versioning provider definitions (README §6).
//!
//! `create` inserts version 1 under a new name; `update` must name the current latest
//! `(name, version)` and atomically inserts the successor version under that name, rejecting a
//! stale base version. Existing rows are never updated or deleted through the management API, so
//! a composite identity is never reused; every payload passes the write-time validation once,
//! here, and runtime never re-validates.

use super::{Error, route};
use crate::{
    context::Context,
    provider::definition::{DefinitionPayload, ProviderDefinition},
    storage, unix_time,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use serde::{Deserialize, Serialize};
use std::num::NonZeroI64;

route! {
    pub(super) ListProviderDefinitions: {
        path: "/provider-definitions:list",
        method: http::Method::POST,
        request: ListProviderDefinitionsRequest,
        response: ListProviderDefinitionsResponse,
    }
}

route! {
    pub(super) CreateProviderDefinition: {
        path: "/provider-definitions:create",
        method: http::Method::POST,
        request: CreateProviderDefinitionRequest,
        response: DefinitionEnvelope,
    }
}

route! {
    pub(super) UpdateProviderDefinition: {
        path: "/provider-definitions:update",
        method: http::Method::POST,
        request: UpdateProviderDefinitionRequest,
        response: DefinitionEnvelope,
    }
}

/// The `provider-definitions:list` request body: the empty request of a no-argument route.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListProviderDefinitionsRequest {}

/// The `provider-definitions:list` success body: every stored version, ordered by name then
/// version. The latest version of a name is the client's derivation (`MAX(version)` over the
/// listing), never a movable alias the server substitutes.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListProviderDefinitionsResponse {
    /// Every stored definition version.
    provider_definitions: Vec<DefinitionResponse>,
}

/// Strictly parsed `provider-definitions:create` request: the complete state of one definition's
/// first version (rule 3), under a fresh name.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateProviderDefinitionRequest {
    /// The new definition's name; must be non-empty.
    name: String,

    /// The definition's payload; validated once at this boundary.
    #[serde(flatten)]
    payload: DefinitionPayload,
}

/// Strictly parsed `provider-definitions:update` request: the successor version's payload plus
/// the exact current version the caller believes it is updating from. The stored latest version
/// must equal the named base, or the write is rejected as stale.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateProviderDefinitionRequest {
    /// The definition name whose history the successor version extends.
    name: String,

    /// The base version the successor succeeds; must be the stored latest.
    base_version: i64,

    /// The successor version's payload; validated once at this boundary.
    #[serde(flatten)]
    payload: DefinitionPayload,
}

/// One stored definition version in its management wire shape: the `(name, version)` identity,
/// the payload, and the creation timestamp.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct DefinitionResponse {
    /// The definition name; versions accumulate under it.
    name: String,

    /// The definition's positive, monotonically increasing version.
    version: i64,

    /// The definition's payload, flattened into the row.
    #[serde(flatten)]
    payload: DefinitionPayload,

    /// Creation timestamp in UTC microseconds.
    created_at: i64,
}

impl DefinitionResponse {
    /// Renders one stored definition version.
    #[must_use]
    fn from_definition(definition: &ProviderDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            version: definition.version.get(),
            payload: definition.payload.clone(),
            created_at: definition.created_at,
        }
    }
}

/// The definition write routes' success body (rule 7): the stored version's identity is the fact
/// the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DefinitionEnvelope {
    /// The stored definition version.
    provider_definition: DefinitionResponse,
}

/// Handles `POST /provider-definitions:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(_request), _): WithRejection<Json<ListProviderDefinitionsRequest>, Error>,
) -> Result<Json<ListProviderDefinitionsResponse>, Error> {
    let definitions = storage::provider_definitions::list_definitions(&context.database).await?;

    Ok(Json(ListProviderDefinitionsResponse {
        provider_definitions: definitions
            .iter()
            .map(DefinitionResponse::from_definition)
            .collect(),
    }))
}

/// Handles `POST /provider-definitions:create`.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateProviderDefinitionRequest>, Error>,
) -> Result<Json<DefinitionEnvelope>, Error> {
    super::validate_identifier("name", &request.name)?;

    request
        .payload
        .validate()
        .map_err(Error::InvalidDefinition)?;

    let stored = storage::provider_definitions::insert_definition(
        &context.database,
        storage::provider_definitions::NewDefinition {
            name: &request.name,
            base_version: None,
            payload: &request.payload,
            created_at: unix_time::unix_now_micros(),
        },
    )
    .await
    .map_err(|error| match error {
        storage::Error::Constraint { .. } => Error::DefinitionNameTaken {
            name: request.name.clone(),
        },
        other => other.into(),
    })?;

    info!("created provider definition `{}` version 1", request.name);

    Ok(Json(DefinitionEnvelope {
        provider_definition: DefinitionResponse::from_definition(&stored),
    }))
}

/// Handles `POST /provider-definitions:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateProviderDefinitionRequest>, Error>,
) -> Result<Json<DefinitionEnvelope>, Error> {
    let stored = handle_update(&context.database, request).await?;

    Ok(Json(DefinitionEnvelope {
        provider_definition: DefinitionResponse::from_definition(&stored),
    }))
}

/// Extends one definition name's history with its validated successor version.
///
/// # Errors
///
/// Returns [`Error`] for an empty name, a non-positive base version, an invalid payload, a
/// stale base version against an existing name, an unknown definition name, and storage
/// failures.
async fn handle_update(
    database: &crate::database::Database,
    request: UpdateProviderDefinitionRequest,
) -> Result<crate::provider::ProviderDefinition, Error> {
    super::validate_identifier("name", &request.name)?;

    let base_version = super::definition_version(request.base_version, "base_version")?;

    request
        .payload
        .validate()
        .map_err(Error::InvalidDefinition)?;

    let stored = storage::provider_definitions::insert_definition(
        database,
        storage::provider_definitions::NewDefinition {
            name: &request.name,
            base_version: Some(base_version),
            payload: &request.payload,
            created_at: unix_time::unix_now_micros(),
        },
    )
    .await
    .map_err(|error| definition_insert_error(error, request.name.clone(), base_version))?;

    info!(
        "created provider definition `{}` version {}",
        request.name,
        stored.version.get()
    );

    Ok(stored)
}

/// Classifies the insert of a successor version: a stored latest version behind the request's
/// base is the stale-version conflict, an unknown definition name is the version's
/// unknown-reference 404 carrying the exact referenced pair — a missing name collides with
/// nothing — and every other storage failure keeps its own classification.
fn definition_insert_error(error: storage::Error, name: String, base_version: NonZeroI64) -> Error {
    match error {
        storage::Error::Conflict { message } => Error::StaleDefinitionVersion { message },

        storage::Error::NotFound { .. } => Error::DefinitionNotFound {
            name,
            version: base_version.get(),
        },

        other => other.into(),
    }
}
