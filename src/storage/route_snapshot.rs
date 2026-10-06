//! Credential-bearing route and physical account snapshots for dispatch selection.

use super::{Error, routing::ModelRoute};
use crate::{
    database::{Database, Transaction, Value, params},
    provider::{Model, Provider},
    routing::ModelRouteTarget,
};
use std::collections::HashMap;

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
                        super::providers::PROVIDER_COLUMNS,
                        super::models::MODEL_COLUMNS_JOINED,
                        super::providers::PROVIDER_JOINED_FROM_MODELS
                    ),
                    params![model_id],
                )
                .await?;
            #[expect(
                clippy::expect_used,
                reason = "model foreign references hold throughout the snapshot"
            )]
            let row = rows.first().expect("stored route's physical target exists");
            let joined = super::models::provider_model_from_row(row)?;
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
                    "SELECT is_enabled FROM provider_pools WHERE id = ?1",
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
                is_enabled: row.get::<i64>(0)? != 0,
                members: read_members(transaction, *pool_id, upstream_id).await?,
            }
        }
    };
    Ok(RouteSnapshot { route, target })
}

/// Loads ordered real members, preserving disabled accounts and absent exact models.
///
/// The member inventory is read in three statements — the authored order, then the members'
/// accounts over their ids, then their exact upstream models over the same ids — mapping each
/// result back by its provider id, so the per-dispatch admission path never pays a statement
/// per member.
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

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let member_ids: Vec<i64> = rows
        .iter()
        .map(|row| row.get(0))
        .collect::<Result<_, _>>()?;

    // Both batched queries spell one `?N` placeholder per member: the provider read numbers
    // the member ids from `?1`, the model read from `?2` behind the shared upstream id.
    let member_placeholders = |start: usize| {
        member_ids
            .iter()
            .enumerate()
            .map(|(index, _)| format!("?{}", index + start))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let member_id_params: Vec<Value> = member_ids.iter().map(|id| Value::from(*id)).collect();

    let provider_rows = transaction
        .query(
            format!(
                "SELECT {} {} WHERE p.id IN ({})",
                super::providers::PROVIDER_COLUMNS,
                super::providers::PROVIDER_JOINED_FROM,
                member_placeholders(1)
            ),
            member_id_params,
        )
        .await?;
    let mut providers = HashMap::with_capacity(provider_rows.len());
    for row in &provider_rows {
        let provider = super::providers::provider_from_row(row)?;
        providers.insert(provider.id, provider);
    }

    let mut model_params = Vec::with_capacity(member_ids.len() + 1);
    model_params.push(Value::Text(upstream_id.to_string()));
    model_params.extend(member_ids.iter().map(|id| Value::from(*id)));
    let model_rows = transaction
        .query(
            format!(
                "SELECT provider_id, {} FROM models \
             WHERE upstream_id = ?1 AND provider_id IN ({})",
                super::models::MODEL_COLUMNS,
                member_placeholders(2)
            ),
            model_params,
        )
        .await?;
    let mut models = HashMap::with_capacity(model_rows.len());
    for row in &model_rows {
        let model = super::models::model_from_row_offset(row, 1)?;
        models.insert(row.get::<i64>(0)?, model);
    }

    let mut members = Vec::with_capacity(rows.len());
    for row in &rows {
        let provider_id: i64 = row.get(0)?;
        #[expect(
            clippy::expect_used,
            reason = "member account foreign references hold throughout the snapshot"
        )]
        let provider = providers
            .remove(&provider_id)
            .expect("stored member's provider exists");
        members.push(MemberSnapshot {
            priority: row.get(1)?,
            provider,
            model: models.remove(&provider_id),
        });
    }
    members.sort_by_key(|member| member.priority);
    Ok(members)
}
