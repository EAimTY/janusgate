//! Model row persistence: routing resolution, listing, discovery replacement, manual creation, and
//! override updates.

use super::Error;
use crate::{
    database::{Database, Transaction, params},
    provider::{Model, ModelRouting},
};
use std::collections::HashMap;

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

/// One `models:create` insertion or `models:update` rewrite about to be applied transactionally:
/// the row's complete desired override state (README §7.1) — every field total, the two override
/// columns mirroring their nullable storage as `None` for "no override".
pub(crate) struct ModelOverrideUpdate<'a> {
    /// Owning provider's id, the lookup key.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_id: &'a str,

    /// Public ID override; `None` composes the ID from the provider's prefix.
    pub(crate) id_override: Option<&'a str>,

    /// Serving state.
    pub(crate) is_enabled: bool,

    /// Canonical thinking levels override in its stored JSON form; `None` stores no override.
    pub(crate) thinking_levels_override: Option<&'a str>,
}

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
        "SELECT {} FROM models m \
         JOIN providers p ON p.id = m.provider_id WHERE m.id = ?1",
        super::PROVIDER_MODEL_COLUMNS
    );

    let rows = database.query(sql, params![public_id]).await?;

    rows.first()
        .map(|row| {
            super::provider_model_from_row(row).map(|joined| ModelRouting {
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
    let sql = format!(
        "SELECT {} FROM models WHERE provider_id = ?1 ORDER BY upstream_id",
        super::MODEL_COLUMNS
    );

    let rows = transaction.query(sql, params![provider_id]).await?;

    rows.iter().map(super::model_from_row).collect()
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
                "SELECT {} FROM models \
                 WHERE provider_id = ?1 AND upstream_id = ?2",
                super::MODEL_COLUMNS
            ),
            params![provider_id, upstream_id],
        )
        .await?;

    rows.first().map(super::model_from_row).transpose()
}

/// Lists the stored models with their provider facts, narrowed to one provider's rows when
/// `provider_id` is `Some`; an unknown provider yields an empty list, because a filter addresses no
/// row (README §7.1).
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_model_entries(
    database: &Database,
    provider_id: Option<i64>,
) -> Result<Vec<ModelEntry>, Error> {
    let sql = match provider_id {
        None => format!(
            "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id ORDER BY m.id",
            super::MODEL_ENTRY_COLUMNS
        ),

        Some(_) => format!(
            "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE m.provider_id = ?1 ORDER BY m.id",
            super::MODEL_ENTRY_COLUMNS
        ),
    };

    let rows = match provider_id {
        None => database.query(sql, ()).await?,
        Some(provider_id) => database.query(sql, params![provider_id]).await?,
    };

    let mut entries = Vec::with_capacity(rows.len());

    for row in &rows {
        entries.push(super::model_entry_from_row(row)?);
    }

    Ok(entries)
}

/// Lists `(public_id, provider_name)` pairs for every effectively enabled model, ordered by public
/// ID.
///
/// A model is effectively enabled if and only if both its owning provider and the model itself are
/// enabled (`p.is_enabled != 0 AND m.is_enabled != 0`)..
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

/// Replaces the model rows of one provider with a freshly discovered set inside a single
/// transaction, returning the stored rows with their provider facts.
///
/// `upstream_ids` must be non-empty: rejecting an empty discovered set is the caller's
/// precondition, enforced by the management surface with a 502 before this function is reached
/// (README §7.1). This function performs no emptiness check — an empty set would irreversibly wipe
/// the provider's curated rows and return an empty list — because an upstream that legitimately
/// reports no models, a fresh pre-onboarded account, or an outage answering 200 with an empty
/// payload must fail loudly instead.
///
/// The provider row is re-read inside the transaction, so the public IDs are composed from the
/// provider's current prefix even when a concurrent `providers:update` changed it after the
/// caller's outer read (README §7.1). Overrides carry over from the stored rows by `upstream_id`;
/// discovered models absent from the stored set vanish with their overrides.
///
/// The composed public IDs undergo the same collision checks as [`super::update_provider`]:
/// pairwise distinctness within the provider and no routing collision with any other model row;
/// both rejections are ID-specific 409 conflicts that write nothing.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, failed statements, or a
/// composed public ID collision.
pub(crate) async fn replace_discovered_models(
    database: &Database,
    provider_id: i64,
    upstream_ids: &[String],
) -> Result<Vec<ModelEntry>, Error> {
    let mut transaction = database.transaction().await?;

    let Some(provider) =
        super::find_provider_by_id_in_transaction(&mut transaction, provider_id).await?
    else {
        return Err(Error::NotFound {
            message: super::provider_not_found_message(provider_id),
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

    super::ensure_ids_pairwise_distinct(&provider.name, &composed)?;

    super::ensure_ids_routable(&mut transaction, provider.id, &composed).await?;

    replace_provider_models(&mut transaction, provider.id, &models).await?;

    transaction.commit().await?;

    // The returned entries mirror exactly what this transaction wrote.
    Ok(models
        .iter()
        .map(|model| ModelEntry {
            provider_id: provider.id,
            provider_name: provider.name.clone(),
            is_provider_enabled: provider.is_enabled,
            model: model.clone(),
        })
        .collect())
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

/// Inserts one manually created model row and composes its public ID inside a single transaction,
/// returning the stored row with its provider facts.
///
/// The inserted row is indistinguishable from a discovered row: a later discovery carries its
/// overrides over by `upstream_id` when the identifier reappears, and drops the row when it does
/// not (README §7.1).
///
/// The provider row is re-read inside the transaction, so the composed public ID uses the
/// provider's current prefix even when a concurrent `providers:update` changed it after the
/// caller's outer read. The collision check matches [`update_model_overrides_checked`]'s quality
/// tier: an `upstream_id` the provider already holds and an ID already routed by another model
/// row each yield a 409 naming the holder, and nothing is written. An unknown provider yields 404.
/// The table's primary key and unique ID are the concurrent backstop behind both checked
/// conflicts.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, an already-stored model,
/// failed statements, or a public ID collision.
pub(crate) async fn insert_model_checked(
    database: &Database,
    update: ModelOverrideUpdate<'_>,
) -> Result<ModelEntry, Error> {
    let mut transaction = database.transaction().await?;

    let Some(provider) =
        super::find_provider_by_id_in_transaction(&mut transaction, update.provider_id).await?
    else {
        return Err(Error::NotFound {
            message: super::provider_not_found_message(update.provider_id),
        });
    };

    if find_model_in_transaction(&mut transaction, provider.id, update.upstream_id)
        .await?
        .is_some()
    {
        return Err(Error::Conflict {
            message: format!(
                "Model `{}` already exists on provider `{}`",
                update.upstream_id, provider.name
            ),
        });
    }

    let new_id = Model::public_id(
        &provider.model_prefix,
        update.upstream_id,
        update.id_override,
    );

    super::ensure_ids_routable(
        &mut transaction,
        provider.id,
        &[(new_id.as_str(), update.upstream_id)],
    )
    .await?;

    transaction
        .execute(
            "INSERT INTO models (provider_id, upstream_id, id, id_override, \
             is_enabled, thinking_levels_override) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                provider.id,
                update.upstream_id,
                new_id,
                update.id_override,
                i64::from(update.is_enabled),
                update.thinking_levels_override,
            ],
        )
        .await?;

    // The read-back entry carries the provider facts the response renders,
    // observed on the write's own snapshot.
    let entry = find_model_entry_in_transaction(&mut transaction, provider.id, update.upstream_id)
        .await?
        .ok_or_else(|| Error::Drift {
            message: "model vanished after insert".to_string(),
        })?;

    transaction.commit().await?;

    Ok(entry)
}

/// Rewrites one model's override state and recomputes its public ID inside a single transaction,
/// returning the updated row with its provider facts.
///
/// The provider row is re-read inside the transaction, so the recomputed public ID uses the
/// provider's current prefix even when a concurrent `providers:update` changed it after the
/// caller's outer read; the stored model row and the routed-ID collision check observe the same
/// snapshot. The collision check matches [`super::update_provider`]'s quality tier: an ID already
/// routed by another model row yields a 409 naming the holder, and nothing is written. Unknown
/// providers or models yield 404.
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
        super::find_provider_by_id_in_transaction(&mut transaction, update.provider_id).await?
    else {
        return Err(Error::NotFound {
            message: super::provider_not_found_message(update.provider_id),
        });
    };

    // The stored row's existence is the only fact needed: every override
    // column is restated by the rewrite.
    if find_model_in_transaction(&mut transaction, provider.id, update.upstream_id)
        .await?
        .is_none()
    {
        return Err(Error::NotFound {
            message: format!(
                "Model `{}` does not exist on provider `{}`",
                update.upstream_id, provider.name
            ),
        });
    }

    let new_id = Model::public_id(
        &provider.model_prefix,
        update.upstream_id,
        update.id_override,
    );

    super::ensure_ids_routable(
        &mut transaction,
        provider.id,
        &[(new_id.as_str(), update.upstream_id)],
    )
    .await?;

    transaction
        .execute(
            "UPDATE models SET id_override = ?1, is_enabled = ?2, \
             thinking_levels_override = ?3, id = ?4 \
             WHERE provider_id = ?5 AND upstream_id = ?6",
            params![
                update.id_override,
                i64::from(update.is_enabled),
                update.thinking_levels_override,
                new_id,
                provider.id,
                update.upstream_id,
            ],
        )
        .await?;

    // The read-back entry carries the provider facts the response renders,
    // observed on the write's own snapshot.
    let entry = find_model_entry_in_transaction(&mut transaction, provider.id, update.upstream_id)
        .await?
        .ok_or_else(|| Error::Drift {
            message: "model vanished after override update".to_string(),
        })?;

    transaction.commit().await?;

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
            format!(
                "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id \
                 WHERE m.provider_id = ?1 AND m.upstream_id = ?2",
                super::MODEL_ENTRY_COLUMNS
            ),
            params![provider_id, upstream_id],
        )
        .await?;

    rows.first().map(super::model_entry_from_row).transpose()
}
