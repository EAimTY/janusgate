//! The storage tier: the single home of every Turso statement and of the shared storage error
//! domain.
//!
//! Every database statement outside the frozen schema lives here, one module per table:
//! [`providers`], [`models`], [`oauth_states`], [`api_keys`], and [`api_logs`]. This module owns
//! the shared error domain ([`Error`]) every surface maps onto its own taxonomy, the Turso value
//! conversions of the provider domain types, the provider and model row codecs, and the public
//! model ID collision checks. Both the provider domain (OAuth refresh, model discovery) and the
//! api-side code (outward authentication, the gateway pipeline, the management surfaces) depend
//! on this tier — the error taxonomy in particular is provider-agnostic, so it lives here rather
//! than under `provider`.
//!
//! The entity split interlocks deliberately, mirroring the README §7.1 transaction contracts:
//! `providers:update` recomputes its models' public IDs from the fresh prefix, so [`providers`]
//! reads model rows through [`models::find_models_in_transaction`], while `models:create`,
//! `models:update`, and model discovery re-read the provider row inside their transactions through
//! [`providers::find_provider_by_id_in_transaction`].

use self::models::ModelEntry;
use crate::{
    database::{Error as DatabaseError, Row, Transaction, Value, params, params::IntoValue},
    provider::{Credentials, DiscoveryState, Model, OAuthProviderType, Provider, ProviderType},
};
use std::collections::HashMap;
use turso::Error as TursoError;

pub(crate) mod api_keys;
pub(crate) mod api_logs;
pub(crate) mod models;
pub(crate) mod oauth_states;
pub(crate) mod providers;

/// Column order of provider selects.
const PROVIDER_COLUMNS: &str = "id, name, model_prefix, type, base_url, credentials, \
     expires_at, is_enabled";

/// Column order of model selects: the row identity first, then the upstream identifier, the
/// routing columns, and the discovery state. The owning provider is never carried in the row
/// itself — every read knows it through the query's filter or its join.
const MODEL_COLUMNS: &str = "id, upstream_id, public_id, id_override, is_enabled, discovery_state";

/// The combined column order of provider-model joins.
const PROVIDER_MODEL_COLUMNS: &str = "p.id, p.name, p.model_prefix, p.type, \
     p.base_url, p.credentials, p.expires_at, p.is_enabled, \
     m.id, m.upstream_id, m.public_id, m.id_override, m.is_enabled, m.discovery_state";

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

