//! Provider row persistence: CRUD, definition rebinding, credential replacement, and the JSON
//! token-refresh lease.
//!
//! The refresh lease lives inside the `credentials` JSON document (the OAuth variant's
//! `refresh_started_at` member), and every lease transition conditions on
//! `json_extract(credentials, …)` — the acquire on the stored expiry and a live competing lease,
//! the persist on the acquired stamp — so a concurrent reauthorization or stale-lease
//! reacquisition is never overwritten by a refresh derived from superseded credentials.

use super::{Error, SqlExecutor};
use crate::{
    database::{
        Database, Error as DatabaseError, Row, Transaction, TursoError, Value, params,
        params::IntoValue,
    },
    provider::{CredentialKind, Provider, StoredCredentials, definition::ProviderDefinition},
};
use serde::Serialize;
use std::sync::LazyLock;

/// Inserts one provider row through the pooled or transactional executor, returning the new
/// row's id: the statement is written once for both the pooled creation path and the device
/// poller's atomic provider-write-plus-outcome commit.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails.
async fn insert_provider_row<E: SqlExecutor>(
    mut executor: E,
    provider: NewProvider<'_>,
) -> Result<i64, Error> {
    let rows = executor
        .query(
            "INSERT INTO providers (name, provider_definition_name, provider_definition_version, \
             is_enabled, base_url, credentials, model_prefix) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
            params![
                provider.name,
                provider.definition.name.as_str(),
                provider.definition.version.get(),
                i64::from(provider.is_enabled),
                provider.base_url,
                provider.credentials.to_json(),
                provider.model_prefix,
            ],
        )
        .await?;

    #[expect(
        clippy::indexing_slicing,
        reason = "a single-row INSERT RETURNING yields exactly the inserted row, or errors"
    )]
    let id: i64 = rows[0].get(0)?;

    Ok(id)
}

/// Inserts one provider row and returns the stored row constructed from the supplied fields,
/// mirroring [`update_provider`]'s contract: the returned row is exactly what the statement
/// wrote, so callers never re-read after creation.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails.
pub(crate) async fn insert_provider(
    database: &Database,
    provider: NewProvider<'_>,
) -> Result<Provider, Error> {
    let id = insert_provider_row(database, provider).await?;

    Ok(Provider {
        quota_generation: 0,
        quota_revision: 0,
        id,
        name: provider.name.to_string(),
        model_prefix: provider.model_prefix.to_string(),
        base_url: provider.base_url.map(str::to_string),
        credentials: provider.credentials.clone(),
        is_enabled: provider.is_enabled,
        definition: provider.definition.clone(),
    })
}

/// Lists every stored provider with its definition, ordered by name.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_providers(database: &Database) -> Result<Vec<Provider>, Error> {
    let rows = database.query(LIST_PROVIDERS_SQL.as_str(), ()).await?;

    rows.iter().map(provider_from_row).collect()
}

/// Fetches one provider by name — the read behind [`is_provider_name_taken`]'s name-taken check;
/// all addressing of an existing provider goes through its id.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_provider(
    database: &Database,
    name: &str,
) -> Result<Option<Provider>, Error> {
    let rows = database
        .query(FIND_PROVIDER_SQL.as_str(), params![name])
        .await?;

    rows.first().map(provider_from_row).transpose()
}

/// Whether any stored provider row already carries `name` — the single-homed pre-insert
/// name-taken check behind creation: `providers:create` and `oauth:start` map its affirmative to
/// their clean 409 before any write or upstream round-trip, and the device poller's success
/// commit maps it to the flow's terminal `name_conflict` outcome. The database's UNIQUE
/// constraint stays the concurrent backstop behind all three (README §7.1, §7.2).
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn is_provider_name_taken(database: &Database, name: &str) -> Result<bool, Error> {
    Ok(find_provider(database, name).await?.is_some())
}

/// The `WHERE id = ?1` provider read generic over the pooled or transactional executor, so the
/// statement is written once for both executors.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn find_provider_by_id_on<E: SqlExecutor>(
    mut executor: E,
    provider_id: i64,
) -> Result<Option<Provider>, Error> {
    let rows = executor
        .query(FIND_PROVIDER_BY_ID_SQL.as_str(), params![provider_id])
        .await?;

    rows.first().map(provider_from_row).transpose()
}

