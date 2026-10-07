//! Automatic model exposure owned by standalone providers and pools.

use super::{
    Error, in_placeholders,
    routing::{route_from_row, target_columns},
};
use crate::{
    database::{Transaction, Value, params},
    routing::ModelRouteTarget,
};
use std::collections::BTreeSet;

/// One desired public entry, keyed by its stable direct or pooled target.
struct Entry {
    /// Composed provider or pool prefix and exact upstream ID.
    public_id: String,
    /// Physical model or pool model union member.
    target: ModelRouteTarget,
}

/// Synchronizes the automatic namespace in the caller's mutation snapshot.
///
/// Membership hides provider entries even for disabled pools. Each pool exposes the union of
/// its members' stored models; execution filters enabled, non-vanished candidates separately.
/// In-place updates preserve surviving rows and their foreign references, permitting prefix renames
/// and direct-to-pool namespace transfers. Authored aliases remain independent. Conflicts roll back
/// the owning inventory, prefix, membership, or deletion mutation without partial exposure.
///
/// # Errors
///
/// Returns public-name conflicts and database failures.
pub(super) async fn synchronize(transaction: &mut Transaction) -> Result<(), Error> {
    let direct = transaction
        .query(
            "SELECT p.model_prefix || m.upstream_id, m.id FROM models m \
             JOIN providers p ON p.id = m.provider_id WHERE NOT EXISTS \
             (SELECT 1 FROM provider_pool_members member WHERE member.provider_id = p.id)",
            (),
        )
        .await?;
    let mut entries = direct
        .iter()
        .map(|row| {
            Ok::<_, Error>(Entry {
                public_id: row.get(0)?,
                target: ModelRouteTarget::Direct {
                    model_id: row.get(1)?,
                },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let pooled = transaction
        .query(
            "SELECT DISTINCT p.model_prefix || m.upstream_id, p.id, m.upstream_id \
             FROM provider_pools p JOIN provider_pool_members member ON member.pool_id = p.id \
             JOIN models m ON m.provider_id = member.provider_id",
            (),
        )
        .await?;
    entries.extend(
        pooled
            .iter()
            .map(|row| {
                Ok::<_, Error>(Entry {
                    public_id: row.get(0)?,
                    target: ModelRouteTarget::Pool {
                        pool_id: row.get(1)?,
                        upstream_id: row.get(2)?,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let existing = transaction
        .query(
            "SELECT id, public_id, is_enabled, model_id, pool_id, upstream_id, is_automatic \
             FROM model_routes WHERE is_automatic = 1",
            (),
        )
        .await?
        .iter()
        .map(route_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    if entries.len() == existing.len()
        && entries.iter().all(|entry| {
            existing
                .iter()
                .any(|route| route.target == entry.target && route.public_id == entry.public_id)
        })
    {
        return Ok(());
    }
    // The authored namespace is probed once for every distinct desired name — a single query
    // over all of them replaces a probe per entry — while the first offending entry in desired
    // order, a name an authored route holds or a duplicate, reports the conflict.
    let mut names = BTreeSet::new();
    let mut distinct = Vec::new();
    let mut duplicate = None;
    for entry in &entries {
        if !names.insert(&entry.public_id) {
            duplicate = Some(&entry.public_id);
            break;
        }
        distinct.push(entry.public_id.as_str());
    }

    let taken = if distinct.is_empty() {
        BTreeSet::new()
    } else {
        transaction
            .query(
                format!(
                    "SELECT public_id FROM model_routes \
                     WHERE is_automatic = 0 AND public_id IN ({})",
                    in_placeholders(distinct.len())
                ),
                distinct
                    .iter()
                    .map(|name| Value::Text((*name).into()))
                    .collect::<Vec<_>>(),
            )
            .await?
            .iter()
            .map(|row| row.get::<String>(0))
            .collect::<Result<BTreeSet<_>, _>>()?
    };

    let offender = entries
        .iter()
        .find(|entry| taken.contains(&entry.public_id))
        .map(|entry| &entry.public_id)
        .or(duplicate);
    if let Some(public_id) = offender {
        return Err(Error::Conflict {
            message: format!("Public model ID `{public_id}` is already taken"),
        });
    }
    // Public routing tokens cannot contain `:`. Park surviving names in this transaction-only
    // namespace so overlapping prefix renames are unique at every statement, without deleting
    // rows and cascading their route-attached rules. Only departed targets lose their identity.
    for route in &existing {
        if entries.iter().any(|entry| entry.target == route.target) {
            transaction
                .execute(
                    "UPDATE model_routes SET public_id = ?1 WHERE id = ?2",
                    params![format!(":automatic:{}", route.id), route.id],
                )
                .await?;
        } else {
            transaction
                .execute("DELETE FROM model_routes WHERE id = ?1", params![route.id])
                .await?;
        }
    }
    for entry in entries {
        if let Some(route) = existing.iter().find(|route| route.target == entry.target) {
            transaction
                .execute(
                    "UPDATE model_routes SET public_id = ?1 WHERE id = ?2",
                    params![entry.public_id, route.id],
                )
                .await?;
            continue;
        }
        let (model_id, pool_id, upstream_id) = target_columns(&entry.target);
        transaction.execute(
            "INSERT INTO model_routes (public_id, model_id, pool_id, upstream_id, is_automatic) \
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![entry.public_id, model_id, pool_id, upstream_id],
        ).await?;
    }
    Ok(())
}
