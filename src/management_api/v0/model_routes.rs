//! Management actions for explicit public model routes.

use super::{EmptyResponse, Error, route};
use crate::{context::Context, routing::ModelRouteTarget, storage::routing};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};

route! {
    pub(super) ListModelRoutes: {
        path: "/model-routes:list",
        method: http::Method::POST,
        request: ListModelRoutesRequest,
        response: ListModelRoutesResponse,
    }
}

route! {
    pub(super) CreateModelRoute: {
        path: "/model-routes:create",
        method: http::Method::POST,
        request: CreateModelRouteRequest,
        response: CreateModelRouteResponse,
    }
}

route! {
    pub(super) UpdateModelRoute: {
        path: "/model-routes:update",
        method: http::Method::POST,
        request: UpdateModelRouteRequest,
        response: EmptyResponse,
    }
}

route! {
    pub(super) DeleteModelRoute: {
        path: "/model-routes:delete",
        method: http::Method::POST,
        request: DeleteModelRouteRequest,
        response: EmptyResponse,
    }
}

/// Complete stored administrator-authored state.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ModelRouteResponse {
    /// Automatic direct entries follow provider prefixes and physical model switches.
    pub is_automatic: bool,
    /// Never-recycled row identifier.
    pub id: i64,
    /// Explicit public model identifier.
    pub public_id: String,
    /// Administrator serving switch.
    pub is_enabled: bool,
    /// One physical model or a pool and exact upstream identifier.
    pub target: ModelRouteTarget,
}

/// Empty request for the complete inventory.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListModelRoutesRequest {}

/// All stored model-routes, including disabled entries.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListModelRoutesResponse {
    /// Rows ordered by public identifier.
    pub model_routes: Vec<ModelRouteResponse>,
}

/// Complete initial authored state.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateModelRouteRequest {
    /// Explicit public model identifier.
    pub public_id: String,
    /// Administrator serving switch.
    pub is_enabled: bool,
    /// One physical model or a pool and exact upstream identifier.
    pub target: ModelRouteTarget,
}

/// New identity assigned by the server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateModelRouteResponse {
    /// Created row identifier.
    pub model_route_id: i64,
}

/// Complete replacement state; preserves the row identity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateModelRouteRequest {
    /// Addressed row identifier.
    pub model_route_id: i64,
    /// Explicit public model identifier.
    pub public_id: String,
    /// Administrator serving switch.
    pub is_enabled: bool,
    /// One physical model or a pool and exact upstream identifier.
    pub target: ModelRouteTarget,
}

/// Delete one stored entity by identity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DeleteModelRouteRequest {
    /// Addressed row identifier.
    pub model_route_id: i64,
}

/// Lists the complete authored inventory without upstream I/O.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(_request), _): WithRejection<Json<ListModelRoutesRequest>, Error>,
) -> Result<Json<ListModelRoutesResponse>, Error> {
    let rows = routing::list_routes(&context.database).await?;
    Ok(Json(ListModelRoutesResponse {
        model_routes: rows
            .into_iter()
            .map(|row| ModelRouteResponse {
                id: row.id,
                is_automatic: row.is_automatic,
                public_id: row.public_id,
                is_enabled: row.is_enabled,
                target: row.target,
            })
            .collect(),
    }))
}

/// Creates the authored state transactionally.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateModelRouteRequest>, Error>,
) -> Result<Json<CreateModelRouteResponse>, Error> {
    validate_fields(&request.public_id, &request.target)?;
    let id = routing::create_route(
        &context.database,
        &request.public_id,
        request.is_enabled,
        &request.target,
    )
    .await?;
    Ok(Json(CreateModelRouteResponse { model_route_id: id }))
}

/// Replaces the complete authored state transactionally.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateModelRouteRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    validate_fields(&request.public_id, &request.target)?;
    routing::update_route(
        &context.database,
        request.model_route_id,
        &request.public_id,
        request.is_enabled,
        &request.target,
    )
    .await?;
    Ok(Json(EmptyResponse {}))
}

/// Deletes the addressed public route without deleting its physical model or pool.
pub(super) async fn delete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DeleteModelRouteRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    routing::delete_route(&context.database, request.model_route_id).await?;
    Ok(Json(EmptyResponse {}))
}

/// Validates the public name and, for a pool target, the exact upstream identifier.
///
/// # Errors
///
/// Returns a client fault for a token outside the routing-safe name contract.
fn validate_fields(public_id: &str, target: &ModelRouteTarget) -> Result<(), Error> {
    super::validate_routing_token("public_id", public_id)?;
    if let ModelRouteTarget::Pool { upstream_id, .. } = target {
        super::validate_routing_token("upstream_id", upstream_id)?;
    }
    Ok(())
}
