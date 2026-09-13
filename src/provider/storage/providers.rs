//! Provider row persistence: CRUD, credential replacement, and the token
//! refresh lease.

use super::{Error, models};
use crate::{
    database::{Database, Transaction, params},
    format::UpstreamFormat,
    provider::{Credentials, Model, NewProvider, Provider},
};

/// Inserts one provider row and returns the stored row constructed from the
/// supplied fields, mirroring [`update_provider`]'s contract: the returned
/// row is exactly what the statement wrote, so callers never re-read after
/// creation.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails.
pub(crate) async fn insert_provider(
    database: &Database,
    provider: &NewProvider<'_>,
) -> Result<Provider, Error> {
    let rows = database
        .query(
            "INSERT INTO providers (name, model_prefix, type, protocol, base_url, credentials, \
             expires_at, is_enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
            params![
                provider.name,
                provider.model_prefix,
                provider.provider_type,
                provider.protocol.map(UpstreamFormat::as_str),
                provider.base_url,
                provider.credentials.to_json(),
                provider.expires_at,
                i64::from(provider.is_enabled),
            ],
        )
        .await?;

    #[expect(
        clippy::indexing_slicing,
        reason = "a single-row INSERT RETURNING yields exactly the inserted row, or errors"
    )]
    let id: i64 = rows[0].get(0)?;

    Ok(Provider {
        id,
        name: provider.name.to_string(),
        model_prefix: provider.model_prefix.to_string(),
        provider_type: provider.provider_type,
        protocol: provider.protocol,
        base_url: provider.base_url.map(str::to_string),
        credentials: provider.credentials.clone(),
        expires_at: provider.expires_at,
        is_enabled: provider.is_enabled,
    })
}

/// Lists every stored provider.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_providers(database: &Database) -> Result<Vec<Provider>, Error> {
    let sql = format!(
        "SELECT {} FROM providers ORDER BY name",
        super::PROVIDER_COLUMNS
    );

    let rows = database.query(sql, ()).await?;

    rows.iter().map(super::provider_from_row).collect()
}

/// The `WHERE name = ?1` provider lookup. Names are unique, so this is the
/// name-taken check behind creation; all addressing of an existing provider
/// goes through its id.
fn find_provider_sql() -> String {
    format!(
        "SELECT {} FROM providers WHERE name = ?1",
        super::PROVIDER_COLUMNS
    )
}

/// The `WHERE id = ?1` provider lookup shared by the pooled and transactional
/// executors, which share no query trait.
fn find_provider_by_id_sql() -> String {
    format!(
        "SELECT {} FROM providers WHERE id = ?1",
        super::PROVIDER_COLUMNS
    )
}

/// Fetches one provider by name, for creation-time name-taken checks.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the row is malformed.
pub(crate) async fn find_provider(
    database: &Database,
    name: &str,
) -> Result<Option<Provider>, Error> {
    let rows = database.query(find_provider_sql(), params![name]).await?;

    rows.first().map(super::provider_from_row).transpose()
}

/// Fetches one provider by id.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the row is malformed.
pub(crate) async fn find_provider_by_id(
    database: &Database,
    provider_id: i64,
) -> Result<Option<Provider>, Error> {
    let rows = database
        .query(find_provider_by_id_sql(), params![provider_id])
        .await?;

    rows.first().map(super::provider_from_row).transpose()
}

/// Fetches one provider by id inside a transaction, mirroring
/// [`find_provider_by_id`] against the transactional executor.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the row is malformed.
pub(crate) async fn find_provider_by_id_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Option<Provider>, Error> {
    let rows = transaction
        .query(find_provider_by_id_sql(), params![provider_id])
        .await?;

    rows.first().map(super::provider_from_row).transpose()
}

/// Fetches one provider by id after a write, failing with an internal error
/// when the row is gone: a vanished row after a successful write is storage
/// drift — an invariant violation — and never a client fault.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the provider does not exist.
pub(crate) async fn reload_provider(
    database: &Database,
    provider_id: i64,
    context: &str,
) -> Result<Provider, Error> {
    find_provider_by_id(database, provider_id)
        .await?
        .ok_or_else(|| Error::Drift {
            message: format!("provider vanished after {context}"),
        })
}

