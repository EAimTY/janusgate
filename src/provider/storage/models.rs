//! Model row persistence: routing resolution, listing, discovery
//! replacement, and override updates.

use super::{
    Error, MODEL_COLUMNS, PROVIDER_MODEL_COLUMNS, ensure_ids_pairwise_distinct,
    ensure_ids_routable, model_from_row, model_from_row_offset, provider_model_from_row,
    providers::find_provider_by_id_in_transaction,
};
use crate::{
    database::{Database, Transaction, Value, params, params_from_iter},
    provider::{Model, ModelRouting},
};
use std::collections::HashMap;

/// Resolves a public model ID to its provider and model row.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_routing(
    database: &Database,
    public_id: &str,
) -> Result<Option<ModelRouting>, Error> {
    let sql = format!(
        "SELECT {PROVIDER_MODEL_COLUMNS} FROM models m \
         JOIN providers p ON p.id = m.provider_id WHERE m.id = ?1"
    );

    let rows = database.query(sql, params![public_id]).await?;

    rows.first()
        .map(|row| {
            provider_model_from_row(row).map(|joined| ModelRouting {
                is_effectively_enabled: joined
                    .model
                    .is_effectively_enabled(joined.provider.is_enabled),
                provider: joined.provider,
                model: joined.model,
            })
        })
        .transpose()
}

/// Fetches every model row of one provider inside a transaction.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(super) async fn find_models_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Vec<Model>, Error> {
    let sql =
        format!("SELECT {MODEL_COLUMNS} FROM models WHERE provider_id = ?1 ORDER BY upstream_id");

    let rows = transaction.query(sql, params![provider_id]).await?;

    rows.iter().map(model_from_row).collect()
}

/// Fetches one model row of a provider inside a transaction.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn find_model_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    upstream_id: &str,
) -> Result<Option<Model>, Error> {
    let rows = transaction
        .query(
            format!(
                "SELECT {MODEL_COLUMNS} FROM models \
                 WHERE provider_id = ?1 AND upstream_id = ?2"
            ),
            params![provider_id, upstream_id],
        )
        .await?;

    rows.first().map(model_from_row).transpose()
}

/// One model row with the provider facts needed for listing.
#[derive(Debug, Clone)]
pub(crate) struct ModelEntry {
    /// Owning provider row identifier.
    pub(crate) provider_id: i64,

    /// Owning provider name.
    pub(crate) provider_name: String,

    /// Whether the owning provider serves traffic.
    pub(crate) is_provider_enabled: bool,

    /// The model row.
    pub(crate) model: Model,
}

/// Lists every stored model with its provider facts.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_model_entries(database: &Database) -> Result<Vec<ModelEntry>, Error> {
    let rows = database
        .query(
            "SELECT p.id, p.name AS provider_name, p.is_enabled AS provider_is_enabled, \
             m.provider_id, m.upstream_id, m.id, m.id_override, m.is_enabled, \
             m.thinking_levels_override FROM models m JOIN providers p ON p.id = m.provider_id \
             ORDER BY m.id",
            (),
        )
        .await?;

    let mut entries = Vec::with_capacity(rows.len());

    for row in &rows {
        entries.push(ModelEntry {
            provider_id: row.get(0).map_err(Error::from)?,
            provider_name: row.get(1).map_err(Error::from)?,
            is_provider_enabled: row.get::<i64>(2).map_err(Error::from)? != 0,
            model: model_from_row_offset(row, 3)?,
        });
    }

    Ok(entries)
}

/// Lists `(public_id, provider_name)` pairs for every effectively enabled
/// model, ordered by public ID.
///
/// A model is effectively enabled if and only if both its owning provider and
/// the model itself are enabled (`p.is_enabled != 0 AND m.is_enabled != 0`).
///
/// # Errors
///
/// Returns [`Error`] when the query fails.
pub(crate) async fn list_enabled_models_with_owner(
    database: &Database,
) -> Result<Vec<(String, String)>, Error> {
    let rows = database
        .query(
            "SELECT m.id, p.name FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE p.is_enabled != 0 AND m.is_enabled != 0 ORDER BY m.id",
            (),
        )
        .await?;

    rows.iter()
        .map(|row| Ok((row.get::<String>(0)?, row.get::<String>(1)?)))
        .collect()
}