impl IntoValue for ProviderType {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl IntoValue for OAuthProviderType {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl IntoValue for DiscoveryState {
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

/// The wire wording for an unknown model row id (README §7 404 `not_found`), owned once here so the
/// row-addressed writes that re-read the row render a missing model identically.
pub(crate) fn model_not_found_message(id: i64) -> String {
    format!("Model {id} does not exist")
}

/// Reads a provider type column, failing with an internal drift error on an unknown or non-text
/// value: the column was written through [`ProviderType::as_str`], so anything else is storage
/// drift.
fn provider_type_from_value(value: Value) -> Result<ProviderType, Error> {
    match value {
        Value::Text(text) => ProviderType::parse(&text).ok_or_else(|| Error::Drift {
            message: format!("unknown provider type `{text}`"),
        }),

        _ => Err(Error::Drift {
            message: "provider type column must be a text value".to_string(),
        }),
    }
}

/// Reads an OAuth state type column; see [`provider_type_from_value`] for the drift classification.
fn oauth_provider_type_from_value(value: Value) -> Result<OAuthProviderType, Error> {
    match value {
        Value::Text(text) => OAuthProviderType::parse(&text).ok_or_else(|| Error::Drift {
            message: format!("unknown OAuth provider type `{text}`"),
        }),

        _ => Err(Error::Drift {
            message: "OAuth state type column must be a text value".to_string(),
        }),
    }
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

/// Parses a provider row in [`PROVIDER_COLUMNS`] order.
fn provider_from_row(row: &Row) -> Result<Provider, Error> {
    let provider_type = provider_type_from_value(row.get_value(3)?)?;

    let name: String = row.get(1)?;
    let credentials_json: String = row.get(5)?;

    // The typed parse failure is wrapped exactly once as internal storage drift: the row was
    // written only through the validated management path, so a payload failing here is never the
    // requesting client's fault.
    let credentials =
        Credentials::parse(provider_type, &credentials_json).map_err(|error| Error::Drift {
            message: format!("provider `{name}` has malformed credentials: {error}"),
        })?;

    Ok(Provider {
        id: row.get(0)?,
        name,
        model_prefix: row.get(2)?,
        provider_type,
        base_url: row.get(4)?,
        credentials,
        expires_at: row.get(6)?,
        is_enabled: row.get::<i64>(7)? != 0,
    })
}

/// Parses a provider-model joined row in [`PROVIDER_MODEL_COLUMNS`] order.
///
/// The join projects the provider columns first and the model columns second; the model's
/// `provider_id` is taken from the provider row.
fn provider_model_from_row(row: &Row) -> Result<ProviderModelJoin, Error> {
    let provider = provider_from_row(row)?;

    let model = Model {
        id: row.get(8)?,
        upstream_id: row.get(9)?,
        public_id: row.get(10)?,
        id_override: row.get(11)?,
        is_enabled: row.get::<i64>(12)? != 0,
        discovery_state: discovery_state_from_value(row.get_value(13)?)?,
    };

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
    use super::{
        models::{
            ModelCreate, ModelUpdate, apply_discovered_models, delete_model_checked, find_routing,
            insert_model_checked, list_enabled_models_with_owner, list_model_entries,
            update_model_overrides_checked,
        },
        oauth_states::{
            consume_oauth_state, delete_expired_oauth_states, find_oauth_state_type,
            insert_oauth_state,
        },
        providers::{ProviderUpdate, insert_provider, list_providers, update_provider},
        *,
    };
    use crate::{
        database::Database,
        provider::{Credentials, NewProvider, OAuthProviderType, ProviderType},
    };
    use std::{error::Error as StdError, num::NonZero, path::PathBuf, time::Duration};

    /// File guard cleaning up the temporary SQLite test database on drop.
    struct TempDbGuard(PathBuf);

    impl Drop for TempDbGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Creates an initialized temporary SQLite test database.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database fails.
    async fn create_test_database() -> (Database, TempDbGuard) {
        let db_path =
            std::env::temp_dir().join(format!("janusgate-test-{}.db", rand::random::<u64>()));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let max_connections = NonZero::new(2).unwrap();
        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, max_connections, Duration::from_secs(5))
            .await
            .unwrap();

        (database, TempDbGuard(db_path))
    }

    /// Verifies the public-ID lifecycle end to end: discovery composes IDs from the provider's
    /// prefix, `models:update` (addressed by row id) applies an override, routing follows the
    /// composed ID, a `providers:update` prefix change recomputes every unoverridden ID while an
    /// overridden one stays, clearing the override recomposes, a colliding override is rejected
    /// without writing, and the two enabled switches — model and provider master switch — gate the
    /// enabled-model listing and routing's effective status.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn overrides_ids_and_recomputes_them_on_prefix_update() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        // 1. Discovery composes public IDs from the prefix; the response carries every fresh row,
        //    ordered by public ID.
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

        // 2. An override applied by row id replaces the composed ID; routing resolves it.
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

        // The old composed ID no longer routes.
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

        // 5. An ID already routed by another row collides and writes nothing.
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

        // 6. Disabling one model takes it out of the enabled listing and its effective status.
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

        // The provider master switch is still on here; the model's own switch is what fails the
        // three-input status.
        assert!(!claude.model.is_effectively_enabled(true));

        let enabled_models = list_enabled_models_with_owner(&database).await?;

        assert_eq!(enabled_models.len(), 1);

        assert_eq!(
            enabled_models
                .first()
                .map(|(public_id, _)| public_id.as_str()),
            Some("new-p1-gpt-4")
        );

        // 7. The provider is the master switch: disabling it disables every model effectively while
        //    the enabled listing empties.
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1-",
                base_url: Some("http://localhost:8080"),
                is_enabled: false,
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

    /// Verifies the manual-row creation rules: an absent identifier becomes a `manual` row with a
    /// prefix-composed ID, a natural-key duplicate and a routed-ID collision each conflict without
    /// writing, an unknown provider is not found — and discovery never touches an unlisted manual
    /// row while listing it flips the row to `discovered` with its curation and identity intact.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn creates_manual_rows_discovery_never_prunes() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        // 1. An absent identifier becomes a manual row with the requested state.
        let created = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "manual-1",
                id_override: None,
                is_enabled: false,
            },
        )
        .await?;

