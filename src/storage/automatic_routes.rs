//! Automatic model exposure owned by standalone providers and pools.

use super::{Error, routing::route_from_row};
use crate::{
    database::{Transaction, params},
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
/// Rebuilding preserves IDs for unchanged targets and permits overlapping prefix renames and
/// direct-to-pool namespace transfers. Authored aliases remain independent. Conflicts roll back
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
    let mut names = BTreeSet::new();
    for entry in &entries {
        if !names.insert(&entry.public_id)
            || !transaction
                .query(
                    "SELECT id FROM model_routes WHERE public_id = ?1 AND is_automatic = 0",
                    params![entry.public_id.as_str()],
                )
                .await?
                .is_empty()
        {
            return Err(Error::Conflict {
                message: format!("Public model ID `{}` is already taken", entry.public_id),
            });
        }
    }
    transaction
        .execute("DELETE FROM model_routes WHERE is_automatic = 1", ())
        .await?;
    for entry in entries {
        let id = existing
            .iter()
            .find(|route| route.target == entry.target)
            .map(|route| route.id);
        let (model_id, pool_id, upstream_id) = match entry.target {
            ModelRouteTarget::Direct { model_id } => (Some(model_id), None, None),
            ModelRouteTarget::Pool {
                pool_id,
                upstream_id,
            } => (None, Some(pool_id), Some(upstream_id)),
        };
        transaction
            .execute(
                "INSERT INTO model_routes \
                 (id, public_id, model_id, pool_id, upstream_id, is_automatic) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                params![id, entry.public_id, model_id, pool_id, upstream_id],
            )
            .await?;
    }
    Ok(())
}
