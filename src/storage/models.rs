//! Model row persistence: routing resolution, listing, discovery application, manual creation and
//! claiming, override updates, and deletion.
//!
//! Addressing follows one rule (README §7.1): a row that exists is addressed by its row `id` —
//! update and delete — while a row that does not exist yet is addressed by its natural key
//! `(provider_id, upstream_id)` — creation. Discovery addresses rows by their natural key inside
//! one provider's refresh.

use super::Error;
use crate::{
    compaction_model::support_facts::CompactionSupportFacts,
    database::{Database, Row, Transaction, TursoError, Value, params, params::IntoValue},
    provider::{DiscoveredModel, DiscoveryState, Model, Provider},
    vocabulary::ReasoningLevel,
};
use std::sync::LazyLock;

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

/// A physical model's complete initial administrator-owned state. Manual base facts start unknown.
pub(crate) struct ModelCreate<'a> {
    /// Owning provider's id, part of the lookup key.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier, the other half of the lookup key.
    pub(crate) upstream_id: &'a str,

    /// Serving state.
    pub(crate) is_enabled: bool,

    /// Thinking-level override; `None` keeps the (unknown, for a fresh row) discovered value.
    pub(crate) thinking_levels_override: Option<&'a [ReasoningLevel]>,

    /// Maximum-input-tokens override; `None` keeps the discovered value.
    pub(crate) max_input_tokens_override: Option<i64>,

    /// Maximum-output-tokens override; `None` keeps the discovered value.
    pub(crate) max_output_tokens_override: Option<i64>,

    /// Compaction support facts override; `None` keeps the (unknown, for a fresh row)
    /// discovered facts. The map carries only `supported`/`unsupported`/`unknown` members,
    /// canonicalized at the validated write boundary.
    pub(crate) compaction_support_override: Option<&'a CompactionSupportFacts>,
}

/// A row-addressed rewrite of the physical serving switch and capability overrides.
/// State and discovered base facts are never changed, so an update cannot restore a vanished row.
pub(crate) struct ModelUpdate<'a> {
    /// Addressed model row identifier.
    pub(crate) model_id: i64,

    /// Serving state.
    pub(crate) is_enabled: bool,

    /// Thinking-level override; `None` keeps the discovered value.
    pub(crate) thinking_levels_override: Option<&'a [ReasoningLevel]>,

    /// Maximum-input-tokens override; `None` keeps the discovered value.
    pub(crate) max_input_tokens_override: Option<i64>,

    /// Maximum-output-tokens override; `None` keeps the discovered value.
    pub(crate) max_output_tokens_override: Option<i64>,

    /// Compaction support facts override; `None` keeps the discovered facts.
    pub(crate) compaction_support_override: Option<&'a CompactionSupportFacts>,
}

/// One effectively enabled model with its effective capability declarations — the outward
/// listings' entry (README §6.5): every value is the effective `override ?? base` composition,
/// and an unknown (`None`) capability is omitted by every listing rather than fabricated. The
/// compaction facts resolve against the row's definition's declared defaults, carrying only the
/// mechanisms whose effective support is known.
#[derive(Debug, Clone)]
pub(crate) struct EnabledModel {
    /// Public routing ID.
    pub(crate) public_id: String,

    /// Owning provider's or pool's current name.
    pub(crate) owner_name: String,

    /// Effective thinking levels; `None` when unknown.
    pub(crate) thinking_levels: Option<Vec<ReasoningLevel>>,

    /// Effective maximum input tokens; `None` when unknown.
    pub(crate) max_input_tokens: Option<i64>,

    /// Effective maximum output tokens; `None` when unknown.
    pub(crate) max_output_tokens: Option<i64>,

    /// Effective compaction support facts, only known mechanisms; `None` when no mechanism is
    /// known.
    pub(crate) compaction: Option<CompactionSupportFacts>,
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
        .query(FIND_MODEL_SQL.as_str(), params![provider_id, upstream_id])
        .await?;

    rows.first().map(model_from_row).transpose()
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
        .query(FIND_MODEL_ENTRY_BY_ID_SQL.as_str(), params![model_id])
        .await?;

    rows.first().map(model_entry_from_row).transpose()
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
            FIND_MODEL_ENTRY_SQL.as_str(),
            params![provider_id, upstream_id],
        )
        .await?;

    rows.first().map(model_entry_from_row).transpose()
}

