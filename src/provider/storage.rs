//! Provider, model, and OAuth state persistence.

use super::{
    Credentials, Model, ModelRouting, NewProvider, Provider, ProviderType, UpstreamProtocol,
};
use crate::{
    database::{Database, Row, Transaction, Value, params, params_from_iter},
    error::ApiError,
};

/// Column order of provider selects.
const PROVIDER_COLUMNS: &str = "id, name, model_prefix, type, protocol, base_url, credentials, \
     expires_at, enabled";

/// The combined column order of provider-model joins.
const PROVIDER_MODEL_COLUMNS: &str = concat!(
    "p.id, p.name, p.model_prefix, p.type, p.protocol, p.base_url, p.credentials, ",
    "p.expires_at, p.enabled, ",
    "m.upstream_name, m.name, m.name_override, m.enabled_override, m.thinking_levels_override"
);

/// Inserts one provider row.
///
/// # Errors
///
/// Returns [`ApiError`] when the insert fails.
pub(crate) async fn insert_provider(
    database: &Database,
    provider: &NewProvider<'_>,
) -> Result<(), ApiError> {
    database
        .execute(
            "INSERT INTO providers (name, model_prefix, type, protocol, base_url, credentials, \
             expires_at, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                provider.name,
                provider.model_prefix,
                provider.provider_type.as_str(),
                provider.protocol.map(UpstreamProtocol::as_str),
                provider.base_url,
                provider.credentials.to_json(),
                provider.expires_at,
                i64::from(provider.enabled),
            ],
        )
        .await?;

    Ok(())
}

/// Lists every stored provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
pub(crate) async fn list_providers(database: &Database) -> Result<Vec<Provider>, ApiError> {
    let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers ORDER BY name");

    let rows = database.query(sql, ()).await?;

    rows.iter().map(provider_from_row).collect()
}

/// Fetches one provider by name.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the row is malformed.
pub(crate) async fn find_provider(
    database: &Database,
    name: &str,
) -> Result<Option<Provider>, ApiError> {
    let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE name = ?1");

    let rows = database.query(sql, params![name]).await?;

    rows.first().map(provider_from_row).transpose()
}

/// Fetches one provider by name inside a transaction.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the row is malformed.
pub(crate) async fn find_provider_in_transaction(
    transaction: &mut Transaction,
    name: &str,
) -> Result<Option<Provider>, ApiError> {
    let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE name = ?1");

    let rows = transaction.query(sql, params![name]).await?;

    rows.first().map(provider_from_row).transpose()
}

/// Deletes one provider by name, returning whether a row was removed.
///
/// # Errors
///
/// Returns [`ApiError`] when the delete fails.
pub(crate) async fn delete_provider(database: &Database, name: &str) -> Result<bool, ApiError> {
    let changes = database
        .execute("DELETE FROM providers WHERE name = ?1", params![name])
        .await?;

    Ok(changes == 1)
}

/// Replaces provider credentials, resets the token expiry, and clears any
/// refresh lease.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn update_credentials(
    database: &Database,
    provider_id: i64,
    credentials: &Credentials,
    expires_at: Option<i64>,
) -> Result<(), ApiError> {
    database
        .execute(
            "UPDATE providers SET credentials = ?1, expires_at = ?2, refresh_started_at = NULL \
             WHERE id = ?3",
            params![credentials.to_json(), expires_at, provider_id],
        )
        .await?;

    Ok(())
}

/// Updates the mutable configuration fields of a provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn update_provider_config(
    database: &Database,
    provider_id: i64,
    model_prefix: &str,
    protocol: Option<UpstreamProtocol>,
    base_url: Option<&str>,
    enabled: bool,
) -> Result<(), ApiError> {
    database
        .execute(
            "UPDATE providers SET model_prefix = ?1, protocol = ?2, base_url = ?3, enabled = ?4 \
             WHERE id = ?5",
            params![
                model_prefix,
                protocol.map(UpstreamProtocol::as_str),
                base_url,
                i64::from(enabled),
                provider_id,
            ],
        )
        .await?;

    Ok(())
}

/// Attempts to acquire the token refresh lease inside a transaction.
///
/// The lease is granted only when the token is still expired and no other
/// holder owns a fresh lease.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn try_acquire_refresh_lease(
    transaction: &mut Transaction,
    provider_id: i64,
    now: i64,
    lease_stale_before: i64,
) -> Result<bool, ApiError> {
    let changes = transaction
        .execute(
            "UPDATE providers SET refresh_started_at = ?1 \
             WHERE id = ?2 AND (expires_at IS NULL OR expires_at <= ?1 + 60) \
             AND (refresh_started_at IS NULL OR refresh_started_at < ?3)",
            params![now, provider_id, lease_stale_before],
        )
        .await?;

    Ok(changes == 1)
}

/// Clears the token refresh lease after a finished or failed refresh.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn clear_refresh_lease(
    database: &Database,
    provider_id: i64,
) -> Result<(), ApiError> {
    database
        .execute(
            "UPDATE providers SET refresh_started_at = NULL WHERE id = ?1",
            params![provider_id],
        )
        .await?;

    Ok(())
}

