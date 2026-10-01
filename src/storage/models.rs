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
    ir::IrReasoningLevel,
    provider::{DiscoveredModel, DiscoveryState, Model, ModelRouting},
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
/// and the three capability overrides mirroring their nullable storage columns as `None` for "no
/// override". The capability base columns are not the request's to state: a manual row starts
/// unknown (`NULL`, README §3).
pub(crate) struct ModelCreate<'a> {
    /// Owning provider's id, part of the lookup key.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier, the other half of the lookup key.
    pub(crate) upstream_id: &'a str,

    /// Public ID override; `None` composes the ID from the provider's prefix.
    pub(crate) id_override: Option<&'a str>,

    /// Serving state.
    pub(crate) is_enabled: bool,

    /// Thinking-level override; `None` keeps the (unknown, for a fresh row) discovered value.
    pub(crate) thinking_levels_override: Option<&'a [IrReasoningLevel]>,

    /// Maximum-input-tokens override; `None` keeps the discovered value.
    pub(crate) max_input_tokens_override: Option<i64>,

    /// Maximum-output-tokens override; `None` keeps the discovered value.
    pub(crate) max_output_tokens_override: Option<i64>,
}

/// One `models:update` rewrite about to be applied transactionally: addressed by the row
/// identifier, carrying the route's five writable columns (README §7.1) — `id_override` and the
/// three capability overrides mirroring their nullable storage columns as `None` for "no
/// override". The update never writes `discovery_state` or the capability base columns, so it
/// cannot restore a vanished row.
pub(crate) struct ModelUpdate<'a> {
    /// Addressed model row identifier.
    pub(crate) model_id: i64,

    /// Public ID override; `None` composes the ID from the provider's prefix.
    pub(crate) id_override: Option<&'a str>,

    /// Serving state.
    pub(crate) is_enabled: bool,

    /// Thinking-level override; `None` keeps the discovered value.
    pub(crate) thinking_levels_override: Option<&'a [IrReasoningLevel]>,

    /// Maximum-input-tokens override; `None` keeps the discovered value.
    pub(crate) max_input_tokens_override: Option<i64>,

    /// Maximum-output-tokens override; `None` keeps the discovered value.
    pub(crate) max_output_tokens_override: Option<i64>,
}

/// One effectively enabled model with its effective capability declarations — the outward
/// listings' entry (README §6.5): every value is the effective `override ?? base` composition,
/// and an unknown (`None`) capability is omitted by every listing rather than fabricated.
#[derive(Debug, Clone)]
pub(crate) struct EnabledModel {
    /// Public routing ID.
    pub(crate) public_id: String,

    /// Owning provider's current name.
    pub(crate) provider_name: String,

    /// Effective thinking levels; `None` when unknown.
    pub(crate) thinking_levels: Option<Vec<IrReasoningLevel>>,

    /// Effective maximum input tokens; `None` when unknown.
    pub(crate) max_input_tokens: Option<i64>,

