//! Provider-definition persistence: immutable `(name, version)` rows, appended by the management
//! surface and read by every engine (README §3, §6).
//!
//! Definitions are never updated or deleted here: an edit inserts the next version under the
//! same name, and `ON DELETE RESTRICT` protects referenced versions at the database boundary.
//! The insert of a successor version is conditional on the caller naming the current latest
//! version, so a stale base version can never fork the history.

use super::{Error, definition_from_row_offset};
use crate::{
    database::{Database, Transaction, params},
    provider::definition::{DefinitionPayload, ProviderDefinition},
};
use std::num::NonZeroI64;

/// Column order of definition selects, consumed positionally by
/// [`definition_from_row_offset`].
pub(crate) const DEFINITION_COLUMNS: &str =
    "name, version, format, inference, credential, provisioning, discovery, created_at";

/// A new definition version about to be inserted.
pub(crate) struct NewDefinition<'a> {
    /// The definition name; version 1 when created, `base_version + 1` when updated.
    pub(crate) name: &'a str,

    /// The version the caller believes to be current; creation names no base and inserts
    /// version 1, an update must name the latest version or the write is rejected.
    pub(crate) base_version: Option<NonZeroI64>,

    /// The validated payload.
    pub(crate) payload: &'a DefinitionPayload,

    /// Creation timestamp in UTC microseconds.
    pub(crate) created_at: i64,
}

/// Renders the JSON storage form of a descriptor column: the exact text the validated payload
/// serializes into.
///
/// # Panics
///
/// Panics if serialization fails, which cannot occur: every descriptor payload is a serde
/// structure without ad-hoc maps.
fn payload_json<T>(payload: &T) -> String
where
    T: serde::Serialize,
{
    #[expect(
        clippy::expect_used,
        reason = "a descriptor payload is a serde structure, which serde_json always renders"
    )]
    serde_json::to_string(payload).expect("definition payload serializes")
}

/// Inserts one definition version, returning the stored row.
///
/// # Panics
///
/// Panics if a descriptor payload fails to serialize, which cannot occur: every payload is a
/// serde structure.
///
/// Creation (no base version) inserts version 1 under a fresh name and fails on a taken name
/// with the constraint violation the caller renders as a conflict. An update (a base version)
/// runs inside a transaction whose insert is conditional on the stored latest version being
/// exactly the caller's base, so a stale base version is rejected as a conflict and nothing is
/// written.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails or a stale base version is named.
pub(crate) async fn insert_definition(
    database: &Database,
    definition: NewDefinition<'_>,
) -> Result<ProviderDefinition, Error> {
    match definition.base_version {
        None => insert_first(database, definition).await,

        Some(base_version) => {
            let mut transaction = database.transaction().await?;

            let (name, payload, created_at) =
                (definition.name, definition.payload, definition.created_at);

            insert_next_in_transaction(
                &mut transaction,
                NewDefinition {
                    name,
                    base_version: None,
                    payload,
                    created_at,
                },
                base_version,
            )
            .await?;

            transaction.commit().await?;

            #[expect(
                clippy::unwrap_used,
                reason = "NonZeroI64 addition by one of a positive version cannot be zero"
            )]
            let next = NonZeroI64::new(base_version.get() + 1).unwrap();

            find_definition(database, definition.name, next)
                .await?
                .ok_or_else(|| Error::Drift {
                    message: format!("definition `{}` vanished after insert", definition.name),
                })
        }
    }
}