/// Resolves a public model name to its provider and model row.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
pub(crate) async fn find_routing(
    database: &Database,
    public_name: &str,
) -> Result<Option<ModelRouting>, ApiError> {
    let sql = format!(
        "SELECT {PROVIDER_MODEL_COLUMNS} FROM models m \
         JOIN providers p ON p.id = m.provider_id WHERE m.name = ?1"
    );

    let rows = database.query(sql, params![public_name]).await?;

    rows.first()
        .map(|row| {
            provider_model_from_row(row).map(|joined| ModelRouting {
                is_enabled: joined
                    .model
                    .enabled_override
                    .unwrap_or(joined.provider.enabled),
                provider: joined.provider,
                model: joined.model,
            })
        })
        .transpose()
}

/// Fetches one model row of a provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
pub(crate) async fn find_model(
    database: &Database,
    provider_id: i64,
    upstream_name: &str,
) -> Result<Option<Model>, ApiError> {
    let rows = database
        .query(
            "SELECT provider_id, upstream_name, name, name_override, enabled_override, \
             thinking_levels_override FROM models \
             WHERE provider_id = ?1 AND upstream_name = ?2",
            params![provider_id, upstream_name],
        )
        .await?;

    rows.first().map(model_from_row).transpose()
}

/// One model row with the provider facts needed for listing.
#[derive(Debug, Clone)]
pub(crate) struct ModelEntry {
    /// Owning provider name.
    pub(crate) provider_name: String,

    /// Whether the owning provider serves traffic.
    pub(crate) is_provider_enabled: bool,

    /// The model row.
    pub(crate) model: Model,
}

/// Lists every model row of one provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
pub(crate) async fn find_models_of_provider(
    database: &Database,
    provider_id: i64,
) -> Result<Vec<Model>, ApiError> {
    let rows = database
        .query(
            "SELECT provider_id, upstream_name, name, name_override, enabled_override, \
             thinking_levels_override FROM models WHERE provider_id = ?1 ORDER BY upstream_name",
            params![provider_id],
        )
        .await?;

    rows.iter().map(model_from_row).collect()
}

/// Lists every stored model with its provider facts.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
pub(crate) async fn list_model_entries(database: &Database) -> Result<Vec<ModelEntry>, ApiError> {
    let rows = database
        .query(
            "SELECT p.name AS provider_name, p.enabled AS provider_enabled, \
             m.provider_id, m.upstream_name, m.name, m.name_override, m.enabled_override, \
             m.thinking_levels_override FROM models m JOIN providers p ON p.id = m.provider_id \
             ORDER BY m.name",
            (),
        )
        .await?;

    let mut entries = Vec::with_capacity(rows.len());

    for row in &rows {
        entries.push(ModelEntry {
            provider_name: row.get(0).map_err(ApiError::from)?,
            is_provider_enabled: row.get::<i64>(1).map_err(ApiError::from)? != 0,
            model: model_from_row_offset(row, 2)?,
        });
    }

    Ok(entries)
}

/// Lists every effectively enabled public model name, ordered by name.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails.
pub(crate) async fn list_enabled_public_names(
    database: &Database,
) -> Result<Vec<String>, ApiError> {
    let rows = database
        .query(
            "SELECT m.name FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE COALESCE(m.enabled_override, p.enabled) != 0 ORDER BY m.name",
            (),
        )
        .await?;

    rows.iter()
        .map(|row| row.get::<String>(0).map_err(ApiError::from))
        .collect()
}

/// Replaces the model rows of a provider inside a transaction.
///
/// # Errors
///
/// Returns [`ApiError`] when any statement fails.
pub(crate) async fn replace_provider_models(
    transaction: &mut Transaction,
    provider_id: i64,
    models: &[Model],
) -> Result<(), ApiError> {
    transaction
        .execute(
            "DELETE FROM models WHERE provider_id = ?1",
            params![provider_id],
        )
        .await?;

    for model in models {
        transaction
            .execute(
                "INSERT INTO models (provider_id, upstream_name, name, name_override, \
                 enabled_override, thinking_levels_override) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    model.provider_id,
                    model.upstream_name.clone(),
                    model.name.clone(),
                    model.name_override.clone(),
                    model.enabled_override.map(i64::from),
                    model.thinking_levels_override.clone(),
                ],
            )
            .await?;
    }

    Ok(())
}

/// Updates the override fields of one model, recomputing its public name.
/// `None` leaves a field unchanged; `Some` writes the inner value, clearing
/// the column when the inner value is `None`.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn update_model_overrides(
    database: &Database,
    provider_id: i64,
    upstream_name: &str,
    new_name: &str,
    name_override: Option<Option<String>>,
    enabled_override: Option<Option<bool>>,
    thinking_levels_override: Option<Option<String>>,
) -> Result<(), ApiError> {
    let mut assignments: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();

    if let Some(override_value) = name_override {
        assignments.push("name_override = ?".to_string());
        values.push(override_value.into());
    }

    if let Some(override_value) = enabled_override {
        assignments.push("enabled_override = ?".to_string());
        values.push(override_value.map(i64::from).into());
    }

    if let Some(override_value) = thinking_levels_override {
        assignments.push("thinking_levels_override = ?".to_string());
        values.push(override_value.into());
    }

    assignments.push("name = ?".to_string());
    values.push(new_name.into());

    values.push(provider_id.into());
    values.push(upstream_name.into());

    let sql = format!(
        "UPDATE models SET {} WHERE provider_id = ? AND upstream_name = ?",
        assignments.join(", ")
    );

    database.execute(sql, params_from_iter(values)).await?;

    Ok(())
}

