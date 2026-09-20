//! Provider, model, and OAuth state persistence, split by entity: [`providers`], [`models`], and
//! [`oauth_states`] own their tables' statements, while this module owns the shared error domain,
//! the Turso value conversions of the provider domain types, the row codecs, and the public model
//! ID collision checks.
//!
//! The entity split interlocks deliberately, mirroring the README §7.1 transaction contracts:
//! `providers:update` recomputes its models' public IDs from the fresh prefix, so [`providers`]
//! reads model rows through [`models::find_models_in_transaction`], while `models:create`,
//! `models:update`, and model discovery re-read the provider row inside their transactions through
//! [`providers::find_provider_by_id_in_transaction`]..

use super::{Credentials, Model, OAuthProviderType, Provider, ProviderType};
use crate::database::{Error as DatabaseError, Row, Transaction, Value, params::IntoValue};
use std::collections::HashMap;
use turso::Error as TursoError;

mod models;
mod oauth_states;
mod providers;

pub(crate) use self::{
    models::{
        ModelEntry, ModelOverrideUpdate, find_routing, insert_model_checked,
        list_enabled_models_with_owner, list_model_entries, replace_discovered_models,
        update_model_overrides_checked,
    },
    oauth_states::{
        consume_oauth_state, delete_expired_oauth_states, find_oauth_state_type, insert_oauth_state,
    },
    providers::{
        ProviderUpdate, clear_refresh_lease, delete_provider, find_provider, find_provider_by_id,
        find_provider_by_id_in_transaction, insert_provider, list_providers, reload_provider,
        try_acquire_refresh_lease, update_credentials, update_credentials_for_lease,
        update_provider,
    },
};

/// Column order of provider selects.
const PROVIDER_COLUMNS: &str = "id, name, model_prefix, type, base_url, credentials, \
     expires_at, is_enabled";

/// Column order of model selects.
const MODEL_COLUMNS: &str = "provider_id, upstream_id, id, id_override, is_enabled, \
     thinking_levels_override";

/// The combined column order of provider-model joins.
const PROVIDER_MODEL_COLUMNS: &str = "p.id, p.name, p.model_prefix, p.type, \
     p.base_url, p.credentials, p.expires_at, p.is_enabled, \
     m.upstream_id, m.id, m.id_override, m.is_enabled, m.thinking_levels_override";

/// The combined column order of the model-entry join, consumed positionally by
/// [`model_entry_from_row`]: the provider id, name, and enabled flag, then the model columns.
const MODEL_ENTRY_COLUMNS: &str = "p.id, p.name, p.is_enabled, \
     m.provider_id, m.upstream_id, m.id, m.id_override, m.is_enabled, \
     m.thinking_levels_override";

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
/// The join projects the provider columns first and the model columns second, without
/// `models.provider_id`, which is taken from the provider row.
fn provider_model_from_row(row: &Row) -> Result<ProviderModelJoin, Error> {
    let provider = provider_from_row(row)?;

    let model = Model {
        provider_id: provider.id,
        upstream_id: row.get(8)?,
        id: row.get(9)?,
        id_override: row.get(10)?,
        is_enabled: row.get::<i64>(11)? != 0,
        thinking_levels_override: row.get(12)?,
    };

    Ok(ProviderModelJoin { provider, model })
}

/// Parses a model row starting at the given column offset.
fn model_from_row_offset(row: &Row, offset: usize) -> Result<Model, Error> {
    Ok(Model {
        provider_id: row.get(offset)?,
        upstream_id: row.get(offset + 1)?,
        id: row.get(offset + 2)?,
        id_override: row.get(offset + 3)?,
        is_enabled: row.get::<i64>(offset + 4)? != 0,
        thinking_levels_override: row.get(offset + 5)?,
    })
}

/// Parses a standalone model row in canonical column order.
fn model_from_row(row: &Row) -> Result<Model, Error> {
    model_from_row_offset(row, 0)
}

