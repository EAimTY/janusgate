//! Complete serving-key policies and atomic, retryable full-state writes.

use super::Error;
use crate::{
    access_control::{
        AccessDecision, ApiKeyAccessControl, ApiKeyOwnerDefault, ApiKeyRouteOverride, RouteOwner,
    },
    database::{Database, Transaction, params},
    unix_time,
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Column order shared by complete-key reads.
const API_KEY_COLUMNS: &str = "id, name, key, is_enabled, created_at, default_access";

/// A stored key and its complete authored policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiKey {
    /// Stable key identity.
    pub(crate) id: i64,
    /// Nonunique human label.
    pub(crate) name: String,
    /// Plaintext serving secret; never included in failures.
    pub(crate) key: String,
    /// Global serving switch.
    pub(crate) is_enabled: bool,
    /// Creation time in UTC microseconds.
    pub(crate) created_at: i64,
    /// Complete policy with canonical sparse arrays.
    pub(crate) access_control: ApiKeyAccessControl,
}

/// Complete desired writable state for both create and update.
#[derive(Debug, Clone)]
pub(crate) struct ApiKeyFields<'a> {
    /// Human label validated at the write boundary.
    pub(crate) name: &'a str,
    /// Unique serving secret validated at the write boundary.
    pub(crate) key: &'a str,
    /// Global serving switch.
    pub(crate) is_enabled: bool,
    /// Complete replacement policy; all references are checked inside the write transaction.
    pub(crate) access_control: &'a ApiKeyAccessControl,
}

/// A rejected rule identity with its collection, independent of bearer secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AccessReference {
    /// Provider default.
    Provider(i64),
    /// Pool default.
    Pool(i64),
    /// Public-route override.
    Route(i64),
}

impl AccessReference {
    /// The reference's rule collection.
    fn collection(self) -> AccessCollection {
        match self {
            Self::Provider(_) => AccessCollection::Provider,
            Self::Pool(_) => AccessCollection::Pool,
            Self::Route(_) => AccessCollection::Route,
        }
    }

    /// The referenced row identity.
    const fn id(self) -> i64 {
        match self {
            Self::Provider(id) | Self::Pool(id) | Self::Route(id) => id,
        }
    }
}

/// One sparse-rule collection of a key's policy: the table its rules live in, and the fixed
/// rule reads and typed rule home that collection's rows move through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AccessCollection {
    /// Provider defaults.
    Provider,
    /// Pool defaults.
    Pool,
    /// Public-route overrides.
    Route,
}

impl AccessCollection {
    /// Every collection in canonical read order.
    const ALL: [Self; 3] = [Self::Provider, Self::Pool, Self::Route];

    /// The table holding the rows this collection's rules reference.
    const fn referenced_table(self) -> &'static str {
        match self {
            Self::Provider => "providers",
            Self::Pool => "provider_pools",
            Self::Route => "model_routes",
        }
    }

    /// The collection's two fixed rule reads — the single key's rules and every key's rules,
    /// both in stored target order.
    const fn rule_reads(self) -> (&'static str, &'static str) {
        match self {
            Self::Provider => (
                "SELECT api_key_id, provider_id, access FROM api_key_provider_defaults \
                 WHERE api_key_id = ?1 ORDER BY provider_id",
                "SELECT api_key_id, provider_id, access FROM api_key_provider_defaults \
                 ORDER BY provider_id",
            ),
            Self::Pool => (
                "SELECT api_key_id, pool_id, access FROM api_key_pool_defaults \
                 WHERE api_key_id = ?1 ORDER BY pool_id",
                "SELECT api_key_id, pool_id, access FROM api_key_pool_defaults \
                 ORDER BY pool_id",
            ),
            Self::Route => (
                "SELECT api_key_id, model_route_id, access FROM api_key_model_route_overrides \
                 WHERE api_key_id = ?1 ORDER BY model_route_id",
                "SELECT api_key_id, model_route_id, access FROM api_key_model_route_overrides \
                 ORDER BY model_route_id",
            ),
        }
    }

    /// Appends one stored rule of this collection to its typed home in `policy`.
    fn push_into(self, policy: &mut ApiKeyAccessControl, target: i64, access: AccessDecision) {
        match self {
            Self::Provider => policy.owner_defaults.push(ApiKeyOwnerDefault::Provider {
                provider_id: target,
                access,
            }),
            Self::Pool => policy.owner_defaults.push(ApiKeyOwnerDefault::Pool {
                pool_id: target,
                access,
            }),
            Self::Route => policy.route_overrides.push(ApiKeyRouteOverride {
                model_route_id: target,
                access,
            }),
        }
    }
}

