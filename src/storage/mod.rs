//! The storage tier: the single home of every Turso statement and of the shared storage error
//! domain.
//!
//! Every database statement outside the frozen schema lives here, one module per table:
//! [`provider_definitions`], [`providers`], [`models`], [`oauth_flows`], [`api_keys`], and
//! [`api_logs`]. This module owns the shared error domain ([`Error`]) every surface maps onto its
//! own taxonomy, the Turso value conversions of the provider domain types, and the provider and
//! model row codecs. The api-side code (outward authentication, the gateway pipeline, the
//! management surfaces) depends on this tier one-way — the error taxonomy in particular is
//! provider-agnostic, so it lives here rather than under `provider`.
//!
//! The provider domain interlocks with this tier in both directions, acknowledged rather than
//! hidden: storage owns every statement and projects its rows onto the domain types it imports
//! from [`crate::provider`] ([`Provider`], [`Model`], [`ProviderDefinition`], [`CredentialKind`],
//! [`DiscoveryState`]), while the OAuth and discovery engines — the domain's executors — call
//! storage to read and write those same rows.
//!
//! The entity split interlocks deliberately, mirroring the management API's transaction
//! contracts: `providers:update` recomputes its models' public IDs from the fresh prefix, so
//! [`providers`] reads model rows through [`models::find_models_in_transaction`], while model
//! writes and the OAuth flows' atomic commits re-read the provider row inside their transactions
//! through [`providers::find_provider_by_id_in_transaction`].

use self::models::ModelEntry;
use crate::{
    database::{
        Database, Error as DatabaseError, IntoParams, Row, Transaction, TursoError, Value, params,
        params::IntoValue,
    },
    provider::{CredentialKind, DiscoveryState, Model, Provider, definition::ProviderDefinition},
};
use std::{collections::HashMap, num::NonZeroI64};

pub(crate) mod api_keys;
pub(crate) mod api_logs;
pub(crate) mod models;
pub(crate) mod oauth_flows;
pub(crate) mod provider_definitions;
pub(crate) mod providers;
#[cfg(test)]
mod test_support;

/// Column order of provider selects: the row identity, the routing columns, the credential
/// state, and then the definition columns the row references, consumed positionally by
/// [`provider_from_row`].
const PROVIDER_COLUMNS: &str = "p.id, p.name, p.model_prefix, p.base_url, p.credentials, \
     p.is_enabled, d.name, d.version, d.format, d.inference, d.credential, d.provisioning, \
     d.discovery, d.created_at";

/// The join behind every provider read: the row and the exact definition version it references.
pub(crate) const PROVIDER_JOINED_FROM: &str = "FROM providers p JOIN provider_definitions d \
     ON d.name = p.provider_definition_name AND d.version = p.provider_definition_version";

/// The provider-model join: models first, then the provider and its definition, filtered by the
/// caller's model predicate.
pub(crate) const PROVIDER_JOINED_FROM_MODELS: &str = "FROM models m \
     JOIN providers p ON p.id = m.provider_id JOIN provider_definitions d \
     ON d.name = p.provider_definition_name AND d.version = p.provider_definition_version";

/// Column order of model selects: the row identity first, then the upstream identifier, the
/// routing columns, and the discovery state. The owning provider is never carried in the row
/// itself — every read knows it through the query's filter or its join.
const MODEL_COLUMNS: &str = "id, upstream_id, public_id, id_override, is_enabled, discovery_state";

/// The model columns as a provider-model join projects them, appended after
/// [`PROVIDER_COLUMNS`].
const MODEL_COLUMNS_JOINED: &str =
    "m.id, m.upstream_id, m.public_id, m.id_override, m.is_enabled, m.discovery_state";

/// The combined column order of the model-entry join, consumed positionally by
/// [`model_entry_from_row`]: the provider id and name, then the model columns.
const MODEL_ENTRY_COLUMNS: &str = "p.id, p.name, \
     m.id, m.upstream_id, m.public_id, m.id_override, m.is_enabled, m.discovery_state";