/// Fetches one provider by id.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_provider_by_id(
    database: &Database,
    provider_id: i64,
) -> Result<Option<Provider>, Error> {
    find_provider_by_id_on(database, provider_id).await
}

/// Fetches one provider by id inside a transaction, the transactional form of
/// [`find_provider_by_id`] the transactional writes re-read the row through.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_provider_by_id_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Option<Provider>, Error> {
    find_provider_by_id_on(transaction, provider_id).await
}

/// Fetches one provider by id after a write, failing with an internal error when the row is gone: a
/// vanished row after a successful write is storage drift — an invariant violation — and never a
/// client fault.
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
    in_retryable_transaction!(database, transaction, async {
        let changes = transaction
            .execute("DELETE FROM providers WHERE id = ?1", params![provider_id])
            .await?;
        super::automatic_routes::synchronize(&mut transaction).await?;
        Ok::<_, Error>(changes == 1)
    })
}

/// Replaces the whole credentials document inside the caller's transaction, so a credentials
/// change commits atomically with the caller's other statements.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn replace_credentials_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    credentials: &StoredCredentials,
) -> Result<(), Error> {
    transaction
        .execute(
            "UPDATE providers SET credentials = ?1 WHERE id = ?2",
            params![credentials.to_json(), provider_id],
        )
        .await?;

    super::quota_blocks::invalidate_in_transaction(transaction, provider_id).await?;

    Ok(())
}

/// Updates one account inside a short transaction, validating the name and exact definition rebind.
/// Credential replacements must agree with the effective definition's kind, and its base-URL
/// requirement must hold. Physical inventory and explicit public routes retain their identities.
///
/// # Errors
///
/// Returns errors for unknown references, taken names, incompatible credentials, missing required
/// base URLs, or failed statements.
pub(crate) async fn update_provider(
    database: &Database,
    provider_id: i64,
    update: ProviderUpdate<'_>,
) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        // The re-read pins the rename and definition checks to the transaction's
        // snapshot.
        let Some(provider) =
            find_provider_by_id_in_transaction(&mut transaction, provider_id).await?
        else {
            return Err(Error::NotFound {
                message: super::provider_not_found_message(provider_id),
            });
        };

        // A rename must not take a name another provider already holds; the provider's own row is
        // excluded, so re-sending the unchanged name is never a conflict.
        let name_holders = transaction
            .query(
                "SELECT id FROM providers WHERE name = ?1 AND id != ?2",
                params![update.name, provider.id],
            )
            .await?;

        if !name_holders.is_empty() {
            return Err(Error::Conflict {
                message: super::provider_name_taken_message(update.name),
            });
        }

        // The definition rebind: validate the target version and the credential-kind agreement
        // inside the transaction, so the reference and its compatible credentials commit together
        // or not at all.
        let rebind_target = match update.definition {
            Some((name, version)) => {
                let Some(definition) = super::provider_definitions::find_definition_in_transaction(
                    &mut transaction,
                    name,
                    version,
                )
                .await?
                else {
                    return Err(Error::NotFound {
                        message: super::provider_definition_not_found_message(name, version.get()),
                    });
                };

                let credentials = update.credentials.unwrap_or(&provider.credentials);

                if credentials.kind() != definition.credential_kind() {
                    return Err(Error::Conflict {
                        message: format!(
                            "Provider definition `{name}` version {version} carries `{}` \
                             credentials, but the provider carries `{}` credentials",
                            definition.credential_kind().as_str(),
                            credentials.kind().as_str(),
                        ),
                    });
                }

                Some(definition)
            }

            None => None,
        };

        // The base-URL requirement pairs the written base URL with the write's own effective
        // definition — the rebind target when one is named, else the row's definition re-read
        // inside this transaction — so no commit can leave a base-addressing provider without its
        // base URL, however a concurrent write interleaves (README §3).
        let effective_definition = rebind_target.as_ref().unwrap_or(&provider.definition);

        if effective_definition.payload.references_base() && update.base_url.is_none() {
            return Err(Error::Conflict {
                message: format!(
                    "Provider definition `{}` version {} addresses its base URL, so the provider \
                     row must carry one",
                    effective_definition.name,
                    effective_definition.version.get(),
                ),
            });
        }

        let should_invalidate_quota = update.base_url != provider.base_url.as_deref()
            || effective_definition.name != provider.definition.name
            || effective_definition.version != provider.definition.version;

        let (definition_name, definition_version) = match rebind_target {
            Some(definition) => (definition.name, definition.version),

            None => (
                provider.definition.name.clone(),
                provider.definition.version,
            ),
        };

        transaction
            .execute(
                "UPDATE providers SET name = ?1, provider_definition_name = ?2, \
                 provider_definition_version = ?3, base_url = ?4, \
                 is_enabled = ?5, model_prefix = ?7 WHERE id = ?6",
                params![
                    update.name,
                    definition_name,
                    definition_version.get(),
                    update.base_url,
                    i64::from(update.is_enabled),
                    provider.id,
                    update.model_prefix,
                ],
            )
            .await?;

        super::automatic_routes::synchronize(&mut transaction).await?;

        if let Some(credentials) = update.credentials {
            replace_credentials_in_transaction(&mut transaction, provider.id, credentials).await?;
        } else if should_invalidate_quota {
            super::quota_blocks::invalidate_in_transaction(&mut transaction, provider.id).await?;
        }

        Ok(())
    })
}