/// Lists the stored models with their provider identity, narrowed to one provider's rows when
/// `provider_id` is `Some`; an unknown provider yields an empty list, because a filter addresses no
/// row (README §7.1). Every row is listed — `vanished` included — ordered by provider ID and
/// upstream ID; the row's own inputs let the caller derive the three-input status (rule 7), which
/// tells serving rows apart.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_model_entries(
    database: &Database,
    provider_id: Option<i64>,
) -> Result<Vec<ModelEntry>, Error> {
    let rows = match provider_id {
        None => database.query(LIST_MODEL_ENTRIES_SQL.as_str(), ()).await?,

        Some(provider_id) => {
            database
                .query(
                    LIST_PROVIDER_MODEL_ENTRIES_SQL.as_str(),
                    params![provider_id],
                )
                .await?
        }
    };

    let mut entries = Vec::with_capacity(rows.len());

    for row in &rows {
        entries.push(model_entry_from_row(row)?);
    }

    Ok(entries)
}

/// Lists effectively enabled explicit public routes with their capability declarations.
///
/// Discovery alone never publishes a model. Direct routes use their physical model's resolved
/// facts; pools combine all structural candidates independently of quota. Unknown facts remain
/// unknown, numerical limits use the minimum, and reasoning levels use intersection. Per-mechanism
/// compaction support is supported only when every candidate supports it, unsupported when any
/// candidate rejects it, and otherwise omitted. Empty or disabled pools produce no listing entry.
/// All route, membership, provider, and model reads share one coherent transaction snapshot.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_enabled_models_with_owner(
    database: &Database,
    api_key_id: i64,
) -> Result<Vec<EnabledModel>, Error> {
    super::route_listing::list_public_models(database, api_key_id).await
}

/// Applies one account's complete nonempty discovery result in a short transaction.
///
/// Previously discovered rows are marked vanished, then each exact upstream ID is upserted.
/// Existing rows retain identity, serving switch, and administrator overrides; discovery rewrites
/// only state and capability base facts. Absent rows are inserted enabled and discovered. Unlisted
/// manual rows remain untouched. No row is deleted; every model receives its automatically managed
/// prefixed direct route. The caller rejects empty discovery results before this write boundary.
///
/// # Errors
///
/// Returns errors for an unknown account or failed statements.
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

        // One upsert per discovered row: an existing natural key flips its state back to
        // `discovered` and rewrites its capability base columns to this refresh's facts —
        // `NULL` for a fact this refresh did not extract, so a stored value is always the
        // latest upstream knowledge, never stale residue (README §3) — while the serving switch
        // and the overrides are never touched; an absent key inserts enabled and discovered.
        for model in discovered {
            transaction
                .execute(
                    "INSERT INTO models (provider_id, upstream_id, is_enabled, discovery_state, \
                     thinking_levels, max_input_tokens, max_output_tokens, compaction_support) \
                     VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7) \
                     ON CONFLICT (provider_id, upstream_id) DO UPDATE SET \
                     discovery_state = excluded.discovery_state, \
                     thinking_levels = excluded.thinking_levels, \
                     max_input_tokens = excluded.max_input_tokens, \
                     max_output_tokens = excluded.max_output_tokens, \
                     compaction_support = excluded.compaction_support",
                    params![
                        provider.id,
                        model.upstream_id.as_str(),
                        DiscoveryState::Discovered,
                        levels_into_value(model.thinking_levels.as_deref())?,
                        model.max_input_tokens,
                        model.max_output_tokens,
                        support_facts_into_value(model.compaction.as_ref())?,
                    ],
                )
                .await?;
        }

        super::automatic_routes::synchronize(&mut transaction).await?;

        // The response carries every row of the provider as this transaction left them.
        let rows = transaction
            .query(
                LIST_PROVIDER_MODEL_ENTRIES_SQL.as_str(),
                params![provider.id],
            )
            .await?;

        let mut entries = Vec::with_capacity(rows.len());

        for row in &rows {
            entries.push(model_entry_from_row(row)?);
        }

        Ok(entries)
    })
}

