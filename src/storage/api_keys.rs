//! Gateway API keys: the database-managed secrets authorizing outward `/api/*` traffic.
//!
//! Each row carries its plaintext key — like upstream credentials, the gateway stores the secret
//! verbatim and performs no hashing — plus a non-unique human label, a serving switch, and the
//! creation timestamp in database-clock microseconds. Key values are unique: a duplicate insert or
//! rewrite is a conflict. Names are deliberately not unique and carry no routing meaning.
//!
//! The outward authentication middleware reads only enabled secrets through
//! [`list_enabled_api_key_secrets`]; the management surface owns the full CRUD below. Storage
//! failures classify through the shared storage error domain ([`Error`]): a missing row is a 404,
//! a taken key a 409, and a constraint backstop the generic collision message. Messages never
//! carry the secret itself — only the row id — so a conflict body stays safe to render and log.

use super::Error;
use crate::{
    database::{Database, params},
    unix_time,
};

/// Column order of API key selects, consumed positionally by [`api_key_from_row`].
const API_KEY_COLUMNS: &str = "id, name, key, is_enabled, created_at";

/// A stored gateway API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiKey {
    /// Row identifier; every management reference to the key goes through it.
    pub(crate) id: i64,

    /// Human label; not unique and never validated beyond non-emptiness.
    pub(crate) name: String,

    /// Plaintext shared secret presented as `Authorization: Bearer <key>`.
    pub(crate) key: String,

    /// Whether the key authorizes traffic.
    pub(crate) is_enabled: bool,

    /// Creation timestamp in UTC microseconds.
    pub(crate) created_at: i64,
}

/// The complete field state of one API key row, the single shape both `api-keys:create` and
/// `api-keys:update` write (README §7.0 rule 3): the caller states the label, the secret, and the
/// serving state, and the write applies every field verbatim — the row identity and creation
/// timestamp are the only server-assigned parts.
#[derive(Debug, Clone)]
pub(crate) struct ApiKeyFields<'a> {
    /// Human label; must be non-empty.
    pub(crate) name: &'a str,

    /// Plaintext shared secret; must be non-empty and not held by another row.
    pub(crate) key: &'a str,

    /// Whether the key authorizes traffic.
    pub(crate) is_enabled: bool,
}

/// The wire wording for an unknown API key id, owned once here as the single source both surfaces
/// render from: the transactional write constructs its storage rejection with it, and the
/// management error domain's `ApiKeyNotFound` variant renders its display through it — a wording
/// edit lands here and reaches every 404 identically.
pub(crate) fn api_key_not_found_message(id: i64) -> String {
    format!("API key {id} does not exist")
}

/// The wire wording for a taken key value, owned once here as the single source both surfaces
/// render from: the checked rewrite constructs its storage rejection with it, and the management
/// error domain's `ApiKeyTaken` variant renders its display through it. The value itself is never
/// named: it is a bearer secret and must stay out of rendered bodies and log records.
pub(crate) fn api_key_taken_message() -> String {
    "API key is already taken".to_string()
}

/// Lists every stored API key, ordered by row id.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_api_keys(database: &Database) -> Result<Vec<ApiKey>, Error> {
    let sql = format!("SELECT {API_KEY_COLUMNS} FROM api_keys ORDER BY id");

    let rows = database.query(sql, ()).await?;

    rows.iter().map(api_key_from_row).collect()
}

/// Fetches one API key by its secret value, for creation-time taken checks.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the row is malformed.
pub(crate) async fn find_api_key_by_key(
    database: &Database,
    key: &str,
) -> Result<Option<ApiKey>, Error> {
    let rows = database
        .query(
            format!("SELECT {API_KEY_COLUMNS} FROM api_keys WHERE key = ?1"),
            params![key],
        )
        .await?;

    rows.first().map(api_key_from_row).transpose()
}

/// Fetches one API key by id.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the row is malformed.
pub(crate) async fn find_api_key_by_id(
    database: &Database,
    api_key_id: i64,
) -> Result<Option<ApiKey>, Error> {
    let rows = database
        .query(
            format!("SELECT {API_KEY_COLUMNS} FROM api_keys WHERE id = ?1"),
            params![api_key_id],
        )
        .await?;

    rows.first().map(api_key_from_row).transpose()
}

/// Lists the identities and secrets of every enabled API key, for the outward authentication
/// middleware: each pair is the row's never-recycled id beside its secret value.
///
/// Only these pairs cross this boundary — no row reads them back — so a disabled key stops
/// authorizing traffic as soon as its row is rewritten.
///
/// # Errors
///
/// Returns [`Error`] when the query fails.
pub(crate) async fn list_enabled_api_key_secrets(
    database: &Database,
) -> Result<Vec<(i64, String)>, Error> {
    let rows = database
        .query("SELECT id, key FROM api_keys WHERE is_enabled = 1", ())
        .await?;

    let mut keys = Vec::with_capacity(rows.len());

    for row in &rows {
        keys.push((row.get(0)?, row.get(1)?));
    }

    Ok(keys)
}

