//! Management actions for ordered account pools.

use super::{EmptyResponse, Error, route};
use crate::{context::Context, routing::ProviderPoolMember, storage::routing};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};

route! {
    pub(super) ListProviderPools: {
        path: "/provider-pools:list",
        method: http::Method::POST,
        request: ListProviderPoolsRequest,
        response: ListProviderPoolsResponse,
    }
}

route! {
    pub(super) CreateProviderPool: {
        path: "/provider-pools:create",
        method: http::Method::POST,
        request: CreateProviderPoolRequest,
        response: CreateProviderPoolResponse,
    }
}

route! {
    pub(super) UpdateProviderPool: {
        path: "/provider-pools:update",
        method: http::Method::POST,
        request: UpdateProviderPoolRequest,
        response: EmptyResponse,
    }
}

route! {
    pub(super) DeleteProviderPool: {
        path: "/provider-pools:delete",
        method: http::Method::POST,
        request: DeleteProviderPoolRequest,
        response: EmptyResponse,
    }
}

/// Complete stored administrator-authored state.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ProviderPoolResponse {
    /// Never-recycled row identifier.
    pub id: i64,
    /// Unique pool label.
    pub name: String,
    /// Prefix concatenated with exact upstream IDs for automatic pool exposure.
    pub model_prefix: String,
    /// Administrator serving switch.
    pub is_enabled: bool,
    /// Complete account membership; priorities define the selection order.
    pub members: Vec<ProviderPoolMember>,
}

/// Empty request for the complete inventory.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListProviderPoolsRequest {}

/// All stored provider-pools, including disabled entries.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListProviderPoolsResponse {
    /// Rows ordered by pool name.
    pub provider_pools: Vec<ProviderPoolResponse>,
}

/// Complete initial authored state.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateProviderPoolRequest {
    /// Unique pool label.
    pub name: String,
    /// Prefix concatenated with exact upstream IDs for automatic pool exposure.
    pub model_prefix: String,
    /// Administrator serving switch.
    pub is_enabled: bool,
    /// Complete account membership; priorities define the selection order.
    pub members: Vec<ProviderPoolMember>,
}

/// New identity assigned by the server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateProviderPoolResponse {
    /// Created row identifier.
    pub provider_pool_id: i64,
}

/// Complete replacement state; preserves the row identity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateProviderPoolRequest {
    /// Addressed row identifier.
    pub provider_pool_id: i64,
    /// Unique pool label.
    pub name: String,
    /// Prefix concatenated with exact upstream IDs for automatic pool exposure.
    pub model_prefix: String,
    /// Administrator serving switch.
    pub is_enabled: bool,
    /// Complete account membership; priorities define the selection order.
    pub members: Vec<ProviderPoolMember>,
}

/// Delete one stored entity by identity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DeleteProviderPoolRequest {
    /// Addressed row identifier.
    pub provider_pool_id: i64,
}

/// Lists the complete authored inventory without upstream I/O.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(_request), _): WithRejection<Json<ListProviderPoolsRequest>, Error>,
) -> Result<Json<ListProviderPoolsResponse>, Error> {
    let rows = routing::list_pools(&context.database).await?;
    Ok(Json(ListProviderPoolsResponse {
        provider_pools: rows
            .into_iter()
            .map(|row| ProviderPoolResponse {
                id: row.id,
                name: row.name,
                model_prefix: row.model_prefix,
                is_enabled: row.is_enabled,
                members: row.members,
            })
            .collect(),
    }))
}

/// Creates the authored state transactionally.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateProviderPoolRequest>, Error>,
) -> Result<Json<CreateProviderPoolResponse>, Error> {
    validate_fields(&request.name, &request.members)?;
    super::validate_model_prefix(&request.model_prefix)?;
    let id = routing::create_pool(
        &context.database,
        &request.name,
        &request.model_prefix,
        request.is_enabled,
        &request.members,
    )
    .await?;
    Ok(Json(CreateProviderPoolResponse {
        provider_pool_id: id,
    }))
}

/// Replaces the complete authored state transactionally.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateProviderPoolRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    validate_fields(&request.name, &request.members)?;
    super::validate_model_prefix(&request.model_prefix)?;
    routing::update_pool(
        &context.database,
        request.provider_pool_id,
        &request.name,
        &request.model_prefix,
        request.is_enabled,
        &request.members,
    )
    .await?;
    Ok(Json(EmptyResponse {}))
}

/// Deletes the addressed entity; referenced pools produce a conflict.
/// Deletion and the affinity handle invalidation share the version's detached mutation owner across
/// caller cancellation: ids are never reused, so a committed write whose invalidation is lost to a
/// cancelled caller would leave a stale handle forever.
///
/// # Panics
///
/// Panics if the mutation task fails to hand off its result, a fail-fast task invariant.
pub(super) async fn delete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DeleteProviderPoolRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    let database = context.database.clone();
    let cache = context.affinity_cache.clone();
    let pool_id = request.provider_pool_id;
    super::detach_mutation(async move {
        routing::delete_pool(&database, pool_id).await?;
        cache.remove_pool(pool_id);
        Ok(())
    })
    .await?;

    Ok(Json(EmptyResponse {}))
}

/// Validates the authored pool label and unique member identities/priorities.
///
/// # Errors
///
/// Returns a client validation failure for an empty label or duplicate member/order key.
fn validate_fields(name: &str, members: &[ProviderPoolMember]) -> Result<(), Error> {
    super::validate_identifier("name", name)?;
    let mut providers = std::collections::BTreeSet::new();
    let mut priorities = std::collections::BTreeSet::new();
    for member in members {
        if !providers.insert(member.provider_id) || !priorities.insert(member.priority) {
            return Err(Error::Validation {
                param: "members",
                message: "Pool members require unique provider IDs and priorities".to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Duplicate identities and order keys are client faults, while gaps and empty pools are valid.
    ///
    /// # Panics
    ///
    /// Panics if authored total-order validation differs from the contract.
    #[test]
    fn member_order_rejects_duplicates_without_requiring_contiguous_priorities() {
        let a = ProviderPoolMember {
            provider_id: 1,
            priority: 4,
        };
        let b = ProviderPoolMember {
            provider_id: 2,
            priority: 9,
        };
        assert!(validate_fields("coding", &[a.clone(), b.clone()]).is_ok());
        assert!(validate_fields("coding", &[]).is_ok());
        assert!(
            validate_fields(
                "coding",
                &[
                    a.clone(),
                    ProviderPoolMember {
                        provider_id: 1,
                        priority: 9
                    }
                ]
            )
            .is_err()
        );
        assert!(
            validate_fields(
                "coding",
                &[
                    a,
                    ProviderPoolMember {
                        provider_id: 2,
                        priority: 4
                    }
                ]
            )
            .is_err()
        );
    }
}
