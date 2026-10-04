//! Transactional public route and ordered account pool persistence.

use super::Error;
use crate::{
    database::{Database, Row, Transaction, params},
    routing::{ModelRouteTarget, ProviderPoolMember},
};

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;

/// Stored pool with its complete ordered membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderPool {
    /// Never-recycled row identifier.
    pub(crate) id: i64,
    /// Unique administrator-authored label.
    pub(crate) name: String,
    /// Prefix used by automatically exposed pool models.
    pub(crate) model_prefix: String,
    /// Administrator serving switch.
    pub(crate) is_enabled: bool,
    /// All members, including disabled accounts, in ascending priority order.
    pub(crate) members: Vec<ProviderPoolMember>,
}

/// Stored route in the sole administrator-authored public namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelRoute {
    /// Whether this direct entry is owned by its provider/model lifecycle.
    pub(crate) is_automatic: bool,
    /// Never-recycled row identifier, preserved on target changes.
    pub(crate) id: i64,
    /// Unique public model name.
    pub(crate) public_id: String,
    /// Administrator serving switch.
    pub(crate) is_enabled: bool,
    /// Exactly one direct or pooled target.
    pub(crate) target: ModelRouteTarget,
}

/// Parses the nullability-encoded target union from a route row.
///
/// # Errors
///
/// Returns database decoding failures.
///
/// # Panics
///
/// Panics if stored target columns violate their schema constraint.
pub(super) fn route_from_row(row: &Row) -> Result<ModelRoute, Error> {
    let model_id: Option<i64> = row.get(3)?;
    let pool_id: Option<i64> = row.get(4)?;
    let upstream_id: Option<String> = row.get(5)?;
    let target = match (model_id, pool_id, upstream_id) {
        (Some(model_id), None, None) => ModelRouteTarget::Direct { model_id },
        (None, Some(pool_id), Some(upstream_id)) => ModelRouteTarget::Pool {
            pool_id,
            upstream_id,
        },
        #[expect(
            clippy::panic,
            reason = "the target union is enforced by the schema CHECK"
        )]
        _ => panic!("stored model route violates its target union"),
    };
    Ok::<_, Error>(ModelRoute {
        is_automatic: row.get::<i64>(6)? != 0,
        id: row.get(0)?,
        public_id: row.get(1)?,
        is_enabled: row.get::<i64>(2)? != 0,
        target,
    })
}

/// Lists pool rows and membership in one coherent database snapshot.
///
/// # Errors
///
/// Returns database errors.
pub(crate) async fn list_pools(database: &Database) -> Result<Vec<ProviderPool>, Error> {
    let mut transaction = database.transaction().await?;
    let rows = transaction
        .query(
            "SELECT id, name, is_enabled, model_prefix FROM provider_pools ORDER BY name",
            (),
        )
        .await?;
    let mut pools = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: i64 = row.get(0)?;
        let members = transaction
            .query(
                "SELECT provider_id, priority FROM provider_pool_members WHERE pool_id = \
                 ?1 ORDER BY priority",
                params![id],
            )
            .await?;
        pools.push(ProviderPool {
            id,
            name: row.get(1)?,
            model_prefix: row.get(3)?,
            is_enabled: row.get::<i64>(2)? != 0,
            members: members
                .iter()
                .map(|member| {
                    Ok::<_, Error>(ProviderPoolMember {
                        provider_id: member.get(0)?,
                        priority: member.get(1)?,
                    })
                })
                .collect::<Result<_, Error>>()?,
        });
    }
    transaction.commit().await?;
    Ok::<_, Error>(pools)
}

/// Lists every authored route, including disabled or presently unavailable targets.
///
/// # Errors
///
/// Returns database errors.
pub(crate) async fn list_routes(database: &Database) -> Result<Vec<ModelRoute>, Error> {
    database
        .query(
            "SELECT id, public_id, is_enabled, model_id, pool_id, upstream_id, is_automatic FROM \
                 model_routes ORDER BY public_id",
            (),
        )
        .await?
        .iter()
        .map(route_from_row)
        .collect()
}

/// Checks the existence of an entity within the mutation's snapshot.
///
/// # Errors
///
/// Returns not-found or database errors.
async fn require_entity(
    transaction: &mut Transaction,
    table: &str,
    label: &str,
    id: i64,
) -> Result<(), Error> {
    if transaction
        .query(format!("SELECT id FROM {table} WHERE id = ?1"), params![id])
        .await?
        .is_empty()
    {
        return Err(Error::NotFound {
            message: format!("{label} {id} does not exist"),
        });
    }
    Ok::<_, Error>(())
}

/// Checks a name against other rows within the mutation's snapshot.
///
/// # Errors
///
/// Returns conflict or database errors.
async fn require_free_name(
    transaction: &mut Transaction,
    table: &str,
    column: &str,
    name: &str,
    excluded_id: Option<i64>,
) -> Result<(), Error> {
    if !transaction
        .query(
            format!("SELECT id FROM {table} WHERE {column} = ?1 AND (?2 IS NULL OR id != ?2)"),
            params![name, excluded_id],
        )
        .await?
        .is_empty()
    {
        return Err(Error::Conflict {
            message: format!("{column} `{name}` is already taken"),
        });
    }
    Ok::<_, Error>(())
}