/// Creates a physical manual row or claims an existing vanished row by its account-local key.
///
/// A fresh row has unknown capability base facts. A claim preserves identity and the last
/// discovered base facts while applying the requested switch and overrides and becoming manual.
/// Existing manual or discovered rows conflict. Automatically managed direct routes are
/// synchronized in the same transaction.
///
/// # Errors
///
/// Returns errors for an unknown account, an already-listed natural key, or failed statements.
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

        match stored {
            None => {
                // A fresh manual row starts unknown: every capability base column stores `NULL`
                // (README §3) while the requested overrides apply.
                transaction
                    .execute(
                        "INSERT INTO models (provider_id, upstream_id, is_enabled, \
                         discovery_state, thinking_levels, \
                         max_input_tokens, max_output_tokens, thinking_levels_override, \
                         max_input_tokens_override, max_output_tokens_override, \
                         compaction_support, compaction_support_override) \
                         VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6, ?7, NULL, ?8)",
                        params![
                            provider.id,
                            create.upstream_id,
                            i64::from(create.is_enabled),
                            DiscoveryState::Manual,
                            levels_into_value(create.thinking_levels_override)?,
                            create.max_input_tokens_override,
                            create.max_output_tokens_override,
                            support_facts_into_value(create.compaction_support_override)?,
                        ],
                    )
                    .await?;
            }

            Some(existing) if existing.discovery_state == DiscoveryState::Vanished => {
                // The claim writes the administrator-owned columns only: the capability base
                // columns keep the last discovery's facts, whatever they were (README §3).
                transaction
                    .execute(
                        "UPDATE models SET is_enabled = ?1, \
                         discovery_state = ?2, thinking_levels_override = ?3, \
                         max_input_tokens_override = ?4, max_output_tokens_override = ?5, \
                         compaction_support_override = ?6 \
                         WHERE id = ?7",
                        params![
                            i64::from(create.is_enabled),
                            DiscoveryState::Manual,
                            levels_into_value(create.thinking_levels_override)?,
                            create.max_input_tokens_override,
                            create.max_output_tokens_override,
                            support_facts_into_value(create.compaction_support_override)?,
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
        super::automatic_routes::synchronize(&mut transaction).await?;

        let entry =
            find_model_entry_in_transaction(&mut transaction, provider.id, create.upstream_id)
                .await?
                .ok_or_else(|| Error::Drift {
                    message: "model vanished after insert".to_string(),
                })?;

        Ok(entry)
    })
}

/// Rewrites a physical model's serving switch and capability overrides in one short transaction.
/// Discovery state and base facts are preserved, including for a vanished model.
///
/// # Errors
///
/// Returns errors for an unknown physical row or failed statements.
pub(crate) async fn update_model_overrides_checked(
    database: &Database,
    update: ModelUpdate<'_>,
) -> Result<ModelEntry, Error> {
    in_retryable_transaction!(database, transaction, async {
        let changes = transaction
            .execute(
                "UPDATE models SET is_enabled = ?1, \
                 thinking_levels_override = ?2, max_input_tokens_override = ?3, \
                 max_output_tokens_override = ?4, compaction_support_override = ?5 \
                 WHERE id = ?6",
                params![
                    i64::from(update.is_enabled),
                    levels_into_value(update.thinking_levels_override)?,
                    update.max_input_tokens_override,
                    update.max_output_tokens_override,
                    support_facts_into_value(update.compaction_support_override)?,
                    update.model_id,
                ],
            )
            .await?;

        // The affected-row count is the existence gate — an UPDATE addressing no row changes
        // nothing — the identical 404-before-write `delete_model_checked` derives.
        if changes == 0 {
            return Err(Error::NotFound {
                message: super::model_not_found_message(update.model_id),
            });
        }

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

    super::automatic_routes::synchronize(&mut transaction).await?;
    transaction.commit().await?;

    Ok(())
}

impl IntoValue for DiscoveryState {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

/// The models-to-providers join: models first, then the owning provider — the `(m, p)` aliases
/// the projections and row-offset mappings over this join depend on.
const MODELS_JOINED_PROVIDERS: &str = "FROM models m JOIN providers p ON p.id = m.provider_id";

/// Column order of model selects: the row identity first, then the upstream identifier, the
/// routing columns, the discovery state, the three capability base columns, the three
/// capability override mirrors, and the two compaction support fact columns. The owning
/// provider is never carried in the row itself — every read knows it through the query's filter
/// or its join.
pub(super) const MODEL_COLUMNS: &str = "id, upstream_id, is_enabled, \
     discovery_state, thinking_levels, max_input_tokens, max_output_tokens, \
     thinking_levels_override, max_input_tokens_override, max_output_tokens_override, \
     compaction_support, compaction_support_override";

/// The model columns as a provider-model join projects them, appended after
/// [`super::providers::PROVIDER_COLUMNS`].
pub(super) const MODEL_COLUMNS_JOINED: &str = "m.id, m.upstream_id, \
     m.is_enabled, m.discovery_state, m.thinking_levels, m.max_input_tokens, \
     m.max_output_tokens, m.thinking_levels_override, m.max_input_tokens_override, \
     m.max_output_tokens_override, m.compaction_support, m.compaction_support_override";

/// The combined column order of the model-entry join, consumed positionally by
/// [`model_entry_from_row`]: the provider id and name, then the model columns.
const MODEL_ENTRY_COLUMNS: &str = "p.id, p.name, m.id, m.upstream_id, \
     m.is_enabled, m.discovery_state, m.thinking_levels, m.max_input_tokens, \
     m.max_output_tokens, m.thinking_levels_override, m.max_input_tokens_override, \
     m.max_output_tokens_override, m.compaction_support, m.compaction_support_override";

/// The natural-key model read behind checked creation: a fully fixed statement over
/// [`MODEL_COLUMNS`], built once.
static FIND_MODEL_SQL: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {MODEL_COLUMNS} FROM models WHERE provider_id = ?1 AND upstream_id = ?2")
});

/// The id-addressed model-entry read behind override updates: a fully fixed statement, built
/// once.
static FIND_MODEL_ENTRY_BY_ID_SQL: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {MODEL_ENTRY_COLUMNS} {MODELS_JOINED_PROVIDERS} WHERE m.id = ?1")
});