/// Provider, model, and OAuth state persistence failures.
///
/// Semantic outcomes the checked operations promise — a missing row, a conflicting write — are
/// their own variants, while every engine failure is classified once here so the HTTP surfaces
/// never inspect the database error domain themselves.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// A checked lookup or precondition failed: the addressed row does not exist.
    #[error("{message}")]
    NotFound {
        /// Human-readable failure message.
        message: String,
    },

    /// The write collides with existing state.
    #[error("{message}")]
    Conflict {
        /// Human-readable failure message.
        message: String,
    },

    /// The engine rejected a write with a constraint violation — the concurrent backstop behind a
    /// checked conflict.
    #[error("database constraint violated: {detail}")]
    Constraint {
        /// Engine-provided violation detail.
        detail: String,
    },

    /// Database contention: the file lock was busy or a read-then-write upgrade was refused with a
    /// stale snapshot.
    ///
    /// The lease caller that must tell the stale-snapshot refusal apart from plain contention
    /// classifies the raw [`DatabaseError`] through [`DatabaseError::is_stale_snapshot`] before
    /// this conversion flattens the distinction away; every rendered surface treats `Busy` as an
    /// internal failure.
    #[error("database is busy: {detail}")]
    Busy {
        /// Engine-provided contention detail.
        detail: String,
    },

    /// Stored data violated an invariant written by the validated management path — database drift.
    #[error("{message}")]
    Drift {
        /// Human-readable failure message.
        message: String,
    },

    /// The database engine failed.
    #[error("database error: {0}")]
    Database(DatabaseError),
}

/// A provider row joined with a model row.
struct ProviderModelJoin {
    /// Owning provider.
    provider: Provider,

    /// Model row.
    model: Model,
}

impl From<TursoError> for Error {
    fn from(error: TursoError) -> Self {
        Self::Database(DatabaseError::Turso(error))
    }
}

impl From<DatabaseError> for Error {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::Turso(TursoError::Constraint(detail)) => Self::Constraint { detail },

            DatabaseError::Turso(TursoError::Busy(detail) | TursoError::BusySnapshot(detail)) => {
                Self::Busy { detail }
            }

            error => Self::Database(error),
        }
    }
}

/// The pooled or transactional SQL executor behind the storage tier's shared statements: a
/// borrowed [`Database`] runs a statement against a pooled connection, an exclusively borrowed
/// [`Transaction`] against its checked-out connection. Statements used by both executors are
/// written once as functions generic over this abstraction instead of being hand-mirrored with
/// duplicated SQL and parameter lists, matching the database module's own delegation of both
/// surfaces onto shared free functions.
trait SqlExecutor {
    /// Executes one row-returning statement, returning all rows eagerly materialized.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if preparing or executing the statement fails.
    async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, DatabaseError>;
}

impl SqlExecutor for &Database {
    async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, DatabaseError> {
        (**self).query(sql, params).await
    }
}

impl SqlExecutor for &mut Transaction {
    async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, DatabaseError> {
        (**self).query(sql, params).await
    }
}