/// Inserts version 1 of a fresh definition name.
///
/// # Errors
///
/// Returns [`Error`] when the name is taken or the statement fails.
async fn insert_first(
    database: &Database,
    definition: NewDefinition<'_>,
) -> Result<ProviderDefinition, Error> {
    database
        .query(
            "INSERT INTO provider_definitions (name, version, format, inference, credential, \
             provisioning, discovery, created_at) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7) \
             RETURNING name, version, format, inference, credential, provisioning, discovery, \
             created_at",
            params![
                definition.name,
                definition.payload.format.as_str(),
                payload_json(&definition.payload.inference),
                payload_json(&definition.payload.credential),
                definition.payload.provisioning.as_ref().map(payload_json),
                definition.payload.discovery.as_ref().map(payload_json),
                definition.created_at,
            ],
        )
        .await?
        .first()
        .map(|row| definition_from_row_offset(row, 0))
        .unwrap_or_else(|| {
            Err(Error::Drift {
                message: "definition insert returned no row".to_string(),
            })
        })
}

/// Inserts the successor of `base_version` inside the caller's transaction, rejecting a stale
/// base version with a conflict before anything is written.
///
/// # Errors
///
/// Returns [`Error::Conflict`] when the stored latest version is not `base_version`, and
/// [`Error`] when any statement fails.
pub(crate) async fn insert_next_in_transaction(
    transaction: &mut Transaction,
    definition: NewDefinition<'_>,
    base_version: NonZeroI64,
) -> Result<(), Error> {
    let latest = latest_version_in_transaction(transaction, definition.name)
        .await?
        .ok_or_else(|| Error::NotFound {
            message: format!("Provider definition `{}` does not exist", definition.name),
        })?;

    if latest != base_version.get() {
        return Err(Error::Conflict {
            message: format!(
                "Provider definition `{}` is at version {latest}; version {} cannot succeed it",
                definition.name,
                base_version.get()
            ),
        });
    }

    transaction
        .execute(
            "INSERT INTO provider_definitions (name, version, format, inference, credential, \
             provisioning, discovery, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                definition.name,
                base_version.get() + 1,
                definition.payload.format.as_str(),
                payload_json(&definition.payload.inference),
                payload_json(&definition.payload.credential),
                definition.payload.provisioning.as_ref().map(payload_json),
                definition.payload.discovery.as_ref().map(payload_json),
                definition.created_at,
            ],
        )
        .await?;

    Ok(())
}

/// Fetches one definition version.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_definition(
    database: &Database,
    name: &str,
    version: NonZeroI64,
) -> Result<Option<ProviderDefinition>, Error> {
    let sql = format!(
        "SELECT {DEFINITION_COLUMNS} FROM provider_definitions WHERE name = ?1 AND version = ?2"
    );

    database
        .query(sql, params![name, version.get()])
        .await?
        .first()
        .map(|row| definition_from_row_offset(row, 0))
        .transpose()
}

/// Fetches one definition version inside a transaction, mirroring [`find_definition`] against the
/// transactional executor.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_definition_in_transaction(
    transaction: &mut Transaction,
    name: &str,
    version: NonZeroI64,
) -> Result<Option<ProviderDefinition>, Error> {
    let sql = format!(
        "SELECT {DEFINITION_COLUMNS} FROM provider_definitions WHERE name = ?1 AND version = ?2"
    );

    transaction
        .query(sql, params![name, version.get()])
        .await?
        .first()
        .map(|row| definition_from_row_offset(row, 0))
        .transpose()
}

/// Lists every definition version, ordered by name then version.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_definitions(
    database: &Database,
) -> Result<Vec<ProviderDefinition>, Error> {
    let sql =
        format!("SELECT {DEFINITION_COLUMNS} FROM provider_definitions ORDER BY name, version");

    database
        .query(sql, ())
        .await?
        .iter()
        .map(|row| definition_from_row_offset(row, 0))
        .collect()
}

/// The stored latest version of one definition name inside a transaction, or `None` when the
/// name is unknown.
///
/// # Errors
///
/// Returns [`Error`] when the query fails.
pub(crate) async fn latest_version_in_transaction(
    transaction: &mut Transaction,
    name: &str,
) -> Result<Option<i64>, Error> {
    let rows = transaction
        .query(
            "SELECT MAX(version) FROM provider_definitions WHERE name = ?1",
            params![name],
        )
        .await?;

    Ok(rows.first().map(|row| row.get(0)).transpose()?)
}
