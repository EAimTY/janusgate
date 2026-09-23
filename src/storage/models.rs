//! Model row persistence: routing resolution, listing, discovery application, manual creation and
//! claiming, override updates, and deletion.
//!
//! Addressing follows one rule (README §7.1): a row that exists is addressed by its row `id` —
//! update and delete — while a row that does not exist yet is addressed by its natural key
//! `(provider_id, upstream_id)` — creation. Discovery addresses rows by their natural key inside
//! one provider's refresh.

use super::Error;
use crate::{
    database::{Database, Transaction, params},
    provider::{DiscoveryState, Model, ModelRouting},
};

/// One model row with the provider identity needed for listing.
#[derive(Debug, Clone)]
pub(crate) struct ModelEntry {
    /// Owning provider row identifier.
    pub(crate) provider_id: i64,

    /// Owning provider name.
    pub(crate) provider_name: String,

    /// The model row.
    pub(crate) model: Model,
}

/// One `models:create` insertion about to be applied transactionally: the row's natural key plus
/// its complete desired initial state (README §7.1) — every writable field total, `id_override`
/// mirroring its nullable storage column as `None` for "no override".
pub(crate) struct ModelCreate<'a> {
    /// Owning provider's id, part of the lookup key.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier, the other half of the lookup key.
    pub(crate) upstream_id: &'a str,

    /// Public ID override; `None` composes the ID from the provider's prefix.
    pub(crate) id_override: Option<&'a str>,

    /// Serving state.
    pub(crate) is_enabled: bool,
}

/// One `models:update` rewrite about to be applied transactionally: addressed by the row
/// identifier, carrying the route's two writable columns (README §7.1) — `id_override` mirroring
/// its nullable storage column as `None` for "no override". The update never writes
/// `discovery_state`, so it cannot restore a vanished row.
pub(crate) struct ModelUpdate<'a> {
    /// Addressed model row identifier.
    pub(crate) model_id: i64,

    /// Public ID override; `None` composes the ID from the provider's prefix.
    pub(crate) id_override: Option<&'a str>,

    /// Serving state.
    pub(crate) is_enabled: bool,
}

/// Resolves a public model ID to its provider and model row.
///
/// Resolution is zero-special-casing: a `vanished` row is looked up exactly like any other, and
/// its `is_effectively_enabled` — the three-input definition — decides whether routing serves it
/// (README §6.5).
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
         JOIN providers p ON p.id = m.provider_id WHERE m.public_id = ?1",
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

/// Fetches every model row of one provider inside a transaction, ordered by upstream identifier.
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

/// Fetches one model row of a provider by its natural key inside a transaction.
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

/// Fetches one model row joined with its provider by the row's identifier inside a transaction —
/// the lookup behind the row-addressed `models:update`.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn find_provider_model_in_transaction(
    transaction: &mut Transaction,
    model_id: i64,
) -> Result<Option<super::ProviderModelJoin>, Error> {
    let rows = transaction
        .query(
            format!(
                "SELECT {} FROM models m \
                 JOIN providers p ON p.id = m.provider_id WHERE m.id = ?1",
                super::PROVIDER_MODEL_COLUMNS
            ),
            params![model_id],
        )
        .await?;

    rows.first().map(super::provider_model_from_row).transpose()
}

/// Reads back one model row with its provider facts by row id inside a transaction.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn find_model_entry_by_id_in_transaction(
    transaction: &mut Transaction,
    model_id: i64,
) -> Result<Option<ModelEntry>, Error> {
    let rows = transaction
        .query(
            format!(
                "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id \
                 WHERE m.id = ?1",
                super::MODEL_ENTRY_COLUMNS
            ),
            params![model_id],
        )
        .await?;

    rows.first().map(super::model_entry_from_row).transpose()
}

/// Reads back one model row with its provider facts by natural key inside a transaction.
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

