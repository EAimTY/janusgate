//! Provider, model, and OAuth state persistence.

use super::{Credentials, Model, ModelRouting, NewProvider, Provider, ProviderType};
use crate::{
    database::{Database, Row, Transaction, Value, params, params_from_iter},
    error::ApiError,
    format::UpstreamFormat,
};
use std::collections::HashMap;

/// Column order of provider selects.
const PROVIDER_COLUMNS: &str = "id, name, model_prefix, type, protocol, base_url, credentials, \
     expires_at, enabled";

/// Column order of model selects.
const MODEL_COLUMNS: &str = "provider_id, upstream_name, name, name_override, enabled_override, \
     thinking_levels_override";

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
                provider.protocol.map(UpstreamFormat::as_str),
                provider.base_url,
                provider.credentials.to_json(),
                provider.expires_at,
                i64::from(provider.is_enabled),
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

/// The `WHERE name = ?1` provider lookup. Names are unique, so this is the
/// name-taken check behind creation; all addressing of an existing provider
/// goes through its id.
fn find_provider_sql() -> String {
    format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE name = ?1")
}

/// The `WHERE id = ?1` provider lookup shared by the pooled and transactional
/// executors, which share no query trait.
fn find_provider_by_id_sql() -> String {
    format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE id = ?1")
}

/// Fetches one provider by name, for creation-time name-taken checks.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the row is malformed.
pub(crate) async fn find_provider(
    database: &Database,
    name: &str,
) -> Result<Option<Provider>, ApiError> {
    let rows = database.query(find_provider_sql(), params![name]).await?;

    rows.first().map(provider_from_row).transpose()
}

/// Fetches one provider by id.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the row is malformed.
pub(crate) async fn find_provider_by_id(
    database: &Database,
    provider_id: i64,
) -> Result<Option<Provider>, ApiError> {
    let rows = database
        .query(find_provider_by_id_sql(), params![provider_id])
        .await?;

    rows.first().map(provider_from_row).transpose()
}

/// Fetches one provider by id inside a transaction, mirroring
/// [`find_provider_by_id`] against the transactional executor.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the row is malformed.
pub(crate) async fn find_provider_by_id_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Option<Provider>, ApiError> {
    let rows = transaction
        .query(find_provider_by_id_sql(), params![provider_id])
        .await?;

    rows.first().map(provider_from_row).transpose()
}

/// Fetches one provider by id after a write, failing with an internal error
/// when the row is gone: a vanished row after a successful write is storage
/// drift — an invariant violation — and never a client fault.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the provider does not exist.
pub(crate) async fn reload_provider(
    database: &Database,
    provider_id: i64,
    context: &str,
) -> Result<Provider, ApiError> {
    find_provider_by_id(database, provider_id)
        .await?
        .ok_or_else(|| ApiError::internal(format!("provider vanished after {context}")))
}

/// Deletes one provider by id, returning whether a row was removed.
///
/// # Errors
///
/// Returns [`ApiError`] when the delete fails.
pub(crate) async fn delete_provider(
    database: &Database,
    provider_id: i64,
) -> Result<bool, ApiError> {
    let changes = database
        .execute("DELETE FROM providers WHERE id = ?1", params![provider_id])
        .await?;

    Ok(changes == 1)
}

/// Replaces provider credentials, resets the token expiry, and clears any
/// refresh lease, returning whether a provider row was still there to update.
///
/// Reporting `false` lets a caller whose outer read observed the row — the
/// OAuth completion — fail with the refresh path's 404 classification when a
/// concurrent `delete-provider` won the race, instead of a silent no-op
/// followed by a storage-drift 500 from the reload.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn update_credentials(
    database: &Database,
    provider_id: i64,
    credentials: &Credentials,
    expires_at: Option<i64>,
) -> Result<bool, ApiError> {
    let changes = database
        .execute(
            "UPDATE providers SET credentials = ?1, expires_at = ?2, refresh_started_at = NULL \
             WHERE id = ?3",
            params![credentials.to_json(), expires_at, provider_id],
        )
        .await?;

    Ok(changes == 1)
}

/// The raw `save-provider` field changes for an existing provider. `name`
/// and `model_prefix` are not optional: the caller states the naming on
/// every request, and both are applied verbatim — supplying a different
/// `name` is how a provider is renamed. The optional fields are
/// supplied-or-absent: `None` means the field is absent from the request and
/// keeps the stored value, which is resolved against the row re-read inside
/// the update transaction.
pub(crate) struct ProviderUpdate<'a> {
    /// Unique provider name; may rename the provider.
    pub(crate) name: &'a str,

    /// Prefix prepended to every public model name; may be empty.
    pub(crate) model_prefix: &'a str,

    /// Upstream wire format.
    pub(crate) protocol: Option<UpstreamFormat>,

    /// Upstream base URL.
    pub(crate) base_url: Option<&'a str>,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: Option<bool>,

    /// Replacement credentials; `None` keeps the stored ones.
    pub(crate) credentials: Option<&'a Credentials>,
}

/// Applies one `save-provider` update to an existing provider inside a single
/// transaction and returns the updated row.
///
/// The provider row is re-read inside the transaction and absent fields are
/// merged from it, so a caller's stale outer snapshot can never revert a
/// field a concurrent `save-provider` just wrote (README §7.1). An unknown
/// provider — including one deleted between the caller's outer read and this
/// call — is rejected with 404 and nothing is written.
///
/// The write owns the provider row — the supplied `name` included, so a
/// different name renames the provider — an optional credentials replacement
/// (which resets `expires_at` and clears any refresh lease), and — upholding
/// the README §3 invariant — the public `name` of every model row, recomputed
/// from the effective prefix through the shared composition rule
/// [`Model::public_name`]. Nothing is written when the new name is already
/// taken by another provider, when any recomputed public name would collide
/// with an existing model row of another provider, or when two of the
/// provider's own models would converge on one public name; all rejections
/// are 409 conflicts. Abandoning the transaction on any error discards every
/// partial write, so the provider row and its model names can never drift
/// apart.
///
/// # Errors
///
/// Returns [`ApiError`] for an unknown provider, a failed statement, a taken
/// name, or a recomputed public name collision.
pub(crate) async fn update_provider(
    database: &Database,
    provider_id: i64,
    update: ProviderUpdate<'_>,
) -> Result<Provider, ApiError> {
    let mut transaction = database.transaction().await?;

    // The merge reads the row inside the transaction, so absent fields keep
    // the newest stored values, never the caller's snapshot.
    let Some(provider) = find_provider_by_id_in_transaction(&mut transaction, provider_id).await?
    else {
        return Err(ApiError::not_found(format!(
            "Provider {provider_id} does not exist"
        )));
    };

    let name = update.name;

    // A rename must not take a name another provider already holds; the
    // provider's own row is excluded, so re-sending the unchanged name is
    // never a conflict.
    let name_holders = transaction
        .query(
            "SELECT id FROM providers WHERE name = ?1 AND id != ?2",
            params![name, provider.id],
        )
        .await?;

    if !name_holders.is_empty() {
        return Err(ApiError::conflict(format!(
            "Provider name `{name}` is already taken"
        )));
    }

    let model_prefix = update.model_prefix;

    let protocol = update.protocol.or(provider.protocol);

    let base_url = update.base_url.or(provider.base_url.as_deref());

    let is_enabled = update.is_enabled.unwrap_or(provider.is_enabled);

    let is_credentials_replaced = update.credentials.is_some();

    let models = find_models_in_transaction(&mut transaction, provider.id).await?;

    let recomputed: Vec<(String, &Model)> = models
        .iter()
        .map(|model| {
            let name = Model::public_name(
                model_prefix,
                &model.upstream_name,
                model.name_override.as_deref(),
            );

            (name, model)
        })
        .collect();

    let composed: Vec<(&str, &str)> = recomputed
        .iter()
        .map(|(name, model)| (name.as_str(), model.upstream_name.as_str()))
        .collect();

    ensure_names_pairwise_distinct(name, &composed)?;

    ensure_names_routable(&mut transaction, provider.id, &composed).await?;

    transaction
        .execute(
            "UPDATE providers SET name = ?1, model_prefix = ?2, protocol = ?3, base_url = ?4, \
             enabled = ?5 WHERE id = ?6",
            params![
                name,
                model_prefix,
                protocol.map(UpstreamFormat::as_str),
                base_url,
                i64::from(is_enabled),
                provider.id,
            ],
        )
        .await?;

    if let Some(credentials) = update.credentials {
        transaction
            .execute(
                "UPDATE providers SET credentials = ?1, expires_at = NULL, \
                 refresh_started_at = NULL WHERE id = ?2",
                params![credentials.to_json(), provider.id],
            )
            .await?;
    }

    for (new_name, model) in &recomputed {
        if new_name == &model.name {
            continue;
        }

        transaction
            .execute(
                "UPDATE models SET name = ?1 WHERE provider_id = ?2 AND upstream_name = ?3",
                params![new_name.as_str(), provider.id, model.upstream_name.as_str()],
            )
            .await?;
    }

    transaction.commit().await.map_err(|error| {
        ApiError::internal(format!("failed to commit provider update: {error}"))
    })?;

    // The returned row mirrors exactly what this transaction wrote.
    Ok(Provider {
        id: provider.id,
        name: name.to_string(),
        model_prefix: model_prefix.to_string(),
        provider_type: provider.provider_type,
        protocol,
        base_url: base_url.map(str::to_string),
        credentials: update.credentials.cloned().unwrap_or(provider.credentials),
        expires_at: if is_credentials_replaced {
            None
        } else {
            provider.expires_at
        },
        is_enabled,
    })
}