/// Replaces the model rows of one provider with a freshly discovered set
/// inside a single transaction, returning the number of stored rows.
///
/// An empty discovered set is rejected with 502 before anything is written:
/// an upstream that legitimately reports no models — a fresh pre-onboarded
/// account, or an outage answering 200 with an empty payload — must fail
/// loudly instead of irreversibly wiping the provider's curated rows
/// (README §7.1).
///
/// The provider row is re-read inside the transaction, so the public IDs are
/// composed from the provider's current prefix even when a concurrent
/// `save-provider` changed it after the caller's outer read (README §7.1).
/// Overrides carry over from the stored rows by `upstream_id`; discovered
/// models absent from the stored set vanish with their overrides.
///
/// The composed public IDs undergo the same collision checks as
/// [`super::update_provider`]: pairwise distinctness within the provider and
/// no routing collision with any other model row; both rejections are ID-
/// specific 409 conflicts that write nothing.
///
/// # Errors
///
/// Returns [`Error`] for an empty discovered set, an unknown provider,
/// failed statements, or a composed public ID collision.
pub(crate) async fn replace_discovered_models(
    database: &Database,
    provider_id: i64,
    upstream_ids: &[String],
) -> Result<usize, Error> {
    let mut transaction = database.transaction().await?;

    let Some(provider) = find_provider_by_id_in_transaction(&mut transaction, provider_id).await?
    else {
        return Err(Error::NotFound {
            message: format!("Provider {provider_id} does not exist"),
        });
    };

    let previous = find_models_in_transaction(&mut transaction, provider.id).await?;

    let stored: HashMap<&str, &Model> = previous
        .iter()
        .map(|model| (model.upstream_id.as_str(), model))
        .collect();

    let mut models: Vec<Model> = Vec::with_capacity(upstream_ids.len());

    for upstream_id in upstream_ids {
        let prior = stored.get(upstream_id.as_str()).copied();

        let id_override = prior.and_then(|model| model.id_override.clone());

        let id = Model::public_id(&provider.model_prefix, upstream_id, id_override.as_deref());

        models.push(Model {
            provider_id: provider.id,
            upstream_id: upstream_id.clone(),
            id,
            id_override,
            is_enabled: prior.is_none_or(|model| model.is_enabled),
            thinking_levels_override: prior
                .and_then(|model| model.thinking_levels_override.clone()),
        });
    }

    let composed: Vec<(&str, &str)> = models
        .iter()
        .map(|model| (model.id.as_str(), model.upstream_id.as_str()))
        .collect();

    ensure_ids_pairwise_distinct(&provider.name, &composed)?;

    ensure_ids_routable(&mut transaction, provider.id, &composed).await?;

    replace_provider_models(&mut transaction, provider.id, &models).await?;

    transaction.commit().await.map_err(|error| Error::Drift {
        message: format!("failed to commit discovered models: {error}"),
    })?;

    Ok(models.len())
}

/// Replaces the model rows of a provider inside a transaction.
///
/// # Errors
///
/// Returns [`Error`] when any statement fails.
async fn replace_provider_models(
    transaction: &mut Transaction,
    provider_id: i64,
    models: &[Model],
) -> Result<(), Error> {
    transaction
        .execute(
            "DELETE FROM models WHERE provider_id = ?1",
            params![provider_id],
        )
        .await?;

    for model in models {
        transaction
            .execute(
                "INSERT INTO models (provider_id, upstream_id, id, id_override, \
                 is_enabled, thinking_levels_override) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    model.provider_id,
                    model.upstream_id.clone(),
                    model.id.clone(),
                    model.id_override.clone(),
                    i64::from(model.is_enabled),
                    model.thinking_levels_override.clone(),
                ],
            )
            .await?;
    }

    Ok(())
}

/// One `save-model` update about to be applied transactionally.
pub(crate) struct ModelOverrideUpdate<'a> {
    /// Owning provider's id, the lookup key.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_id: &'a str,

    /// Public ID override; `None` leaves it unchanged, `Some` writes the
    /// inner value, clearing the column when the inner value is `None`.
    pub(crate) id_override: Option<Option<String>>,

    /// Serving state; `None` leaves it unchanged, `Some` sets it.
    pub(crate) is_enabled: Option<bool>,

    /// Canonical thinking levels override; same tri-state semantics.
    pub(crate) thinking_levels_override: Option<Option<String>>,
}