/// Lists the stored models with their provider identity, narrowed to one provider's rows when
/// `provider_id` is `Some`; an unknown provider yields an empty list, because a filter addresses no
/// row (README §7.1). Every row is listed — `vanished` included — ordered by public ID; the row's
/// own inputs let the caller derive the three-input status (rule 7), which tells serving rows
/// apart.
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
            "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id \
             ORDER BY m.public_id",
            super::MODEL_ENTRY_COLUMNS
        ),

        Some(_) => format!(
            "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE m.provider_id = ?1 ORDER BY m.public_id",
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
/// ID — the source of the outward `GET /api/openai-v1/models` listing.
///
/// A model is effectively enabled if and only if the one three-input definition holds
/// (README §3/§6.5): the owning provider is enabled, the model itself is enabled, and the row is
/// not `vanished`.
///
/// # Errors
///
/// Returns [`Error`] when the query fails.
pub(crate) async fn list_enabled_models_with_owner(
    database: &Database,
) -> Result<Vec<(String, String)>, Error> {
    let rows = database
        .query(
            "SELECT m.public_id, p.name FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE p.is_enabled != 0 AND m.is_enabled != 0 \
             AND m.discovery_state != 'vanished' ORDER BY m.public_id",
            (),
        )
        .await?;

    rows.iter()
        .map(|row| Ok((row.get::<String>(0)?, row.get::<String>(1)?)))
        .collect()
}

/// Applies a freshly discovered identifier set to one provider's rows inside a single transaction —
/// mark plus upsert — returning all of the provider's rows as the transaction left them, ordered
/// by public ID.
///
/// The algorithm, in order:
/// 1. Re-read the provider row (unknown provider is 404); the composed public IDs use its current
///    prefix even when a concurrent `providers:update` changed it after the caller's outer read.
/// 2. Mark every `discovered` row of the provider `vanished` — including rows about to return; they
///    are flipped back by the upsert below, so the blanket mark avoids excluding them one by one. A
///    `manual` row is never marked.
/// 3. Upsert each incoming identifier: an existing row — of any state — has only its
///    `discovery_state` flipped to `discovered` (a listed `manual` row flips too), every other
///    column left untouched; an absent row is inserted fresh: `discovered`, `is_enabled = 1`, no
///    override, `public_id = model_prefix + upstream_id`.
/// 4. A freshly composed public ID is collision-checked against every row of the table — `vanished`
///    and `manual` holders and `id_override` holders alike reserve their IDs — and a collision
///    rejects the whole refresh with 409, writing nothing. Batch-internal pairwise checks are
///    unnecessary: new rows carry no override, and the incoming identifiers are distinct, so
///    composed IDs cannot converge within one batch. `providers:update` keeps its pairwise check.
///
/// The write-permission invariant holds by construction (README §3): for existing rows discovery
/// writes only `discovery_state` — never `is_enabled`, `id_override`, `id`, or `public_id` — and
/// never deletes a row; the entire carry-over machinery is gone because untouched rows simply keep
/// their state. The vanished rows' "service suspended" effect derives from the three-input
/// effective status, not from any switch being written.
///
/// `upstream_ids` must be non-empty: rejecting an empty discovered set is the caller's
/// precondition, enforced by the management surface with a 502 before this function is reached
/// (README §7.1). This function performs no emptiness check — an empty set would mark the whole
/// provider's rows `vanished` and return them all as such — because an upstream that legitimately
/// reports no models, a fresh pre-onboarded account, or an outage answering 200 with an empty
/// payload must fail loudly instead.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, failed statements, or a composed public ID
/// collision.
pub(crate) async fn apply_discovered_models(
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

    // Blanket mark: every currently-discovered row not listed below ends up `vanished`.
    transaction
        .execute(
            "UPDATE models SET discovery_state = 'vanished' \
             WHERE provider_id = ?1 AND discovery_state = 'discovered'",
            params![provider.id],
        )
        .await?;

    for upstream_id in upstream_ids {
        if let Some(existing) =
            find_model_in_transaction(&mut transaction, provider.id, upstream_id).await?
        {
            // The row exists: flip only its state back to `discovered`.
            transaction
                .execute(
                    "UPDATE models SET discovery_state = 'discovered' WHERE id = ?1",
                    params![existing.id],
                )
                .await?;

            continue;
        }

        // The row is new: compose its public ID and reject a collision against any holder —
        // `vanished`/`manual` rows and `id_override` holders reserve their IDs — before inserting.
        let public_id = Model::public_id(&provider.model_prefix, upstream_id, None);

        super::ensure_ids_routable(
            &mut transaction,
            provider.id,
            &[(public_id.as_str(), upstream_id.as_str())],
        )
        .await?;

        transaction
            .execute(
                "INSERT INTO models (provider_id, upstream_id, is_enabled, discovery_state, \
                 public_id, id_override) VALUES (?1, ?2, 1, 'discovered', ?3, NULL)",
                params![provider.id, upstream_id.clone(), public_id],
            )
            .await?;
    }

    // The response carries every row of the provider as this transaction left them.
    let rows = transaction
        .query(
            format!(
                "SELECT {} FROM models m JOIN providers p ON p.id = m.provider_id \
                 WHERE m.provider_id = ?1 ORDER BY m.public_id",
                super::MODEL_ENTRY_COLUMNS
            ),
            params![provider.id],
        )
        .await?;

    let mut entries = Vec::with_capacity(rows.len());

    for row in &rows {
        entries.push(super::model_entry_from_row(row)?);
    }

    transaction.commit().await?;

    Ok(entries)
}

/// Inserts one manually created model row — or claims a vanished one — and returns the applied row
/// with its provider facts, inside a single transaction.
///
/// Three outcomes, addressed by the natural key `(provider_id, upstream_id)`:
/// - **Absent** → a brand-new `manual` row: the requested state applies and the public ID composes
///   from the provider's current prefix (the provider row is re-read inside the transaction, so a
///   concurrent `providers:update` cannot be written through).
/// - **`vanished` row** → claimed (README §7.1): the row identity (`id`) is kept, the requested
///   `id_override`/`is_enabled` apply, the public ID recomposes from the requested values, the
///   state becomes `manual`. This is one of the two manual recovery paths for a vanished row.
/// - **`discovered` or `manual` row** → 409, nothing written.
///
/// The public ID collision check matches [`update_model_overrides_checked`]'s quality tier: an ID
/// already routed by any other model row yields a 409 naming the holder, and nothing is written.
/// An unknown provider yields 404. The table's unique constraints are the concurrent backstop
/// behind both checked conflicts.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, an already-listed model, failed statements, or a
/// public ID collision.
pub(crate) async fn insert_model_checked(
    database: &Database,
    create: ModelCreate<'_>,
) -> Result<ModelEntry, Error> {
    let mut transaction = database.transaction().await?;

    let Some(provider) =
        super::find_provider_by_id_in_transaction(&mut transaction, create.provider_id).await?
    else {
        return Err(Error::NotFound {
            message: super::provider_not_found_message(create.provider_id),
        });
    };

    let stored =
        find_model_in_transaction(&mut transaction, provider.id, create.upstream_id).await?;

    let public_id = Model::public_id(
        &provider.model_prefix,
        create.upstream_id,
        create.id_override,
    );

    match stored {
        None => {
            super::ensure_ids_routable(
                &mut transaction,
                provider.id,
                &[(public_id.as_str(), create.upstream_id)],
            )
            .await?;

            transaction
                .execute(
                    "INSERT INTO models (provider_id, upstream_id, is_enabled, discovery_state, \
                     public_id, id_override) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        provider.id,
                        create.upstream_id,
                        i64::from(create.is_enabled),
                        DiscoveryState::Manual,
                        public_id,
                        create.id_override,
                    ],
                )
                .await?;
        }

        Some(existing) if existing.discovery_state == DiscoveryState::Vanished => {
            super::ensure_ids_routable(
                &mut transaction,
                provider.id,
                &[(public_id.as_str(), create.upstream_id)],
            )
            .await?;

            transaction
                .execute(
                    "UPDATE models SET id_override = ?1, is_enabled = ?2, public_id = ?3, \
                     discovery_state = ?4 WHERE id = ?5",
                    params![
                        create.id_override,
                        i64::from(create.is_enabled),
                        public_id,
                        DiscoveryState::Manual,
                        existing.id,
                    ],
                )
                .await?;
        }

        Some(_) => {
            return Err(Error::Conflict {
                message: format!(
                    "Model `{}` already exists on provider `{}`",
                    create.upstream_id, provider.name
                ),
            });
        }
    }

    // The read-back entry carries the provider facts the response renders, observed on the
    // write's own snapshot.
    let entry = find_model_entry_in_transaction(&mut transaction, provider.id, create.upstream_id)
        .await?
        .ok_or_else(|| Error::Drift {
            message: "model vanished after insert".to_string(),
        })?;

    transaction.commit().await?;

    Ok(entry)
}