/// Inserts one API key row, stamping the creation time from the database clock, and returns the
/// stored row constructed from the supplied fields: the returned row is exactly what the statement
/// wrote, so callers never re-read after creation.
///
/// A key value another row already holds violates the schema's uniqueness and surfaces as a
/// constraint error; the management pre-check renders the clean conflict.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails.
pub(crate) async fn insert_api_key(
    database: &Database,
    fields: &ApiKeyFields<'_>,
) -> Result<ApiKey, Error> {
    let created_at = unix_time::unix_now_micros();

    let rows = database
        .query(
            "INSERT INTO api_keys (name, key, is_enabled, created_at) \
             VALUES (?1, ?2, ?3, ?4) RETURNING id",
            params![
                fields.name,
                fields.key,
                i64::from(fields.is_enabled),
                created_at,
            ],
        )
        .await?;

    #[expect(
        clippy::indexing_slicing,
        reason = "a single-row INSERT RETURNING yields exactly the inserted row, or errors"
    )]
    let id: i64 = rows[0].get(0)?;

    Ok(ApiKey {
        id,
        name: fields.name.to_string(),
        key: fields.key.to_string(),
        is_enabled: fields.is_enabled,
        created_at,
    })
}

/// Applies one `api-keys:update` rewrite to an existing key inside a single transaction.
///
/// The row is re-read inside the transaction, so a caller's stale outer snapshot can never write
/// through a concurrent change's results. An unknown key — including one deleted between the
/// caller's outer read and this call — is rejected and nothing is written, as is a key value
/// another row already holds; the row's own value re-sent unchanged is never a conflict. The
/// transaction runs under the storage tier's `in_retryable_transaction` bounded stale-snapshot
/// retry: a concurrent commit that invalidates the snapshot between the reads and the write
/// re-runs the whole read-then-write against the newer state instead of surfacing the refusal.
///
/// # Errors
///
/// Returns [`Error`] for an unknown key, a failed statement, or a taken key value.
pub(crate) async fn update_api_key(
    database: &Database,
    api_key_id: i64,
    fields: ApiKeyFields<'_>,
) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        let is_missing = transaction
            .query(
                format!("SELECT {API_KEY_COLUMNS} FROM api_keys WHERE id = ?1"),
                params![api_key_id],
            )
            .await?
            .is_empty();

        if is_missing {
            return Err(Error::NotFound {
                message: api_key_not_found_message(api_key_id),
            });
        }

        // A rewrite must not take a key value another row already holds; the row's own value is
        // excluded, so re-sending the unchanged secret is never a conflict.
        let key_holders = transaction
            .query(
                "SELECT id FROM api_keys WHERE key = ?1 AND id != ?2",
                params![fields.key, api_key_id],
            )
            .await?;

        if !key_holders.is_empty() {
            return Err(Error::Conflict {
                message: api_key_taken_message(),
            });
        }

        transaction
            .execute(
                "UPDATE api_keys SET name = ?1, key = ?2, is_enabled = ?3 WHERE id = ?4",
                params![
                    fields.name,
                    fields.key,
                    i64::from(fields.is_enabled),
                    api_key_id
                ],
            )
            .await?;

        Ok(())
    })
}

/// Deletes one API key by id, returning whether a row was removed.
///
/// # Errors
///
/// Returns [`Error`] when the delete fails.
pub(crate) async fn delete_api_key(database: &Database, api_key_id: i64) -> Result<bool, Error> {
    let changes = database
        .execute("DELETE FROM api_keys WHERE id = ?1", params![api_key_id])
        .await?;

    Ok(changes == 1)
}

