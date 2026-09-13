//! Provider, model, and OAuth state persistence, split by entity:
//! [`providers`], [`models`], and [`oauth_states`] own their tables'
//! statements, while this module owns the shared error domain, the Turso
//! value conversions of the provider domain types, the row codecs, and the
//! public model ID collision checks.
//!
//! The entity split interlocks deliberately, mirroring the README §7.1
//! transaction contracts: `save-provider` recomputes its models' public IDs
//! from the fresh prefix, so [`providers`] reads model rows through
//! [`models::find_models_in_transaction`], while `save-model` and model
//! discovery re-read the provider row inside their transactions through
//! [`providers::find_provider_by_id_in_transaction`].

mod models;
mod oauth_states;
mod providers;

pub(crate) use self::{
    models::{
        ModelEntry, ModelOverrideUpdate, find_routing, list_enabled_models_with_owner,
        list_model_entries, replace_discovered_models, update_model_overrides_checked,
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
use super::{Credentials, Model, OAuthProviderType, Provider, ProviderType};
use crate::{
    database::{Error as DatabaseError, Row, Transaction, Value, params::IntoValue},
    format::UpstreamFormat,
};
use std::collections::HashMap;
use turso::Error as TursoError;

/// Provider, model, and OAuth state persistence failures.
///
/// Semantic outcomes the checked operations promise — a missing row, a
/// conflicting write — are their own variants, while every engine failure is
/// classified once here so the HTTP surfaces never inspect the database
/// error domain themselves.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// A checked lookup or precondition failed: the addressed row does not
    /// exist.
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

    /// The engine rejected a write with a constraint violation — the
    /// concurrent backstop behind a checked conflict.
    #[error("database constraint violated: {detail}")]
    Constraint {
        /// Engine-provided violation detail.
        detail: String,
    },

    /// Database contention: the file lock was busy or a read-then-write
    /// upgrade was refused with a stale snapshot.
    ///
    /// The lease caller that must tell the stale-snapshot refusal apart from
    /// plain contention classifies the raw [`DatabaseError`] through
    /// [`DatabaseError::is_stale_snapshot`] before this conversion flattens
    /// the distinction away; every rendered surface treats `Busy` as an
    /// internal failure.
    #[error("database is busy: {detail}")]
    Busy {
        /// Engine-provided contention detail.
        detail: String,
    },

    /// Stored data violated an invariant written by the validated management
    /// path — database drift.
    #[error("{message}")]
    Drift {
        /// Human-readable failure message.
        message: String,
    },

    /// The database engine failed.
    #[error("database error: {0}")]
    Database(DatabaseError),
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

/// The documented wire wording for a rejected provider name (README §7.1),
/// owned once here so the checked rename and both management name-taken
/// pre-checks render a taken name identically.
pub(crate) fn provider_name_taken_message(name: &str) -> String {
    format!("Provider name `{name}` is already taken")
}

/// The wire wording for an unknown provider id (README §7 404 `not_found`),
/// owned once here so the transactional writes that re-read the row and the
/// management lookups that pre-check it render a missing provider
/// identically.
pub(crate) fn provider_not_found_message(id: i64) -> String {
    format!("Provider {id} does not exist")
}

/// Column order of provider selects.
const PROVIDER_COLUMNS: &str = "id, name, model_prefix, type, protocol, base_url, credentials, \
     expires_at, is_enabled";

/// Column order of model selects.
const MODEL_COLUMNS: &str = "provider_id, upstream_id, id, id_override, is_enabled, \
     thinking_levels_override";

/// The combined column order of provider-model joins.
const PROVIDER_MODEL_COLUMNS: &str = concat!(
    "p.id, p.name, p.model_prefix, p.type, p.protocol, p.base_url, p.credentials, ",
    "p.expires_at, p.is_enabled, ",
    "m.upstream_id, m.id, m.id_override, m.is_enabled, m.thinking_levels_override"
);

/// The combined column order of the model-entry join, consumed positionally by
/// [`model_entry_from_row`]: the provider id, name, and enabled flag, then the
/// model columns.
const MODEL_ENTRY_COLUMNS: &str = concat!(
    "p.id, p.name, p.is_enabled, ",
    "m.provider_id, m.upstream_id, m.id, m.id_override, m.is_enabled, \
     m.thinking_levels_override"
);

impl IntoValue for ProviderType {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl TryFrom<&Value> for ProviderType {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Text(text) => Self::parse(text).ok_or_else(|| Error::Drift {
                message: format!("unknown provider type `{text}`"),
            }),
            _ => Err(Error::Drift {
                message: "provider type column must be a text value".to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for ProviderType {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

impl IntoValue for OAuthProviderType {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl TryFrom<&Value> for OAuthProviderType {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Text(text) => Self::parse(text).ok_or_else(|| Error::Drift {
                message: format!("unknown OAuth provider type `{text}`"),
            }),
            _ => Err(Error::Drift {
                message: "OAuth state type column must be a text value".to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for OAuthProviderType {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

/// Parses a provider row in [`PROVIDER_COLUMNS`] order.
fn provider_from_row(row: &Row) -> Result<Provider, Error> {
    let provider_type = ProviderType::try_from(row.get_value(3)?)?;

    let protocol =
        match row.get::<Option<String>>(4)? {
            None => None,
            Some(protocol_name) => Some(UpstreamFormat::parse(&protocol_name).ok_or_else(
                || Error::Drift {
                    message: format!("provider row carries unknown protocol `{protocol_name}`"),
                },
            )?),
        };

    let name: String = row.get(1)?;
    let credentials_json: String = row.get(6)?;

    // The typed parse failure is wrapped exactly once as internal storage
    // drift: the row was written only through the validated management path,
    // so a payload failing here is never the requesting client's fault.
    let credentials =
        Credentials::parse(provider_type, &credentials_json).map_err(|error| Error::Drift {
            message: format!("provider `{name}` has malformed credentials: {error}"),
        })?;

    Ok(Provider {
        id: row.get(0)?,
        name,
        model_prefix: row.get(2)?,
        provider_type,
        protocol,
        base_url: row.get(5)?,
        credentials,
        expires_at: row.get(7)?,
        is_enabled: row.get::<i64>(8)? != 0,
    })
}

/// A provider row joined with a model row.
struct ProviderModelJoin {
    /// Owning provider.
    provider: Provider,

    /// Model row.
    model: Model,
}

/// Parses a provider-model joined row in [`PROVIDER_MODEL_COLUMNS`] order.
///
/// The join projects the provider columns first and the model columns second,
/// without `models.provider_id`, which is taken from the provider row.
fn provider_model_from_row(row: &Row) -> Result<ProviderModelJoin, Error> {
    let provider = provider_from_row(row)?;

    let model = Model {
        provider_id: provider.id,
        upstream_id: row.get(9)?,
        id: row.get(10)?,
        id_override: row.get(11)?,
        is_enabled: row.get::<i64>(12)? != 0,
        thinking_levels_override: row.get(13)?,
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

/// Parses a model entry row in [`MODEL_ENTRY_COLUMNS`] order: the provider
/// id, name, and enabled flag, then the model columns at offset 3.
fn model_entry_from_row(row: &Row) -> Result<ModelEntry, Error> {
    Ok(ModelEntry {
        provider_id: row.get(0)?,
        provider_name: row.get(1)?,
        is_provider_enabled: row.get::<i64>(2)? != 0,
        model: model_from_row_offset(row, 3)?,
    })
}

/// Rejects one provider's freshly composed public IDs when two of its rows
/// would converge on one ID.
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

/// Rejects freshly composed public IDs that are already routed by another
/// model row — of any provider, including another model of the same provider.
/// An ID held by the very row being written is not a collision.
///
/// `composed` carries `(public_id, upstream_id)` pairs of the provider
/// identified by `provider_id`. Runs inside the caller's transaction, so the
/// routed rows it observes cannot interleave with the caller's writes.
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
        format::UpstreamFormat,
        provider::{Credentials, NewProvider, OAuthProviderType, ProviderType},
    };
    use std::{num::NonZero, path::PathBuf, time::Duration};

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

    /// Verifies the full model ID lifecycle: prefix composition on discovery,
    /// direct replacement when overridden, invariant preservation on provider
    /// prefix update, clearing overrides, and routing conflict rejection.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn test_model_id_override_and_prefix_behavior() {
        let (database, _guard) = create_test_database().await;

        let credentials = Credentials::ApiKey("test-secret".to_string());

        #[expect(clippy::unwrap_used, reason = "provider inserts cleanly")]
        insert_provider(
            &database,
            &NewProvider {
                name: "provider-1",
                model_prefix: "p1/",
                provider_type: ProviderType::OpenAiCompatible,
                protocol: Some(UpstreamFormat::OpenAiChatCompletion),
                base_url: Some("http://localhost:8080"),
                credentials: &credentials,
                expires_at: None,
                is_enabled: true,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::unwrap_used, reason = "provider list succeeds")]
        let providers = list_providers(&database).await.unwrap();
        #[expect(clippy::indexing_slicing, reason = "one provider was inserted")]
        let provider_id = providers[0].id;

        // 1. Discover models: "gpt-4" and "claude"
        #[expect(clippy::unwrap_used, reason = "model discovery succeeds")]
        let count = replace_discovered_models(
            &database,
            provider_id,
            &["gpt-4".to_string(), "claude".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(count, 2);

        #[expect(clippy::unwrap_used, reason = "list models succeeds")]
        let entries = list_model_entries(&database).await.unwrap();
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

        // 2. Set id_override on "gpt-4" to "top-gpt" (bypasses "p1/" prefix)
        #[expect(clippy::unwrap_used, reason = "override update succeeds")]
        let updated = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "gpt-4",
                id_override: Some(Some("top-gpt".to_string())),
                is_enabled: None,
                thinking_levels_override: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.model.id, "top-gpt");
        assert_eq!(updated.model.id_override.as_deref(), Some("top-gpt"));
        assert!(updated.model.is_enabled);

        // Routing lookup resolves "top-gpt" to this model
        #[expect(clippy::unwrap_used, reason = "routing query succeeds")]
        let routing = find_routing(&database, "top-gpt").await.unwrap();
        assert!(routing.is_some());
        #[expect(clippy::unwrap_used, reason = "routing row is present")]
        let routing = routing.unwrap();
        assert_eq!(routing.model.upstream_id, "gpt-4");
        assert!(routing.is_effectively_enabled);

        // Old prefixed ID "p1/gpt-4" no longer routes
        #[expect(clippy::unwrap_used, reason = "routing query succeeds")]
        let old_routing = find_routing(&database, "p1/gpt-4").await.unwrap();
        assert!(old_routing.is_none());

        // 3. Update provider's model_prefix to "new-p1/"
        #[expect(clippy::unwrap_used, reason = "provider prefix update succeeds")]
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1/",
                protocol: None,
                base_url: None,
                is_enabled: None,
                credentials: None,
            },
        )
        .await
        .unwrap();

        // Check: "gpt-4" retains "top-gpt", while "claude" is recomputed as
        // "new-p1/claude"
        #[expect(clippy::unwrap_used, reason = "list models succeeds")]
        let entries_after_prefix = list_model_entries(&database).await.unwrap();
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
        #[expect(clippy::unwrap_used, reason = "clearing override succeeds")]
        let cleared = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "gpt-4",
                id_override: Some(None),
                is_enabled: None,
                thinking_levels_override: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(cleared.model.id, "new-p1/gpt-4");
        assert_eq!(cleared.model.id_override, None);

        // 5. Collision check: try to set claude's id_override to "new-p1/gpt-4"
        let collision_result = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "claude",
                id_override: Some(Some("new-p1/gpt-4".to_string())),
                is_enabled: None,
                thinking_levels_override: None,
            },
        )
        .await;
        assert!(collision_result.is_err());

        // 6. Disable claude individually
        #[expect(clippy::unwrap_used, reason = "disabling model succeeds")]
        let disabled_claude = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_id: "claude",
                id_override: None,
                is_enabled: Some(false),
                thinking_levels_override: None,
            },
        )
        .await
        .unwrap();
        assert!(!disabled_claude.model.is_enabled);
        assert!(
            !disabled_claude
                .model
                .is_effectively_enabled(disabled_claude.is_provider_enabled)
        );

        // Enabled models list includes gpt-4 but excludes claude
        #[expect(clippy::unwrap_used, reason = "list enabled models succeeds")]
        let enabled_models = list_enabled_models_with_owner(&database).await.unwrap();
        assert_eq!(enabled_models.len(), 1);
        #[expect(clippy::indexing_slicing, reason = "length 1 asserted above")]
        let first_enabled = &enabled_models[0];
        assert_eq!(first_enabled.0, "new-p1/gpt-4");

        // 7. Master switch test: disable provider -> ALL models effectively
        //    disabled
        #[expect(clippy::unwrap_used, reason = "disabling provider succeeds")]
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "provider-1",
                model_prefix: "new-p1/",
                protocol: None,
                base_url: None,
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await
        .unwrap();

        // Routing lookup for gpt-4 reports the effective status as disabled
        #[expect(clippy::unwrap_used, reason = "routing query succeeds")]
        let gpt4_routing = find_routing(&database, "new-p1/gpt-4").await.unwrap();
        assert!(gpt4_routing.is_some());
        #[expect(clippy::unwrap_used, reason = "routing is some")]
        let gpt4_routing = gpt4_routing.unwrap();
        assert!(!gpt4_routing.is_effectively_enabled);

        // Enabled models list is now empty because provider is disabled
        #[expect(clippy::unwrap_used, reason = "list enabled models succeeds")]
        let enabled_models_after_provider_disable =
            list_enabled_models_with_owner(&database).await.unwrap();
        assert!(enabled_models_after_provider_disable.is_empty());
    }

    /// Verifies the OAuth state lifecycle and typed database roundtrips for
    /// `OAuthProviderType` and `ProviderType`.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn test_oauth_state_and_provider_type_database_lifecycle() {
        let (database, _guard) = create_test_database().await;

        let now = 1_000_000;
        let expires_at = now + 600;

        // 1. Insert OAuth states with OAuthProviderType
        #[expect(clippy::unwrap_used, reason = "insert oauth state succeeds")]
        insert_oauth_state(
            &database,
            OAuthProviderType::OpenAiOauth,
            "openai-state-1",
            "openai-verifier-1",
            expires_at,
        )
        .await
        .unwrap();

        #[expect(clippy::unwrap_used, reason = "insert oauth state succeeds")]
        insert_oauth_state(
            &database,
            OAuthProviderType::AntigravityOauth,
            "antigravity-state-1",
            "",
            expires_at,
        )
        .await
        .unwrap();

        // 2. Query type without consuming
        #[expect(clippy::unwrap_used, reason = "find oauth state type succeeds")]
        let found_type = find_oauth_state_type(&database, "openai-state-1", now)
            .await
            .unwrap();
        assert_eq!(found_type, Some(OAuthProviderType::OpenAiOauth));

        #[expect(clippy::unwrap_used, reason = "find oauth state type succeeds")]
        let found_ag = find_oauth_state_type(&database, "antigravity-state-1", now)
            .await
            .unwrap();
        assert_eq!(found_ag, Some(OAuthProviderType::AntigravityOauth));

        // 3. Atomically consume
        #[expect(clippy::unwrap_used, reason = "consume oauth state succeeds")]
        let consumed = consume_oauth_state(&database, "openai-state-1", now)
            .await
            .unwrap();
        #[expect(clippy::unwrap_used, reason = "state is present")]
        let consumed = consumed.unwrap();
        assert_eq!(consumed.state_type, OAuthProviderType::OpenAiOauth);
        assert_eq!(consumed.verifier, "openai-verifier-1");

        // Second consume returns None
        #[expect(clippy::unwrap_used, reason = "second consume succeeds")]
        let second_consume = consume_oauth_state(&database, "openai-state-1", now)
            .await
            .unwrap();
        assert!(second_consume.is_none());

        // 4. Create provider from consumed state and verify typed read back
        let credentials = Credentials::OpenAiOauth {
            access_token: "test-token".to_string(),
            refresh_token: "test-refresh".to_string(),
            account_id: "test-account".to_string(),
        };
        #[expect(clippy::unwrap_used, reason = "insert provider succeeds")]
        insert_provider(
            &database,
            &NewProvider {
                name: "oauth-provider",
                model_prefix: "openai/",
                provider_type: consumed.state_type.as_provider_type(),
                protocol: None,
                base_url: None,
                credentials: &credentials,
                expires_at: Some(expires_at),
                is_enabled: true,
            },
        )
        .await
        .unwrap();

        #[expect(clippy::unwrap_used, reason = "list providers succeeds")]
        let providers = list_providers(&database).await.unwrap();
        #[expect(clippy::unwrap_used, reason = "provider was inserted")]
        let provider = providers
            .iter()
            .find(|p| p.name == "oauth-provider")
            .unwrap();
        assert_eq!(provider.provider_type, ProviderType::OpenAiOauth);

        // 5. Sweep expired states
        #[expect(clippy::unwrap_used, reason = "delete expired states succeeds")]
        delete_expired_oauth_states(&database, expires_at + 10)
            .await
            .unwrap();

        #[expect(clippy::unwrap_used, reason = "find expired state succeeds")]
        let swept = find_oauth_state_type(&database, "antigravity-state-1", now)
            .await
            .unwrap();
        assert!(swept.is_none());
    }

    /// Verifies that the schema itself enforces the `providers.protocol`
    /// closed domain: a drifted value is rejected at write time and classifies
    /// as a storage constraint error, instead of surviving until the read-side
    /// protocol parse turns every dispatch for the provider into drift.
    /// First-party write paths validate the protocol before inserting, so the
    /// probe uses a raw statement.
    ///
    /// # Panics
    ///
    /// Panics if the drifted insert unexpectedly succeeds.
    #[tokio::test]
    async fn rejects_unknown_provider_protocol_at_write_time() {
        let (database, _guard) = create_test_database().await;

        let outcome = database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, protocol, credentials) \
                 VALUES ('drifted', 'd/', 'openai_compatible', 'responses_api', '{}')",
                (),
            )
            .await;

        #[expect(clippy::expect_used, reason = "the schema CHECK rejects the drift")]
        let error = outcome.expect_err("drifted protocol must violate the schema CHECK");

        assert!(
            matches!(Error::from(error), Error::Constraint { .. }),
            "a schema CHECK violation must classify as a storage constraint error"
        );
    }

    /// Verifies value conversions between database values and provider types.
    ///
    /// # Panics
    ///
    /// Panics if value serialization or deserialization fails.
    #[test]
    fn converts_provider_type_database_values() {
        for provider_type in [
            ProviderType::OpenAiOauth,
            ProviderType::AntigravityOauth,
            ProviderType::OpenAiCompatible,
        ] {
            #[expect(clippy::unwrap_used, reason = "into_value returns Ok for ProviderType")]
            let value = provider_type.into_value().unwrap();
            assert_eq!(value, Value::Text(provider_type.as_str().to_string()));

            #[expect(clippy::unwrap_used, reason = "try_from returns Ok for valid Text")]
            let parsed = ProviderType::try_from(&value).unwrap();
            assert_eq!(parsed, provider_type);
        }

        let invalid = Value::Text("unknown_type".to_string());
        assert!(ProviderType::try_from(&invalid).is_err());

        let non_text = Value::Integer(42);
        assert!(ProviderType::try_from(&non_text).is_err());
    }

    /// Verifies value conversions between database values and OAuth provider
    /// types.
    ///
    /// # Panics
    ///
    /// Panics if value serialization or deserialization fails.
    #[test]
    fn converts_oauth_provider_type_database_values() {
        for oauth_type in [
            OAuthProviderType::OpenAiOauth,
            OAuthProviderType::AntigravityOauth,
        ] {
            #[expect(
                clippy::unwrap_used,
                reason = "into_value returns Ok for OAuthProviderType"
            )]
            let value = oauth_type.into_value().unwrap();
            assert_eq!(value, Value::Text(oauth_type.as_str().to_string()));

            #[expect(clippy::unwrap_used, reason = "try_from returns Ok for valid Text")]
            let parsed = OAuthProviderType::try_from(&value).unwrap();
            assert_eq!(parsed, oauth_type);
        }

        let incompatible = Value::Text("openai_compatible".to_string());
        assert!(OAuthProviderType::try_from(&incompatible).is_err());

        let invalid = Value::Text("unknown_type".to_string());
        assert!(OAuthProviderType::try_from(&invalid).is_err());

        let non_text = Value::Integer(42);
        assert!(OAuthProviderType::try_from(&non_text).is_err());
    }
}