/// Shared wire wording for an unknown key.
pub(crate) fn api_key_not_found_message(id: i64) -> String {
    format!("API key {id} does not exist")
}

/// Shared wire wording for a taken secret, without exposing its value.
pub(crate) fn api_key_taken_message() -> String {
    "API key is already taken".into()
}

/// Reads the closed stored access vocabulary — the decode side of
/// [`AccessDecision::as_str`] — treating a foreign value as stored drift.
fn access_decision_from_stored(value: &str) -> Result<AccessDecision, Error> {
    match value {
        "allow" => Ok(AccessDecision::Allow),
        "deny" => Ok(AccessDecision::Deny),
        _ => Err(Error::Drift {
            message: format!("unknown access decision `{value}`"),
        }),
    }
}

/// Lists complete keys using one transaction and three batched rule reads.
pub(crate) async fn list_api_keys(database: &Database) -> Result<Vec<ApiKey>, Error> {
    read_keys(database, "", None).await
}

/// Fetches a complete key by secret in one coherent transaction.
#[cfg(test)]
pub(crate) async fn find_api_key_by_key(
    database: &Database,
    key: &str,
) -> Result<Option<ApiKey>, Error> {
    Ok(read_keys(
        database,
        "WHERE key = ?1",
        Some(crate::database::Value::Text(key.into())),
    )
    .await?
    .pop())
}

/// Fetches a complete key by identity in one coherent transaction.
#[cfg(test)]
pub(crate) async fn find_api_key_by_id(
    database: &Database,
    id: i64,
) -> Result<Option<ApiKey>, Error> {
    Ok(read_keys(
        database,
        "WHERE id = ?1",
        Some(crate::database::Value::Integer(id)),
    )
    .await?
    .pop())
}

/// Reads one collection's stored rules — the single key's rules when `api_key_id` is `Some`,
/// every key's rules otherwise — as `(api key id, target identity, access decision)` triples
/// in stored order, the one shared rule read of both [`read_keys`] and [`read_enabled_policy`].
/// Each collection's two statement shapes are fixed consts, spelled once in
/// [`AccessCollection::rule_reads`].
///
/// # Errors
///
/// Returns query or stored-decoding failures.
async fn read_rules(
    transaction: &mut Transaction,
    collection: AccessCollection,
    api_key_id: Option<i64>,
) -> Result<Vec<(i64, i64, AccessDecision)>, Error> {
    let (single_key_sql, all_keys_sql) = collection.rule_reads();
    let rows = match api_key_id {
        Some(id) => transaction.query(single_key_sql, params![id]).await?,

        None => transaction.query(all_keys_sql, ()).await?,
    };

    rows.iter()
        .map(|row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                access_decision_from_stored(&row.get::<String>(2)?)?,
            ))
        })
        .collect()
}

/// Reads key fields and batches all matching sparse collections before releasing the snapshot.
async fn read_keys(
    database: &Database,
    predicate: &str,
    value: Option<crate::database::Value>,
) -> Result<Vec<ApiKey>, Error> {
    let mut transaction = database.transaction().await?;
    // A single-key read narrows rule reads; list reads remain one query per collection.
    let is_single_key = value.is_some();
    let arguments: Vec<_> = value.into_iter().collect();
    let rows = transaction
        .query(
            format!("SELECT {API_KEY_COLUMNS} FROM api_keys {predicate} ORDER BY id"),
            arguments,
        )
        .await?;
    let mut keys = BTreeMap::new();
    for row in &rows {
        let key = api_key_from_row(row)?;
        keys.insert(key.id, key);
    }
    let only_id = if is_single_key {
        keys.keys().next().copied()
    } else {
        None
    };
    if !keys.is_empty() {
        for collection in AccessCollection::ALL {
            for (api_key_id, target, access) in
                read_rules(&mut transaction, collection, only_id).await?
            {
                if let Some(key) = keys.get_mut(&api_key_id) {
                    collection.push_into(&mut key.access_control, target, access);
                }
            }
        }
    }
    transaction.commit().await?;
    Ok(keys.into_values().collect())
}