/// Rejects one provider's freshly composed public names when two of its rows
/// would converge on one name. Healthy rows can never converge — distinct rows
/// carry distinct `name_override ?? upstream_name` bases and share one prefix —
/// so this guards the invariant even against drifted stored names.
///
/// `composed` carries `(public_name, upstream_name)` pairs.
fn ensure_names_pairwise_distinct(
    provider_name: &str,
    composed: &[(&str, &str)],
) -> Result<(), ApiError> {
    let mut seen: HashMap<&str, &str> = HashMap::with_capacity(composed.len());

    for (new_name, upstream_name) in composed {
        if let Some(first_upstream_name) = seen.insert(new_name, upstream_name) {
            return Err(ApiError::conflict(format!(
                "Public model name `{new_name}` collides between upstream models \
                 `{first_upstream_name}` and `{upstream_name}` of provider `{provider_name}`"
            )));
        }
    }

    Ok(())
}

/// Rejects freshly composed public names that are already routed by another
/// model row — of any provider, including another model of the same provider.
/// A name held by the very row being written is not a collision.
///
/// `composed` carries `(public_name, upstream_name)` pairs of the provider
/// identified by `provider_id`. Runs inside the caller's transaction, so the
/// routed rows it observes cannot interleave with the caller's writes.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a name collides.
async fn ensure_names_routable(
    transaction: &mut Transaction,
    provider_id: i64,
    composed: &[(&str, &str)],
) -> Result<(), ApiError> {
    let routed_rows = transaction
        .query(
            "SELECT m.name, m.provider_id, m.upstream_name, p.name \
             FROM models m JOIN providers p ON p.id = m.provider_id",
            (),
        )
        .await?;

    let mut routed: HashMap<String, (i64, String, String)> =
        HashMap::with_capacity(routed_rows.len());

    for row in &routed_rows {
        routed.insert(row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?));
    }

    for (new_name, upstream_name) in composed {
        if let Some((holder_provider_id, holder_upstream_name, holder_provider_name)) =
            routed.get(*new_name)
            && (*holder_provider_id != provider_id || holder_upstream_name != upstream_name)
        {
            return Err(ApiError::conflict(format!(
                "Public model name `{new_name}` is already routed by provider \
                 `{holder_provider_name}`"
            )));
        }
    }

    Ok(())
}

/// Fetches every model row of one provider inside a transaction.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
async fn find_models_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Vec<Model>, ApiError> {
    let sql =
        format!("SELECT {MODEL_COLUMNS} FROM models WHERE provider_id = ?1 ORDER BY upstream_name");

    let rows = transaction.query(sql, params![provider_id]).await?;

    rows.iter().map(model_from_row).collect()
}

/// Attempts to acquire the token refresh lease inside a transaction.
///
/// The lease is granted only when the token is still expired (no later than
/// `freshness_margin` seconds into the future) and no other holder owns a
/// lease acquired after `lease_stale_before`. Returns the acquired lease
/// stamp, or `None` when another holder owns a live lease.
///
/// The raw [`crate::database::Error`] is returned instead of an [`ApiError`]
/// because the caller must classify a stale-snapshot busy failure — a
/// concurrent commit invalidating the read-then-write upgrade — as "not
/// acquired" and retry through its poll loop, rather than render it as a
/// failed request.
///
/// # Errors
///
/// Returns the underlying database error when the update fails.
pub(crate) async fn try_acquire_refresh_lease(
    transaction: &mut Transaction,
    provider_id: i64,
    now: i64,
    lease_stale_before: i64,
    freshness_margin: i64,
) -> Result<Option<i64>, crate::database::Error> {
    let changes = transaction
        .execute(
            "UPDATE providers SET refresh_started_at = ?1 \
             WHERE id = ?2 AND (expires_at IS NULL OR expires_at <= ?1 + ?4) \
             AND (refresh_started_at IS NULL OR refresh_started_at < ?3)",
            params![now, provider_id, lease_stale_before, freshness_margin],
        )
        .await?;

    Ok((changes == 1).then_some(now))
}

/// Replaces provider credentials and resets the token expiry, but only while
/// the caller still owns the refresh lease stamped `lease_started_at`.
///
/// Conditioning the persist on lease ownership closes the lost-update window
/// against a concurrent credentials replacement or stale-lease reacquisition
/// (README §3): when the stored stamp no longer matches, tokens derived from
/// superseded credentials are discarded, the newer state's row — its lease
/// included — is left untouched, and `false` is returned.
///
/// # Errors
///
/// Returns [`ApiError`] when the update fails.
pub(crate) async fn update_credentials_for_lease(
    database: &Database,
    provider_id: i64,
    lease_started_at: i64,
    credentials: &Credentials,
    expires_at: Option<i64>,
) -> Result<bool, ApiError> {
    let changes = database
        .execute(
            "UPDATE providers SET credentials = ?1, expires_at = ?2, refresh_started_at = NULL \
             WHERE id = ?3 AND refresh_started_at = ?4",
            params![
                credentials.to_json(),
                expires_at,
                provider_id,
                lease_started_at
            ],
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
                    .is_effectively_enabled(joined.provider.is_enabled),
                provider: joined.provider,
                model: joined.model,
            })
        })
        .transpose()
}