    /// Effective maximum output tokens; `None` when unknown.
    pub(crate) max_output_tokens: Option<i64>,
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
        "SELECT {}, {} {} WHERE m.public_id = ?1",
        super::PROVIDER_COLUMNS,
        super::MODEL_COLUMNS_JOINED,
        super::PROVIDER_JOINED_FROM_MODELS
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
                "SELECT {}, {} {} WHERE m.id = ?1",
                super::PROVIDER_COLUMNS,
                super::MODEL_COLUMNS_JOINED,
                super::PROVIDER_JOINED_FROM_MODELS
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
                "SELECT {} {} WHERE m.id = ?1",
                super::MODEL_ENTRY_COLUMNS,
                super::MODELS_JOINED_PROVIDERS
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
                "SELECT {} {} WHERE m.provider_id = ?1 AND m.upstream_id = ?2",
                super::MODEL_ENTRY_COLUMNS,
                super::MODELS_JOINED_PROVIDERS
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
            "SELECT {} {} ORDER BY m.public_id",
            super::MODEL_ENTRY_COLUMNS,
            super::MODELS_JOINED_PROVIDERS
        ),

        Some(_) => format!(
            "SELECT {} {} WHERE m.provider_id = ?1 ORDER BY m.public_id",
            super::MODEL_ENTRY_COLUMNS,
            super::MODELS_JOINED_PROVIDERS
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

/// Lists the effectively enabled models with their effective capability declarations — the
/// source of every outward `GET …/models` listing (README §6.5).
///
/// A model is effectively enabled if and only if the one three-input definition holds
/// (README §3/§6.5): the owning provider is enabled, the model itself is enabled, and the row is
/// not `vanished`. Each entry's capability values are the effective `override ?? base`
/// composition — the declaration a listing renders, with `None` (unknown) omitted by the
/// renderer, never fabricated.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_enabled_models_with_owner(
    database: &Database,
) -> Result<Vec<EnabledModel>, Error> {
    let rows = database
        .query(
            format!(
                "SELECT m.public_id, p.name, {} {} WHERE p.is_enabled != 0 AND m.is_enabled != 0 \
                 AND m.discovery_state != ?1 ORDER BY m.public_id",
                super::MODEL_COLUMNS_JOINED,
                super::MODELS_JOINED_PROVIDERS
            ),
            params![DiscoveryState::Vanished],
        )
        .await?;

    rows.iter()
        .map(|row| {
            let model = super::model_from_row_offset(row, 2)?;

            Ok(EnabledModel {
                public_id: row.get(0)?,
                provider_name: row.get(1)?,
                thinking_levels: model
                    .effective_thinking_levels()
                    .map(<[IrReasoningLevel]>::to_vec),
                max_input_tokens: model.effective_max_input_tokens(),
                max_output_tokens: model.effective_max_output_tokens(),
            })
        })
        .collect()
}