impl IntoValue for DiscoveryState {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl IntoValue for CredentialKind {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

/// The documented wire wording for a rejected provider name (README §7.1), owned once here so the
/// checked rename and both management name-taken pre-checks render a taken name identically.
pub(crate) fn provider_name_taken_message(name: &str) -> String {
    format!("Provider name `{name}` is already taken")
}

/// The wire wording for an unknown provider id (README §7 404 `not_found`), owned once here so the
/// transactional writes that re-read the row and the management lookups that pre-check it render a
/// missing provider identically.
pub(crate) fn provider_not_found_message(id: i64) -> String {
    format!("Provider {id} does not exist")
}

/// The wire wording for a referenced provider-definition version that does not exist, owned once
/// here so the storage rebind path and the management error variant render a missing definition
/// identically — the same single-homing its sibling wordings carry.
pub(crate) fn provider_definition_not_found_message(name: &str, version: i64) -> String {
    format!("Provider definition `{name}` version {version} does not exist")
}

/// The wire wording for an unknown model row id (README §7 404 `not_found`), owned once here so the
/// row-addressed writes that re-read the row render a missing model identically.
pub(crate) fn model_not_found_message(id: i64) -> String {
    format!("Model {id} does not exist")
}

/// Reads a discovery-state column, failing with an internal drift error on an unknown or non-text
/// value: the column is written only through [`DiscoveryState::as_str`] and its schema `CHECK`
/// pins the closed set, so anything else is storage drift.
fn discovery_state_from_value(value: Value) -> Result<DiscoveryState, Error> {
    match value {
        Value::Text(text) => DiscoveryState::parse(&text).ok_or_else(|| Error::Drift {
            message: format!("unknown discovery state `{text}`"),
        }),

        _ => Err(Error::Drift {
            message: "discovery state column must be a text value".to_string(),
        }),
    }
}

/// The column count of [`PROVIDER_COLUMNS`]: a provider-model join appends the model columns
/// after it.
pub(crate) const PROVIDER_COLUMN_COUNT: usize = 14;

/// Parses a definition payload JSON column, wrapped exactly once as internal storage drift: the
/// column was written only through the validated management path, so a payload failing here is
/// never the requesting client's fault.
fn payload_from_json<T>(json: Option<String>, what: &str) -> Result<T, Error>
where
    T: serde::de::DeserializeOwned,
{
    let json = json.ok_or_else(|| Error::Drift {
        message: format!("{what} column must not be null"),
    })?;

    serde_json::from_str(&json).map_err(|error| Error::Drift {
        message: format!("{what} column carries a malformed payload: {error}"),
    })
}

/// Parses a provider definition row in the definition column order of
/// [`provider_definitions::DEFINITION_COLUMNS`] at the given column offset.
pub(crate) fn definition_from_row_offset(
    row: &Row,
    offset: usize,
) -> Result<ProviderDefinition, Error> {
    let name: String = row.get(offset)?;
    let version: i64 = row.get(offset + 1)?;

    let version = NonZeroI64::new(version).ok_or_else(|| Error::Drift {
        message: format!("definition `{name}` carries a zero version"),
    })?;

    let format: String = row.get(offset + 2)?;

    let format = crate::format::UpstreamFormat::parse(&format).ok_or_else(|| Error::Drift {
        message: format!("definition `{name}` carries an unknown format `{format}`"),
    })?;

    Ok(ProviderDefinition {
        name,
        version,
        payload: crate::provider::DefinitionPayload {
            format,
            inference: payload_from_json(row.get(offset + 3)?, "inference")?,
            credential: payload_from_json(row.get(offset + 4)?, "credential")?,
            provisioning: row
                .get::<Option<String>>(offset + 5)?
                .map(|json| payload_from_json(Some(json), "provisioning"))
                .transpose()?,
            discovery: row
                .get::<Option<String>>(offset + 6)?
                .map(|json| payload_from_json(Some(json), "discovery"))
                .transpose()?,
        },
        created_at: row.get(offset + 7)?,
    })
}

/// Parses a provider row in [`PROVIDER_COLUMNS`] order: the provider columns, then the exact
/// definition version the row references.
pub(crate) fn provider_from_row(row: &Row) -> Result<Provider, Error> {
    let name: String = row.get(1)?;
    let credentials_json: String = row.get(4)?;

    // The typed parse failure is wrapped exactly once as internal storage drift: the row was
    // written only through the validated management path, so a payload failing here is never the
    // requesting client's fault.
    let credentials = serde_json::from_str::<crate::provider::StoredCredentials>(&credentials_json)
        .map_err(|error| Error::Drift {
            message: format!("provider `{name}` has malformed credentials: {error}"),
        })?;

    Ok(Provider {
        id: row.get(0)?,
        name,
        model_prefix: row.get(2)?,
        base_url: row.get(3)?,
        credentials,
        is_enabled: row.get::<i64>(5)? != 0,
        definition: definition_from_row_offset(row, 6)?,
    })
}

/// Parses a provider-model joined row: the provider columns first, the model columns second.
fn provider_model_from_row(row: &Row) -> Result<ProviderModelJoin, Error> {
    let provider = provider_from_row(row)?;

    let model = model_from_row_offset(row, PROVIDER_COLUMN_COUNT)?;

    Ok(ProviderModelJoin { provider, model })
}

/// Parses a model row starting at the given column offset.
fn model_from_row_offset(row: &Row, offset: usize) -> Result<Model, Error> {
    Ok(Model {
        id: row.get(offset)?,
        upstream_id: row.get(offset + 1)?,
        public_id: row.get(offset + 2)?,
        id_override: row.get(offset + 3)?,
        is_enabled: row.get::<i64>(offset + 4)? != 0,
        discovery_state: discovery_state_from_value(row.get_value(offset + 5)?)?,
    })
}

/// Parses a standalone model row in canonical column order.
fn model_from_row(row: &Row) -> Result<Model, Error> {
    model_from_row_offset(row, 0)
}

/// Parses a model entry row in [`MODEL_ENTRY_COLUMNS`] order: the provider id and name, then the
/// model columns at offset 2.
fn model_entry_from_row(row: &Row) -> Result<ModelEntry, Error> {
    Ok(ModelEntry {
        provider_id: row.get(0)?,
        provider_name: row.get(1)?,
        model: model_from_row_offset(row, 2)?,
    })
}

/// Rejects one provider's freshly composed public IDs when two of its rows would converge on one
/// ID.
///
/// `composed` carries `(public_id, upstream_id)` pairs.
fn ensure_ids_pairwise_distinct(
    provider_name: &str,
    composed: &[(&str, &str)],
) -> Result<(), Error> {
    let mut seen: HashMap<&str, &str> = HashMap::with_capacity(composed.len());

    for (new_id, upstream_id) in composed {
        if let Some(first_upstream_id) = seen.insert(new_id, upstream_id) {
            return Err(Error::Conflict {
                message: format!(
                    "Public model ID `{new_id}` collides between upstream models \
                     `{first_upstream_id}` and `{upstream_id}` of provider `{provider_name}`"
                ),
            });
        }
    }

    Ok(())
}

/// The holder facts of one routed model row that the public-ID collision checks consult.
struct RoutedHolder {
    /// Owning provider's row identifier.
    provider_id: i64,