/// Attempts to acquire the token refresh lease inside a transaction, stamping the OAuth
/// credentials' `refresh_started_at` member.
///
/// The lease is granted only when the stored expiry is known and elapsed (no later than
/// `freshness_margin` microseconds into the future) and no other holder owns a lease acquired
/// after `lease_stale_before`. Returns the acquired lease stamp, or `None` when no lease was
/// granted — another holder owns a live lease, the stored expiry is still beyond the freshness
/// margin, or the stored expiry is `null`, the no-scheduled-refresh state for which no lease
/// can ever be granted. The `null` case is the caller's dispatch-as-is contract (README §3):
/// the engine's loop decides it from the freshly read row before waiting, so `None` there
/// always means a live holder.
///
/// The raw [`DatabaseError`] is returned instead of an [`Error`] because the caller must classify
/// a stale-snapshot busy failure — a concurrent commit invalidating the read-then-write upgrade —
/// as "not acquired" and retry through its poll loop, rather than render it as a failed request.
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
) -> Result<Option<i64>, DatabaseError> {
    let changes = transaction
        .execute(
            "UPDATE providers SET credentials = json_set(credentials, \
             '$.refresh_started_at', ?1) \
             WHERE id = ?2 \
             AND json_extract(credentials, '$.expires_at') IS NOT NULL \
             AND json_extract(credentials, '$.expires_at') <= ?1 + ?3 \
             AND (json_extract(credentials, '$.refresh_started_at') IS NULL \
                  OR json_extract(credentials, '$.refresh_started_at') < ?4)",
            params![now, provider_id, freshness_margin, lease_stale_before],
        )
        .await?;

    Ok((changes == 1).then_some(now))
}

/// Persists the renewed credentials — the refreshed access token, its expiry, any rotation, and
/// the cleared lease stamp — but only while the caller still owns the refresh lease stamped
/// `lease_started_at`.
///
/// Conditioning the persist on lease ownership closes the lost-update window against a concurrent
/// credentials replacement or stale-lease reacquisition: when the stored stamp no longer matches,
/// tokens derived from superseded credentials are discarded, the newer state's row — its lease
/// included — is left untouched, and `false` is returned.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn persist_renewed_credentials_for_lease(
    database: &Database,
    provider_id: i64,
    lease_started_at: i64,
    credentials: &StoredCredentials,
) -> Result<bool, Error> {
    let changes = database
        .execute(
            "UPDATE providers SET credentials = ?1 WHERE id = ?2 \
             AND json_extract(credentials, '$.refresh_started_at') = ?3",
            params![credentials.to_json(), provider_id, lease_started_at],
        )
        .await?;

    Ok(changes == 1)
}