/// Inserts one pending OAuth state row.
///
/// # Errors
///
/// Returns [`ApiError`] when the insert fails.
pub(crate) async fn insert_oauth_state(
    database: &Database,
    state_type: ProviderType,
    state: &str,
    verifier: &str,
    expires_at: i64,
) -> Result<(), ApiError> {
    database
        .execute(
            "INSERT INTO oauth_states (type, state, verifier, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![state_type.as_str(), state, verifier, expires_at],
        )
        .await?;

    Ok(())
}

/// One pending OAuth state row.
#[derive(Debug, Clone)]
pub(crate) struct OAuthState {
    /// Provider type the flow is for.
    pub(crate) state_type: ProviderType,

    /// PKCE verifier, when the flow uses PKCE.
    pub(crate) verifier: String,
}

/// Consumes the OAuth state row for `state`, returning its facts only when it
/// is unexpired.
///
/// The row is deleted in either case, so a state can never complete twice.
///
/// # Errors
///
/// Returns [`ApiError`] when the read or delete fails.
pub(crate) async fn consume_oauth_state(
    database: &Database,
    state: &str,
    now: i64,
) -> Result<Option<OAuthState>, ApiError> {
    let rows = database
        .query(
            "SELECT type, verifier FROM oauth_states WHERE state = ?1 AND expires_at > ?2",
            params![state, now],
        )
        .await?;

    let Some(row) = rows.first() else {
        database
            .execute("DELETE FROM oauth_states WHERE state = ?1", params![state])
            .await?;

        return Ok(None);
    };

    let type_name: String = row.get(0)?;
    let verifier: String = row.get(1)?;

    database
        .execute("DELETE FROM oauth_states WHERE state = ?1", params![state])
        .await?;

    let state_type = ProviderType::parse(&type_name).ok_or_else(|| {
        ApiError::internal(format!(
            "stored OAuth state carries unknown provider type `{type_name}`"
        ))
    })?;

    Ok(Some(OAuthState {
        state_type,
        verifier,
    }))
}

/// Deletes every expired OAuth state row.
///
/// # Errors
///
/// Returns [`ApiError`] when the delete fails.
pub(crate) async fn delete_expired_oauth_states(
    database: &Database,
    now: i64,
) -> Result<(), ApiError> {
    database
        .execute(
            "DELETE FROM oauth_states WHERE expires_at <= ?1",
            params![now],
        )
        .await?;

    Ok(())
}

/// Parses a provider row in [`PROVIDER_COLUMNS`] order.
fn provider_from_row(row: &Row) -> Result<Provider, ApiError> {
    let type_name: String = row.get(3)?;

    let provider_type = ProviderType::parse(&type_name).ok_or_else(|| {
        ApiError::internal(format!("provider row carries unknown type `{type_name}`"))
    })?;

    let protocol = match row.get::<Option<String>>(4)? {
        None => None,
        Some(protocol_name) => Some(UpstreamProtocol::parse(&protocol_name).ok_or_else(|| {
            ApiError::internal(format!(
                "provider row carries unknown protocol `{protocol_name}`"
            ))
        })?),
    };

    let name: String = row.get(1)?;
    let credentials_json: String = row.get(6)?;

    let credentials = Credentials::parse(provider_type, &credentials_json).map_err(|error| {
        ApiError::internal(format!(
            "provider `{name}` has malformed credentials: {error}"
        ))
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
        enabled: row.get::<i64>(8)? != 0,
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
fn provider_model_from_row(row: &Row) -> Result<ProviderModelJoin, ApiError> {
    let provider = provider_from_row(row)?;

    let model = Model {
        provider_id: provider.id,
        upstream_name: row.get(9)?,
        name: row.get(10)?,
        name_override: row.get(11)?,
        enabled_override: row.get::<Option<i64>>(12)?.map(|value| value != 0),
        thinking_levels_override: row.get(13)?,
    };

    Ok(ProviderModelJoin { provider, model })
}

/// Parses a model row starting at the given column offset.
fn model_from_row_offset(row: &Row, offset: usize) -> Result<Model, ApiError> {
    Ok(Model {
        provider_id: row.get(offset)?,
        upstream_name: row.get(offset + 1)?,
        name: row.get(offset + 2)?,
        name_override: row.get(offset + 3)?,
        enabled_override: row.get::<Option<i64>>(offset + 4)?.map(|value| value != 0),
        thinking_levels_override: row.get(offset + 5)?,
    })
}

/// Parses a standalone model row in canonical column order.
fn model_from_row(row: &Row) -> Result<Model, ApiError> {
    model_from_row_offset(row, 0)
}
