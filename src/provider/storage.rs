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
