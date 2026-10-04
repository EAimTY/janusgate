//! Coherent route and physical account snapshots shared by listing and dispatch selection.

use super::{Error, routing::ModelRoute};
use crate::{
    database::{Database, Transaction, params},
    provider::{Model, Provider},
    routing::ModelRouteTarget,
};

/// One authored pool member, including unavailable accounts and missing physical inventory.
#[derive(Debug, Clone)]
pub(crate) struct MemberSnapshot {
    /// Nonnegative authored selection order.
    pub(crate) priority: u32,
    /// Concrete credential-bearing account.
    pub(crate) provider: Provider,
    /// Exact upstream model, when present in the account's inventory.
    pub(crate) model: Option<Model>,
}

impl MemberSnapshot {
    /// Whether this member has an enabled, non-vanished physical target on an enabled account.
    pub(crate) fn is_structural_candidate(&self) -> bool {
        self.model
            .as_ref()
            .is_some_and(|model| model.is_effectively_enabled(self.provider.is_enabled))
    }
}

/// A route's complete target state at one database snapshot.
#[derive(Debug, Clone)]
pub(crate) enum TargetSnapshot {
    /// One physical model and its real account.
    Direct {
        /// Concrete credential-bearing account.
        provider: Box<Provider>,
        /// Exact physical model.
        model: Box<Model>,
    },
    /// An ordered authored membership, retaining members that cannot currently serve.
    Pool {
        /// Stable pool identity.
        id: i64,
        /// Public owner label.
        name: String,
        /// Administrator serving switch.
        is_enabled: bool,
        /// Authored member order and physical state.
        members: Vec<MemberSnapshot>,
    },
}

/// An explicit public route and its coherent concrete target state.
#[derive(Debug, Clone)]
pub(crate) struct RouteSnapshot {
    /// Authored route identity, public name, and serving switch.
    pub(crate) route: ModelRoute,
    /// Physical target or pool membership state.
    pub(crate) target: TargetSnapshot,
}

impl RouteSnapshot {
    /// Current typed owner in this coherent snapshot; pooled members never affect ownership.
    pub(crate) fn owner(&self) -> crate::access_control::RouteOwner {
        match &self.target {
            TargetSnapshot::Direct { provider, .. } => {
                crate::access_control::RouteOwner::Provider(provider.id)
            }
            TargetSnapshot::Pool { id, .. } => crate::access_control::RouteOwner::Pool(*id),
        }
    }
}

/// Reads one public route and its physical targets without holding a transaction across I/O.
///
/// # Errors
///
/// Returns database failures or malformed stored rows.
pub(crate) async fn find_admitted_route(
    database: &Database,
    public_id: &str,
    api_key_id: i64,
) -> Result<Option<RouteSnapshot>, Error> {
    let mut transaction = database.transaction().await?;
    let snapshot = read_admitted_route(&mut transaction, public_id, api_key_id).await?;
    transaction.commit().await?;
    Ok(snapshot)
}

/// Resolves key state, policy, route identity, and owner within the caller's snapshot.
pub(super) async fn read_admitted_route(
    transaction: &mut Transaction,
    public_id: &str,
    api_key_id: i64,
) -> Result<Option<RouteSnapshot>, Error> {
    let policy = super::api_keys::read_enabled_policy(transaction, api_key_id).await?;
    let rows = transaction
        .query(
            "SELECT id, public_id, is_enabled, model_id, pool_id, upstream_id, is_automatic \
         FROM model_routes WHERE public_id = ?1",
            params![public_id],
        )
        .await?;
    let snapshot = match rows.first() {
        Some(row) => Some(read_target(transaction, super::routing::route_from_row(row)?).await?),
        None => None,
    };
    Ok(snapshot.filter(|snapshot| {
        policy.resolve(snapshot.route.id, snapshot.owner())
            == crate::access_control::AccessDecision::Allow
    }))
}

/// Structural fixture read through the same production admission path.
#[cfg(test)]
pub(crate) async fn find_route(
    database: &Database,
    public_id: &str,
) -> Result<Option<RouteSnapshot>, Error> {
    let api_key_id = crate::test_support::structural_api_key(database).await?;
    find_admitted_route(database, public_id, api_key_id).await
}

/// Reads a route's target within the caller's coherent snapshot.
///
/// # Errors
///
/// Returns database failures or malformed stored rows.
///
/// # Panics
///
/// Panics if an enforced foreign reference is absent in the same transaction.
pub(super) async fn read_target(
    transaction: &mut Transaction,
    route: ModelRoute,
) -> Result<RouteSnapshot, Error> {
    let target = match &route.target {
        ModelRouteTarget::Direct { model_id } => {
            let rows = transaction
                .query(
                    format!(
                        "SELECT {}, {} {} WHERE m.id = ?1",
                        super::PROVIDER_COLUMNS,
                        super::MODEL_COLUMNS_JOINED,
                        super::PROVIDER_JOINED_FROM_MODELS
                    ),
                    params![model_id],
                )
                .await?;
            #[expect(
                clippy::expect_used,
                reason = "model foreign references hold throughout the snapshot"
            )]
            let row = rows.first().expect("stored route's physical target exists");
            let joined = super::provider_model_from_row(row)?;
            TargetSnapshot::Direct {
                provider: Box::new(joined.provider),
                model: Box::new(joined.model),
            }
        }
        ModelRouteTarget::Pool {
            pool_id,
            upstream_id,
        } => {
            let rows = transaction
                .query(
                    "SELECT name, is_enabled FROM provider_pools WHERE id = ?1",
                    params![pool_id],
                )
                .await?;
            #[expect(
                clippy::expect_used,
                reason = "pool foreign references hold throughout the snapshot"
            )]
            let row = rows.first().expect("stored route's pool exists");
            TargetSnapshot::Pool {
                id: *pool_id,
                name: row.get(0)?,
                is_enabled: row.get::<i64>(1)? != 0,
                members: read_members(transaction, *pool_id, upstream_id).await?,
            }
        }
    };
    Ok(RouteSnapshot { route, target })
}

/// Loads ordered real members, preserving disabled accounts and absent exact models.
///
/// # Errors
///
/// Returns database failures or malformed stored rows.
///
/// # Panics
///
/// Panics if an enforced account reference is absent in the same transaction.
async fn read_members(
    transaction: &mut Transaction,
    pool_id: i64,
    upstream_id: &str,
) -> Result<Vec<MemberSnapshot>, Error> {
    let rows = transaction
        .query(
            "SELECT provider_id, priority FROM provider_pool_members \
         WHERE pool_id = ?1 ORDER BY priority",
            params![pool_id],
        )
        .await?;
    let mut members = Vec::with_capacity(rows.len());
    for row in &rows {
        let provider_id: i64 = row.get(0)?;
        let provider =
            super::providers::find_provider_by_id_in_transaction(transaction, provider_id).await?;
        #[expect(
            clippy::expect_used,
            reason = "member account foreign references hold throughout the snapshot"
        )]
        let provider = provider.expect("stored member's provider exists");
        let rows = transaction
            .query(
                format!(
                    "SELECT {} FROM models WHERE provider_id = ?1 AND upstream_id = ?2",
                    super::MODEL_COLUMNS
                ),
                params![provider_id, upstream_id],
            )
            .await?;
        members.push(MemberSnapshot {
            priority: row.get(1)?,
            provider,
            model: rows.first().map(super::model_from_row).transpose()?,
        });
    }
    members.sort_by_key(|member| member.priority);
    Ok(members)
}