/// Validates desired account references before deleting any existing membership.
///
/// # Errors
///
/// Returns not-found or database errors.
async fn require_members(
    transaction: &mut Transaction,
    members: &[ProviderPoolMember],
) -> Result<(), Error> {
    for member in members {
        require_entity(transaction, "providers", "Provider", member.provider_id).await?;
    }
    Ok::<_, Error>(())
}

/// Replaces the ordered membership without unique-priority intermediate collisions.
///
/// # Errors
///
/// Returns database errors.
async fn replace_members(
    transaction: &mut Transaction,
    pool_id: i64,
    members: &[ProviderPoolMember],
) -> Result<(), Error> {
    // Delete/reinsert occurs within the same transaction as the pool's fields, so priority swaps
    // satisfy uniqueness statement by statement and failed updates expose no partial state.
    transaction
        .execute(
            "DELETE FROM provider_pool_members WHERE pool_id = ?1",
            params![pool_id],
        )
        .await?;
    for member in members {
        transaction
            .execute(
                "INSERT INTO provider_pool_members (pool_id, provider_id, priority) \
                 VALUES (?1, ?2, ?3)",
                params![pool_id, member.provider_id, i64::from(member.priority)],
            )
            .await?;
    }
    Ok::<_, Error>(())
}

/// Creates a pool and its complete membership atomically.
///
/// # Errors
///
/// Returns unknown-reference, name-conflict, or database errors.
///
/// # Panics
///
/// Panics if a successful single-row INSERT RETURNING yields no row.
pub(crate) async fn create_pool(
    database: &Database,
    name: &str,
    model_prefix: &str,
    is_enabled: bool,
    members: &[ProviderPoolMember],
) -> Result<i64, Error> {
    in_retryable_transaction!(database, transaction, async {
        require_free_name(&mut transaction, "provider_pools", "name", name, None).await?;
        require_members(&mut transaction, members).await?;
        let rows = transaction
            .query(
                "INSERT INTO provider_pools (name, is_enabled, model_prefix) \
                 VALUES (?1, ?2, ?3) RETURNING id",
                params![name, i64::from(is_enabled), model_prefix],
            )
            .await?;
        #[expect(
            clippy::indexing_slicing,
            reason = "single-row INSERT RETURNING yields the inserted row"
        )]
        let id: i64 = rows[0].get(0)?;
        replace_members(&mut transaction, id, members).await?;
        super::automatic_routes::synchronize(&mut transaction).await?;
        Ok::<_, Error>(id)
    })
}

/// Replaces all pool fields and membership in one retryable transaction.
///
/// # Errors
///
/// Returns unknown-reference, name-conflict, or database errors.
pub(crate) async fn update_pool(
    database: &Database,
    id: i64,
    name: &str,
    model_prefix: &str,
    is_enabled: bool,
    members: &[ProviderPoolMember],
) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        require_entity(&mut transaction, "provider_pools", "Provider pool", id).await?;
        require_free_name(&mut transaction, "provider_pools", "name", name, Some(id)).await?;
        require_members(&mut transaction, members).await?;
        transaction
            .execute(
                "UPDATE provider_pools SET name = ?1, is_enabled = ?2, model_prefix = ?4 \
                 WHERE id = ?3",
                params![name, i64::from(is_enabled), id, model_prefix],
            )
            .await?;
        replace_members(&mut transaction, id, members).await?;
        super::automatic_routes::synchronize(&mut transaction).await
    })
}

/// Deletes a pool only when no public route refers to it.
///
/// # Errors
///
/// Returns not-found, reference-conflict, or database errors.
pub(crate) async fn delete_pool(database: &Database, id: i64) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        require_entity(&mut transaction, "provider_pools", "Provider pool", id).await?;
        if !transaction
            .query(
                "SELECT id FROM model_routes WHERE pool_id = ?1 AND is_automatic = 0",
                params![id],
            )
            .await?
            .is_empty()
        {
            return Err(Error::Conflict {
                message: format!("Provider pool {id} is referenced by a model route"),
            });
        }
        transaction
            .execute(
                "DELETE FROM model_routes WHERE pool_id = ?1 AND is_automatic = 1",
                params![id],
            )
            .await?;
        transaction
            .execute("DELETE FROM provider_pools WHERE id = ?1", params![id])
            .await?;
        super::automatic_routes::synchronize(&mut transaction).await?;
        Ok::<_, Error>(())
    })
}

/// Checks the authored target's reference without requiring runtime capacity.
///
/// # Errors
///
/// Returns not-found or database errors.
async fn require_target(
    transaction: &mut Transaction,
    target: &ModelRouteTarget,
) -> Result<(), Error> {
    match target {
        ModelRouteTarget::Direct { model_id } => {
            require_entity(transaction, "models", "Model", *model_id).await
        }
        ModelRouteTarget::Pool { pool_id, .. } => {
            require_entity(transaction, "provider_pools", "Provider pool", *pool_id).await
        }
    }
}

