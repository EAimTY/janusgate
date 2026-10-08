//! Read-only, provider-independent account quota extraction and execution.

pub(crate) mod coordinator;
pub(crate) mod extraction;
mod types;

use super::{Error as ProviderError, Provider, control, oauth};
use crate::{database::Database, provider::definition::eval::ValueEnv};
use extraction::extract;
use reqwest::Client;
pub(crate) use types::{QuotaApplicability, QuotaEntry, QuotaReset, SafeDuration, SafeTimestamp};

/// A completed observation, stamped once before atomic extraction.
#[derive(Debug, Clone)]
pub(crate) struct Observation {
    /// UTC microseconds at successful response decoding.
    pub(crate) observed_at: SafeTimestamp,
    /// Complete normalized observations.
    pub(crate) quotas: Vec<QuotaEntry>,
}

/// Quota execution failures, preserving renewal classification and safe extraction context.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum Error {
    /// The exact current definition carries no quota query.
    #[error("provider {provider_id} does not declare quota viewing")]
    NotConfigured {
        /// Addressed provider identity.
        provider_id: i64,
    },
    /// A renewal or upstream request failed; all foreign body text is removed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Normal credential renewal failed before the quota read.
    #[error(transparent)]
    Renewal(ProviderError),
    /// A response failed to decode as JSON, or its body exceeded the bounded success-body read
    /// before ending — either way without retaining foreign content.
    #[error("quota response could not be read as JSON")]
    MalformedResponse,
    /// Atomic extraction failed without carrying foreign values.
    #[error(transparent)]
    Extraction(#[from] extraction::ExtractionError),
}

/// One read's complete observation and authoritative predispatch ordering fence.
struct FencedObservation {
    /// Complete atomically extracted reading.
    observation: Observation,
    /// Fence captured after ordinary credential renewal and before upstream execution.
    fence: crate::storage::quota_blocks::QuotaFence,
}

/// Queries current credentials, capturing interpretation and evidence immediately before I/O.
///
/// # Panics
///
/// Panics when first-party configuration or the gateway clock violates its documented contract.
async fn query_fenced(
    database: &Database,
    client: &Client,
    provider: &Provider,
) -> Result<FencedObservation, Error> {
    if provider.definition.payload.quota.is_none() {
        return Err(Error::NotConfigured {
            provider_id: provider.id,
        });
    }
    let provider = oauth::ensure_fresh_token(database, client, provider)
        .await
        .map_err(|error| Error::Renewal(redact_renewal(error)))?;
    let descriptor = provider
        .definition
        .payload
        .quota
        .as_ref()
        .ok_or(Error::NotConfigured {
            provider_id: provider.id,
        })?;
    let environment = ValueEnv::for_stored_credential(
        provider.credentials.slots(),
        provider.base_url.as_deref(),
        provider.credentials.secret(),
    );
    #[expect(
        clippy::expect_used,
        reason = "stored quota expressions satisfy write-time stage validation"
    )]
    let request = control::build_request(client, &descriptor.request, &environment, "quota")
        .expect("stored quota request must render");
    #[expect(
        clippy::expect_used,
        reason = "first-party stored request configuration must produce a valid HTTP request"
    )]
    let request = request.build().expect("stored quota request must build");
    let fence = crate::storage::quota_blocks::QuotaFence {
        generation: provider.quota_generation,
        revision: provider.quota_revision,
    };
    let response = client.execute(request).await.map_err(|error| {
        if error.is_timeout() {
            ProviderError::Timeout {
                detail: "quota request timed out".into(),
            }
        } else {
            ProviderError::Unavailable {
                detail: "quota request transport failed".into(),
            }
        }
    })?;
    if !response.status().is_success() {
        return Err(ProviderError::Rejected {
            status: response.status().as_u16(),
            detail: "quota request rejected".into(),
        }
        .into());
    }
    // The success body rides the shared bounded read (README §4), so a hostile quota upstream
    // cannot grow gateway memory without limit; the bytes then decode as one JSON document.
    let bytes = crate::http_body::read_success_body(response)
        .await
        .map_err(|error| match error {
            // An oversized body names no readable observation and, like a decode failure,
            // retains no foreign content.
            crate::http_body::SuccessBodyError::Oversized => Error::MalformedResponse,

            // `Read` wraps the reqwest error, so a body-read timeout stays a timeout — the
            // classification the coordinator throttles on — not a malformed observation.
            crate::http_body::SuccessBodyError::Read(error) if error.is_timeout() => {
                Error::Provider(ProviderError::Timeout {
                    detail: "quota response read timed out".into(),
                })
            }

            crate::http_body::SuccessBodyError::Read(_) => {
                Error::Provider(ProviderError::Unavailable {
                    detail: "quota response transport failed".into(),
                })
            }
        })?;
    let body = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|_| Error::MalformedResponse)?;
    #[expect(
        clippy::expect_used,
        reason = "the gateway clock must satisfy its timestamp contract"
    )]
    let observed_at = SafeTimestamp::try_from(crate::unix_time::unix_now_micros())
        .expect("observation timestamp is safe");
    let quotas = extract(&body, descriptor)?;
    Ok(FencedObservation {
        observation: Observation {
            observed_at,
            quotas,
        },
        fence,
    })
}

/// Removes potentially secret foreign error details at the quota boundary, including renewal.
fn redact_renewal(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::Rejected { status, .. } => ProviderError::Rejected {
            status,
            detail: "quota credential renewal rejected".into(),
        },
        ProviderError::Unavailable { .. } => ProviderError::Unavailable {
            detail: "quota credential renewal failed".into(),
        },
        ProviderError::Timeout { .. } => ProviderError::Timeout {
            detail: "quota credential renewal timed out".into(),
        },
        other => other,
    }
}