/// The natural-key model-entry read behind checked creation's read-back: a fully fixed
/// statement, built once.
static FIND_MODEL_ENTRY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {MODEL_ENTRY_COLUMNS} {MODELS_JOINED_PROVIDERS} \
         WHERE m.provider_id = ?1 AND m.upstream_id = ?2"
    )
});

/// The every-provider model-entry listing, ordered by provider id and upstream id: a fully
/// fixed statement, built once.
static LIST_MODEL_ENTRIES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {MODEL_ENTRY_COLUMNS} {MODELS_JOINED_PROVIDERS} \
         ORDER BY m.provider_id, m.upstream_id"
    )
});

/// The one-provider model-entry listing, ordered by provider id and upstream id: the filtered
/// form of [`list_model_entries`] and discovery application's read-back. A fully fixed
/// statement, built once.
static LIST_PROVIDER_MODEL_ENTRIES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {MODEL_ENTRY_COLUMNS} {MODELS_JOINED_PROVIDERS} \
         WHERE m.provider_id = ?1 ORDER BY m.provider_id, m.upstream_id"
    )
});

/// A provider row joined with a model row.
pub(super) struct ProviderModelJoin {
    /// Owning provider.
    pub(super) provider: Provider,

    /// Model row.
    pub(super) model: Model,
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

/// Reads a thinking-level set column (README §3): `NULL` is the unknown state ([`None`]); a JSON
/// array of vocabulary strings is the canonicalized set; anything else — non-JSON text, a JSON
/// value that is not an array, an array element that is not a string or not a vocabulary member —
/// is storage drift, because both write paths (discovery ingestion and the management override)
/// write only canonicalized vocabulary sets.
fn levels_from_column(
    value: Option<String>,
    column: &str,
) -> Result<Option<Vec<ReasoningLevel>>, Error> {
    let Some(json) = value else {
        return Ok(None);
    };

    let parsed = serde_json::from_str::<Vec<String>>(&json).map_err(|error| Error::Drift {
        message: format!("{column} column carries a malformed level set: {error}"),
    })?;

    parsed
        .into_iter()
        .map(|level| {
            ReasoningLevel::from_vocabulary_str(&level).ok_or_else(|| Error::Drift {
                message: format!("{column} column carries the foreign level `{level}`"),
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Renders a thinking-level set column value for a statement parameter: [`Value::Null`] for the
/// unknown state, else the set's canonical JSON array text. The caller passes a canonicalized set
/// (both write paths canonicalize before storage, README §3), so the rendering needs no
/// re-canonicalization.
fn levels_into_value(levels: Option<&[ReasoningLevel]>) -> Result<Value, TursoError> {
    let Some(levels) = levels else {
        return Ok(Value::Null);
    };

    let spelled = levels
        .iter()
        .map(|level| level.as_str())
        .collect::<Vec<_>>();

    serde_json::to_string(&spelled)
        .map(Value::Text)
        .map_err(|error| TursoError::ToSqlConversionFailure(error.into()))
}

/// The support-value domain one compaction facts column admits: the discovery-owned base
/// column stores only known facts — discovery extracts booleans, so an explicit `unknown` in it
/// is stored drift — while the administrator-owned override column additionally admits the
/// explicit `unknown` that suppresses the lower layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupportFactDomain {
    /// Only `supported`/`unsupported` members are legal.
    Known,

    /// The explicit `unknown` member value is legal too.
    WithUnknown,
}

/// Parses one compaction support facts column: `Ok(None)` for `NULL` and for the empty
/// object — both spell the nothing-known state — else the parsed map, rejecting a stored fact
/// outside the column's domain as storage drift. The closed mechanism and support vocabularies
/// are the serde types' own, so a foreign spelling fails the parse the same way.
fn support_facts_from_column(
    value: Option<String>,
    column: &str,
    domain: SupportFactDomain,
) -> Result<Option<CompactionSupportFacts>, Error> {
    let Some(json) = value else {
        return Ok(None);
    };

    let facts =
        serde_json::from_str::<CompactionSupportFacts>(&json).map_err(|error| Error::Drift {
            message: format!("{column} column carries a malformed support map: {error}"),
        })?;

    if domain == SupportFactDomain::Known && facts.carries_unknown() {
        return Err(Error::Drift {
            message: format!("{column} column carries an explicit `unknown` fact"),
        });
    }

    Ok(CompactionSupportFacts::from_pairs(facts.iter()))
}

/// Renders a compaction support facts column value for a statement parameter: [`Value::Null`]
/// for the nothing-known state, else the map's canonical JSON object text. The caller passes a
/// map built at a validated boundary, so the rendering re-checks nothing.
fn support_facts_into_value(facts: Option<&CompactionSupportFacts>) -> Result<Value, TursoError> {
    let Some(facts) = facts else {
        return Ok(Value::Null);
    };

    serde_json::to_string(facts)
        .map(Value::Text)
        .map_err(|error| TursoError::ToSqlConversionFailure(error.into()))
}

/// Parses a provider-model joined row: the provider columns first, the model columns second.
pub(super) fn provider_model_from_row(row: &Row) -> Result<ProviderModelJoin, Error> {
    let provider = super::providers::provider_from_row(row)?;

    let model = model_from_row_offset(row, super::providers::PROVIDER_COLUMN_COUNT)?;

    Ok(ProviderModelJoin { provider, model })
}

/// Parses a model row starting at the given column offset.
pub(super) fn model_from_row_offset(row: &Row, offset: usize) -> Result<Model, Error> {
    Ok(Model {
        id: row.get(offset)?,
        upstream_id: row.get(offset + 1)?,
        is_enabled: row.get::<i64>(offset + 2)? != 0,
        discovery_state: discovery_state_from_value(row.get_value(offset + 3)?)?,
        thinking_levels: levels_from_column(row.get(offset + 4)?, "thinking_levels")?,
        max_input_tokens: row.get(offset + 5)?,
        max_output_tokens: row.get(offset + 6)?,
        thinking_levels_override: levels_from_column(
            row.get(offset + 7)?,
            "thinking_levels_override",
        )?,
        max_input_tokens_override: row.get(offset + 8)?,
        max_output_tokens_override: row.get(offset + 9)?,
        compaction_support: support_facts_from_column(
            row.get(offset + 10)?,
            "compaction_support",
            SupportFactDomain::Known,
        )?,
        compaction_support_override: support_facts_from_column(
            row.get(offset + 11)?,
            "compaction_support_override",
            SupportFactDomain::WithUnknown,
        )?,
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