/// Applies a freshly discovered model set to one provider's rows inside a single transaction —
/// mark plus upsert — returning all of the provider's rows as the transaction left them, ordered
/// by public ID.
///
/// The algorithm, in order:
/// 1. Re-read the provider row (unknown provider is 404); the composed public IDs use its current
///    prefix even when a concurrent `providers:update` changed it after the caller's outer read.
/// 2. Mark every `discovered` row of the provider `vanished` — including rows about to return; they
///    are flipped back by the upsert below, so the blanket mark avoids excluding them one by one. A
///    `manual` row is never marked.
/// 3. Upsert each incoming model: an existing row — of any state — has its `discovery_state`
///    flipped to `discovered` and its three capability base columns rewritten to this refresh's
///    extracted facts (README §5.5; a listed `manual` row flips and rewrites too), every other
///    column left untouched; an absent row is inserted fresh: `discovered`, `is_enabled = 1`, no
///    override, the extracted capability bases (`NULL` when nothing was extracted), `public_id =
///    model_prefix + upstream_id`.
/// 4. A freshly composed public ID is collision-checked against every row of the table — `vanished`
///    and `manual` holders and `id_override` holders alike reserve their IDs — and a collision
///    rejects the whole refresh with 409, writing nothing. Batch-internal pairwise checks are
///    unnecessary: new rows carry no override, and the incoming identifiers are distinct, so
///    composed IDs cannot converge within one batch. `providers:update` keeps its pairwise check.
///
/// The write-permission invariant holds by construction (README §3): for existing rows discovery
/// writes only `discovery_state` and the three capability base columns — never `is_enabled`,
/// `id_override`, `id`, `public_id`, or a capability override — and never deletes a row; the entire
/// carry-over machinery is gone because untouched rows simply keep their state. The vanished rows'
/// "service suspended" effect derives from the three-input effective status, not from any switch
/// being written.
///
/// `discovered` must be non-empty: rejecting an empty discovered set is the caller's
/// precondition, enforced by the management surface with a 502 before this function is reached
/// (README §7.1). This function performs no emptiness check — an empty set would mark the whole
/// provider's rows `vanished` and return them all as such — because an upstream that legitimately
/// reports no models, a fresh pre-onboarded account, or an outage answering 200 with an empty
/// payload must fail loudly instead.
///
/// The transaction runs under the storage tier's `in_retryable_transaction` bounded
/// stale-snapshot retry: a concurrent commit that invalidates the snapshot between the re-read
/// and the first write re-runs the whole mark-plus-upsert against the newer state instead of
/// surfacing the refusal.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, failed statements, or a composed public ID
/// collision.
pub(crate) async fn apply_discovered_models(
    database: &Database,
    provider_id: i64,
    discovered: &[DiscoveredModel],
) -> Result<Vec<ModelEntry>, Error> {
    in_retryable_transaction!(database, transaction, async {
        let Some(provider) =
            super::providers::find_provider_by_id_in_transaction(&mut transaction, provider_id)
                .await?
        else {
            return Err(Error::NotFound {
                message: super::provider_not_found_message(provider_id),
            });
        };

        // Blanket mark: every currently-discovered row not listed below ends up `vanished`.
        transaction
            .execute(
                "UPDATE models SET discovery_state = ?2 \
                 WHERE provider_id = ?1 AND discovery_state = ?3",
                params![
                    provider.id,
                    DiscoveryState::Vanished,
                    DiscoveryState::Discovered
                ],
            )
            .await?;

        // One routed snapshot serves every collision check of the loop below. A check consults
        // the map only by the candidate's public ID, and nothing the loop writes between
        // iterations can enter that set: the state flips and capability rewrites touch no routing
        // column, and a fresh insert composes `model_prefix + upstream_id`, which equals a later
        // candidate's composed ID only for the same upstream identifier — but that occurrence
        // finds the just-inserted row through `find_model_in_transaction` and never reaches the
        // check. Batch-internal convergence is impossible anyway (distinct upstream identifiers,
        // new rows carry no override — step 4 of the doc above), so the pre-loop snapshot is exact,
        // not an approximation.
        let routed = super::read_routed_holders_in_transaction(&mut transaction).await?;

        for model in discovered {
            if let Some(existing) =
                find_model_in_transaction(&mut transaction, provider.id, &model.upstream_id).await?
            {
                // The row exists: flip its state back to `discovered` and rewrite its capability
                // base columns to this refresh's facts — `NULL` for a fact this refresh did not
                // extract, so a stored value is always the latest upstream knowledge, never stale
                // residue (README §3).
                transaction
                    .execute(
                        "UPDATE models SET discovery_state = ?2, thinking_levels = ?3, \
                         max_input_tokens = ?4, max_output_tokens = ?5 WHERE id = ?1",
                        params![
                            existing.id,
                            DiscoveryState::Discovered,
                            super::levels_into_value(model.thinking_levels.as_deref())?,
                            model.max_input_tokens,
                            model.max_output_tokens,
                        ],
                    )
                    .await?;

                continue;
            }

            // The row is new: compose its public ID and reject a collision against any holder —
            // `vanished`/`manual` rows and `id_override` holders reserve their IDs — before
            // inserting.
            let public_id = Model::public_id(&provider.model_prefix, &model.upstream_id, None);

            super::ensure_id_routable_against(
                &routed,
                provider.id,
                &public_id,
                &model.upstream_id,
            )?;

            transaction
                .execute(
                    "INSERT INTO models (provider_id, upstream_id, is_enabled, discovery_state, \
                     public_id, id_override, thinking_levels, max_input_tokens, max_output_tokens) \
                     VALUES (?1, ?2, 1, ?3, ?4, NULL, ?5, ?6, ?7)",
                    params![
                        provider.id,
                        model.upstream_id.clone(),
                        DiscoveryState::Discovered,
                        public_id,
                        super::levels_into_value(model.thinking_levels.as_deref())?,
                        model.max_input_tokens,
                        model.max_output_tokens,
                    ],
                )
                .await?;
        }

        // The response carries every row of the provider as this transaction left them.
        let rows = transaction
            .query(
                format!(
                    "SELECT {} {} WHERE m.provider_id = ?1 ORDER BY m.public_id",
                    super::MODEL_ENTRY_COLUMNS,
                    super::MODELS_JOINED_PROVIDERS
                ),
                params![provider.id],
            )
            .await?;

        let mut entries = Vec::with_capacity(rows.len());

        for row in &rows {
            entries.push(super::model_entry_from_row(row)?);
        }

        Ok(entries)
    })
}