/// Rewrites one model row's override columns and recomputes its public ID inside a single
/// transaction, returning the updated row with its provider facts.
///
/// The row is addressed by its `model_id`; the provider row it joins supplies the prefix, re-read
/// inside the transaction so the recomposed public ID uses the current prefix even when a
/// concurrent `providers:update` changed it after the caller's outer read. Only `id_override` and
/// `is_enabled` are written — `discovery_state` is never touched, so this route cannot restore a
/// vanished row (README §7.1). The collision check matches [`super::update_provider`]'s quality
/// tier: an ID already routed by another model row yields a 409 naming the holder, and nothing is
/// written. An unknown row yields 404.
///
/// # Errors
///
/// Returns [`Error`] for an unknown row, failed statements, or a public ID collision.
pub(crate) async fn update_model_overrides_checked(
    database: &Database,
    update: ModelUpdate<'_>,
) -> Result<ModelEntry, Error> {
    let mut transaction = database.transaction().await?;

    let Some(joined) =
        find_provider_model_in_transaction(&mut transaction, update.model_id).await?
    else {
        return Err(Error::NotFound {
            message: super::model_not_found_message(update.model_id),
        });
    };

    let public_id = Model::public_id(
        &joined.provider.model_prefix,
        &joined.model.upstream_id,
        update.id_override,
    );

    super::ensure_ids_routable(
        &mut transaction,
        joined.provider.id,
        &[(public_id.as_str(), joined.model.upstream_id.as_str())],
    )
    .await?;

    transaction
        .execute(
            "UPDATE models SET id_override = ?1, is_enabled = ?2, public_id = ?3 WHERE id = ?4",
            params![
                update.id_override,
                i64::from(update.is_enabled),
                public_id,
                update.model_id,
            ],
        )
        .await?;

    // The read-back entry carries the provider facts the response renders, observed on the
    // write's own snapshot.
    let entry = find_model_entry_by_id_in_transaction(&mut transaction, update.model_id)
        .await?
        .ok_or_else(|| Error::Drift {
            message: "model vanished after override update".to_string(),
        })?;

    transaction.commit().await?;

    Ok(entry)
}

/// Deletes one model row by its identifier inside a single transaction.
///
/// Deletion is administrator-only (`models:delete`); discovery never deletes a row. A deleted
/// `discovered` row is rebuilt by the next discovery as a brand-new row with a fresh,
/// never-recycled identity.
///
/// # Errors
///
/// Returns [`Error`] for an unknown row or a failed statement.
pub(crate) async fn delete_model_checked(database: &Database, model_id: i64) -> Result<(), Error> {
    let mut transaction = database.transaction().await?;

    let changes = transaction
        .execute("DELETE FROM models WHERE id = ?1", params![model_id])
        .await?;

    if changes == 0 {
        return Err(Error::NotFound {
            message: super::model_not_found_message(model_id),
        });
    }

    transaction.commit().await?;

    Ok(())
}