/// The JSON merge patch one reprovisioning outcome applies to the stored credentials document:
/// the members every reprovisioning rewrites, plus the optional rotation. The patch is a typed
/// document whose `refresh_token` member is present only when the provisioning sequence
/// extracted a rotation — merge-patch semantics leave an absent member untouched, so an absent
/// extraction patches nothing, exactly like an omitted rotation in the grant path (README §3).
#[derive(Serialize)]
struct ReprovisionPatch<'a> {
    /// The renewed access token.
    access_token: &'a str,

    /// The renewed expiry; a `null` member deletes the stored one under merge-patch semantics,
    /// the no-scheduled-refresh state the renewal path's runtime guard never produces.
    expires_at: Option<i64>,

    /// The rotated refresh token; serialized only when present, so the stored token survives a
    /// rotationless reprovisioning verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<&'a str>,

    /// The cleared lease stamp, always serialized — as the explicit JSON `null` the merge
    /// needs, so the patch never depends on `json_set`'s handling of SQL NULL.
    refresh_started_at: Option<i64>,
}

/// Persists a reprovisioning outcome — the new access token, its expiry, a refresh-token
/// rotation the sequence extracted when it carried one, and optionally a new base URL — while
/// the caller still owns the refresh lease stamped `lease_started_at`. An absent rotation
/// omits the patch's `refresh_token` member, leaving the stored token untouched (README §3).
/// The merge happens in SQL over the stored document, so the durable slots and refresh token a
/// concurrent write renewed are never read stale into Rust and overwritten.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
///
/// # Panics
///
/// Panics if the reprovision patch fails to serialize, which cannot occur: the patch is a
/// struct of strings and numbers, a shape serde_json always renders.
pub(crate) async fn persist_reprovision_for_lease(
    database: &Database,
    provider_id: i64,
    lease_started_at: i64,
    access_token: &str,
    expires_at: Option<i64>,
    refresh_token: Option<&str>,
    base_url: Option<&str>,
) -> Result<bool, Error> {
    let patch = ReprovisionPatch {
        access_token,
        expires_at,
        refresh_token,
        refresh_started_at: None,
    };

    #[expect(
        clippy::expect_used,
        reason = "the reprovision patch always serializes: it is a struct of strings and \
                  numbers"
    )]
    let patch = serde_json::to_string(&patch).expect("reprovision patch serializes");

    let changes = database
        .execute(
            "UPDATE providers SET credentials = json_patch(credentials, ?1), \
             base_url = COALESCE(?2, base_url) \
             WHERE id = ?3 AND json_extract(credentials, '$.refresh_started_at') = ?4",
            params![patch, base_url, provider_id, lease_started_at],
        )
        .await?;

    Ok(changes == 1)
}

/// Clears the token refresh lease after a finished or failed refresh — but only while the
/// stored stamp is still the caller's own `lease_started_at`, conditioning the clear on lease
/// ownership exactly like [`persist_renewed_credentials_for_lease`] conditions its persist. A
/// caller that lost the lease meanwhile — a concurrent reauthorization replaced the
/// credentials, or a stale-lease reacquisition re-stamped it — must not erase the winner's
/// live lease. Returns whether the caller still owned the lease.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn clear_refresh_lease(
    database: &Database,
    provider_id: i64,
    lease_started_at: i64,
) -> Result<bool, Error> {
    let changes = database
        .execute(
            "UPDATE providers SET credentials = json_set(\
             credentials, '$.refresh_started_at', json('null')) WHERE id = ?1 \
             AND json_extract(credentials, '$.refresh_started_at') = ?2",
            params![provider_id, lease_started_at],
        )
        .await?;

    Ok(changes == 1)
}

/// Inserts one provider row inside the caller's transaction — the device poller's atomic
/// provider-write-plus-outcome commit — returning the new row's id: the transactional form of
/// [`insert_provider_row`].
///
/// # Errors
///
/// Returns [`Error`] when the insert fails.
pub(crate) async fn insert_provider_in_transaction(
    transaction: &mut Transaction,
    provider: NewProvider<'_>,
) -> Result<i64, Error> {
    insert_provider_row(transaction, provider).await
}

/// Replaces one provider's credentials and definition reference inside the caller's transaction —
/// the reauthorize flow's atomic credentials-plus-reference commit — returning whether the row
/// was still there to update.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn rebind_credentials_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    definition: &ProviderDefinition,
    credentials: &StoredCredentials,
) -> Result<bool, Error> {
    let changes = transaction
        .execute(
            "UPDATE providers SET provider_definition_name = ?1, \
             provider_definition_version = ?2, credentials = ?3 \
             WHERE id = ?4",
            params![
                definition.name.as_str(),
                definition.version.get(),
                credentials.to_json(),
                provider_id
            ],
        )
        .await?;

    if changes == 1 {
        super::quota_blocks::invalidate_in_transaction(transaction, provider_id).await?;
    }
    Ok(changes == 1)
}