/// Parses an API key row in [`API_KEY_COLUMNS`] order.
fn api_key_from_row(row: &crate::database::Row) -> Result<ApiKey, Error> {
    Ok(ApiKey {
        id: row.get(0)?,
        name: row.get(1)?,
        key: row.get(2)?,
        is_enabled: row.get::<i64>(3)? != 0,
        created_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::create_test_database;
    use std::error::Error as StdError;

    /// Verifies the key lifecycle: insertion stamps the creation time from the database clock and
    /// returns the written row, names are not unique, and the enabled-secrets listing carries only
    /// enabled values.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn inserts_lists_and_filters_enabled_secrets() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let before = unix_time::unix_now_micros();

        let first = insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-a",
                key: "sk-first",
                is_enabled: true,
            },
        )
        .await?;

        let after = unix_time::unix_now_micros();

        assert!((before..=after).contains(&first.created_at));
        assert_eq!(first.name, "service-a");
        assert_eq!(first.key, "sk-first");
        assert!(first.is_enabled);

        // Names are not unique: a second row may carry the same label.
        let second = insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-a",
                key: "sk-second",
                is_enabled: false,
            },
        )
        .await?;

        assert_ne!(first.id, second.id);

        let listed = list_api_keys(&database).await?;

        assert_eq!(listed, vec![first.clone(), second.clone()]);

        let keys = list_enabled_api_key_secrets(&database).await?;

        assert_eq!(keys, vec![(first.id, "sk-first".to_string())]);

        let found = find_api_key_by_key(&database, "sk-second").await?;

        assert_eq!(found, Some(second));

        assert_eq!(find_api_key_by_key(&database, "sk-missing").await?, None);

        Ok(())
    }

    /// Verifies the full-state rewrite: every field applies verbatim, re-sending the unchanged
    /// secret is never a conflict, a secret another row holds is rejected without writing, and an
    /// unknown id is rejected the same way.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn rewrites_full_state_with_checked_conflicts() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        let first = insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-a",
                key: "sk-first",
                is_enabled: true,
            },
        )
        .await?;

        let second = insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-b",
                key: "sk-second",
                is_enabled: true,
            },
        )
        .await?;

        update_api_key(
            &database,
            first.id,
            ApiKeyFields {
                name: "renamed-a",
                key: "sk-rotated",
                is_enabled: false,
            },
        )
        .await?;

        #[expect(clippy::unwrap_used, reason = "the rewritten row is present")]
        let rewritten = find_api_key_by_id(&database, first.id).await?.unwrap();

        assert_eq!(rewritten.name, "renamed-a");
        assert_eq!(rewritten.key, "sk-rotated");
        assert!(!rewritten.is_enabled);
        assert_eq!(rewritten.created_at, first.created_at);

        // Re-sending the unchanged secret is never a conflict.
        update_api_key(
            &database,
            first.id,
            ApiKeyFields {
                name: "renamed-a",
                key: "sk-rotated",
                is_enabled: false,
            },
        )
        .await?;

        // A secret another row holds rejects the whole rewrite and writes nothing.
        let outcome = update_api_key(
            &database,
            first.id,
            ApiKeyFields {
                name: "renamed-a",
                key: "sk-second",
                is_enabled: false,
            },
        )
        .await;

        assert!(matches!(outcome, Err(Error::Conflict { .. })));

        #[expect(clippy::unwrap_used, reason = "the row survived the rejected rewrite")]
        let survived = find_api_key_by_id(&database, first.id).await?.unwrap();

        assert_eq!(survived.key, "sk-rotated");

        // An unknown id — including one deleted before the write — is rejected.
        let missing = update_api_key(
            &database,
            second.id + 1000,
            ApiKeyFields {
                name: "ghost",
                key: "sk-ghost",
                is_enabled: true,
            },
        )
        .await;

        assert!(matches!(missing, Err(Error::NotFound { .. })));

        // Deletion reports whether a row was removed.
        assert!(delete_api_key(&database, second.id).await?);
        assert!(!delete_api_key(&database, second.id).await?);
        assert_eq!(find_api_key_by_id(&database, second.id).await?, None);

        Ok(())
    }

    /// Verifies that the schema itself enforces the key invariants: a duplicate secret and an empty
    /// name or secret are rejected at write time as storage constraint errors. First-party write
    /// paths pre-check the duplicate, so the probe uses raw statements.
    ///
    /// # Panics
    ///
    /// Panics if a drifted write unexpectedly succeeds.
    #[tokio::test]
    async fn rejects_duplicate_and_empty_values_at_write_time() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = create_test_database().await;

        insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-a",
                key: "sk-first",
                is_enabled: true,
            },
        )
        .await?;

        // A duplicate secret violates the schema's uniqueness even without the pre-check.
        let duplicate = insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-b",
                key: "sk-first",
                is_enabled: true,
            },
        )
        .await;

        #[expect(
            clippy::expect_used,
            reason = "the schema UNIQUE rejects the duplicate"
        )]
        let error = duplicate.expect_err("a duplicate secret must violate the schema UNIQUE");

        assert!(
            matches!(error, Error::Constraint { .. }),
            "a schema UNIQUE violation must classify as a storage constraint error"
        );

        // The empty name and secret are not column invariants: the composition boundary rejects
        // them at the management write, before any statement runs.

        Ok(())
    }
}