/// Inserts one manually created model row — or claims a vanished one — and returns the applied row
/// with its provider facts, inside a single transaction.
///
/// Three outcomes, addressed by the natural key `(provider_id, upstream_id)`:
/// - **Absent** → a brand-new `manual` row: the requested state applies, the capability base
///   columns store `NULL` — a manual row starts unknown (README §3) — and the public ID composes
///   from the provider's current prefix (the provider row is re-read inside the transaction, so a
///   concurrent `providers:update` cannot be written through).
/// - **`vanished` row** → claimed (README §7.1): the row identity (`id`) is kept, the requested
///   `id_override`/`is_enabled`/capability overrides apply while the capability base columns keep
///   the last discovery's real facts, the public ID recomposes from the requested values, the state
///   becomes `manual`. This is one of the two manual recovery paths for a vanished row.
/// - **`discovered` or `manual` row** → 409, nothing written.
///
/// The public ID collision check matches [`update_model_overrides_checked`]'s quality tier: an ID
/// already routed by any other model row yields a 409 naming the holder, and nothing is written.
/// An unknown provider yields 404. The table's unique constraints are the concurrent backstop
/// behind both checked conflicts. The transaction runs under the storage tier's
/// `in_retryable_transaction` bounded stale-snapshot retry: a concurrent commit that invalidates
/// the snapshot between the reads and the write re-runs the whole read-then-write against the
/// newer state instead of surfacing the refusal.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, an already-listed model, failed statements, or a
/// public ID collision.
pub(crate) async fn insert_model_checked(
    database: &Database,
    create: ModelCreate<'_>,
) -> Result<ModelEntry, Error> {
    in_retryable_transaction!(database, transaction, async {
        let Some(provider) = super::providers::find_provider_by_id_in_transaction(
            &mut transaction,
            create.provider_id,
        )
        .await?
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
                super::ensure_id_routable_in_transaction(
                    &mut transaction,
                    provider.id,
                    &public_id,
                    create.upstream_id,
                )
                .await?;

                transaction
                    .execute(
                        "INSERT INTO models (provider_id, upstream_id, is_enabled, \
                         discovery_state, public_id, id_override, thinking_levels, \
                         max_input_tokens, max_output_tokens, thinking_levels_override, \
                         max_input_tokens_override, max_output_tokens_override) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, ?7, ?8, ?9)",
                        params![
                            provider.id,
                            create.upstream_id,
                            i64::from(create.is_enabled),
                            DiscoveryState::Manual,
                            public_id,
                            create.id_override,
                            super::levels_into_value(create.thinking_levels_override)?,
                            create.max_input_tokens_override,
                            create.max_output_tokens_override,
                        ],
                    )
                    .await?;
            }

            Some(existing) if existing.discovery_state == DiscoveryState::Vanished => {
                super::ensure_id_routable_in_transaction(
                    &mut transaction,
                    provider.id,
                    &public_id,
                    create.upstream_id,
                )
                .await?;

                // The claim writes the administrator-owned columns only: the capability base
                // columns keep the last discovery's facts, whatever they were (README §3).
                transaction
                    .execute(
                        "UPDATE models SET id_override = ?1, is_enabled = ?2, public_id = ?3, \
                         discovery_state = ?4, thinking_levels_override = ?5, \
                         max_input_tokens_override = ?6, max_output_tokens_override = ?7 \
                         WHERE id = ?8",
                        params![
                            create.id_override,
                            i64::from(create.is_enabled),
                            public_id,
                            DiscoveryState::Manual,
                            super::levels_into_value(create.thinking_levels_override)?,
                            create.max_input_tokens_override,
                            create.max_output_tokens_override,
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
        let entry =
            find_model_entry_in_transaction(&mut transaction, provider.id, create.upstream_id)
                .await?
                .ok_or_else(|| Error::Drift {
                    message: "model vanished after insert".to_string(),
                })?;

        Ok(entry)
    })
}

/// Rewrites one model row's administrator-owned columns — `id_override`, `is_enabled`, and the
/// three capability overrides — and recomputes its public ID inside a single transaction,
/// returning the updated row with its provider facts.
///
/// The row is addressed by its `model_id`; the provider row it joins supplies the prefix, re-read
/// inside the transaction so the recomposed public ID uses the current prefix even when a
/// concurrent `providers:update` changed it after the caller's outer read. Only the five
/// administrator-owned columns are written — `discovery_state` and the capability base columns
/// are never touched, so this route cannot restore a vanished row and cannot smuggle a capability
/// fact past the ownership boundary (README §7.1). The collision check matches
/// [`super::providers::update_provider`]'s quality tier: an ID already routed by another model row
/// yields a 409 naming the holder, and nothing is written. An unknown row yields 404. The
/// transaction runs under the storage tier's `in_retryable_transaction` bounded stale-snapshot
/// retry: a concurrent commit that invalidates the snapshot between the read and the write
/// re-runs the whole read-then-write against the newer state instead of surfacing the refusal.
///
/// # Errors
///
/// Returns [`Error`] for an unknown row, failed statements, or a public ID collision.
pub(crate) async fn update_model_overrides_checked(
    database: &Database,
    update: ModelUpdate<'_>,
) -> Result<ModelEntry, Error> {
    in_retryable_transaction!(database, transaction, async {
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

        super::ensure_id_routable_in_transaction(
            &mut transaction,
            joined.provider.id,
            &public_id,
            &joined.model.upstream_id,
        )
        .await?;

        transaction
            .execute(
                "UPDATE models SET id_override = ?1, is_enabled = ?2, public_id = ?3, \
                 thinking_levels_override = ?4, max_input_tokens_override = ?5, \
                 max_output_tokens_override = ?6 WHERE id = ?7",
                params![
                    update.id_override,
                    i64::from(update.is_enabled),
                    public_id,
                    super::levels_into_value(update.thinking_levels_override)?,
                    update.max_input_tokens_override,
                    update.max_output_tokens_override,
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

        Ok(entry)
    })
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

/// One discovered fixture carrying no capability facts: everything unknown (README §3) — the
/// shared test fixture of this module's and the storage tier's tests.
#[cfg(test)]
pub(super) fn discovered(upstream_id: &str) -> DiscoveredModel {
    DiscoveredModel {
        upstream_id: upstream_id.to_string(),
        thinking_levels: None,
        max_input_tokens: None,
        max_output_tokens: None,
    }
}

/// One discovered fixture carrying every capability fact: two canonicalized levels and both
/// token caps.
#[cfg(test)]
pub(super) fn discovered_with_capabilities(upstream_id: &str) -> DiscoveredModel {
    DiscoveredModel {
        upstream_id: upstream_id.to_string(),
        thinking_levels: Some(vec![IrReasoningLevel::Low, IrReasoningLevel::High]),
        max_input_tokens: Some(200_000),
        max_output_tokens: Some(32_768),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryState, Error, IrReasoningLevel, ModelCreate, ModelUpdate, apply_discovered_models,
        delete_model_checked, discovered, discovered_with_capabilities, find_routing,
        insert_model_checked, list_enabled_models_with_owner, list_model_entries,
        update_model_overrides_checked,
    };
    use crate::{
        storage::test_support::{entry, insert_test_provider},
        test_support,
    };
    use std::error::Error as StdError;

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
        let (database, _guard) = test_support::create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        let created = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "manual-1",
                id_override: None,
                is_enabled: false,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
            },
        )
        .await?;

        assert_eq!(created.model.public_id, "p1-manual-1");
        assert_eq!(created.model.discovery_state, DiscoveryState::Manual);
        assert!(!created.model.is_enabled);

        let duplicate = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "manual-1",
                id_override: Some("other-id"),
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
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

        let other_provider_id = insert_test_provider(&database, "provider-2", "p2-").await?;

        let colliding = insert_model_checked(
            &database,
            ModelCreate {
                provider_id: other_provider_id,
                upstream_id: "manual-1",
                id_override: Some("p1-manual-1"),
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
            },
        )
        .await;

        assert!(matches!(colliding, Err(Error::Conflict { .. })));
        assert!(
            list_model_entries(&database, Some(other_provider_id))
                .await?
                .is_empty()
        );

        let missing = insert_model_checked(
            &database,
            ModelCreate {
                provider_id: other_provider_id + 1000,
                upstream_id: "ghost",
                id_override: None,
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
            },
        )
        .await;

        assert!(matches!(missing, Err(Error::NotFound { .. })));

        let rows =
            apply_discovered_models(&database, provider_id, &[discovered("fresh-1")]).await?;

        assert_eq!(rows.len(), 2);

        let entries = list_model_entries(&database, Some(provider_id)).await?;
        let manual = entry(&entries, "manual-1");

        assert_eq!(manual.model.discovery_state, DiscoveryState::Manual);
        assert!(!manual.model.is_enabled);
        assert_eq!(manual.model.public_id, "p1-manual-1");

        let fresh = entry(&entries, "fresh-1");

        assert_eq!(fresh.model.discovery_state, DiscoveryState::Discovered);
        assert!(fresh.model.is_enabled);

        let manual_id = manual.model.id;

        apply_discovered_models(
            &database,
            provider_id,
            &[discovered("fresh-1"), discovered("manual-1")],
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

    /// Verifies the vanished semantics and the write-permission invariant, through the
    /// `discovered`/`vanished` state flip and its restore.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn marks_vanished_rows_and_restores_them_by_state_flip_only()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        apply_discovered_models(
            &database,
            provider_id,
            &[discovered("a"), discovered("b"), discovered("c")],
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let a_id = entry(&entries, "a").model.id;

        update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: a_id,
                id_override: Some("custom-a"),
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
            },
        )
        .await?;

        apply_discovered_models(&database, provider_id, &[discovered("a")]).await?;

        let entries = list_model_entries(&database, None).await?;
        let a = entry(&entries, "a");
        let c = entry(&entries, "c");

        assert_eq!(a.model.id, a_id);
        assert_eq!(a.model.discovery_state, DiscoveryState::Discovered);
        assert_eq!(a.model.public_id, "custom-a");

        assert_eq!(c.model.discovery_state, DiscoveryState::Vanished);
        assert!(c.model.is_enabled);
        assert_eq!(c.model.public_id, "p1-c");

        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the lookup follows it"
        )]
        let routing = find_routing(&database, "p1-c")
            .await?
            .expect("a vanished row is still resolved by its public ID");

        assert!(!routing.is_effectively_enabled);

        let enabled_models = list_enabled_models_with_owner(&database).await?;

        assert_eq!(enabled_models.len(), 1);

        apply_discovered_models(
            &database,
            provider_id,
            &[discovered("a"), discovered("b"), discovered("c")],
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let c = entry(&entries, "c");

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

        Ok(())
    }

    /// Verifies the row-addressed update and delete rules, and the identity rebuild of a deleted
    /// discovered row.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn updates_and_deletes_models_by_row_id() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        apply_discovered_models(&database, provider_id, &[discovered("a"), discovered("b")])
            .await?;

        let entries = list_model_entries(&database, None).await?;
        let a_id = entry(&entries, "a").model.id;

        let missing_delete = delete_model_checked(&database, a_id + 1000).await;

        assert!(matches!(missing_delete, Err(Error::NotFound { .. })));

        apply_discovered_models(&database, provider_id, &[discovered("b")]).await?;

        delete_model_checked(&database, a_id).await?;

        let entries = list_model_entries(&database, None).await?;

        assert_eq!(entries.len(), 1);

        let missing_delete = delete_model_checked(&database, a_id).await;

        assert!(matches!(missing_delete, Err(Error::NotFound { .. })));

        let rebuilt =
            apply_discovered_models(&database, provider_id, &[discovered("a"), discovered("b")])
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

    /// Verifies the capability-declaration lifecycle end to end (README §3): a discovery
    /// carrying facts writes the base columns of new and existing rows alike; a later
    /// capability-less refresh resets the base columns to `NULL` (unknown) instead of leaving
    /// stale facts, while the overrides survive — the two ownership classes never touch each
    /// other's columns; the enabled-model listing renders the effective `override ?? base`
    /// composition; a manual create stores `NULL` bases; and a claim of a vanished row keeps
    /// its base columns while applying the requested overrides.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn applies_capability_declarations_with_their_ownership_rules()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;
        let provider_id = insert_test_provider(&database, "provider-1", "p1-").await?;

        // A fresh insert carries the extracted facts; a fact-less entry stays unknown.
        apply_discovered_models(
            &database,
            provider_id,
            &[discovered_with_capabilities("a"), discovered("b")],
        )
        .await?;

        let entries = list_model_entries(&database, None).await?;
        let a = entry(&entries, "a");

        assert_eq!(
            a.model.thinking_levels,
            Some(vec![IrReasoningLevel::Low, IrReasoningLevel::High])
        );
        assert_eq!(a.model.max_input_tokens, Some(200_000));
        assert_eq!(a.model.max_output_tokens, Some(32_768));
        assert_eq!(entry(&entries, "b").model.thinking_levels, None);

        // An override replaces one field's declaration only; the others keep their bases.
        let a_id = a.model.id;

        update_model_overrides_checked(
            &database,
            ModelUpdate {
                model_id: a_id,
                id_override: None,
                is_enabled: true,
                thinking_levels_override: Some(&[IrReasoningLevel::Max]),
                max_input_tokens_override: None,
                max_output_tokens_override: Some(999_999),
            },
        )
        .await?;

        // The listing composes effective values: an overridden field wins, a base field rides
        // through, an unknown field renders `None` — the value every outward listing omits.
        let listed = list_enabled_models_with_owner(&database).await?;

        #[expect(
            clippy::expect_used,
            reason = "the row was just written and the listing follows it"
        )]
        let listed_a = listed
            .iter()
            .find(|model| model.public_id == "p1-a")
            .expect("the row is listed");

        assert_eq!(listed_a.thinking_levels, Some(vec![IrReasoningLevel::Max]));
        assert_eq!(listed_a.max_input_tokens, Some(200_000));
        assert_eq!(listed_a.max_output_tokens, Some(999_999));

        #[expect(
            clippy::expect_used,
            reason = "the fact-less row was just written and the listing follows it"
        )]
        let listed_b = listed
            .iter()
            .find(|model| model.public_id == "p1-b")
            .expect("the fact-less row is listed");

        assert_eq!(listed_b.thinking_levels, None);
        assert_eq!(listed_b.max_input_tokens, None);
        assert_eq!(listed_b.max_output_tokens, None);

        // A capability-less refresh resets the base columns to unknown; the overrides survive.
        apply_discovered_models(&database, provider_id, &[discovered("a"), discovered("b")])
            .await?;

        let entries = list_model_entries(&database, None).await?;
        let a = entry(&entries, "a");

        assert_eq!(a.model.thinking_levels, None);
        assert_eq!(a.model.max_input_tokens, None);
        assert_eq!(a.model.max_output_tokens, None);
        assert_eq!(
            a.model.thinking_levels_override,
            Some(vec![IrReasoningLevel::Max])
        );
        assert_eq!(a.model.max_output_tokens_override, Some(999_999));

        // Vanish `a`, then claim it: the claim keeps the base columns (here `NULL`) and applies
        // the requested overrides (README §7.1).
        apply_discovered_models(&database, provider_id, &[discovered("b")]).await?;

        let claimed = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "a",
                id_override: None,
                is_enabled: true,
                thinking_levels_override: Some(&[IrReasoningLevel::None]),
                max_input_tokens_override: Some(1_000_000),
                max_output_tokens_override: None,
            },
        )
        .await?;

        assert_eq!(claimed.model.discovery_state, DiscoveryState::Manual);
        assert_eq!(claimed.model.thinking_levels, None);
        assert_eq!(claimed.model.max_input_tokens, None);
        assert_eq!(
            claimed.model.thinking_levels_override,
            Some(vec![IrReasoningLevel::None])
        );
        assert_eq!(claimed.model.max_input_tokens_override, Some(1_000_000));

        // A manual create stores `NULL` bases while carrying its overrides.
        let manual = insert_model_checked(
            &database,
            ModelCreate {
                provider_id,
                upstream_id: "manual-x",
                id_override: None,
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: Some(8_192),
                max_output_tokens_override: None,
            },
        )
        .await?;

        assert_eq!(manual.model.thinking_levels, None);
        assert_eq!(manual.model.max_input_tokens, None);
        assert_eq!(manual.model.max_output_tokens, None);
        assert_eq!(manual.model.max_input_tokens_override, Some(8_192));

        Ok(())
    }
}