/// Updates one provider's base URL inside the caller's transaction — the working URL a
/// provisioning sequence extracted, committed with the reauthorization.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn set_base_url_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
    base_url: &str,
) -> Result<(), Error> {
    transaction
        .execute(
            "UPDATE providers SET base_url = ?1 WHERE id = ?2",
            params![base_url, provider_id],
        )
        .await?;

    Ok(())
}

/// The every-provider listing, ordered by name: a fully fixed statement over the shared column
/// consts, built once.
static LIST_PROVIDERS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {} {} ORDER BY p.name",
        PROVIDER_COLUMNS, PROVIDER_JOINED_FROM
    )
});

/// The `WHERE name = ?1` provider lookup. Names are unique, so this is the name-taken check behind
/// creation; all addressing of an existing provider goes through its id. A fully fixed statement
/// over the shared column consts, built once.
static FIND_PROVIDER_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {} {} WHERE p.name = ?1",
        PROVIDER_COLUMNS, PROVIDER_JOINED_FROM
    )
});

/// The `WHERE id = ?1` provider lookup shared by the pooled and transactional executors through
/// [`find_provider_by_id`]: a fully fixed statement over the shared column consts, built once.
static FIND_PROVIDER_BY_ID_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {} {} WHERE p.id = ?1",
        PROVIDER_COLUMNS, PROVIDER_JOINED_FROM
    )
});

/// The fields of a provider about to be inserted.
#[derive(Clone, Copy)]
pub(crate) struct NewProvider<'a> {
    /// Unique, non-empty, renameable label; a later `providers:update` request may rename it.
    pub(crate) name: &'a str,

    /// Prefix of automatically exposed direct model names.
    pub(crate) model_prefix: &'a str,

    /// Upstream base URL; required by definitions whose URLs reference the base.
    pub(crate) base_url: Option<&'a str>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: &'a StoredCredentials,

    /// The exact definition version the row binds to.
    pub(crate) definition: &'a ProviderDefinition,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,
}

/// The `providers:update` field state for an existing provider: the caller restates the row's
/// complete readable state, so every readable field is total — the write applies each one
/// verbatim — and only the write-only `credentials` is supplied-or-absent, with `None` meaning
/// absent and keeping the stored secret. A named definition rebind is explicit and validated
/// inside the write's transaction.
pub(crate) struct ProviderUpdate<'a> {
    /// Unique provider name; may rename the provider.
    pub(crate) name: &'a str,

    /// Prefix of automatically exposed direct model names.
    pub(crate) model_prefix: &'a str,

    /// Upstream base URL; `null` when the definition's URLs never reference the base.
    pub(crate) base_url: Option<&'a str>,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,

    /// The definition version to rebind to; absent keeps the stored reference.
    pub(crate) definition: Option<(&'a str, std::num::NonZeroI64)>,

    /// Replacement credentials; `None` keeps the stored ones.
    pub(crate) credentials: Option<&'a StoredCredentials>,
}

