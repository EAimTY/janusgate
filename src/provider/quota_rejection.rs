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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::definition::quota_exhaustion::QuotaExhaustion;
    use serde_json::json;

    /// Ordered exact object-member rules resolve normal restriction tuples, never rule names.
    /// Non-string/missing codes, wrong statuses, case changes, and trailing malformed JSON cannot
    /// establish exhaustion. Requested-model recipes retain the exact physical upstream ID.
    ///
    /// # Panics
    ///
    /// Panics if a fixture cannot parse or classification broadens the authored contract.
    #[test]
    fn exact_rules_resolve_ordered_identity_and_applicability() {
        #[expect(
            clippy::expect_used,
            reason = "the deliberate ordered classifier recipe parses"
        )]
        let rules: QuotaExhaustion = serde_json::from_value(json!({"rules":[
            {"name":"weekly","statuses":[429,403],
                "code":{"at":["error","code"],"value":"quota_exhausted"},
                "restriction_key":[{"type":"literal","value":"account"},
                    {"type":"literal","value":"weekly"}],
                "applies_to":{"type":"all_models"}},
            {"name":"model","statuses":[429],
                "code":{"at":["error","code"],"value":"model_exhausted"},
                "restriction_key":[{"type":"literal","value":"models"},{"type":"requested_model"}],
                "applies_to":{"type":"requested_model"}},
            {"name":"overlapping","statuses":[429],
                "code":{"at":["error","code"],"value":"quota_exhausted"},
                "restriction_key":[{"type":"literal","value":"later"}],
                "applies_to":{"type":"requested_model"}}
        ]}))
        .expect("recipe parses");
        #[expect(
            clippy::expect_used,
            reason = "the configured exact quota code matches"
        )]
        let weekly = classify_json(
            429,
            &json!({"error":{"code":"quota_exhausted"}}),
            &rules,
            "M",
        )
        .expect("weekly matches");
        assert_eq!(weekly.key, ["account", "weekly"]);
        assert_eq!(
            weekly.applies_to,
            crate::provider::quota::QuotaApplicability::AllModels
        );
        #[expect(
            clippy::expect_used,
            reason = "the configured exact model quota code matches"
        )]
        let model = classify_json(
            429,
            &json!({"error":{"code":"model_exhausted"}}),
            &rules,
            "exact-M",
        )
        .expect("model matches");
        assert_eq!(model.key, ["models", "exact-M"]);
        assert_eq!(
            model.applies_to,
            crate::provider::quota::QuotaApplicability::Models {
                upstream_ids: vec!["exact-M".into()]
            }
        );
        for status in [200, 400, 404, 500] {
            assert!(
                classify_json(
                    status,
                    &json!({"error":{"code":"quota_exhausted"}}),
                    &rules,
                    "M"
                )
                .is_none()
            );
        }
        for body in [
            json!({}),
            json!({"error":null}),
            json!({"error":{"code":null}}),
            json!({"error":{"code":429}}),
            json!({"error":{"code":false}}),
            json!({"error":{"code":"QUOTA_EXHAUSTED"}}),
            json!({"error":[{"code":"quota_exhausted"}]}),
        ] {
            assert!(classify_json(429, &body, &rules, "M").is_none());
        }
    }

    /// A rule requires complete discriminating evidence and cannot reduce to status-only policy.
    ///
    /// # Panics
    ///
    /// Panics if an incomplete or ambiguous authored policy is accepted at the write boundary.
    #[test]
    fn rule_validation_rejects_status_only_or_ambiguous_policies() {
        let mut payload = crate::test_support::static_key_definition_payload(
            crate::format::UpstreamFormat::OpenAiResponse,
        );
        let valid = json!({"rules":[{"name":"weekly","statuses":[429],
            "code":{"at":["error","code"],"value":"quota_exhausted"},
            "restriction_key":[{"type":"literal","value":"account"}],
            "applies_to":{"type":"all_models"}}]});
        for (path, value) in [
            ("/rules", json!([])),
            ("/rules/0/statuses", json!([])),
            ("/rules/0/code/at", json!([])),
            ("/rules/0/restriction_key", json!([])),
            ("/rules/0/name", json!("")),
            ("/rules/0/code/at", json!([""])),
            (
                "/rules/0/restriction_key",
                json!([{"type":"literal","value":""}]),
            ),
        ] {
            let mut invalid = valid.clone();
            #[expect(
                clippy::expect_used,
                reason = "all mutation paths exist in the deliberate policy fixture"
            )]
            let target = invalid.pointer_mut(path).expect("fixture path exists");
            *target = value;
            #[expect(
                clippy::expect_used,
                reason = "the shape remains typed; its cross-field contract is invalid"
            )]
            let invalid = serde_json::from_value(invalid).expect("typed rule parses");
            payload.inference.quota_exhaustion = Some(invalid);
            assert!(payload.validate().is_err());
        }
        let duplicate = json!({"rules":[
            valid.pointer("/rules/0"), valid.pointer("/rules/0")
        ]});
        #[expect(clippy::expect_used, reason = "duplicate names remain a typed policy")]
        let duplicate = serde_json::from_value(duplicate).expect("typed duplicate parses");
        payload.inference.quota_exhaustion = Some(duplicate);
        assert!(payload.validate().is_err());
        for status in [0, 99, 200, 299, 1000] {
            let mut invalid = valid.clone();
            #[expect(clippy::expect_used, reason = "the configured statuses path exists")]
            let statuses = invalid
                .pointer_mut("/rules/0/statuses")
                .expect("fixture path exists");
            *statuses = json!([status]);
            assert!(serde_json::from_value::<QuotaExhaustion>(invalid).is_err());
        }
    }
}