    /// The holder's upstream identifier — half of the natural key that exempts a row from
    /// colliding with itself.
    upstream_id: String,

    /// Owning provider's name, rendered by the conflict message.
    provider_name: String,
}

/// Parses a routed-holder row in `(provider_id, upstream_id, provider_name)` column order at the
/// given column offset.
fn routed_holder_from_row_offset(row: &Row, offset: usize) -> Result<RoutedHolder, Error> {
    Ok(RoutedHolder {
        provider_id: row.get(offset)?,
        upstream_id: row.get(offset + 1)?,
        provider_name: row.get(offset + 2)?,
    })
}

/// Whether a routed holder genuinely collides with a write: every holder collides except the very
/// row being written, identified by its natural key `(provider_id, upstream_id)`.
fn is_id_collision(holder: &RoutedHolder, provider_id: i64, upstream_id: &str) -> bool {
    holder.provider_id != provider_id || holder.upstream_id != upstream_id
}

/// The conflict for a composed public ID already routed by another model row.
fn id_collision_error(new_id: &str, holder: &RoutedHolder) -> Error {
    Error::Conflict {
        message: format!(
            "Public model ID `{new_id}` is already routed by provider `{}`",
            holder.provider_name
        ),
    }
}

/// Reads one routed-map snapshot inside a transaction: every model row's public ID mapped to its
/// [`RoutedHolder`] — a `vanished` or `manual` row and an `id_override` holder all appear, since
/// every holder reserves its public ID regardless of its discovery state.
///
/// Runs inside the caller's transaction, so the routed rows it observes cannot interleave with the
/// caller's writes.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn read_routed_holders_in_transaction(
    transaction: &mut Transaction,
) -> Result<HashMap<String, RoutedHolder>, Error> {
    let routed_rows = transaction
        .query(
            "SELECT m.public_id, m.provider_id, m.upstream_id, p.name \
             FROM models m JOIN providers p ON p.id = m.provider_id",
            (),
        )
        .await?;

    let mut routed: HashMap<String, RoutedHolder> = HashMap::with_capacity(routed_rows.len());

    for row in &routed_rows {
        routed.insert(row.get(0)?, routed_holder_from_row_offset(row, 1)?);
    }

    Ok(routed)
}

/// Rejects a freshly composed public ID that a [`read_routed_holders_in_transaction`] snapshot
/// shows to be routed by another model row. The caller owns the snapshot's freshness: the check is
/// exact only while no write since the snapshot can have introduced a holder of `new_id`.
///
/// # Errors
///
/// Returns [`Error`] when the ID collides.
fn ensure_id_routable_against(
    routed: &HashMap<String, RoutedHolder>,
    provider_id: i64,
    new_id: &str,
    upstream_id: &str,
) -> Result<(), Error> {
    if let Some(holder) = routed.get(new_id)
        && is_id_collision(holder, provider_id, upstream_id)
    {
        return Err(id_collision_error(new_id, holder));
    }

    Ok(())
}

/// Rejects freshly composed public IDs that are already routed by another model row — of any
/// provider, including another model of the same provider, and regardless of the holder's
/// discovery state: a `vanished` or `manual` row and an `id_override` holder all keep their public
/// ID reserved. An ID held by the very row being written is not a collision — the holder is
/// identified by its natural key `(provider_id, upstream_id)`.
///
/// `composed` carries `(public_id, upstream_id)` pairs of the provider identified by `provider_id`.
/// Runs inside the caller's transaction, so the routed rows it observes cannot interleave with the
/// caller's writes.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or an ID collides.
async fn ensure_ids_routable(
    transaction: &mut Transaction,
    provider_id: i64,
    composed: &[(&str, &str)],
) -> Result<(), Error> {
    let routed = read_routed_holders_in_transaction(transaction).await?;

    for (new_id, upstream_id) in composed {
        ensure_id_routable_against(&routed, provider_id, new_id, upstream_id)?;
    }

    Ok(())
}

/// Rejects a freshly composed public ID that is already routed by another model row — the
/// single-pair form of [`ensure_ids_routable`] with identical collision semantics: of any
/// provider, regardless of the holder's discovery state, exempting the row being written by its
/// natural key `(provider_id, upstream_id)`.
///
/// The probe reads only the rows matching the candidate — `models.public_id` is unique, so this is
/// an indexed single-row read rather than the batch form's full-table scan — and a holder of a
/// different public ID is never consulted, exactly as the batch form's map lookup never consulted
/// one. Runs inside the caller's transaction, so the routed rows it observes cannot interleave
/// with the caller's writes.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the ID collides.
async fn ensure_id_routable_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    new_id: &str,
    upstream_id: &str,
) -> Result<(), Error> {
    let rows = transaction
        .query(
            "SELECT m.provider_id, m.upstream_id, p.name \
             FROM models m JOIN providers p ON p.id = m.provider_id WHERE m.public_id = ?1",
            params![new_id],
        )
        .await?;

    // `models.public_id` is unique: at most one holder row matches the probe.
    if let Some(row) = rows.first() {
        let holder = routed_holder_from_row_offset(row, 0)?;

        if is_id_collision(&holder, provider_id, upstream_id) {
            return Err(id_collision_error(new_id, &holder));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic,
        reason = "a fixture violating its own invariant fails the test loudly"
    )]

    use super::{
        models::{
            ModelUpdate, apply_discovered_models, find_routing, list_enabled_models_with_owner,
            list_model_entries, update_model_overrides_checked,
        },
        providers::{ProviderUpdate, list_providers, update_provider},
        test_support::{entry, insert_test_provider, oauth_definition},
        *,
    };
    use crate::{
        provider::{
            CredentialKind,
            definition::credential::{CredentialAcquisition, TokenRefresh},
        },
        test_support,
    };
    use std::error::Error as StdError;

    /// Verifies the public-ID lifecycle end to end: discovery composes IDs from the provider's
    /// prefix, `models:update` (addressed by row id) applies an override, routing follows the
    /// composed ID, a `providers:update` prefix change recomputes every unoverridden ID while an
    /// overridden one stays, clearing the override recomposes, a colliding override is rejected
    /// without writing, and the two enabled switches gate the enabled-model listing and routing's
    /// effective status.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn overrides_ids_and_recomputes_them_on_prefix_update() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        let discovered = apply_discovered_models(
            &database,
            provider_id,
            &["gpt-4".to_string(), "claude".to_string()],
        )
        .await?;

        assert_eq!(discovered.len(), 2);

        assert_eq!(
            discovered
                .first()
                .map(|entry| entry.model.public_id.as_str()),
            Some("p1-claude")
        );

        assert_eq!(
            discovered
                .get(1)
                .map(|entry| entry.model.public_id.as_str()),
            Some("p1-gpt-4")
        );

        let entries = list_model_entries(&database, None).await?;
        let gpt4 = entry(&entries, "gpt-4");

        assert_eq!(gpt4.model.public_id, "p1-gpt-4");
        assert_eq!(gpt4.model.id_override, None);
        assert!(gpt4.model.is_enabled);
        assert_eq!(gpt4.model.discovery_state, DiscoveryState::Discovered);
        assert!(gpt4.model.is_effectively_enabled(true));

        let gpt4_id = gpt4.model.id;

        let updated = update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: gpt4_id,
                id_override: Some("top-gpt"),
                is_enabled: true,
            },
        )
        .await?;

        assert_eq!(updated.model.id, gpt4_id);
        assert_eq!(updated.model.public_id, "top-gpt");
        assert_eq!(updated.model.id_override.as_deref(), Some("top-gpt"));

        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "top-gpt")
            .await?
            .expect("the overridden ID routes");

        assert_eq!(routing.model.upstream_id, "gpt-4");
        assert!(routing.is_effectively_enabled);

        assert!(find_routing(&database, "p1-gpt-4").await?.is_none());

        // 3. A prefix change recomputes unoverridden IDs; the overridden row keeps its override.
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1-",
                base_url: Some("http://localhost:8080"),
                is_enabled: true,
                definition: None,
                credentials: None,
            },
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;

        assert_eq!(entry(&entries, "gpt-4").model.public_id, "top-gpt");
        assert_eq!(entry(&entries, "claude").model.public_id, "new-p1-claude");

        // 4. Clearing the override recomposes from the current prefix.
        let cleared = update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: gpt4_id,
                id_override: None,
                is_enabled: true,
            },
        )
        .await?;

        assert_eq!(cleared.model.public_id, "new-p1-gpt-4");
        assert_eq!(cleared.model.id_override, None);

        let claude_id = entry(&entries, "claude").model.id;

        let collision = update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: claude_id,
                id_override: Some("new-p1-gpt-4"),
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(collision, Err(Error::Conflict { .. })));

        let entries = list_model_entries(&database, None).await?;

        assert_eq!(entry(&entries, "claude").model.public_id, "new-p1-claude");

        update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: claude_id,
                id_override: None,
                is_enabled: false,
            },
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let claude = entry(&entries, "claude");

        assert!(!claude.model.is_enabled);
        assert!(!claude.model.is_effectively_enabled(true));

        let enabled_models = list_enabled_models_with_owner(&database).await?;

        assert_eq!(enabled_models.len(), 1);

        assert_eq!(
            enabled_models
                .first()
                .map(|(public_id, _)| public_id.as_str()),
            Some("new-p1-gpt-4")
        );

        // The provider is the master switch: disabling it disables every model effectively.
        let provider = list_providers(&database).await?;

        assert_eq!(provider.len(), 1);

        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1-",
                base_url: Some("http://localhost:8080"),
                is_enabled: false,
                definition: None,
                credentials: None,
            },
        )
        .await?;

        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "new-p1-gpt-4")
            .await?
            .expect("the row still resolves");

        assert!(!routing.is_effectively_enabled);
        assert!(list_enabled_models_with_owner(&database).await?.is_empty());

        Ok(())
    }

    /// Verifies the credential kind conversions: the storage form and the wire form are one
    /// vocabulary.
    ///
    /// # Panics
    ///
    /// Panics if value serialization fails.
    #[test]
    fn converts_credential_kind_database_values() {
        for kind in [CredentialKind::ApiKey, CredentialKind::OAuth] {
            #[expect(clippy::unwrap_used, reason = "into_value returns Ok for the kind")]
            let value = kind.into_value().unwrap();
            assert_eq!(value, Value::Text(kind.as_str().to_string()));
        }
    }

    /// Verifies that the schema itself enforces the `provider_definitions.format` closed domain:
    /// a drifted value is rejected at write time and classifies as a storage constraint error.
    ///
    /// # Panics
    ///
    /// Panics if the drifted insert unexpectedly succeeds.
    #[tokio::test]
    async fn rejects_unknown_definition_format_at_write_time() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let outcome = database
            .execute(
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('drifted', 1, 'bogus_format', '{}', '{}', 1)",
                (),
            )
            .await;

        #[expect(clippy::expect_used, reason = "the schema CHECK rejects the drift")]
        let error = outcome.expect_err("drifted format must violate the schema CHECK");

        assert!(
            matches!(Error::from(error), Error::Constraint { .. }),
            "a schema CHECK violation must classify as a storage constraint error"
        );

        Ok(())
    }

    /// The refresh-mode of the fixture OAuth definition is `none`: the engine never enters the
    /// lease for it, which the renewable predicate pins.
    ///
    /// # Panics
    ///
    /// Panics if the fixture disagrees with the predicate.
    #[test]
    fn the_none_refresh_mode_never_renews() {
        let definition = oauth_definition();

        let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
            panic!("the fixture carries OAuth credentials");
        };

        assert!(matches!(recipe.refresh, TokenRefresh::None));
    }
}