impl IntoValue for CredentialKind {
    fn into_value(self) -> Result<Value, TursoError> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

/// Column order of provider selects: the row identity, the routing columns, the credential
/// state, and then the definition columns the row references, consumed positionally by
/// [`provider_from_row`].
pub(super) const PROVIDER_COLUMNS: &str = "p.id, p.name, p.base_url, p.credentials, \
     p.is_enabled, p.quota_generation, p.quota_revision, d.name, d.version, \
     d.format, d.inference, d.credential, d.provisioning, \
     d.discovery, d.compaction, d.quota, d.created_at, p.model_prefix";

/// The join behind every provider read: the row and the exact definition version it references.
pub(super) const PROVIDER_JOINED_FROM: &str = "FROM providers p JOIN provider_definitions d \
     ON d.name = p.provider_definition_name AND d.version = p.provider_definition_version";

/// The provider-model join: models first, then the provider and its definition, filtered by the
/// caller's model predicate.
pub(super) const PROVIDER_JOINED_FROM_MODELS: &str = "FROM models m \
     JOIN providers p ON p.id = m.provider_id JOIN provider_definitions d \
     ON d.name = p.provider_definition_name AND d.version = p.provider_definition_version";

/// The column count of [`PROVIDER_COLUMNS`]: a provider-model join appends the model columns
/// after it.
pub(super) const PROVIDER_COLUMN_COUNT: usize = 18;

/// Parses a provider row in [`PROVIDER_COLUMNS`] order: the provider columns, then the exact
/// definition version the row references.
pub(super) fn provider_from_row(row: &Row) -> Result<Provider, Error> {
    let name: String = row.get(1)?;
    let credentials_json: String = row.get(3)?;

    // The typed parse failure is wrapped exactly once as internal storage drift: the row was
    // written only through the validated management path, so a payload failing here is never the
    // requesting client's fault.
    let credentials = serde_json::from_str::<crate::provider::StoredCredentials>(&credentials_json)
        .map_err(|error| Error::Drift {
            message: format!("provider `{name}` has malformed credentials: {error}"),
        })?;

    Ok(Provider {
        id: row.get(0)?,
        name,
        model_prefix: row.get(17)?,
        base_url: row.get(2)?,
        credentials,
        is_enabled: row.get::<i64>(4)? != 0,
        quota_generation: row.get(5)?,
        quota_revision: row.get(6)?,
        definition: super::provider_definitions::definition_from_row_offset(row, 7)?,
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "the fixtures are constructed so each expect lands on its documented side"
    )]
    #![expect(
        clippy::panic,
        reason = "a fixture violating its own invariant fails the test loudly"
    )]

    use super::{
        clear_refresh_lease, persist_renewed_credentials_for_lease, persist_reprovision_for_lease,
        try_acquire_refresh_lease,
    };
    use crate::{
        provider::StoredCredentials,
        storage::{
            self as storage_cr,
            test_support::{insert_oauth_definition, oauth_definition},
        },
        test_support,
    };
    use std::{collections::BTreeMap, error::Error as StdError};

    /// Verifies the JSON lease mechanics against the real engine: the acquire stamps the
    /// credentials' `refresh_started_at` only for an elapsed known expiry, a stale lease is
    /// stealable, the lease-conditioned persist lands only under the acquired stamp, and the
    /// reprovision persist merges its fields over the stored document.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn json_refresh_lease_acquires_persists_and_recovers() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = oauth_definition();

        insert_oauth_definition(&database, &definition).await?;

        let provider = test_support::insert_provider(
            &database,
            &definition,
            "leased",
            Some("http://localhost:8080"),
            StoredCredentials::OAuth {
                access_token: "old".to_string(),
                refresh_token: Some("refresh".to_string()),
                slots: BTreeMap::new(),
                expires_at: Some(1),
                refresh_started_at: None,
            },
        )
        .await;

        let now = 1_000;
        let stale_before = now - 90;

        let mut transaction = database.transaction().await?;

        // The expired token acquires the lease.
        let lease = try_acquire_refresh_lease(&mut transaction, provider.id, now, stale_before, 0)
            .await?
            .expect("the expired token acquires");

        transaction.commit().await?;

        // A concurrent acquirer sees the live lease.
        let mut transaction = database.transaction().await?;

        let is_acquired =
            try_acquire_refresh_lease(&mut transaction, provider.id, now, stale_before, 0)
                .await
                .map_err(|error| Box::new(error) as Box<dyn StdError>)?;

        assert!(is_acquired.is_none());

        transaction.rollback().await?;

        // The lease-conditioned persist lands only under the acquired stamp, clearing it.
        let renewed = StoredCredentials::OAuth {
            access_token: "new".to_string(),
            refresh_token: Some("refresh".to_string()),
            slots: BTreeMap::new(),
            expires_at: Some(2_000),
            refresh_started_at: None,
        };

        assert!(
            !persist_renewed_credentials_for_lease(&database, provider.id, lease + 1, &renewed)
                .await?,
            "a wrong stamp never persists"
        );

        assert!(
            persist_renewed_credentials_for_lease(&database, provider.id, lease, &renewed).await?
        );

        // A null expiry never acquires: the not-scheduled state never refreshes.
        let unscheduled = StoredCredentials::OAuth {
            access_token: "old".to_string(),
            refresh_token: Some("refresh".to_string()),
            slots: BTreeMap::new(),
            expires_at: None,
            refresh_started_at: None,
        };

        let mut transaction = database.transaction().await?;

        storage_cr::providers::replace_credentials_in_transaction(
            &mut transaction,
            provider.id,
            &unscheduled,
        )
        .await?;

        transaction.commit().await?;

        let mut transaction = database.transaction().await?;

        let is_acquired =
            try_acquire_refresh_lease(&mut transaction, provider.id, now, stale_before, 0)
                .await
                .map_err(|error| Box::new(error) as Box<dyn StdError>)?;

        assert!(is_acquired.is_none());

        transaction.rollback().await?;

        // The reprovision persist merges its fields over the stored document under the lease.
        let mut transaction = database.transaction().await?;

        storage_cr::providers::replace_credentials_in_transaction(
            &mut transaction,
            provider.id,
            &renewed,
        )
        .await?;

        transaction.commit().await?;

        let mut transaction = database.transaction().await?;

        let lease =
            try_acquire_refresh_lease(&mut transaction, provider.id, 3_000, stale_before, 0)
                .await
                .map_err(|error| Box::new(error) as Box<dyn StdError>)?
                .expect("the expired token acquires again");

        transaction.commit().await?;

        // The rotationless persist omits the patch's `refresh_token` member, so the merge
        // leaves the stored token untouched (README §3).
        assert!(
            persist_reprovision_for_lease(
                &database,
                provider.id,
                lease,
                "provisioned",
                Some(9_000_000),
                None,
                Some("http://new-base.example"),
            )
            .await?
        );

        let reloaded = storage_cr::providers::find_provider_by_id(&database, provider.id)
            .await?
            .expect("the provider persists");

        assert_eq!(reloaded.credentials.secret(), "provisioned");
        assert_eq!(
            reloaded.base_url.as_deref(),
            Some("http://new-base.example")
        );

        let StoredCredentials::OAuth {
            refresh_token,
            refresh_started_at,
            ..
        } = &reloaded.credentials
        else {
            panic!("the provider carries OAuth credentials");
        };

        assert_eq!(refresh_token.as_deref(), Some("refresh"));
        assert_eq!(*refresh_started_at, None);

        Ok(())
    }

    /// A failed refresh's lease clear is conditioned on the caller's own acquired stamp: a
    /// stale loser clearing under an older stamp never erases a newer holder's live lease,
    /// while the holder's own clear lands.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn clear_refresh_lease_clears_only_the_acquired_stamp() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = oauth_definition();

        insert_oauth_definition(&database, &definition).await?;

        let provider = test_support::insert_provider(
            &database,
            &definition,
            "leased",
            Some("http://localhost:8080"),
            StoredCredentials::OAuth {
                access_token: "old".to_string(),
                refresh_token: Some("refresh".to_string()),
                slots: BTreeMap::new(),
                expires_at: Some(1),
                refresh_started_at: None,
            },
        )
        .await;

        // The winner acquires the lease at stamp 1_000.
        let mut transaction = database.transaction().await?;

        let lease = try_acquire_refresh_lease(&mut transaction, provider.id, 1_000, 910, 0)
            .await?
            .expect("the expired token acquires");

        transaction.commit().await?;

        assert_eq!(lease, 1_000);

        // A stale loser's clear under an older stamp is refused: the winner's live stamp and
        // its lease protection survive.
        assert!(
            !clear_refresh_lease(&database, provider.id, 999).await?,
            "a stale loser never clears a newer holder's lease"
        );

        let reloaded = storage_cr::providers::find_provider_by_id(&database, provider.id)
            .await?
            .expect("the provider persists");

        let StoredCredentials::OAuth {
            refresh_started_at, ..
        } = &reloaded.credentials
        else {
            panic!("the provider carries OAuth credentials");
        };

        assert_eq!(*refresh_started_at, Some(1_000));

        // The holder's own clear lands.
        assert!(clear_refresh_lease(&database, provider.id, lease).await?);

        let reloaded = storage_cr::providers::find_provider_by_id(&database, provider.id)
            .await?
            .expect("the provider persists");

        let StoredCredentials::OAuth {
            refresh_started_at, ..
        } = &reloaded.credentials
        else {
            panic!("the provider carries OAuth credentials");
        };

        assert_eq!(*refresh_started_at, None);

        Ok(())
    }
}