/// Middleware secrets deliberately carry identity without a separately stale policy snapshot.
pub(crate) async fn list_enabled_api_key_secrets(
    database: &Database,
) -> Result<Vec<(i64, String)>, Error> {
    let rows = database
        .query("SELECT id, key FROM api_keys WHERE is_enabled = 1", ())
        .await?;
    rows.iter()
        .map(|row| Ok((row.get(0)?, row.get(1)?)))
        .collect()
}

/// Loads enabled key state and its policy inside the caller's route/listing snapshot.
pub(super) async fn read_enabled_policy(
    transaction: &mut Transaction,
    id: i64,
) -> Result<ApiKeyAccessControl, Error> {
    let rows = transaction
        .query(
            "SELECT default_access FROM api_keys WHERE id = ?1 AND is_enabled = 1",
            params![id],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Err(Error::InactiveApiKey { id });
    };
    let mut policy = ApiKeyAccessControl {
        default_access: access_decision_from_stored(&row.get::<String>(0)?)?,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    };
    for collection in AccessCollection::ALL {
        for (_, target, access) in read_rules(transaction, collection, Some(id)).await? {
            collection.push_into(&mut policy, target, access);
        }
    }
    Ok(policy)
}

/// Validates the payload's typed rule identities without reading mutable database state.
fn policy_references(policy: &ApiKeyAccessControl) -> Result<Vec<AccessReference>, Error> {
    let mut seen = HashSet::new();
    let references = policy
        .owner_defaults
        .iter()
        .map(|rule| match rule.parts().0 {
            RouteOwner::Provider(id) => AccessReference::Provider(id),
            RouteOwner::Pool(id) => AccessReference::Pool(id),
        })
        .chain(
            policy
                .route_overrides
                .iter()
                .map(|rule| AccessReference::Route(rule.model_route_id)),
        )
        .collect::<Vec<_>>();
    for reference in &references {
        if !seen.insert(*reference) {
            return Err(Error::DuplicateAccessReference {
                reference: *reference,
            });
        }
    }
    Ok(references)
}

