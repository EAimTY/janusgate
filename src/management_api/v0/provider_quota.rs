//! Forced upstream quota observation and fenced account availability reconciliation.

use super::{Error, route};
use crate::{context::Context, provider::quota};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};

route! {
    pub(super) GetProviderQuota: {
        path: "/providers:quota",
        method: http::Method::POST,
        request: GetProviderQuotaRequest,
        response: GetProviderQuotaResponse,
    }
}

/// Strict quota query request; provider serving state does not constrain it.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct GetProviderQuotaRequest {
    /// Provider row whose exact definition and credentials are queried.
    provider_id: i64,
}

/// One complete upstream reading; unknown facts remain explicit variants.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct GetProviderQuotaResponse {
    /// Gateway observation time, UTC microseconds, stamped before extraction.
    observed_at: quota::SafeTimestamp,
    /// Independently identified upstream allowance pools, in deterministic order.
    quotas: Vec<quota::QuotaEntry>,
}

/// Forces or joins a current-generation read and reconciles its complete derived evidence.
pub(super) async fn get(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<GetProviderQuotaRequest>, Error>,
) -> Result<Json<GetProviderQuotaResponse>, Error> {
    let observation = context
        .quota_coordinator
        .refresh(
            &context.database,
            &context.control_plane,
            request.provider_id,
        )
        .await?;
    Ok(Json(GetProviderQuotaResponse {
        observed_at: observation.observed_at,
        quotas: observation.quotas,
    }))
}