/// Fetches one model row of a provider inside a transaction.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
async fn find_model_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    upstream_name: &str,
) -> Result<Option<Model>, ApiError> {
    let rows = transaction
        .query(
            format!(
                "SELECT {MODEL_COLUMNS} FROM models \
                 WHERE provider_id = ?1 AND upstream_name = ?2"
            ),
            params![provider_id, upstream_name],
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
/// Returns [`ApiError`] when the query fails or a row is malformed.
pub(crate) async fn list_model_entries(database: &Database) -> Result<Vec<ModelEntry>, ApiError> {
    let rows = database
        .query(
            "SELECT p.id, p.name AS provider_name, p.enabled AS provider_enabled, \
             m.provider_id, m.upstream_name, m.name, m.name_override, m.enabled_override, \
             m.thinking_levels_override FROM models m JOIN providers p ON p.id = m.provider_id \
             ORDER BY m.name",
            (),
        )
        .await?;

    let mut entries = Vec::with_capacity(rows.len());

    for row in &rows {
        entries.push(ModelEntry {
            provider_id: row.get(0).map_err(ApiError::from)?,
            provider_name: row.get(1).map_err(ApiError::from)?,
            is_provider_enabled: row.get::<i64>(2).map_err(ApiError::from)? != 0,
            model: model_from_row_offset(row, 3)?,
        });
    }

    Ok(entries)
}

/// Lists `(public_name, provider_name)` pairs for every effectively enabled
/// model, ordered by public name.
///
/// The `COALESCE(m.enabled_override, p.enabled) != 0` predicate is the SQL
/// form of [`Model::is_effectively_enabled`], the canonical Rust rule. It is
/// applied in SQL rather than Rust-side filtering on purpose: disabled rows
/// never leave the engine, so the Rust side materializes only what the
/// outward model listing serves.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails.
pub(crate) async fn list_enabled_models_with_owner(
    database: &Database,
) -> Result<Vec<(String, String)>, ApiError> {
    let rows = database
        .query(
            "SELECT m.name, p.name FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE COALESCE(m.enabled_override, p.enabled) != 0 ORDER BY m.name",
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
/// The provider row is re-read inside the transaction, so the public names are
/// composed from the provider's current prefix even when a concurrent
/// `save-provider` changed it after the caller's outer read (README §7.1).
/// Overrides carry over from the stored rows by `upstream_name`; discovered
/// models absent from the stored set vanish with their overrides.
///
/// The composed public names undergo the same collision checks as
/// [`update_provider`]: pairwise distinctness within the provider and no
/// routing collision with any other model row; both rejections are name-
/// specific 409 conflicts that write nothing.
///
/// # Errors
///
/// Returns [`ApiError`] for an empty discovered set, an unknown provider,
/// failed statements, or a composed public name collision.
pub(crate) async fn replace_discovered_models(
    database: &Database,
    provider_id: i64,
    upstream_names: &[String],
) -> Result<usize, ApiError> {
    if upstream_names.is_empty() {
        return Err(ApiError::upstream_unavailable(
            "upstream reported no models; refusing to replace the provider's model rows",
        ));
    }

    let mut transaction = database.transaction().await?;

    let Some(provider) = find_provider_by_id_in_transaction(&mut transaction, provider_id).await?
    else {
        return Err(ApiError::not_found(format!(
            "Provider {provider_id} does not exist"
        )));
    };

    let previous = find_models_in_transaction(&mut transaction, provider.id).await?;

    let stored: HashMap<&str, &Model> = previous
        .iter()
        .map(|model| (model.upstream_name.as_str(), model))
        .collect();

    let mut models: Vec<Model> = Vec::with_capacity(upstream_names.len());

    for upstream_name in upstream_names {
        let prior = stored.get(upstream_name.as_str()).copied();

        let name_override = prior.and_then(|model| model.name_override.clone());

        let name = Model::public_name(
            &provider.model_prefix,
            upstream_name,
            name_override.as_deref(),
        );

        models.push(Model {
            provider_id: provider.id,
            upstream_name: upstream_name.clone(),
            name,
            name_override,
            enabled_override: prior.and_then(|model| model.enabled_override),
            thinking_levels_override: prior
                .and_then(|model| model.thinking_levels_override.clone()),
        });
    }

    let composed: Vec<(&str, &str)> = models
        .iter()
        .map(|model| (model.name.as_str(), model.upstream_name.as_str()))
        .collect();

    ensure_names_pairwise_distinct(&provider.name, &composed)?;

    ensure_names_routable(&mut transaction, provider.id, &composed).await?;

    replace_provider_models(&mut transaction, provider.id, &models).await?;

    transaction.commit().await.map_err(|error| {
        ApiError::internal(format!("failed to commit discovered models: {error}"))
    })?;

    Ok(models.len())
}

/// Replaces the model rows of a provider inside a transaction.
///
/// # Errors
///
/// Returns [`ApiError`] when any statement fails.
async fn replace_provider_models(
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

/// One `save-model` override update about to be applied transactionally.
pub(crate) struct ModelOverrideUpdate<'a> {
    /// Owning provider's id, the lookup key.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_name: &'a str,

    /// Public name override; `None` leaves it unchanged, `Some` writes the
    /// inner value, clearing the column when the inner value is `None`.
    pub(crate) name_override: Option<Option<String>>,

    /// Serving state override; same tri-state semantics.
    pub(crate) enabled_override: Option<Option<bool>>,

    /// Canonical thinking levels override; same tri-state semantics.
    pub(crate) thinking_levels_override: Option<Option<String>>,
}

/// Writes one model's overrides and recomputes its public name inside a single
/// transaction, returning the updated row with its provider facts.
///
/// The provider row is re-read inside the transaction, so the recomputed
/// public name uses the provider's current prefix even when a concurrent
/// `save-provider` changed it after the caller's outer read; the stored model
/// row and the routed-name collision check observe the same snapshot. The
/// collision check matches [`update_provider`]'s quality tier: a name already
/// routed by another model row yields a 409 naming the holder, and nothing is
/// written. Unknown providers or models yield 404.
///
/// # Errors
///
/// Returns [`ApiError`] for unknown rows, failed statements, or a public name
/// collision.
pub(crate) async fn update_model_overrides_checked(
    database: &Database,
    update: ModelOverrideUpdate<'_>,
) -> Result<ModelEntry, ApiError> {
    let mut transaction = database.transaction().await?;

    let Some(provider) =
        find_provider_by_id_in_transaction(&mut transaction, update.provider_id).await?
    else {
        return Err(ApiError::not_found(format!(
            "Provider {} does not exist",
            update.provider_id
        )));
    };

    let Some(model) =
        find_model_in_transaction(&mut transaction, provider.id, update.upstream_name).await?
    else {
        return Err(ApiError::not_found(format!(
            "Model `{}` does not exist on provider `{}`",
            update.upstream_name, provider.name
        )));
    };

    let new_name_override = update
        .name_override
        .clone()
        .unwrap_or_else(|| model.name_override.clone());

    let new_name = Model::public_name(
        &provider.model_prefix,
        update.upstream_name,
        new_name_override.as_deref(),
    );

    ensure_names_routable(
        &mut transaction,
        provider.id,
        &[(new_name.as_str(), update.upstream_name)],
    )
    .await?;

    let mut assignments: Vec<String> = Vec::new();

    let mut values: Vec<Value> = Vec::new();

    if let Some(override_value) = update.name_override {
        assignments.push("name_override = ?".to_string());

        values.push(override_value.into());
    }

    if let Some(override_value) = update.enabled_override {
        assignments.push("enabled_override = ?".to_string());

        values.push(override_value.map(i64::from).into());
    }

    if let Some(override_value) = update.thinking_levels_override {
        assignments.push("thinking_levels_override = ?".to_string());

        values.push(override_value.into());
    }

    assignments.push("name = ?".to_string());

    values.push(new_name.into());

    values.push(provider.id.into());

    values.push(update.upstream_name.into());

    let sql = format!(
        "UPDATE models SET {} WHERE provider_id = ? AND upstream_name = ?",
        assignments.join(", ")
    );

    transaction.execute(sql, params_from_iter(values)).await?;

    let entry =
        find_model_entry_in_transaction(&mut transaction, provider.id, update.upstream_name)
            .await?
            .ok_or_else(|| ApiError::internal("model vanished after override update"))?;

    transaction.commit().await.map_err(|error| {
        ApiError::internal(format!("failed to commit model override update: {error}"))
    })?;

    Ok(entry)
}

/// Reads back one model row with its provider facts inside a transaction.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or a row is malformed.
async fn find_model_entry_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    upstream_name: &str,
) -> Result<Option<ModelEntry>, ApiError> {
    let rows = transaction
        .query(
            "SELECT p.id, p.name, p.enabled, m.provider_id, m.upstream_name, m.name, \
             m.name_override, m.enabled_override, m.thinking_levels_override \
             FROM models m JOIN providers p ON p.id = m.provider_id \
             WHERE m.provider_id = ?1 AND m.upstream_name = ?2",
            params![provider_id, upstream_name],
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

/// Reads the provider type of an unexpired OAuth state row without consuming
/// it, so a completion that cannot succeed can be rejected before the atomic
/// consume spends the state.
///
/// # Errors
///
/// Returns [`ApiError`] when the query fails or the stored type is unknown.
pub(crate) async fn find_oauth_state_type(
    database: &Database,
    state: &str,
    now: i64,
) -> Result<Option<ProviderType>, ApiError> {
    let rows = database
        .query(
            "SELECT type FROM oauth_states WHERE state = ?1 AND expires_at > ?2",
            params![state, now],
        )
        .await?;

    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let type_name: String = row.get(0)?;

    let state_type = ProviderType::parse(&type_name).ok_or_else(|| {
        ApiError::internal(format!(
            "stored OAuth state carries unknown provider type `{type_name}`"
        ))
    })?;

    Ok(Some(state_type))
}

/// Consumes the OAuth state row for `state`, returning its facts only when it
/// is unexpired.
///
/// Consumption is a single atomic `DELETE … RETURNING` statement, so exactly
/// one concurrent completion can ever observe the row and a state can never
/// complete twice (README §3). An expired or unknown state consumes nothing
/// here; expired rows are swept separately by [`delete_expired_oauth_states`].
///
/// # Errors
///
/// Returns [`ApiError`] when the statement fails.
pub(crate) async fn consume_oauth_state(
    database: &Database,
    state: &str,
    now: i64,
) -> Result<Option<OAuthState>, ApiError> {
    let rows = database
        .query(
            "DELETE FROM oauth_states WHERE state = ?1 AND expires_at > ?2 \
             RETURNING type, verifier",
            params![state, now],
        )
        .await?;

    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let type_name: String = row.get(0)?;
    let verifier: String = row.get(1)?;

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
        Some(protocol_name) => Some(UpstreamFormat::parse(&protocol_name).ok_or_else(|| {
            ApiError::internal(format!(
                "provider row carries unknown protocol `{protocol_name}`"
            ))
        })?),
    };

    let name: String = row.get(1)?;
    let credentials_json: String = row.get(6)?;

    // The typed parse failure is wrapped exactly once as internal storage
    // drift: the row was written only through the validated management path,
    // so a payload failing here is never the requesting client's fault.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use std::{error::Error as StdError, sync::Arc, time::Duration};

    /// Number of consumers racing on one OAuth state in the exactly-once test.
    const STATE_CONSUMERS: usize = 8;

    /// Pool timeout long enough that tests never observe acquisition timeouts.
    const TEST_POOL_TIMEOUT: Duration = Duration::from_secs(5);

    /// Inserts one provider row with the given token expiry and refresh lease,
    /// returning its row identifier.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while inserting or reading back the row.
    async fn insert_lease_provider(
        database: &Database,
        name: &str,
        expires_at: Option<i64>,
        refresh_started_at: Option<i64>,
    ) -> Result<i64, Box<dyn StdError>> {
        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials, expires_at, \
                 refresh_started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    name,
                    "",
                    "openai_compatible",
                    r#"{"apiKey":"test-key"}"#,
                    expires_at,
                    refresh_started_at
                ],
            )
            .await?;

        let rows = database
            .query("SELECT id FROM providers WHERE name = ?1", params![name])
            .await?;

        Ok(rows.first().ok_or("missing provider row")?.get(0)?)
    }

    /// Reads back the stored refresh lease timestamp of a provider.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while querying the row.
    async fn read_refresh_lease(
        database: &Database,
        provider_id: i64,
    ) -> Result<Option<i64>, Box<dyn StdError>> {
        let rows = database
            .query(
                "SELECT refresh_started_at FROM providers WHERE id = ?1",
                params![provider_id],
            )
            .await?;

        Ok(rows.first().ok_or("missing provider row")?.get(0)?)
    }

    /// Verifies the lease is granted when the token is expired and no lease is
    /// held, and that acquiring stamps the lease timestamp.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_granted_when_token_expired_without_lease() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id =
            insert_lease_provider(&database, "expired", Some(now - 100), None).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        assert_eq!(lease_started_at, Some(now));

        assert_eq!(read_refresh_lease(&database, provider_id).await?, Some(now));

        Ok(())
    }

    /// Verifies the lease is denied while another holder owns a live lease,
    /// leaving the holder's timestamp untouched.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_denied_while_live_lease_held() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id =
            insert_lease_provider(&database, "held", Some(now - 100), Some(now - 10)).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        assert_eq!(lease_started_at, None);

        assert_eq!(
            read_refresh_lease(&database, provider_id).await?,
            Some(now - 10)
        );

        Ok(())
    }

    /// Verifies a lease abandoned past the staleness horizon is reclaimed by a
    /// new acquirer, which is the stale-then-retry path promised by README §3.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_reclaims_stale_lease() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id =
            insert_lease_provider(&database, "stale", Some(now - 100), Some(now - 91)).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        assert_eq!(lease_started_at, Some(now));

        assert_eq!(read_refresh_lease(&database, provider_id).await?, Some(now));

        Ok(())
    }

    /// Verifies the engine raises the stale-snapshot busy error when the
    /// lease acquire's read-then-write upgrade loses its snapshot to an
    /// interleaved commit, and that the error is classified as a stale
    /// snapshot: K concurrent acquirers on one expired token all hit this
    /// exact failure, and the token-refresh caller must recover it as "not
    /// acquired" — the poll-and-wait path of README §3 — instead of failing
    /// the request with a database error.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_acquire_after_interleaved_commit_classifies_stale_snapshot()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, TEST_POOL_TIMEOUT).await?;
        let now = crate::unix_time::unix_now();

        let provider_id = insert_lease_provider(&database, "racer", Some(now - 100), None).await?;

        // Winner and loser both read the provider row, pinning one read
        // snapshot for both transactions.
        let mut winner = database.transaction().await?;

        let _ = find_provider_by_id_in_transaction(&mut winner, provider_id).await?;

        let mut loser = database.transaction().await?;

        let _ = find_provider_by_id_in_transaction(&mut loser, provider_id).await?;

        // The winner acquires the lease and commits, landing a write after
        // the loser's snapshot was taken.
        let winner_lease =
            try_acquire_refresh_lease(&mut winner, provider_id, now, now - 90, 60).await?;

        assert_eq!(winner_lease, Some(now));

        winner.commit().await?;

        // The loser's lease write upgrades its stale snapshot and must fail
        // with the engine's stale-snapshot busy error, classified so the
        // refresh loop can treat the attempt as "not acquired".
        let error = try_acquire_refresh_lease(&mut loser, provider_id, now, now - 90, 60)
            .await
            .err()
            .ok_or("the stale-snapshot acquire must fail")?;

        assert!(
            crate::error::is_stale_snapshot(&error),
            "the interleaved acquire must fail with the stale-snapshot busy error: {error}"
        );

        drop(loser);

        Ok(())
    }

    /// Verifies the lease is denied while the token is still fresh and granted
    /// exactly at the freshness boundary (`expires_at == now + 60`).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_denied_while_token_fresh() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id = insert_lease_provider(&database, "fresh", Some(now + 61), None).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.rollback().await?;

        assert_eq!(lease_started_at, None);

        database
            .execute(
                "UPDATE providers SET expires_at = ?1 WHERE id = ?2",
                params![now + 60, provider_id],
            )
            .await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        assert_eq!(lease_started_at, Some(now));

        Ok(())
    }

    /// Verifies the lease is granted when the provider carries no token expiry
    /// at all.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_granted_when_expiry_absent() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id = insert_lease_provider(&database, "no_expiry", None, None).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        assert_eq!(lease_started_at, Some(now));

        Ok(())
    }

    /// Verifies the freshness margin drives the expired-token predicate from
    /// its parameter instead of a hardcoded constant: with margin `0`, a token
    /// one second in the future is fresh while a boundary token is expired.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn lease_honors_freshness_margin_parameter() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id = insert_lease_provider(&database, "margin", Some(now + 1), None).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 0).await?;

        transaction.rollback().await?;

        assert_eq!(lease_started_at, None);

        database
            .execute(
                "UPDATE providers SET expires_at = ?1 WHERE id = ?2",
                params![now, provider_id],
            )
            .await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 0).await?;

        transaction.commit().await?;

        assert_eq!(lease_started_at, Some(now));

        Ok(())
    }

    /// Verifies an unexpired state returns its facts exactly once: the first
    /// consume observes them, the row is deleted, and every later consume
    /// reports an unknown state.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn consume_oauth_state_returns_facts_exactly_once() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        insert_oauth_state(
            &database,
            ProviderType::OpenAiOauth,
            "state-live",
            "verifier",
            now + 60,
        )
        .await?;

        let consumed = consume_oauth_state(&database, "state-live", now).await?;

        let consumed = consumed.ok_or("first consume must observe the state")?;

        assert_eq!(consumed.state_type, ProviderType::OpenAiOauth);
        assert_eq!(consumed.verifier, "verifier");

        assert!(
            consume_oauth_state(&database, "state-live", now)
                .await?
                .is_none()
        );

        Ok(())
    }

    /// Verifies an expired state is rejected as unknown.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn consume_oauth_state_rejects_expired_state() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        insert_oauth_state(
            &database,
            ProviderType::AntigravityOauth,
            "state-expired",
            "",
            now - 1,
        )
        .await?;

        assert!(
            consume_oauth_state(&database, "state-expired", now)
                .await?
                .is_none()
        );

        Ok(())
    }

    /// Verifies the exactly-once completion contract under concurrency: many
    /// simultaneous consumers of one state produce exactly one winner, so a
    /// state can never complete two providers (README §3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn consume_oauth_state_completes_exactly_once_under_concurrency()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        for round in 0..24 {
            let state = format!("state-race-{round}");

            insert_oauth_state(
                &database,
                ProviderType::OpenAiOauth,
                &state,
                "verifier",
                now + 60,
            )
            .await?;

            let barrier = Arc::new(tokio::sync::Barrier::new(STATE_CONSUMERS));

            let mut join_handles = Vec::new();

            for _ in 0..STATE_CONSUMERS {
                let database = database.clone();
                let barrier = Arc::clone(&barrier);
                let state = state.clone();

                join_handles.push(tokio::spawn(async move {
                    barrier.wait().await;

                    consume_oauth_state(&database, &state, now).await
                }));
            }

            let mut winner_count = 0;

            for handle in join_handles {
                if handle.await??.is_some() {
                    winner_count += 1;
                }
            }

            assert_eq!(winner_count, 1, "state {state} completed more than once");
        }

        Ok(())
    }

    /// Inserts one `openai_compatible` provider row carrying the given model
    /// prefix, returning its row identifier.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while inserting or reading back the row.
    async fn insert_prefix_provider(
        database: &Database,
        name: &str,
        model_prefix: &str,
    ) -> Result<i64, Box<dyn StdError>> {
        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, protocol, base_url, credentials) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    name,
                    model_prefix,
                    "openai_compatible",
                    "openai_chat_completion",
                    "https://upstream.example",
                    r#"{"apiKey":"test-key"}"#
                ],
            )
            .await?;

        let rows = database
            .query("SELECT id FROM providers WHERE name = ?1", params![name])
            .await?;

        Ok(rows.first().ok_or("missing provider row")?.get(0)?)
    }

    /// Inserts one model row with explicitly supplied stored values, bypassing
    /// the write-time composition of the public name.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while inserting the row.
    async fn insert_model_row(
        database: &Database,
        provider_id: i64,
        upstream_name: &str,
        name: &str,
        name_override: Option<&str>,
    ) -> Result<(), Box<dyn StdError>> {
        database
            .execute(
                "INSERT INTO models (provider_id, upstream_name, name, name_override) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![provider_id, upstream_name, name, name_override],
            )
            .await?;

        Ok(())
    }

    /// Reads back one model's stored public name.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while querying the row.
    async fn read_model_name(
        database: &Database,
        provider_id: i64,
        upstream_name: &str,
    ) -> Result<Option<String>, Box<dyn StdError>> {
        let rows = database
            .query(
                "SELECT name FROM models WHERE provider_id = ?1 AND upstream_name = ?2",
                params![provider_id, upstream_name],
            )
            .await?;

        Ok(rows.first().map(|row| row.get(0)).transpose()?)
    }

    /// Reads back one provider row's mutable configuration.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while querying the row.
    async fn read_provider_row(
        database: &Database,
        name: &str,
    ) -> Result<(String, i64, Option<String>, Option<String>), Box<dyn StdError>> {
        let rows = database
            .query(
                "SELECT model_prefix, enabled, protocol, base_url FROM providers WHERE name = ?1",
                params![name],
            )
            .await?;

        let row = rows.first().ok_or("missing provider row")?;

        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    }

    /// Verifies a `save-provider` update recomputes every public model name
    /// from the new prefix through the shared composition rule — including
    /// models carrying a `name_override` — and rewrites the provider row.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_recomputes_all_public_names() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "p-").await?;

        insert_model_row(&database, provider_id, "gpt-x", "p-gpt-x", None).await?;
        insert_model_row(&database, provider_id, "gpt-y", "p-custom", Some("custom")).await?;

        let stored = update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "svc/",
                protocol: Some(UpstreamFormat::OpenAiResponse),
                base_url: Some("https://other.example"),
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await?;

        assert_eq!(stored.name, "alpha");
        assert_eq!(stored.model_prefix, "svc/");
        assert!(!stored.is_enabled);
        assert_eq!(
            stored.credentials,
            Credentials::ApiKey("test-key".to_string())
        );

        assert_eq!(
            read_model_name(&database, provider_id, "gpt-x").await?,
            Some("svc/gpt-x".to_string())
        );
        assert_eq!(
            read_model_name(&database, provider_id, "gpt-y").await?,
            Some("svc/custom".to_string())
        );

        let (model_prefix, enabled, protocol, base_url) =
            read_provider_row(&database, "alpha").await?;

        assert_eq!(model_prefix, "svc/");
        assert_eq!(enabled, 0);
        assert_eq!(protocol.as_deref(), Some("openai_response"));
        assert_eq!(base_url.as_deref(), Some("https://other.example"));

        Ok(())
    }

    /// Verifies a prefix change whose recomputed public name is already routed
    /// by another provider is rejected with 409 and leaves every row — the
    /// updated provider's own row included — untouched.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_rejects_foreign_name_collision_and_rolls_back()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let alpha_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, alpha_id, "m1", "a-m1", None).await?;

        let beta_id = insert_prefix_provider(&database, "beta", "").await?;

        insert_model_row(&database, beta_id, "m1", "m1", None).await?;

        let error = update_provider(
            &database,
            alpha_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "",
                protocol: None,
                base_url: None,
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await
        .err()
        .ok_or("prefix change must be rejected when a recomputed name is already routed")?;

        assert_eq!(error.status, 409);

        let (model_prefix, enabled, _, _) = read_provider_row(&database, "alpha").await?;

        assert_eq!(model_prefix, "a-");
        assert_eq!(enabled, 1, "provider row must be untouched after rollback");
        assert_eq!(
            read_model_name(&database, alpha_id, "m1").await?,
            Some("a-m1".to_string())
        );
        assert_eq!(
            read_model_name(&database, beta_id, "m1").await?,
            Some("m1".to_string())
        );

        Ok(())
    }

    /// Verifies two of one provider's own models converging on one recomputed
    /// public name are rejected with 409 and nothing is written. Healthy rows
    /// can never converge — distinct `name_override ?? upstream_name` bases
    /// stay distinct under any prefix — so this drifted pre-state is pinned
    /// through direct inserts to guard the invariant check itself.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_rejects_same_provider_name_convergence()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "gamma", "p-").await?;

        insert_model_row(&database, provider_id, "u1", "keep-1", None).await?;
        insert_model_row(&database, provider_id, "u2", "keep-2", Some("u1")).await?;

        let error = update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "gamma",
                model_prefix: "",
                protocol: None,
                base_url: None,
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await
        .err()
        .ok_or("converging rename must be rejected")?;

        assert_eq!(error.status, 409);

        let (model_prefix, _, _, _) = read_provider_row(&database, "gamma").await?;

        assert_eq!(model_prefix, "p-");
        assert_eq!(
            read_model_name(&database, provider_id, "u1").await?,
            Some("keep-1".to_string())
        );
        assert_eq!(
            read_model_name(&database, provider_id, "u2").await?,
            Some("keep-2".to_string())
        );

        Ok(())
    }

    /// Verifies updating a provider that owns no models succeeds trivially and
    /// still rewrites the provider row.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_with_no_models_succeeds() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "delta", "p-").await?;

        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "delta",
                model_prefix: "q-",
                protocol: None,
                base_url: None,
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await?;

        let (model_prefix, enabled, _, _) = read_provider_row(&database, "delta").await?;

        assert_eq!(model_prefix, "q-");
        assert_eq!(enabled, 0);

        Ok(())
    }

    /// Verifies an update applying an empty model prefix stores it verbatim:
    /// the prefix is non-null but may be empty, so the empty string is a
    /// legitimate value and never replaced by any fallback (README §3, §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_applies_empty_prefix_verbatim() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        let stored = update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "",
                protocol: None,
                base_url: None,
                is_enabled: None,
                credentials: None,
            },
        )
        .await?;

        assert_eq!(stored.model_prefix, "");

        let (model_prefix, _, _, _) = read_provider_row(&database, "alpha").await?;

        assert_eq!(model_prefix, "");

        Ok(())
    }

    /// Verifies a `save-provider` update merges absent fields from the row
    /// re-read inside its transaction, so a stale outer snapshot can never
    /// revert a field a concurrent update already wrote (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_merges_absent_fields_from_fresh_row() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let alpha_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        // The caller's outer read observes the provider enabled.
        let outer = find_provider(&database, "alpha")
            .await?
            .ok_or("missing provider row")?;

        assert!(outer.is_enabled);

        // A concurrent save-provider disables the provider before the
        // caller's update commits, restating the stored prefix.
        update_provider(
            &database,
            alpha_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "a-",
                protocol: None,
                base_url: None,
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await?;

        // The caller commits an update supplying only the new prefix.
        let stored = update_provider(
            &database,
            alpha_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "b-",
                protocol: None,
                base_url: None,
                is_enabled: None,
                credentials: None,
            },
        )
        .await?;

        assert_eq!(stored.model_prefix, "b-");

        assert!(
            !stored.is_enabled,
            "the stale outer read must not revert the newer stored enabled state"
        );

        let (model_prefix, enabled, _, _) = read_provider_row(&database, "alpha").await?;

        assert_eq!(model_prefix, "b-");
        assert_eq!(enabled, 0);

        Ok(())
    }

    /// Verifies updating a provider deleted between the caller's outer read
    /// and the update is rejected with 404 instead of silently writing no
    /// rows.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn provider_update_rejects_vanished_provider_as_not_found()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let alpha_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        // The caller's outer read observes the row.
        let _outer = find_provider(&database, "alpha").await?;

        delete_provider(&database, alpha_id).await?;

        let error = update_provider(
            &database,
            alpha_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "a-",
                protocol: None,
                base_url: None,
                is_enabled: Some(false),
                credentials: None,
            },
        )
        .await
        .err()
        .ok_or("a vanished provider must be rejected")?;

        assert_eq!(error.status, 404);
        assert!(error.message.contains("does not exist"));

        Ok(())
    }

    /// Reads back the stored credentials JSON of a provider.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while querying the row.
    async fn read_provider_credentials(
        database: &Database,
        provider_id: i64,
    ) -> Result<String, Box<dyn StdError>> {
        let rows = database
            .query(
                "SELECT credentials FROM providers WHERE id = ?1",
                params![provider_id],
            )
            .await?;

        Ok(rows.first().ok_or("missing provider row")?.get(0)?)
    }

    /// Verifies the lease-guarded credential persist succeeds only under the
    /// caller's own lease stamp, persisting the credentials and clearing the
    /// lease.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn credential_persist_requires_own_lease_stamp() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id = insert_lease_provider(&database, "stamp", Some(now - 100), None).await?;

        let mut transaction = database.transaction().await?;

        let lease_started_at =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        let lease_started_at = lease_started_at.ok_or("the lease must be acquired")?;

        let is_persisted = update_credentials_for_lease(
            &database,
            provider_id,
            lease_started_at,
            &Credentials::ApiKey("renewed-key".to_string()),
            Some(now + 3_600),
        )
        .await?;

        assert!(is_persisted);

        assert_eq!(
            read_provider_credentials(&database, provider_id).await?,
            r#"{"apiKey":"renewed-key"}"#
        );

        assert_eq!(read_refresh_lease(&database, provider_id).await?, None);

        Ok(())
    }

    /// Verifies a refresh whose lease was stolen by a stale-lease
    /// reacquisition cannot persist tokens derived from superseded
    /// credentials: the conditional persist reports the loss and the newer
    /// holder's lease and credentials survive untouched.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn credential_persist_loses_to_lease_reacquisition() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        // The in-flight refresher acquired its lease 100 seconds ago.
        let provider_id =
            insert_lease_provider(&database, "stolen", Some(now - 100), Some(now - 100)).await?;

        // A waiter reacquires the abandoned lease now.
        let mut transaction = database.transaction().await?;

        let is_lease_acquired =
            try_acquire_refresh_lease(&mut transaction, provider_id, now, now - 90, 60).await?;

        transaction.commit().await?;

        assert_eq!(is_lease_acquired, Some(now));

        let is_persisted = update_credentials_for_lease(
            &database,
            provider_id,
            now - 100,
            &Credentials::ApiKey("superseded-key".to_string()),
            Some(now + 3_600),
        )
        .await?;

        assert!(!is_persisted, "the stale stamp must not persist");

        assert_eq!(
            read_provider_credentials(&database, provider_id).await?,
            r#"{"apiKey":"test-key"}"#
        );

        assert_eq!(
            read_refresh_lease(&database, provider_id).await?,
            Some(now),
            "the newer holder's lease must survive"
        );

        Ok(())
    }

    /// Verifies a refresh whose lease was cleared by a concurrent admin
    /// credentials replacement cannot overwrite the replacement with tokens
    /// derived from the pre-replacement refresh token.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn credential_persist_loses_to_credentials_replacement() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id =
            insert_lease_provider(&database, "replaced", Some(now - 100), Some(now - 5)).await?;

        // The admin replaces the credentials, clearing the lease.
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "replaced",
                model_prefix: "",
                protocol: None,
                base_url: None,
                is_enabled: Some(true),
                credentials: Some(&Credentials::ApiKey("admin-key".to_string())),
            },
        )
        .await?;

        let is_persisted = update_credentials_for_lease(
            &database,
            provider_id,
            now - 5,
            &Credentials::ApiKey("superseded-key".to_string()),
            Some(now + 3_600),
        )
        .await?;

        assert!(!is_persisted, "the in-flight refresher must lose the row");

        assert_eq!(
            read_provider_credentials(&database, provider_id).await?,
            r#"{"apiKey":"admin-key"}"#
        );

        Ok(())
    }

    /// Verifies the OAuth-completion credentials write reports whether it
    /// wrote: a provider deleted between the caller's outer read and the
    /// update is reported as unwritten — rendered as the refresh path's 404
    /// by the completion handler — instead of silently writing nothing.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn credential_update_reports_vanished_provider() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        let provider_id =
            insert_lease_provider(&database, "deleted", Some(now - 100), None).await?;

        // The completion's outer read observes the row before a concurrent
        // delete-provider removes it.
        let existing = find_provider(&database, "deleted")
            .await?
            .ok_or("missing provider row")?;

        assert_eq!(existing.id, provider_id);

        delete_provider(&database, provider_id).await?;

        let is_updated = update_credentials(
            &database,
            existing.id,
            &Credentials::ApiKey("exchanged-key".to_string()),
            Some(now + 3_600),
        )
        .await?;

        assert!(
            !is_updated,
            "a vanished provider must be reported as unwritten"
        );

        Ok(())
    }

    /// Verifies a stored provider row carrying malformed credentials decodes
    /// as a 500 internal error that names the drift and wraps the typed parse
    /// failure exactly once: stored credentials are written only through the
    /// validated management path, so a payload failing here is database drift
    /// and never a client fault.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn malformed_stored_credentials_decode_as_internal_wrapped_once()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        // An OpenAI OAuth row missing `refreshToken` and carrying a stray
        // `expiresAt` field is double drift; the decode must fail either way.
        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials) \
                 VALUES ('drifted', '', 'openai_oauth', '{\"accessToken\":\"a\"}')",
                (),
            )
            .await?;

        #[expect(clippy::panic, reason = "an unexpected success fails the test")]
        let error = find_provider(&database, "drifted")
            .await
            .err()
            .unwrap_or_else(|| panic!("malformed stored credentials must fail decoding"));

        assert_eq!(error.status, 500);

        assert_eq!(error.code.as_deref(), Some("internal"));

        assert!(
            error
                .message
                .starts_with("provider `drifted` has malformed credentials: "),
            "the drift must be named exactly once at the wrap: {error}"
        );

        assert_eq!(
            error.message.matches("malformed credentials").count(),
            1,
            "the wrap must not double-prefix the cause: {error}"
        );

        Ok(())
    }

    /// Reads back one model's stored override fields alongside its name.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while querying the row.
    async fn read_model_row(
        database: &Database,
        provider_id: i64,
        upstream_name: &str,
    ) -> Result<(String, Option<String>, Option<i64>, Option<String>), Box<dyn StdError>> {
        let rows = database
            .query(
                "SELECT name, name_override, enabled_override, thinking_levels_override \
                 FROM models WHERE provider_id = ?1 AND upstream_name = ?2",
                params![provider_id, upstream_name],
            )
            .await?;

        let row = rows.first().ok_or("missing model row")?;

        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    }

    /// Verifies a discovery replace keys overrides over by `upstream_name`,
    /// drops undiscovered rows, and recomputes every public name from the
    /// provider's current prefix.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn discover_replaces_models_and_carries_overrides() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, provider_id, "u1", "a-custom", Some("custom")).await?;
        insert_model_row(&database, provider_id, "gone", "a-gone", None).await?;

        let discovered = replace_discovered_models(
            &database,
            provider_id,
            &["u1".to_string(), "u2".to_string()],
        )
        .await?;

        assert_eq!(discovered, 2);

        let (name, name_override, enabled_override, _) =
            read_model_row(&database, provider_id, "u1").await?;

        assert_eq!(name, "a-custom");
        assert_eq!(name_override.as_deref(), Some("custom"));
        assert!(enabled_override.is_none());

        let (name, name_override, _, _) = read_model_row(&database, provider_id, "u2").await?;

        assert_eq!(name, "a-u2");
        assert_eq!(name_override, None);

        assert!(
            read_model_name(&database, provider_id, "gone")
                .await?
                .is_none(),
            "undiscovered rows must vanish with their overrides"
        );

        Ok(())
    }

    /// Verifies a discovery replace carrying an empty upstream set is
    /// rejected with 502 and leaves every stored row — overrides included —
    /// untouched: an upstream outage answering 200 with an empty payload, or
    /// a fresh pre-onboarded account, must fail loudly instead of wiping the
    /// provider's curated routing state.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn discover_rejects_empty_upstream_set_without_wiping_rows()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, provider_id, "u1", "a-custom", Some("custom")).await?;

        let error = replace_discovered_models(&database, provider_id, &[])
            .await
            .err()
            .ok_or("an empty discovered set must be rejected")?;

        assert_eq!(error.status, 502);
        assert!(
            error.message.contains("no models"),
            "the error must state the upstream reported no models: {error}"
        );

        let (name, name_override, _, _) = read_model_row(&database, provider_id, "u1").await?;

        assert_eq!(name, "a-custom");
        assert_eq!(name_override.as_deref(), Some("custom"));

        Ok(())
    }

    /// Verifies a discovery replace that would route a name already held by
    /// another provider is rejected with a name-specific 409 and writes
    /// nothing.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn discover_rejects_routed_collision_with_name_specific_conflict()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        // The empty prefix lets alpha's composed name collide with beta's.
        let alpha_id = insert_prefix_provider(&database, "alpha", "").await?;

        insert_model_row(&database, alpha_id, "u1", "u1", None).await?;

        let beta_id = insert_prefix_provider(&database, "beta", "").await?;

        insert_model_row(&database, beta_id, "m1", "m1", None).await?;

        let error =
            replace_discovered_models(&database, alpha_id, &["u1".to_string(), "m1".to_string()])
                .await
                .err()
                .ok_or("the routed collision must be rejected")?;

        assert_eq!(error.status, 409);
        assert!(
            error.message.contains("m1") && error.message.contains("beta"),
            "the conflict must name the colliding name and holder: {error}"
        );

        assert_eq!(
            read_model_name(&database, alpha_id, "u1").await?,
            Some("u1".to_string()),
            "the colliding refresh must not write anything"
        );

        assert!(read_model_name(&database, alpha_id, "m1").await?.is_none());

        Ok(())
    }

    /// Verifies the discovery replace composes public names from the provider
    /// prefix re-read inside its transaction: a prefix changed between the
    /// caller's outer read and the replace call decides the stored names, so a
    /// concurrent `save-provider` prefix change is never silently reverted.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn discover_composes_names_from_prefix_re_read_in_transaction()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        // The caller's outer read observes the old prefix.
        let outer = find_provider(&database, "alpha")
            .await?
            .ok_or("missing provider row")?;

        assert_eq!(outer.model_prefix, "a-");

        // A concurrent save-provider changes the prefix before the replace.
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "b-",
                protocol: None,
                base_url: None,
                is_enabled: Some(true),
                credentials: None,
            },
        )
        .await?;

        replace_discovered_models(&database, provider_id, &["gpt-x".to_string()]).await?;

        assert_eq!(
            read_model_name(&database, provider_id, "gpt-x").await?,
            Some("b-gpt-x".to_string()),
            "the fresh in-transaction prefix must decide the public name"
        );

        Ok(())
    }

    /// Verifies a discovery replace whose carried-over overrides converge two
    /// of the provider's own models on one public name is rejected with 409
    /// and writes nothing.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn discover_rejects_own_name_convergence() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "gamma", "x-").await?;

        // The drifted stored name of `u2` keeps the direct insert itself
        // collision-free; the override makes the recomputed names converge.
        insert_model_row(&database, provider_id, "u1", "x-u1", None).await?;
        insert_model_row(&database, provider_id, "u2", "x-keep", Some("u1")).await?;

        let error = replace_discovered_models(
            &database,
            provider_id,
            &["u1".to_string(), "u2".to_string()],
        )
        .await
        .err()
        .ok_or("the convergence must be rejected")?;

        assert_eq!(error.status, 409);
        assert!(
            error.message.contains("u1") && error.message.contains("u2"),
            "the conflict must name both converging upstream models: {error}"
        );

        assert_eq!(
            read_model_name(&database, provider_id, "u1").await?,
            Some("x-u1".to_string())
        );

        Ok(())
    }

    /// Verifies `save-model` recomputes the public name from the provider
    /// prefix re-read inside its transaction and writes the supplied
    /// overrides atomically with the collision check.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_model_applies_overrides_with_fresh_prefix() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, provider_id, "u1", "a-u1", None).await?;

        // A concurrent save-provider changes the prefix after the outer read
        // the handler would have made.
        update_provider(
            &database,
            provider_id,
            ProviderUpdate {
                name: "alpha",
                model_prefix: "b-",
                protocol: None,
                base_url: None,
                is_enabled: Some(true),
                credentials: None,
            },
        )
        .await?;

        let entry = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_name: "u1",
                name_override: Some(Some("hero".to_string())),
                enabled_override: Some(Some(false)),
                thinking_levels_override: None,
            },
        )
        .await?;

        assert_eq!(entry.provider_name, "alpha");

        assert!(
            entry.is_provider_enabled,
            "is_provider_enabled reflects the provider row, which stays enabled"
        );

        assert_eq!(entry.model.name, "b-hero", "the fresh prefix must decide");
        assert_eq!(entry.model.name_override.as_deref(), Some("hero"));
        assert_eq!(entry.model.enabled_override, Some(false));

        Ok(())
    }

    /// Verifies `save-model` rejecting a public-name collision with another
    /// provider's model yields a name-specific 409 and writes nothing.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_model_rejects_collision_and_writes_nothing() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        // The empty prefix lets alpha's override name collide with beta's.
        let alpha_id = insert_prefix_provider(&database, "alpha", "").await?;

        insert_model_row(&database, alpha_id, "u1", "u1", None).await?;

        let beta_id = insert_prefix_provider(&database, "beta", "").await?;

        insert_model_row(&database, beta_id, "m1", "m1", None).await?;

        let error = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id: alpha_id,
                upstream_name: "u1",
                name_override: Some(Some("m1".to_string())),
                enabled_override: None,
                thinking_levels_override: None,
            },
        )
        .await
        .err()
        .ok_or("the collision must be rejected")?;

        assert_eq!(error.status, 409);
        assert!(
            error.message.contains("m1") && error.message.contains("beta"),
            "the conflict must name the colliding name and holder: {error}"
        );

        let (name, name_override, enabled_override, _) =
            read_model_row(&database, alpha_id, "u1").await?;

        assert_eq!(name, "u1");
        assert_eq!(name_override, None);
        assert_eq!(enabled_override, None);

        Ok(())
    }

    /// Verifies `save-model` clearing the name override with `null` recomputes
    /// the public name from the upstream name.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_model_clears_name_override_with_null() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, provider_id, "u1", "a-custom", Some("custom")).await?;

        let entry = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_name: "u1",
                name_override: Some(None),
                enabled_override: None,
                thinking_levels_override: None,
            },
        )
        .await?;

        assert_eq!(entry.model.name, "a-u1");
        assert_eq!(entry.model.name_override, None);

        Ok(())
    }

    /// Verifies `save-model` rejects unknown providers and unknown models
    /// with 404.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_model_rejects_unknown_rows() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let provider_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, provider_id, "u1", "a-u1", None).await?;

        let unknown_provider = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id: 999_999,
                upstream_name: "u1",
                name_override: None,
                enabled_override: None,
                thinking_levels_override: None,
            },
        )
        .await
        .err()
        .ok_or("unknown provider must be rejected")?;

        assert_eq!(unknown_provider.status, 404);

        let unknown_model = update_model_overrides_checked(
            &database,
            ModelOverrideUpdate {
                provider_id,
                upstream_name: "missing",
                name_override: None,
                enabled_override: None,
                thinking_levels_override: None,
            },
        )
        .await
        .err()
        .ok_or("unknown model must be rejected")?;

        assert_eq!(unknown_model.status, 404);
        assert!(unknown_model.message.contains("missing"));

        Ok(())
    }

    /// Verifies the enabled-model listing pairs every effectively enabled
    /// public name with its owning provider's name, excludes rows disabled by
    /// override or by provider, and stays ordered by public name.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn list_enabled_models_with_owner_maps_names_to_providers()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;

        let alpha_id = insert_prefix_provider(&database, "alpha", "a-").await?;

        insert_model_row(&database, alpha_id, "u2", "a-m2", None).await?;
        insert_model_row(&database, alpha_id, "u1", "a-m1", None).await?;
        insert_model_row(&database, alpha_id, "u3", "a-off", None).await?;

        database
            .execute(
                "UPDATE models SET enabled_override = 0 WHERE upstream_name = 'u3'",
                (),
            )
            .await?;

        let beta_id = insert_prefix_provider(&database, "beta", "b-").await?;

        insert_model_row(&database, beta_id, "u1", "b-m1", None).await?;

        database
            .execute("UPDATE providers SET enabled = 0 WHERE name = 'beta'", ())
            .await?;

        let listed = list_enabled_models_with_owner(&database).await?;

        assert_eq!(
            listed,
            vec![
                ("a-m1".to_string(), "alpha".to_string()),
                ("a-m2".to_string(), "alpha".to_string()),
            ]
        );

        Ok(())
    }

    /// Verifies the expired-state sweep deletes only rows past their expiry,
    /// leaving pending flows untouched: the sweep runs whenever
    /// `start-provider-oauth` starts a new flow (README §3, §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn delete_expired_oauth_states_sweeps_only_expired_rows() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(STATE_CONSUMERS, TEST_POOL_TIMEOUT)
                .await?;
        let now = crate::unix_time::unix_now();

        insert_oauth_state(
            &database,
            ProviderType::OpenAiOauth,
            "state-expired",
            "",
            now - 1,
        )
        .await?;

        insert_oauth_state(
            &database,
            ProviderType::AntigravityOauth,
            "state-live",
            "verifier",
            now + 60,
        )
        .await?;

        delete_expired_oauth_states(&database, now).await?;

        let rows = database.query("SELECT state FROM oauth_states", ()).await?;

        let survivors: Vec<String> = rows
            .iter()
            .map(|row| row.get(0))
            .collect::<Result<_, _>>()?;

        assert_eq!(survivors, vec!["state-live".to_string()]);

        Ok(())
    }
}