/// Rejects duplicate identities before any writes, and checks references in the write snapshot.
///
/// Each collection's referenced identities are probed with one `WHERE id IN` query per
/// collection — never one query per authored rule — and the first reference, in authored order,
/// whose row is absent is reported: the identical [`Error::MissingAccessReference`] outcome the
/// per-rule probes produced.
async fn validate_policy(
    transaction: &mut Transaction,
    policy: &ApiKeyAccessControl,
) -> Result<(), Error> {
    let references = policy_references(policy)?;

    // Authored identities are already unique, so each collection's probe list carries distinct
    // ids; the found sets then answer every reference's presence without further queries.
    let mut present = HashMap::<AccessCollection, HashSet<i64>>::new();
    for collection in AccessCollection::ALL {
        let ids: Vec<i64> = references
            .iter()
            .filter(|reference| reference.collection() == collection)
            .copied()
            .map(AccessReference::id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let rows = transaction
            .query(
                format!(
                    "SELECT id FROM {} WHERE id IN ({})",
                    collection.referenced_table(),
                    super::in_placeholders(ids.len()),
                ),
                ids,
            )
            .await?;
        let found = present.entry(collection).or_default();
        for row in rows {
            found.insert(row.get(0)?);
        }
    }

    for reference in references {
        let is_present = present
            .get(&reference.collection())
            .is_some_and(|ids| ids.contains(&reference.id()));
        if !is_present {
            return Err(Error::MissingAccessReference { reference });
        }
    }
    Ok(())
}

/// Checks secret uniqueness in the same snapshot as the subsequent write and reference checks.
async fn check_secret(
    transaction: &mut Transaction,
    secret: &str,
    id: Option<i64>,
) -> Result<(), Error> {
    let rows = transaction
        .query(
            "SELECT id FROM api_keys WHERE key = ?1 AND (?2 IS NULL OR id != ?2)",
            params![secret, id],
        )
        .await?;
    if !rows.is_empty() {
        return Err(Error::Conflict {
            message: api_key_taken_message(),
        });
    }
    Ok(())
}

/// Replaces sparse rules, keeping explicit pins even when they equal inherited defaults.
async fn replace_policy(
    transaction: &mut Transaction,
    id: i64,
    policy: &ApiKeyAccessControl,
) -> Result<(), Error> {
    for table in [
        "api_key_provider_defaults",
        "api_key_pool_defaults",
        "api_key_model_route_overrides",
    ] {
        transaction
            .execute(
                format!("DELETE FROM {table} WHERE api_key_id = ?1"),
                params![id],
            )
            .await?;
    }
    for rule in &policy.owner_defaults {
        let (owner, access) = rule.parts();
        let (table, column, target) = match owner {
            RouteOwner::Provider(target) => ("api_key_provider_defaults", "provider_id", target),
            RouteOwner::Pool(target) => ("api_key_pool_defaults", "pool_id", target),
        };
        transaction
            .execute(
                format!("INSERT INTO {table} (api_key_id, {column}, access) VALUES (?1, ?2, ?3)"),
                params![id, target, access.as_str()],
            )
            .await?;
    }
    for rule in &policy.route_overrides {
        transaction
            .execute(
                "INSERT INTO api_key_model_route_overrides (api_key_id, model_route_id, access) \
             VALUES (?1, ?2, ?3)",
                params![id, rule.model_route_id, rule.access.as_str()],
            )
            .await?;
    }
    Ok(())
}

/// Creates ordinary fields and the full policy atomically, retrying every validation together.
///
/// # Panics
/// Panics if a successful single-row INSERT RETURNING produces no identity.
pub(crate) async fn insert_api_key(
    database: &Database,
    fields: &ApiKeyFields<'_>,
) -> Result<ApiKey, Error> {
    in_retryable_transaction!(database, transaction, async {
        check_secret(&mut transaction, fields.key, None).await?;
        validate_policy(&mut transaction, fields.access_control).await?;
        let created_at = unix_time::unix_now_micros();
        let rows = transaction
            .query(
                "INSERT INTO api_keys (name, key, is_enabled, created_at, default_access) \
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
                params![
                    fields.name,
                    fields.key,
                    i64::from(fields.is_enabled),
                    created_at,
                    fields.access_control.default_access.as_str()
                ],
            )
            .await?;
        #[expect(
            clippy::expect_used,
            reason = "successful single-row INSERT RETURNING yields its identity"
        )]
        let id = rows.first().expect("inserted key identity").get(0)?;
        replace_policy(&mut transaction, id, fields.access_control).await?;
        let mut access_control = fields.access_control.clone();
        access_control.canonicalize();
        Ok::<_, Error>(ApiKey {
            id,
            name: fields.name.into(),
            key: fields.key.into(),
            is_enabled: fields.is_enabled,
            created_at,
            access_control,
        })
    })
}

/// Replaces ordinary fields and the entire policy in one bounded retryable transaction.
pub(crate) async fn update_api_key(
    database: &Database,
    id: i64,
    fields: ApiKeyFields<'_>,
) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        if transaction
            .query("SELECT id FROM api_keys WHERE id = ?1", params![id])
            .await?
            .is_empty()
        {
            return Err(Error::NotFound {
                message: api_key_not_found_message(id),
            });
        }
        check_secret(&mut transaction, fields.key, Some(id)).await?;
        validate_policy(&mut transaction, fields.access_control).await?;
        transaction
            .execute(
                "UPDATE api_keys SET name = ?1, key = ?2, is_enabled = ?3, \
             default_access = ?4 WHERE id = ?5",
                params![
                    fields.name,
                    fields.key,
                    i64::from(fields.is_enabled),
                    fields.access_control.default_access.as_str(),
                    id
                ],
            )
            .await?;
        replace_policy(&mut transaction, id, fields.access_control).await?;
        Ok(())
    })
}

/// Deletes a key and cascades its rules while historical accounting remains separate.
pub(crate) async fn delete_api_key(database: &Database, id: i64) -> Result<bool, Error> {
    Ok(database
        .execute("DELETE FROM api_keys WHERE id = ?1", params![id])
        .await?
        == 1)
}

