//! One data-plane send boundary: dispatch stamp, observed headers, quota evidence, and acceptance.

use super::Error;
use crate::{
    context::Context,
    http_body,
    provider::Provider,
    storage::{self, api_logs::ApiLogDraft},
};
use reqwest::StatusCode;

/// Whether a rejection participates in inference quota policy.
#[derive(Clone, Copy)]
pub(crate) enum QuotaPolicy {
    /// Generation and native compaction classify and persist rejection evidence.
    Classify,
    /// Token counters carry no inference quota evidence.
    Disabled,
}

/// Headers and bounded rejection evidence from one dispatched request.
pub(crate) enum DispatchOutcome {
    /// Successful headers commit account ownership before body consumption.
    Accepted(reqwest::Response),
    /// Nonquota rejection retained for operation-specific unsupported classification.
    Rejected {
        /// Actual upstream response status.
        status: StatusCode,
        /// Bounded body read once, retaining complete-input evidence.
        body: http_body::ErrorBody,
    },
}

impl DispatchOutcome {
    /// Converts an ordinary rejection to its outward upstream error, carrying the complete
    /// bounded body as the classification evidence; the outward boundary renders its excerpt.
    ///
    /// # Errors
    ///
    /// Returns the upstream status and the complete bounded body on rejected headers.
    pub(crate) fn into_accepted(self) -> Result<reqwest::Response, Error> {
        match self {
            Self::Accepted(response) => Ok(response),
            Self::Rejected { status, body } => Err(Error::Upstream {
                status,
                detail: body.lossy_text(),
            }),
        }
    }
}

/// Sends once and commits headers before parsing or unsupported classification.
///
/// # Errors
///
/// Returns transport, positive quota, or quota persistence failures. Quota always takes
/// precedence over any caller-specific unsupported classifier. Finalization stays with the caller.
pub(crate) async fn dispatch(
    context: &Context,
    request: reqwest::RequestBuilder,
    provider: &Provider,
    upstream_model: &str,
    draft: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
    quota_policy: QuotaPolicy,
) -> Result<DispatchOutcome, Error> {
    draft.stamp();
    let response = request.send().await.map_err(|error| Error::Unavailable {
        detail: format!("upstream unreachable: {error}"),
    })?;
    let status = response.status();
    draft.observe_response(status);
    if status.is_success() {
        if let Some(ownership) = ownership {
            ownership.accept(provider.id);
        }
        return Ok(DispatchOutcome::Accepted(response));
    }
    let body = http_body::read_bounded_error_body(response).await;
    if matches!(quota_policy, QuotaPolicy::Classify)
        && let Some(error) =
            quota_error_of(context, provider, upstream_model, status, &body).await?
    {
        return Err(error);
    }
    Ok(DispatchOutcome::Rejected { status, body })
}
/// Applies exact quota rules to a rejected generation/native body and orders durable evidence.
/// Token counters and every control-plane operation deliberately never call this function.
///
/// # Errors
///
/// Returns storage failures; a stale generation or deleted account never recreates evidence.
async fn quota_error_of(
    context: &Context,
    provider: &Provider,
    upstream_id: &str,
    status: StatusCode,
    body: &http_body::ErrorBody,
) -> Result<Option<Error>, Error> {
    let Some(restriction) = crate::provider::quota_rejection::classify_rejection(
        status.as_u16(),
        body,
        provider
            .definition
            .payload
            .inference
            .quota_exhaustion
            .as_ref(),
        upstream_id,
    ) else {
        return Ok(None);
    };
    storage::quota_blocks::record_rejection(
        &context.database,
        provider.id,
        provider.quota_generation,
        &restriction.key,
        &restriction.applies_to,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    Ok(Some(Error::QuotaRejected {
        status,
        detail: http_body::truncate_excerpt(&body.lossy_text()),
        restriction: Box::new(restriction),
    }))
}
