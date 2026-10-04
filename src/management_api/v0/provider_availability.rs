//! Read-only availability administration and explicit account evidence reset.

use super::{Error, route};
use crate::{
    context::Context, provider::quota::coordinator::LastRefresh, storage::quota_blocks::QuotaBlock,
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};

route! {
    pub(super) GetProviderAvailability: {
        path: "/providers:availability",
        method: http::Method::POST,
        request: GetProviderAvailabilityRequest,
        response: GetProviderAvailabilityResponse,
    }
}

route! {
    pub(super) ResetProviderAvailability: {
        path: "/providers:reset-availability",
        method: http::Method::POST,
        request: ResetProviderAvailabilityRequest,
        response: ResetProviderAvailabilityResponse,
    }
}

/// Read-only account availability request; serving switches do not constrain administration.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct GetProviderAvailabilityRequest {
    /// Addressed account identifier.
    pub provider_id: i64,
}

/// Committed restrictions and current-process safe refresh metadata.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct GetProviderAvailabilityResponse {
    /// Durable account restrictions sorted lexicographically by exact identity tuple.
    pub blocks: Vec<QuotaBlock>,
    /// A tagged completed-check outcome; absence after restart is `not_observed`.
    pub last_refresh: LastRefresh,
}

/// Explicit reset request; resets account evidence shared by every pool.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct ResetProviderAvailabilityRequest {
    /// Addressed account identifier.
    pub provider_id: i64,
}

/// Empty completion of the generation-advancing availability reset.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct ResetProviderAvailabilityResponse {}

/// Reads availability without renewal, quota I/O, or derived-state writes.
pub(super) async fn get(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<GetProviderAvailabilityRequest>, Error>,
) -> Result<Json<GetProviderAvailabilityResponse>, Error> {
    let (blocks, last_refresh) = context
        .quota_coordinator
        .availability(&context.database, request.provider_id)
        .await?
        .ok_or(Error::ProviderNotFound {
            id: request.provider_id,
        })?;
    Ok(Json(GetProviderAvailabilityResponse {
        blocks,
        last_refresh,
    }))
}

/// Clears restrictions and installs one automatic-refresh interval of unknown/unblocked grace.
pub(super) async fn reset(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<ResetProviderAvailabilityRequest>, Error>,
) -> Result<Json<ResetProviderAvailabilityResponse>, Error> {
    context
        .quota_coordinator
        .reset(&context.database, request.provider_id)
        .await?;
    log::info!("provider {} availability reset", request.provider_id);
    Ok(Json(ResetProviderAvailabilityResponse {}))
}
