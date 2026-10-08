//! Positive quota rejection classification from complete bounded foreign JSON.

use crate::{
    http_body::ErrorBody,
    provider::{
        definition::quota_exhaustion::{
            QuotaExhaustion, RejectionApplicability, RestrictionKeySegment,
        },
        quota::QuotaApplicability,
    },
};

/// One normal resolved restriction identity from a positively matched rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedQuotaRejection {
    /// Ordered exact identity tuple, shared with readable observation identities.
    pub(crate) key: Vec<String>,
    /// Account-wide or exact requested upstream model applicability.
    pub(crate) applies_to: QuotaApplicability,
}

/// Classifies only a complete bounded valid foreign JSON body, before outward excerpt truncation.
/// No foreign response content survives the returned typed identity/applicability.
pub(crate) fn classify_rejection(
    status: u16,
    body: &ErrorBody,
    descriptor: Option<&QuotaExhaustion>,
    upstream_id: &str,
) -> Option<ClassifiedQuotaRejection> {
    classify_json(status, &body.complete_json()?, descriptor?, upstream_id)
}

/// Evaluates ordered exact object-member rules using the shared response-path resolver.
fn classify_json(
    status: u16,
    body: &serde_json::Value,
    descriptor: &QuotaExhaustion,
    upstream_id: &str,
) -> Option<ClassifiedQuotaRejection> {
    let rule = descriptor.rules.iter().find(|rule| {
        rule.statuses
            .iter()
            .any(|configured| u16::from(*configured) == status)
            && rule
                .code
                .at
                .resolve(body)
                .and_then(serde_json::Value::as_str)
                == Some(rule.code.value.as_str())
    })?;
    let key = rule
        .restriction_key
        .iter()
        .map(|part| match part {
            RestrictionKeySegment::Literal { value } => value.clone(),
            RestrictionKeySegment::RequestedModel => upstream_id.to_string(),
        })
        .collect();
    let applies_to = match rule.applies_to {
        RejectionApplicability::AllModels => QuotaApplicability::AllModels,
        RejectionApplicability::RequestedModel => QuotaApplicability::Models {
            upstream_ids: vec![upstream_id.to_string()],
        },
    };
    Some(ClassifiedQuotaRejection { key, applies_to })
}