        assert_eq!(created.model.public_id, "p1-manual-1");
        assert_eq!(created.model.discovery_state, DiscoveryState::Manual);
        assert!(!created.model.is_enabled);

        // 2. A natural-key duplicate conflicts and writes nothing.
        let duplicate = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "manual-1",
                id_override: Some("other-id"),
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(duplicate, Err(Error::Conflict { .. })));

        assert_eq!(
            list_model_entries(&database, Some(provider_id))
                .await?
                .len(),
            1
        );

        // 3. A public ID already routed by another provider's row conflicts and writes nothing.
        let other_provider_id = insert_test_provider(&database, "provider-2", "p2-").await?;

        let colliding = insert_model_checked(
            &database,
            ModelCreate {
                provider_id: other_provider_id,
                upstream_id: "manual-1",
                id_override: Some("p1-manual-1"),
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(colliding, Err(Error::Conflict { .. })));
        assert!(
            list_model_entries(&database, Some(other_provider_id))
                .await?
                .is_empty()
        );

        // 4. An unknown provider yields not-found.
        let missing = insert_model_checked(
            &database,
            ModelCreate {
                provider_id: other_provider_id + 1000,
                upstream_id: "ghost",
                id_override: None,
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(missing, Err(Error::NotFound { .. })));

        // 5. A discovery not listing the manual identifier leaves the row alone — never pruned,
        //    never marked — and the response carries every row of the provider.
        let rows =
            apply_discovered_models(&database, provider_id, &["fresh-1".to_string()]).await?;

        assert_eq!(rows.len(), 2);

        let entries = list_model_entries(&database, Some(provider_id)).await?;
        let manual = entry(&entries, "manual-1");

        assert_eq!(manual.model.discovery_state, DiscoveryState::Manual);
        assert!(!manual.model.is_enabled);
        assert_eq!(manual.model.public_id, "p1-manual-1");

        let fresh = entry(&entries, "fresh-1");

        assert_eq!(fresh.model.discovery_state, DiscoveryState::Discovered);
        assert!(fresh.model.is_enabled);

        // 6. A discovery listing the manual identifier flips it to `discovered`, keeping its row
        //    identity and its curation.
        let manual_id = manual.model.id;

        apply_discovered_models(
            &database,
            provider_id,
            &["fresh-1".to_string(), "manual-1".to_string()],
        )
        .await?;

        let entries = list_model_entries(&database, Some(provider_id)).await?;
        let manual = entry(&entries, "manual-1");

        assert_eq!(manual.model.id, manual_id);
        assert_eq!(manual.model.discovery_state, DiscoveryState::Discovered);
        assert!(!manual.model.is_enabled);
        assert_eq!(manual.model.public_id, "p1-manual-1");

        Ok(())
    }

    /// Verifies the vanished semantics and the write-permission invariant: a discovery marks only
    /// `discovered` rows `vanished`, a vanished row keeps routing resolution intact while its
    /// three-input effective status turns false — excluding it from the enabled listing and the
    /// outward `GET /models` source while the management listing still shows it — and a reappearing
    /// row is restored by flipping only its state column, leaving curated columns and its identity
    /// untouched.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn marks_vanished_rows_and_restores_them_by_state_flip_only()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        apply_discovered_models(
            &database,
            provider_id,
            &["a".to_string(), "b".to_string(), "c".to_string()],
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let (a_id, b_id, c_id) = (
            entry(&entries, "a").model.id,
            entry(&entries, "b").model.id,
            entry(&entries, "c").model.id,
        );

        // Curate: `a` gets an override, `b` is switched off, `c` stays at its defaults.
        update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: a_id,
                id_override: Some("custom-a"),
                is_enabled: true,
            },
        )
        .await?;

        update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: b_id,
                id_override: None,
                is_enabled: false,
            },
        )
        .await?;

        // A discovery listing only `a` marks `b` and `c` vanished and answers with every row.
        let rows = apply_discovered_models(&database, provider_id, &["a".to_string()]).await?;

        assert_eq!(rows.len(), 3);

        let entries = list_model_entries(&database, None).await?;
        let a = entry(&entries, "a");
        let b = entry(&entries, "b");
        let c = entry(&entries, "c");

        // The returning row keeps identity and every curated column.
        assert_eq!(a.model.id, a_id);
        assert_eq!(a.model.discovery_state, DiscoveryState::Discovered);
        assert_eq!(a.model.public_id, "custom-a");
        assert!(a.model.is_enabled);

        // Vanish touches only the state column: `b` stays curated off, `c` stays on.
        assert_eq!(b.model.discovery_state, DiscoveryState::Vanished);
        assert!(!b.model.is_enabled);
        assert_eq!(b.model.public_id, "p1-b");

        assert_eq!(c.model.discovery_state, DiscoveryState::Vanished);
        assert!(c.model.is_enabled);
        assert_eq!(c.model.public_id, "p1-c");

        // A vanished row with `is_enabled = 1` still resolves in routing — zero special-casing —
        // but its three-input effective status is false, which is what turns into the outward 404.
        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "p1-c")
            .await?
            .expect("a vanished row is still resolved by its public ID");

        assert!(!routing.is_effectively_enabled);

        // The enabled listing — the source of `GET /models` — excludes it; the management listing
        // shows it with a false effective status.
        let enabled_models = list_enabled_models_with_owner(&database).await?;

        assert_eq!(enabled_models.len(), 1);

        assert_eq!(
            enabled_models
                .first()
                .map(|(public_id, _)| public_id.as_str()),
            Some("custom-a")
        );

        let c = entry(&entries, "c");

        assert!(c.model.is_enabled);

        // Vanish suspends service even with the provider and the row's own switch both on.
        assert!(!c.model.is_effectively_enabled(true));

        // Reappearance flips only the state column back: identity and curation survive, the
        // enabled switch is exactly what the administrator left behind.
        apply_discovered_models(
            &database,
            provider_id,
            &["a".to_string(), "b".to_string(), "c".to_string()],
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let a = entry(&entries, "a");
        let b = entry(&entries, "b");
        let c = entry(&entries, "c");

        assert_eq!(a.model.id, a_id);
        assert_eq!(a.model.public_id, "custom-a");
        assert!(a.model.is_enabled);

        assert_eq!(b.model.id, b_id);
        assert_eq!(b.model.discovery_state, DiscoveryState::Discovered);
        assert!(!b.model.is_enabled);

        assert_eq!(c.model.id, c_id);
        assert_eq!(c.model.discovery_state, DiscoveryState::Discovered);
        assert!(c.model.is_enabled);

        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "p1-c")
            .await?
            .expect("the restored row routes");

        assert!(routing.is_effectively_enabled);

        let enabled_models = list_enabled_models_with_owner(&database).await?;

        assert_eq!(enabled_models.len(), 2);

        Ok(())
    }

    /// Verifies that a newly inserted discovered row's public ID collides against every row of the
    /// table — a vanished holder and an `id_override` holder of other providers included — with the
    /// whole refresh rejected as a conflict that writes nothing.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn rejects_new_public_ids_held_by_any_row() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        // Provider A discovers `gpt-4`, then drops it: the vanished row keeps `p1/gpt-4` reserved.
        let provider_a = insert_test_provider(&database, "provider-a", "p1-").await?;

        apply_discovered_models(&database, provider_a, &["gpt-4".to_string()]).await?;
        apply_discovered_models(&database, provider_a, &["other".to_string()]).await?;

        // Provider B shares the prefix and re-discovers the identifier: its brand-new row would
        // take the vanished holder's ID, so the refresh is rejected and B stays empty.
        let provider_b = insert_test_provider(&database, "provider-b", "p1-").await?;

        let collision =
            apply_discovered_models(&database, provider_b, &["gpt-4".to_string()]).await;

        assert!(matches!(collision, Err(Error::Conflict { .. })));
        assert!(
            list_model_entries(&database, Some(provider_b))
                .await?
                .is_empty()
        );

        // An `id_override` holder reserves its ID the same way: a manual row of provider C holds
        // `d/m`, so provider D's discovery composing `d/m` is rejected.
        let provider_c = insert_test_provider(&database, "provider-c", "c-").await?;

        insert_model_checked(
            &database,
            ModelCreate {
                provider_id: provider_c,
                upstream_id: "m",
                id_override: Some("d-m"),
                is_enabled: true,
            },
        )
        .await?;

        let provider_d = insert_test_provider(&database, "provider-d", "d-").await?;

        let collision = apply_discovered_models(&database, provider_d, &["m".to_string()]).await;

        assert!(matches!(collision, Err(Error::Conflict { .. })));
        assert!(
            list_model_entries(&database, Some(provider_d))
                .await?
                .is_empty()
        );

        Ok(())
    }

    /// Verifies the claim path of `models:create`: a vanished row is claimed into a `manual` row
    /// keeping its row identity while the requested state — override, switch, recomposed public ID
    /// — applies, the claim resumes service per the requested `is_enabled`, and a row the upstream
    /// currently lists is never claimable.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn claims_a_vanished_row_keeping_its_identity() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        apply_discovered_models(
            &database,
            provider_id,
            &["gpt-4".to_string(), "keep".to_string()],
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let gpt4_id = entry(&entries, "gpt-4").model.id;

        // The administrator disables the row before it vanishes; curation must survive the claim.
        update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: gpt4_id,
                id_override: None,
                is_enabled: false,
            },
        )
        .await?;

        apply_discovered_models(&database, provider_id, &["keep".to_string()]).await?;

        // The claim keeps the row id, applies the requested state, recomposes the public ID, and
        // lands as `manual`.
        let claimed = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "gpt-4",
                id_override: Some("claimed-gpt"),
                is_enabled: true,
            },
        )
        .await?;

        assert_eq!(claimed.model.id, gpt4_id);
        assert_eq!(claimed.model.discovery_state, DiscoveryState::Manual);
        assert_eq!(claimed.model.public_id, "claimed-gpt");
        assert_eq!(claimed.model.id_override.as_deref(), Some("claimed-gpt"));
        assert!(claimed.model.is_enabled);

        // The claim is the manual recovery path: the row serves per its requested switch.
        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "claimed-gpt")
            .await?
            .expect("the claimed row routes");

        assert!(routing.is_effectively_enabled);
        assert!(find_routing(&database, "p1-gpt-4").await?.is_none());

        // A row the upstream currently lists is `discovered` and can never be claimed.
        let conflict = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "keep",
                id_override: None,
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(conflict, Err(Error::Conflict { .. })));

        Ok(())
    }

    /// Verifies the row-addressed update and delete rules: unknown row ids are not found, an
    /// update writes only the override columns — never `discovery_state`, so a vanished row stays
    /// unservable after an update — a recomposed-ID collision is rejected without writing, a
    /// deleted row disappears, and a later discovery rebuilds a deleted `discovered` row under a
    /// fresh, never-recycled identity.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn updates_and_deletes_models_by_row_id() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        apply_discovered_models(&database, provider_id, &["a".to_string(), "b".to_string()])
            .await?;

        let entries = list_model_entries(&database, None).await?;
        let a_id = entry(&entries, "a").model.id;
        let b_id = entry(&entries, "b").model.id;

        // Unknown row ids are not found, on both routes.
        let missing_update = update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: a_id + b_id + 1000,
                id_override: None,
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(missing_update, Err(Error::NotFound { .. })));

        let missing_delete = delete_model_checked(&database, a_id + b_id + 1000).await;

        assert!(matches!(missing_delete, Err(Error::NotFound { .. })));

        // Vanish `a`, then update it: the update writes the override columns but never
        // `discovery_state`, so the row stays unservable.
        apply_discovered_models(&database, provider_id, &["b".to_string()]).await?;

        let updated = update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: a_id,
                id_override: Some("va"),
                is_enabled: true,
            },
        )
        .await?;

        assert_eq!(updated.model.discovery_state, DiscoveryState::Vanished);
        assert_eq!(updated.model.public_id, "va");

        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "va")
            .await?
            .expect("the updated row still resolves");

        assert!(!routing.is_effectively_enabled);

        // A colliding recomposed ID is rejected and writes nothing.
        let collision = update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: b_id,
                id_override: Some("va"),
                is_enabled: false,
            },
        )
        .await;

        assert!(matches!(collision, Err(Error::Conflict { .. })));

        let entries = list_model_entries(&database, None).await?;
        let b = entry(&entries, "b");

        assert_eq!(b.model.public_id, "p1-b");
        assert!(b.model.is_enabled);

        // Deleting by row id removes the row; the same id is then unknown.
        delete_model_checked(&database, a_id).await?;

        let entries = list_model_entries(&database, None).await?;

        assert_eq!(entries.len(), 1);

        assert_eq!(
            entries
                .first()
                .map(|entry| entry.model.upstream_id.as_str()),
            Some("b")
        );

        let missing_delete = delete_model_checked(&database, a_id).await;

        assert!(matches!(missing_delete, Err(Error::NotFound { .. })));

        // A deleted discovered row is rebuilt by the next discovery as a new row: the identity is
        // never recycled.
        let rebuilt =
            apply_discovered_models(&database, provider_id, &["a".to_string(), "b".to_string()])
                .await?;

        #[expect(
            clippy::expect_used,
            reason = "the row was just rebuilt by the refresh above"
        )]
        let fresh_a = rebuilt
            .iter()
            .find(|entry| entry.model.upstream_id == "a")
            .expect("the rebuilt row is listed");

        assert_ne!(fresh_a.model.id, a_id);

        Ok(())
    }

    /// Inserts one enabled static-key test provider, returning its stored row.
    ///
    /// # Panics
    ///
    /// Panics if the insert fails.
    async fn insert_test_provider(
        database: &Database,
        name: &str,
        model_prefix: &str,
    ) -> Result<i64, Box<dyn StdError>> {
        let provider = insert_provider(
            database,
            &NewProvider {
                name,
                model_prefix,
                provider_type: ProviderType::OpenAiChatCompletion,
                base_url: Some("http://localhost:8080"),
                credentials: &Credentials::ApiKey("test-secret".to_string()),
                expires_at: None,
                is_enabled: true,
            },
        )
        .await?;

        Ok(provider.id)
    }

    /// Fetches one listing entry by its upstream identifier, expecting it to be present.
    ///
    /// # Panics
    ///
    /// Panics if the entry is absent; every caller looks up a row the test just verified exists.
    fn entry<'a>(entries: &'a [ModelEntry], upstream_id: &str) -> &'a ModelEntry {
        entries
            .iter()
            .find(|entry| entry.model.upstream_id == upstream_id)
            .unwrap_or_else(|| {
                #[expect(
                    clippy::panic,
                    reason = "every caller looks up a row the test just verified exists"
                )]
                {
                    panic!("model row `{upstream_id}` must be listed")
                }
            })
    }

    /// Verifies the OAuth state lifecycle and typed database roundtrips for `OAuthProviderType` and
    /// `ProviderType`.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn test_oauth_state_and_provider_type_lifecycle() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let now = 1_700_000_000_000_000;
        let expires_at = now + 600_000_000;

        // 1. Insert OAuth states with OAuthProviderType
        insert_oauth_state(
            &database,
            OAuthProviderType::CodexOauth,
            "codex-state-1",
            "codex-verifier-1",
            expires_at,
        )
        .await?;

        insert_oauth_state(
            &database,
            OAuthProviderType::AntigravityOauth,
            "antigravity-state-1",
            "",
            expires_at,
        )
        .await?;

        // 2. Query type without consuming
        let found_type = find_oauth_state_type(&database, "codex-state-1", now).await?;
        assert_eq!(found_type, Some(OAuthProviderType::CodexOauth));

        let found_ag = find_oauth_state_type(&database, "antigravity-state-1", now).await?;
        assert_eq!(found_ag, Some(OAuthProviderType::AntigravityOauth));

        // 3. Atomically consume
        let consumed = consume_oauth_state(&database, "codex-state-1", now).await?;
        #[expect(clippy::unwrap_used, reason = "state is present")]
        let consumed = consumed.unwrap();
        assert_eq!(consumed.state_type, OAuthProviderType::CodexOauth);
        assert_eq!(consumed.verifier, "codex-verifier-1");

        // Second consume returns None
        let second_consume = consume_oauth_state(&database, "codex-state-1", now).await?;
        assert!(second_consume.is_none());

        // 4. Create provider from consumed state and verify typed read back
        let credentials = Credentials::CodexOauth {
            access_token: "test-token".to_string(),
            refresh_token: "test-refresh".to_string(),
            account_id: "test-account".to_string(),
        };
        insert_provider(
            &database,
            &NewProvider {
                name: "oauth-provider",
                model_prefix: "openai-",
                provider_type: consumed.state_type.as_provider_type(),
                base_url: None,
                credentials: &credentials,
                expires_at: Some(expires_at),
                is_enabled: true,
            },
        )
        .await?;

        let providers = list_providers(&database).await?;
        #[expect(clippy::unwrap_used, reason = "provider was inserted")]
        let provider = providers
            .iter()
            .find(|p| p.name == "oauth-provider")
            .unwrap();
        assert_eq!(provider.provider_type, ProviderType::CodexOauth);

        // 5. Sweep expired states
        delete_expired_oauth_states(&database, expires_at + 10_000).await?;

        let swept = find_oauth_state_type(&database, "antigravity-state-1", now).await?;
        assert!(swept.is_none());

        Ok(())
    }

    /// Verifies that the schema itself enforces the `providers.type` closed domain: a drifted
    /// value is rejected at write time and classifies as a storage constraint error, instead of
    /// surviving until the read-side type parse turns every dispatch for the provider into
    /// drift. First-party write paths only ever insert parsed types, so the probe uses a
    /// raw statement.
    ///
    /// # Panics
    ///
    /// Panics if the drifted insert unexpectedly succeeds.
    #[tokio::test]
    async fn rejects_unknown_provider_type_at_write_time() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let outcome = database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, base_url, credentials) \
                 VALUES ('drifted', 'd-', 'openai_compatible', 'https://x.example', '{}')",
                (),
            )
            .await;

        #[expect(clippy::expect_used, reason = "the schema CHECK rejects the drift")]
        let error = outcome.expect_err("drifted type must violate the schema CHECK");

        assert!(
            matches!(Error::from(error), Error::Constraint { .. }),
            "a schema CHECK violation must classify as a storage constraint error"
        );

        Ok(())
    }

    /// Verifies value conversions between database values and provider types.
    ///
    /// # Panics
    ///
    /// Panics if value serialization or deserialization fails.
    #[test]
    fn converts_provider_type_database_values() {
        for provider_type in [
            ProviderType::OpenAiResponse,
            ProviderType::OpenAiChatCompletion,
            ProviderType::GeminiGenerateContent,
            ProviderType::GeminiInteraction,
            ProviderType::AnthropicMessages,
            ProviderType::CodexOauth,
            ProviderType::AntigravityOauth,
        ] {
            #[expect(clippy::unwrap_used, reason = "into_value returns Ok for ProviderType")]
            let value = provider_type.into_value().unwrap();
            assert_eq!(value, Value::Text(provider_type.as_str().to_string()));

            #[expect(clippy::unwrap_used, reason = "the helper returns Ok for valid Text")]
            let parsed = provider_type_from_value(value).unwrap();
            assert_eq!(parsed, provider_type);
        }

        let invalid = Value::Text("unknown_type".to_string());
        assert!(provider_type_from_value(invalid).is_err());

        assert!(provider_type_from_value(Value::Integer(42)).is_err());
    }

    /// Verifies value conversions between database values and OAuth provider types.
    ///
    /// # Panics
    ///
    /// Panics if value serialization or deserialization fails.
    #[test]
    fn converts_oauth_provider_type_database_values() {
        for oauth_type in [
            OAuthProviderType::CodexOauth,
            OAuthProviderType::AntigravityOauth,
        ] {
            #[expect(
                clippy::unwrap_used,
                reason = "into_value returns Ok for OAuthProviderType"
            )]
            let value = oauth_type.into_value().unwrap();
            assert_eq!(value, Value::Text(oauth_type.as_str().to_string()));

            #[expect(clippy::unwrap_used, reason = "the helper returns Ok for valid Text")]
            let parsed = oauth_provider_type_from_value(value).unwrap();
            assert_eq!(parsed, oauth_type);
        }

        let incompatible = Value::Text("openai_compatible".to_string());
        assert!(oauth_provider_type_from_value(incompatible).is_err());

        let invalid = Value::Text("unknown_type".to_string());
        assert!(oauth_provider_type_from_value(invalid).is_err());

        assert!(oauth_provider_type_from_value(Value::Integer(42)).is_err());
    }
}