/// Deletes one provider by id, returning whether a row was removed.
///
/// # Errors
///
/// Returns [`Error`] when the delete fails.
pub(crate) async fn delete_provider(database: &Database, provider_id: i64) -> Result<bool, Error> {
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
/// Returns [`Error`] when the update fails.
pub(crate) async fn update_credentials(
    database: &Database,
    provider_id: i64,
    credentials: &Credentials,
    expires_at: Option<i64>,
) -> Result<bool, Error> {
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
/// the README §3 invariant — the public `id` of unoverridden model rows,
/// recomputed from the effective prefix through the shared composition rule
/// [`Model::public_id`]. Nothing is written when the new name is already
/// taken by another provider, when any recomputed public ID would collide
/// with an existing model row of another provider, or when two of the
/// provider's own models would converge on one public ID; all rejections
/// are 409 conflicts. Abandoning the transaction on any error discards every
/// partial write, so the provider row and its model IDs can never drift
/// apart.
///
/// # Errors
///
/// Returns [`Error`] for an unknown provider, a failed statement, a taken
/// name, or a recomputed public ID collision.
pub(crate) async fn update_provider(
    database: &Database,
    provider_id: i64,
    update: ProviderUpdate<'_>,
) -> Result<Provider, Error> {
    let mut transaction = database.transaction().await?;

    // The merge reads the row inside the transaction, so absent fields keep
    // the newest stored values, never the caller's snapshot.
    let Some(provider) = find_provider_by_id_in_transaction(&mut transaction, provider_id).await?
    else {
        return Err(Error::NotFound {
            message: super::provider_not_found_message(provider_id),
        });
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
        return Err(Error::Conflict {
            message: super::provider_name_taken_message(name),
        });
    }

    let model_prefix = update.model_prefix;

    let protocol = update.protocol.or(provider.protocol);

    let base_url = update.base_url.or(provider.base_url.as_deref());

    let is_enabled = update.is_enabled.unwrap_or(provider.is_enabled);

    let is_credentials_replaced = update.credentials.is_some();

    let models = models::find_models_in_transaction(&mut transaction, provider.id).await?;

    let recomputed: Vec<(String, &Model)> = models
        .iter()
        .map(|model| {
            let id = Model::public_id(
                model_prefix,
                &model.upstream_id,
                model.id_override.as_deref(),
            );

            (id, model)
        })
        .collect();

    let composed: Vec<(&str, &str)> = recomputed
        .iter()
        .map(|(id, model)| (id.as_str(), model.upstream_id.as_str()))
        .collect();

    super::ensure_ids_pairwise_distinct(name, &composed)?;

    super::ensure_ids_routable(&mut transaction, provider.id, &composed).await?;

    transaction
        .execute(
            "UPDATE providers SET name = ?1, model_prefix = ?2, protocol = ?3, base_url = ?4, \
             is_enabled = ?5 WHERE id = ?6",
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

    for (new_id, model) in &recomputed {
        if new_id == &model.id {
            continue;
        }

        transaction
            .execute(
                "UPDATE models SET id = ?1 WHERE provider_id = ?2 AND upstream_id = ?3",
                params![new_id.as_str(), provider.id, model.upstream_id.as_str()],
            )
            .await?;
    }

    transaction.commit().await?;

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

/// Attempts to acquire the token refresh lease inside a transaction.
///
/// The lease is granted only when the token is still expired (no later than
/// `freshness_margin` seconds into the future) and no other holder owns a
/// lease acquired after `lease_stale_before`. Returns the acquired lease
/// stamp, or `None` when another holder owns a live lease.
///
/// The raw [`crate::database::Error`] is returned instead of an [`Error`]
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
/// Returns [`Error`] when the update fails.
pub(crate) async fn update_credentials_for_lease(
    database: &Database,
    provider_id: i64,
    lease_started_at: i64,
    credentials: &Credentials,
    expires_at: Option<i64>,
) -> Result<bool, Error> {
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
/// Returns [`Error`] when the update fails.
pub(crate) async fn clear_refresh_lease(
    database: &Database,
    provider_id: i64,
) -> Result<(), Error> {
    database
        .execute(
            "UPDATE providers SET refresh_started_at = NULL WHERE id = ?1",
            params![provider_id],
        )
        .await?;

    Ok(())
}