/// Parses a model entry row in [`MODEL_ENTRY_COLUMNS`] order: the provider id, name, and enabled
/// flag, then the model columns at offset 3.
fn model_entry_from_row(row: &Row) -> Result<ModelEntry, Error> {
    Ok(ModelEntry {
        provider_id: row.get(0)?,
        provider_name: row.get(1)?,
        is_provider_enabled: row.get::<i64>(2)? != 0,
        model: model_from_row_offset(row, 3)?,
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

/// Rejects freshly composed public IDs that are already routed by another model row — of any
/// provider, including another model of the same provider. An ID held by the very row being written
/// is not a collision.
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
    let routed_rows = transaction
        .query(
            "SELECT m.id, m.provider_id, m.upstream_id, p.name \
             FROM models m JOIN providers p ON p.id = m.provider_id",
            (),
        )
        .await?;

    let mut routed: HashMap<String, (i64, String, String)> =
        HashMap::with_capacity(routed_rows.len());

    for row in &routed_rows {
        routed.insert(row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?));
    }

    for (new_id, upstream_id) in composed {
        if let Some((holder_provider_id, holder_upstream_id, holder_provider_name)) =
            routed.get(*new_id)
            && (*holder_provider_id != provider_id || holder_upstream_id != upstream_id)
        {
            return Err(Error::Conflict {
                message: format!(
                    "Public model ID `{new_id}` is already routed by provider \
                     `{holder_provider_name}`"
                ),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Verifies the full model ID lifecycle: prefix composition on discovery, direct replacement
    /// when overridden, invariant preservation on provider prefix update, clearing overrides, and
    /// routing conflict rejection.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn test_model_id_override_and_prefix_behavior() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let credentials = Credentials::ApiKey("test-secret".to_string());

        insert_provider(
            &database,
            &NewProvider {
                name: "provider-1",
                model_prefix: "p1/",
                provider_type: ProviderType::OpenAiChatCompletion,
                base_url: Some("http://localhost:8080"),
                credentials: &credentials,
                expires_at: None,
                is_enabled: true,
            },
        )
        .await?;

        let providers = list_providers(&database).await?;
        #[expect(clippy::indexing_slicing, reason = "one provider was inserted")]
        let provider_id = providers[0].id;

        // 1. Discover models: "gpt-4" and "claude"
        let discovered = replace_discovered_models(
            &database,
            provider_id,
            &["gpt-4".to_string(), "claude".to_string()],
        )
        .await?;

        assert_eq!(discovered.len(), 2);

        let entries = list_model_entries(&database, None).await?;

        assert_eq!(entries.len(), 2);
        #[expect(clippy::unwrap_used, reason = "gpt-4 is in discovered entries")]
        let gpt4 = entries
            .iter()
            .find(|e| e.model.upstream_id == "gpt-4")
            .unwrap();
        assert_eq!(gpt4.model.id, "p1/gpt-4");
        assert_eq!(gpt4.model.id_override, None);
        assert!(gpt4.model.is_enabled);
        assert!(gpt4.model.is_effectively_enabled(gpt4.is_provider_enabled));

        // 2. Set id_override on "gpt-4" to "top-gpt" (bypasses "p1/" prefix); the rewrite restates
        //    the row's full override state
        let updated = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "gpt-4",
                id_override: Some("top-gpt"),
                is_enabled: true,
                thinking_levels_override: None,
            },
        )
        .await?;

        assert_eq!(updated.model.id, "top-gpt");
        assert_eq!(updated.model.id_override.as_deref(), Some("top-gpt"));
        assert!(updated.model.is_enabled);

        // Routing lookup resolves "top-gpt" to this model
        let routing = find_routing(&database, "top-gpt").await?;

        assert!(routing.is_some());
        #[expect(clippy::unwrap_used, reason = "routing row is present")]
        let routing = routing.unwrap();
        assert_eq!(routing.model.upstream_id, "gpt-4");
        assert!(routing.is_effectively_enabled);

        // Old prefixed ID "p1/gpt-4" no longer routes
        let old_routing = find_routing(&database, "p1/gpt-4").await?;

        assert!(old_routing.is_none());

        // 3. Update provider's model_prefix to "new-p1/"; the rewrite restates the compatible
        //    provider's full state
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1/",
                base_url: Some("http://localhost:8080"),
                is_enabled: true,
                credentials: None,
            },
        )
        .await?;

        // Check: "gpt-4" retains "top-gpt", while "claude" is recomputed as "new-p1/claude"
        let entries_after_prefix = list_model_entries(&database, None).await?;
        #[expect(clippy::unwrap_used, reason = "gpt-4 is in entries")]
        let gpt4_after = entries_after_prefix
            .iter()
            .find(|e| e.model.upstream_id == "gpt-4")
            .unwrap();
        assert_eq!(gpt4_after.model.id, "top-gpt");
        #[expect(clippy::unwrap_used, reason = "claude is in entries")]
        let claude_after = entries_after_prefix
            .iter()
            .find(|e| e.model.upstream_id == "claude")
            .unwrap();
        assert_eq!(claude_after.model.id, "new-p1/claude");

        // 4. Clear id_override on "gpt-4": reverts to "new-p1/gpt-4"
        let cleared = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "gpt-4",
                id_override: None,
                is_enabled: true,
                thinking_levels_override: None,
            },
        )
        .await?;

        assert_eq!(cleared.model.id, "new-p1/gpt-4");
        assert_eq!(cleared.model.id_override, None);

        // 5. Collision check: try to set claude's id_override to "new-p1/gpt-4"
        let collision_result = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "claude",
                id_override: Some("new-p1/gpt-4"),
                is_enabled: true,
                thinking_levels_override: None,
            },
        )
        .await;
        assert!(collision_result.is_err());

        // 6. Disable claude individually
        let disabled_claude = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "claude",
                id_override: None,
                is_enabled: false,
                thinking_levels_override: None,
            },
        )
        .await?;

        assert!(!disabled_claude.model.is_enabled);
        assert!(
            !disabled_claude
                .model
                .is_effectively_enabled(disabled_claude.is_provider_enabled)
        );

        // Enabled models list includes gpt-4 but excludes claude
        let enabled_models = list_enabled_models_with_owner(&database).await?;

        assert_eq!(enabled_models.len(), 1);
        #[expect(clippy::indexing_slicing, reason = "length 1 asserted above")]
        let first_enabled = &enabled_models[0];
        assert_eq!(first_enabled.0, "new-p1/gpt-4");

        // 7. Master switch test: disable provider -> ALL models effectively disabled
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1/",
                base_url: Some("http://localhost:8080"),
                is_enabled: false,
                credentials: None,
            },
        )
        .await?;

        // Routing lookup for gpt-4 reports the effective status as disabled
        let gpt4_routing = find_routing(&database, "new-p1/gpt-4").await?;

        assert!(gpt4_routing.is_some());
        #[expect(clippy::unwrap_used, reason = "routing is some")]
        let gpt4_routing = gpt4_routing.unwrap();
        assert!(!gpt4_routing.is_effectively_enabled);

        // Enabled models list is now empty because provider is disabled
        let enabled_models_after_provider_disable =
            list_enabled_models_with_owner(&database).await?;
        assert!(enabled_models_after_provider_disable.is_empty());

        Ok(())
    }

    /// Verifies the manual model lifecycle: insertion composes the public ID from the provider's
    /// current prefix, a repeated `upstream_id` and a routed public ID each conflict without
    /// writing, and a later discovery carries the manual row's overrides over by `upstream_id`
    /// — dropping the row only when the identifier no longer appears.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn test_manual_model_insert_and_discovery_carry_over() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let credentials = Credentials::ApiKey("test-secret".to_string());

        insert_provider(
            &database,
            &NewProvider {
                name: "provider-1",
                model_prefix: "p1/",
                provider_type: ProviderType::OpenAiChatCompletion,
                base_url: Some("http://localhost:8080"),
                credentials: &credentials,
                expires_at: None,
                is_enabled: true,
            },
        )
        .await?;

        let providers = list_providers(&database).await?;
        #[expect(clippy::indexing_slicing, reason = "one provider was inserted")]
        let provider_id = providers[0].id;

        // 1. Insert a manual row: the public ID composes from the current prefix.
        let created = insert_model_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "manual-1",
                id_override: None,
                is_enabled: false,
                thinking_levels_override: Some(r#"["low"]"#),
            },
        )
        .await?;

        assert_eq!(created.model.id, "p1/manual-1");
        assert!(!created.model.is_enabled);
        assert_eq!(
            created.model.thinking_levels_override.as_deref(),
            Some(r#"["low"]"#)
        );

        // 2. A repeated `upstream_id` on the same provider conflicts and writes nothing.
        let duplicate = insert_model_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "manual-1",
                id_override: Some("other-id"),
                is_enabled: true,
                thinking_levels_override: None,
            },
        )
        .await;
        assert!(matches!(duplicate, Err(Error::Conflict { .. })));

        // The failed insert left the stored row untouched.
        let entries = list_model_entries(&database, Some(provider_id)).await?;
        assert_eq!(entries.len(), 1);

        // 3. A public ID already routed by another provider's row conflicts and writes nothing.
        insert_provider(
            &database,
            &NewProvider {
                name: "provider-2",
                model_prefix: "p2/",
                provider_type: ProviderType::OpenAiChatCompletion,
                base_url: Some("http://localhost:8080"),
                credentials: &credentials,
                expires_at: None,
                is_enabled: true,
            },
        )
        .await?;

        let providers = list_providers(&database).await?;
        #[expect(clippy::unwrap_used, reason = "provider-2 was inserted")]
        let other_provider_id = providers
            .iter()
            .find(|provider| provider.name == "provider-2")
            .unwrap()
            .id;

        let colliding = insert_model_checked(
            &database,
            ModelOverrideUpdate {
                provider_id: other_provider_id,
                upstream_id: "manual-1",
                id_override: Some("p1/manual-1"),
                is_enabled: true,
                thinking_levels_override: None,
            },
        )
        .await;
        assert!(matches!(colliding, Err(Error::Conflict { .. })));

        let other_entries = list_model_entries(&database, Some(other_provider_id)).await?;
        assert!(other_entries.is_empty());

        // 4. An unknown provider yields not-found.
        let missing = insert_model_checked(
            &database,
            ModelOverrideUpdate {
                provider_id: other_provider_id + 1000,
                upstream_id: "ghost",
                id_override: None,
                is_enabled: true,
                thinking_levels_override: None,
            },
        )
        .await;
        assert!(matches!(missing, Err(Error::NotFound { .. })));

        // 5. A later discovery carrying the manual identifier keeps its overrides; a fresh
        //    identifier defaults to enabled with no overrides.
        let rediscovered = replace_discovered_models(
            &database,
            provider_id,
            &["manual-1".to_string(), "fresh-1".to_string()],
        )
        .await?;

        assert_eq!(rediscovered.len(), 2);
        #[expect(clippy::unwrap_used, reason = "manual-1 is re-discovered")]
        let carried = rediscovered
            .iter()
            .find(|entry| entry.model.upstream_id == "manual-1")
            .unwrap();
        assert_eq!(carried.model.id, "p1/manual-1");
        assert!(!carried.model.is_enabled);
        assert_eq!(
            carried.model.thinking_levels_override.as_deref(),
            Some(r#"["low"]"#)
        );

        // 6. A discovery no longer listing the manual identifier drops the row.
        let pruned =
            replace_discovered_models(&database, provider_id, &["fresh-1".to_string()]).await?;

        assert_eq!(pruned.len(), 1);
        #[expect(clippy::indexing_slicing, reason = "length 1 asserted above")]
        let survivor = &pruned[0];
        assert_eq!(survivor.model.upstream_id, "fresh-1");

        Ok(())
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
                model_prefix: "openai/",
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
                 VALUES ('drifted', 'd/', 'openai_compatible', 'https://x.example', '{}')",
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