/// Encodes the concrete target union as the nullable SQL columns.
fn target_columns(target: &ModelRouteTarget) -> (Option<i64>, Option<i64>, Option<&str>) {
    match target {
        ModelRouteTarget::Direct { model_id } => (Some(*model_id), None, None),
        ModelRouteTarget::Pool {
            pool_id,
            upstream_id,
        } => (None, Some(*pool_id), Some(upstream_id)),
    }
}

/// Creates an explicit public route; target aliases are permitted.
///
/// # Errors
///
/// Returns unknown-reference, public-name-conflict, or database errors.
///
/// # Panics
///
/// Panics if a successful single-row INSERT RETURNING yields no row.
pub(crate) async fn create_route(
    database: &Database,
    public_id: &str,
    is_enabled: bool,
    target: &ModelRouteTarget,
) -> Result<i64, Error> {
    in_retryable_transaction!(database, transaction, async {
        require_free_name(
            &mut transaction,
            "model_routes",
            "public_id",
            public_id,
            None,
        )
        .await?;
        require_target(&mut transaction, target).await?;
        let (model_id, pool_id, upstream_id) = target_columns(target);
        let rows = transaction
            .query(
                "INSERT INTO model_routes (public_id, is_enabled, model_id, pool_id, \
                 upstream_id) VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
                params![
                    public_id,
                    i64::from(is_enabled),
                    model_id,
                    pool_id,
                    upstream_id
                ],
            )
            .await?;
        #[expect(
            clippy::indexing_slicing,
            reason = "single-row INSERT RETURNING yields the inserted row"
        )]
        Ok::<_, Error>(rows[0].get(0)?)
    })
}

/// Replaces a route's complete authored state while preserving its identity.
///
/// # Errors
///
/// Returns unknown-reference, public-name-conflict, or database errors.
pub(crate) async fn update_route(
    database: &Database,
    id: i64,
    public_id: &str,
    is_enabled: bool,
    target: &ModelRouteTarget,
) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        require_entity(&mut transaction, "model_routes", "Model route", id).await?;
        if !transaction
            .query(
                "SELECT id FROM model_routes WHERE id = ?1 AND is_automatic = 1",
                params![id],
            )
            .await?
            .is_empty()
        {
            return Err(Error::Conflict {
                message: "Automatic routes are managed through their provider or pool".into(),
            });
        }
        require_free_name(
            &mut transaction,
            "model_routes",
            "public_id",
            public_id,
            Some(id),
        )
        .await?;
        require_target(&mut transaction, target).await?;
        let (model_id, pool_id, upstream_id) = target_columns(target);
        transaction
            .execute(
                "UPDATE model_routes SET public_id = ?1, is_enabled = ?2, model_id = ?3, \
                 pool_id = ?4, upstream_id = ?5 WHERE id = ?6",
                params![
                    public_id,
                    i64::from(is_enabled),
                    model_id,
                    pool_id,
                    upstream_id,
                    id
                ],
            )
            .await?;
        Ok::<_, Error>(())
    })
}

/// Deletes a route by its stable row identifier.
///
/// # Errors
///
/// Returns not-found or database errors.
pub(crate) async fn delete_route(database: &Database, id: i64) -> Result<(), Error> {
    in_retryable_transaction!(database, transaction, async {
        require_entity(&mut transaction, "model_routes", "Model route", id).await?;
        if !transaction
            .query(
                "SELECT id FROM model_routes WHERE id = ?1 AND is_automatic = 1",
                params![id],
            )
            .await?
            .is_empty()
        {
            return Err(Error::Conflict {
                message: "Automatic routes are managed through their provider or pool".into(),
            });
        }
        transaction
            .execute("DELETE FROM model_routes WHERE id = ?1", params![id])
            .await?;
        Ok::<_, Error>(())
    })
}

/// Committed entity presence used to fence a first affinity acquisition after deletion.
pub(crate) struct NamespacePresence {
    /// Whether the authenticated key still exists.
    pub(crate) has_api_key: bool,
    /// Whether the authored pool still exists.
    pub(crate) has_pool: bool,
}

/// Reads deletion fences after acquiring an ephemeral cache generation.
///
/// # Errors
///
/// Returns database read or row conversion failures.
///
/// # Panics
///
/// Panics if a scalar SELECT without a FROM clause does not return its single row.
pub(crate) async fn namespace_presence(
    database: &Database,
    api_key_id: i64,
    pool_id: i64,
) -> Result<NamespacePresence, Error> {
    let rows = database
        .query(
            "SELECT EXISTS(SELECT 1 FROM api_keys WHERE id = ?1), \
        EXISTS(SELECT 1 FROM provider_pools WHERE id = ?2)",
            params![api_key_id, pool_id],
        )
        .await?;
    #[expect(
        clippy::expect_used,
        reason = "a scalar SELECT always produces exactly one row"
    )]
    let row = rows.first().expect("scalar namespace presence row");
    Ok(NamespacePresence {
        has_api_key: row.get::<i64>(0)? != 0,
        has_pool: row.get::<i64>(1)? != 0,
    })
}