/// Parses ordinary fields; callers load sparse rules within the same transaction.
fn api_key_from_row(row: &crate::database::Row) -> Result<ApiKey, Error> {
    Ok(ApiKey {
        id: row.get(0)?,
        name: row.get(1)?,
        key: row.get(2)?,
        is_enabled: row.get::<i64>(3)? != 0,
        created_at: row.get(4)?,
        access_control: ApiKeyAccessControl {
            default_access: access_decision_from_stored(&row.get::<String>(5)?)?,
            owner_defaults: Vec::new(),
            route_overrides: Vec::new(),
        },
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
            },
        )
        .await?;

        let second = insert_api_key(
            &database,
            &ApiKeyFields {
                name: "service-b",
                key: "sk-second",
                is_enabled: true,
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
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
                access_control: &crate::test_support::allow_all_access(),
            },
        )
        .await;

        #[expect(
            clippy::expect_used,
            reason = "checked creation rejects the duplicate secret"
        )]
        let error = duplicate.expect_err("a duplicate secret must conflict");

        assert!(
            matches!(error, Error::Conflict { .. }),
            "a duplicate secret must classify as a checked conflict"
        );

        // The empty name and secret are not column invariants: the composition boundary rejects
        // them at the management write, before any statement runs.

        Ok(())
    }

    /// A refused snapshot repeats the real write validation on a fresh transaction: a deleted
    /// owner becomes a missing reference and a concurrently claimed secret becomes a conflict.
    ///
    /// # Errors
    /// Returns fixture, query, or decoding failures.
    ///
    /// # Panics
    /// Panics if stale writes succeed or retry validation ignores the concurrent commit.
    #[tokio::test]
    async fn retries_revalidate_changed_references_and_secrets() -> Result<(), Box<dyn StdError>> {
        for is_reference_deleted in [false, true] {
            let (database, _guard) = create_test_database().await;
            let provider_id =
                crate::storage::test_support::insert_test_provider(&database, "A").await?;
            let policy = ApiKeyAccessControl {
                default_access: AccessDecision::Deny,
                owner_defaults: vec![crate::access_control::ApiKeyOwnerDefault::Provider {
                    provider_id,
                    access: AccessDecision::Allow,
                }],
                route_overrides: Vec::new(),
            };
            let key = insert_api_key(
                &database,
                &ApiKeyFields {
                    name: "original",
                    key: "original-secret",
                    is_enabled: true,
                    access_control: &policy,
                },
            )
            .await?;
            let attempts = std::cell::Cell::new(0);
            let result: Result<(), Error> =
                in_retryable_transaction!(&database, transaction, async {
                    attempts.set(attempts.get() + 1);
                    check_secret(&mut transaction, "replacement-secret", Some(key.id)).await?;
                    validate_policy(&mut transaction, &policy).await?;
                    if attempts.get() == 1 {
                        // Deterministic commit between validation and the attempted snapshot
                        // upgrade.
                        if is_reference_deleted {
                            database
                                .execute(
                                    "DELETE FROM providers WHERE id = ?1",
                                    params![provider_id],
                                )
                                .await?;
                        } else {
                            insert_api_key(
                                &database,
                                &ApiKeyFields {
                                    name: "concurrent",
                                    key: "replacement-secret",
                                    is_enabled: true,
                                    access_control: &crate::test_support::allow_all_access(),
                                },
                            )
                            .await?;
                        }
                    }
                    transaction
                        .execute(
                            "UPDATE api_keys SET key = ?1 WHERE id = ?2",
                            params!["replacement-secret", key.id],
                        )
                        .await?;
                    Ok::<(), Error>(())
                });
            assert_eq!(attempts.get(), 2);
            if is_reference_deleted {
                assert!(matches!(result, Err(Error::MissingAccessReference {
                    reference: AccessReference::Provider(id)
                }) if id == provider_id));
            } else {
                assert!(matches!(result, Err(Error::Conflict { .. })));
            }
            let survived = find_api_key_by_id(&database, key.id)
                .await?
                .ok_or("original key")?;
            assert_eq!(survived.key, key.key);
            assert_eq!(survived.name, key.name);
            assert_eq!(
                survived.access_control.owner_defaults.is_empty(),
                is_reference_deleted
            );
        }
        Ok(())
    }
}
