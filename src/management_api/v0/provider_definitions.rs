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
    pub provider_definitions: Vec<DefinitionResponse>,
}

/// Strictly parsed `provider-definitions:create` request: the complete state of one definition's
/// first version (rule 3), under a fresh name.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateProviderDefinitionRequest {
    /// The new definition's name; must be non-empty.
    pub name: String,

    /// The definition's payload; validated once at this boundary.
    #[serde(flatten)]
    pub payload: DefinitionPayload,
}

/// Strictly parsed `provider-definitions:update` request: the successor version's payload plus
/// the exact current version the caller believes it is updating from. The stored latest version
/// must equal the named base, or the write is rejected as stale.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateProviderDefinitionRequest {
    /// The definition name whose history the successor version extends.
    pub name: String,

    /// The base version the successor succeeds; must be the stored latest.
    pub base_version: i64,

    /// The successor version's payload; validated once at this boundary.
    #[serde(flatten)]
    pub payload: DefinitionPayload,
}

/// One stored definition version in its management wire shape: the `(name, version)` identity,
/// the payload, and the creation timestamp.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DefinitionResponse {
    /// The definition name; versions accumulate under it.
    pub name: String,

    /// The definition's positive, monotonically increasing version.
    pub version: i64,

    /// The definition's payload, flattened into the row.
    #[serde(flatten)]
    pub payload: DefinitionPayload,

    /// Creation timestamp in UTC microseconds.
    pub created_at: i64,
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
    pub provider_definition: DefinitionResponse,
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

#[cfg(test)]
mod tests {
    use super::{Error, UpdateProviderDefinitionRequest, definition_insert_error, handle_update};
    use crate::{format::UpstreamFormat, storage, test_support};
    use std::num::NonZeroI64;

    /// Builds one `provider-definitions:update` request: the fixture payload under the given
    /// name, succeeding the given base version.
    fn update_request(name: &str, base_version: i64) -> UpdateProviderDefinitionRequest {
        UpdateProviderDefinitionRequest {
            name: name.to_string(),
            base_version,
            payload: crate::test_support::static_key_definition_payload(
                UpstreamFormat::OpenAiChatCompletion,
            ),
        }
    }

    /// The successor-version insert classifies its two reference failures apart: a stored latest
    /// version behind the request's base is the conflict, an unknown definition name is the
    /// version's unknown-reference 404 carrying the exact referenced pair, and every other
    /// storage failure keeps its own classification.
    ///
    /// # Panics
    ///
    /// Panics if a classification diverges.
    #[test]
    fn classifies_the_successor_insert_failures() {
        #[expect(clippy::expect_used, reason = "the fixture base version is positive")]
        let base_version = NonZeroI64::new(1).expect("the fixture base version is positive");

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::StaleDefinitionVersion { message } = definition_insert_error(
            storage::Error::Conflict {
                message: "stale".to_string(),
            },
            "static-def".to_string(),
            base_version,
        ) else {
            panic!("a stale base version classifies as the conflict");
        };

        assert_eq!(message, "stale");

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::DefinitionNotFound { name, version } = definition_insert_error(
            storage::Error::NotFound {
                message: "does not exist".to_string(),
            },
            "absent-def".to_string(),
            base_version,
        ) else {
            panic!("an unknown definition name classifies as the unknown-reference 404");
        };

        assert_eq!(name, "absent-def");
        assert_eq!(version, 1);

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::Storage(storage::Error::Database(_)) = definition_insert_error(
            storage::Error::Database(crate::database::Error::PoolClosed),
            "static-def".to_string(),
            base_version,
        ) else {
            panic!("an unrelated storage failure keeps its own classification");
        };
    }

    /// A non-positive base version renders the shared malformed-parameter rejection — never the
    /// stale-version conflict against an existing name — because the schema's
    /// `CHECK version > 0` makes such a version unrepresentable (README §3, §7.1).
    ///
    /// # Panics
    ///
    /// Panics if the fixture fails to insert or a rejection diverges.
    #[tokio::test]
    async fn a_non_positive_base_version_renders_the_parameter_rejection() {
        let (database, _guard) = test_support::create_test_database().await;

        test_support::insert_static_key_definition(
            &database,
            "static-def",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        for base_version in [0, -3] {
            #[expect(
                clippy::unwrap_used,
                reason = "the rejection is the documented side of this call"
            )]
            let error = handle_update(&database, update_request("static-def", base_version))
                .await
                .unwrap_err();

            assert!(
                matches!(error, Error::InvalidDefinition(_)),
                "base version {base_version} rendered a divergent rejection: {error:?}"
            );
        }
    }

    /// `provider-definitions:update` against an existing name whose stored latest version is
    /// behind the request's base renders the stale-version conflict through the live write path.
    ///
    /// # Panics
    ///
    /// Panics if the fixture fails to insert or the rejection diverges.
    #[tokio::test]
    async fn a_stale_base_against_an_existing_name_renders_the_conflict() {
        let (database, _guard) = test_support::create_test_database().await;

        test_support::insert_static_key_definition(
            &database,
            "static-def",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        #[expect(
            clippy::unwrap_used,
            reason = "the rejection is the documented side of this call"
        )]
        let error = handle_update(&database, update_request("static-def", 2))
            .await
            .unwrap_err();

        #[expect(
            clippy::panic,
            reason = "a divergent rejection variant is the failure report"
        )]
        let Error::StaleDefinitionVersion { message } = error else {
            panic!("a stale base version against an existing name renders the conflict");
        };

        assert!(message.contains("static-def"), "{message}");
    }
}