/// Writes one model's overrides and recomputes its public ID inside a single
/// transaction, returning the updated row with its provider facts.
///
/// The provider row is re-read inside the transaction, so the recomputed
/// public ID uses the provider's current prefix even when a concurrent
/// `save-provider` changed it after the caller's outer read; the stored model
/// row and the routed-ID collision check observe the same snapshot. The
/// collision check matches [`super::update_provider`]'s quality tier: an ID
/// already routed by another model row yields a 409 naming the holder, and
/// nothing is written. Unknown providers or models yield 404.
///
/// # Errors
///
/// Returns [`Error`] for unknown rows, failed statements, or a public ID
/// collision.
pub(crate) async fn update_model_overrides_checked(
    database: &Database,
    update: ModelOverrideUpdate<'_>,
) -> Result<ModelEntry, Error> {
    let mut transaction = database.transaction().await?;

    let Some(provider) =
        find_provider_by_id_in_transaction(&mut transaction, update.provider_id).await?
    else {
        return Err(Error::NotFound {
            message: format!("Provider {} does not exist", update.provider_id),
        });
    };

    let Some(model) =
        find_model_in_transaction(&mut transaction, provider.id, update.upstream_id).await?
    else {
        return Err(Error::NotFound {
            message: format!(
                "Model `{}` does not exist on provider `{}`",
                update.upstream_id, provider.name
            ),
        });
    };

    let new_id_override = update
        .id_override
        .clone()
        .unwrap_or_else(|| model.id_override.clone());

    let new_id = Model::public_id(
        &provider.model_prefix,
        update.upstream_id,
        new_id_override.as_deref(),
    );

    ensure_ids_routable(
        &mut transaction,
        provider.id,
        &[(new_id.as_str(), update.upstream_id)],
    )
    .await?;

    let mut assignments: Vec<String> = Vec::new();

    let mut values: Vec<Value> = Vec::new();

    if let Some(override_value) = update.id_override {
        assignments.push("id_override = ?".to_string());

        values.push(override_value.into());
    }

    if let Some(is_enabled) = update.is_enabled {
        assignments.push("is_enabled = ?".to_string());

        values.push(i64::from(is_enabled).into());
    }

    if let Some(override_value) = update.thinking_levels_override {
        assignments.push("thinking_levels_override = ?".to_string());

        values.push(override_value.into());
    }

    assignments.push("id = ?".to_string());

    values.push(new_id.into());

    values.push(provider.id.into());

    values.push(update.upstream_id.into());

    let sql = format!(
        "UPDATE models SET {} WHERE provider_id = ? AND upstream_id = ?",
        assignments.join(", ")
    );

    transaction.execute(sql, params_from_iter(values)).await?;

    let entry = find_model_entry_in_transaction(&mut transaction, provider.id, update.upstream_id)
        .await?
        .ok_or_else(|| Error::Drift {
            message: "model vanished after override update".to_string(),
        })?;

    transaction.commit().await.map_err(|error| Error::Drift {
        message: format!("failed to commit model override update: {error}"),
    })?;

    Ok(entry)
}

/// Reads back one model row with its provider facts inside a transaction.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn find_model_entry_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    upstream_id: &str,
) -> Result<Option<ModelEntry>, Error> {
    let rows = transaction
        .query(
            "SELECT p.id, p.name, p.is_enabled, m.provider_id, m.upstream_id, m.id, \
             m.id_override, m.is_enabled, m.thinking_levels_override \
             FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE m.provider_id = ?1 AND m.upstream_id = ?2",
            params![provider_id, upstream_id],
        )
        .await?;

    rows.first()
        .map(|row| {
            Ok(ModelEntry {
                provider_id: row.get(0)?,
                provider_name: row.get(1)?,
                is_provider_enabled: row.get::<i64>(2)? != 0,
                model: model_from_row_offset(row, 3)?,
            })
        })
        .transpose()
}
