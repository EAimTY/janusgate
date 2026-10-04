//! Authored exact quota rejection descriptors for rejected generation and native compaction.

use super::{ValidationError, template::ResponsePath};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// An HTTP status that can identify a rejected dispatch, excluding every success status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "management-api-typescript-bindings", ts(type = "number"))]
pub struct RejectedStatus(u16);

impl TryFrom<u16> for RejectedStatus {
    type Error = &'static str;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if !(100..=999).contains(&value) || (200..300).contains(&value) {
            return Err("quota rejection status must be a non-success HTTP status");
        }
        Ok(Self(value))
    }
}

impl From<RejectedStatus> for u16 {
    fn from(value: RejectedStatus) -> Self {
        value.0
    }
}

/// Ordered, nonempty exact JSON rejection rules. First matching rule wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct QuotaExhaustion {
    /// Authored evaluation order, validated once at the management boundary.
    pub rules: Vec<QuotaExhaustionRule>,
}

/// One positive discrimination rule; status alone can never establish exhaustion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct QuotaExhaustionRule {
    /// Unique author-defined diagnostic name, excluded from the restriction identity.
    pub name: String,
    /// Nonempty set of rejected HTTP statuses.
    pub statuses: BTreeSet<RejectedStatus>,
    /// Exact string member required in a complete valid JSON object.
    pub code: ExactQuotaCode,
    /// Nonempty ordered restriction identity recipe.
    pub restriction_key: Vec<RestrictionKeySegment>,
    /// Account-wide or exact requested physical model applicability.
    pub applies_to: RejectionApplicability,
}

/// One object-member string code, matched case-sensitively without coercion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ExactQuotaCode {
    /// Nonempty object-member chain; array addressing is unavailable.
    pub at: ResponsePath,
    /// Exact required string value.
    pub value: String,
}

/// One ordered restriction identity segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum RestrictionKeySegment {
    /// Author-defined identity text.
    Literal {
        /// Nonempty exact segment.
        value: String,
    },
    /// Exact physical model identifier supplied to this dispatch.
    RequestedModel,
}

/// Resolved applicability recipe for one positively classified rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum RejectionApplicability {
    /// Blocks every exact model on the account.
    AllModels,
    /// Blocks only this dispatch's requested exact upstream model.
    RequestedModel,
}

impl QuotaExhaustion {
    /// Rejects incomplete discrimination or ambiguous authored rule identities at write time.
    ///
    /// # Errors
    ///
    /// Returns errors for empty rules, names, status sets, paths, or restriction recipes, and
    /// duplicate rule names. String code values remain exact, including an explicitly empty code.
    pub(super) fn validate(&self) -> Result<(), ValidationError> {
        if self.rules.is_empty() {
            return Err(ValidationError::new(
                "quota exhaustion rules must not be empty",
            ));
        }
        let mut names = BTreeSet::new();
        for rule in &self.rules {
            if rule.name.is_empty() || !names.insert(&rule.name) {
                return Err(ValidationError::new(
                    "quota exhaustion rule names must be nonempty and unique",
                ));
            }
            if rule.statuses.is_empty() {
                return Err(ValidationError::new(
                    "quota exhaustion statuses must not be empty",
                ));
            }
            if rule.code.at.0.is_empty() || rule.code.at.0.iter().any(String::is_empty) {
                return Err(ValidationError::new(
                    "quota exhaustion code requires a nonempty object-member path",
                ));
            }
            let has_empty_segment = rule.restriction_key.iter().any(
                |part| matches!(part, RestrictionKeySegment::Literal { value } if value.is_empty()),
            );
            if rule.restriction_key.is_empty() || has_empty_segment {
                return Err(ValidationError::new(
                    "quota restriction keys require nonempty segments",
                ));
            }
        }
        Ok(())
    }
}
