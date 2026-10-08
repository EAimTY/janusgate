//! The compaction descriptor: the optional remote-compaction section of a provider definition
//! (README §5) — the strategy, the gateway budgets, the five per-mechanism support declarations
//! with their operation data, and the optional native token-count operation.
//!
//! The section is definition-level connection data only: it never decides a dispatch by itself.
//! A mechanism's effective support resolves per selected model from an administrator override,
//! then a discovery-owned model fact, then the declaration's `default` — an absent declaration
//! is the unknown state, which selects gateway summarization without a speculative native call.
//! The write-time validation enforces the per-format gating and the supported-requires-operation
//! shape once, at the management boundary ([`super::validate`]).

use super::{
    quota_exhaustion::RejectedStatus,
    template::{ParamRow, StringExpr, TemplateNode},
};
use crate::compaction_model::mechanism::{Mechanism, MechanismSupport};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// The remote-compaction section of one provider definition payload (README §5).
///
/// Every member carries a shipped default: a payload may name the section as `{}` and receive
/// the native-preferred strategy with the default budgets and no declared mechanism — the
/// fully-unknown posture that selects gateway summarization for every compaction request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactionSection {
    /// The compaction strategy the gateway follows for providers of this definition.
    #[serde(default)]
    pub(crate) strategy: CompactionStrategy,

    /// The gateway's summary-output budget in tokens: the bound applied to a gateway-generated
    /// summary, a gateway budget rather than a vendor capability fact.
    #[serde(default = "default_summary_budget")]
    pub(crate) summary_budget: NonZeroU64,

    /// The gateway's retained-input budget in tokens: the soft target the retained suffix of a
    /// gateway checkpoint selects under.
    #[serde(default = "default_retention_budget")]
    pub(crate) retention_budget: NonZeroU64,

    /// Whether readable native compaction text may become ordinary summary text when the
    /// selected target cannot consume the native block — an intentional, documented downgrade
    /// that never authorizes discarding opaque reasoning.
    #[serde(default)]
    pub(crate) can_export_native_summary: bool,

    /// The per-mechanism support declarations; an absent member is the unknown state.
    #[serde(default)]
    pub(crate) mechanisms: MechanismDeclarations,

    /// The optional native token-count operation backing threshold measurement; absent means
    /// text-only estimation serves every threshold decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) count_tokens: Option<CountTokensOperation>,
}

impl CompactionSection {
    /// The section's declared default support of one mechanism: the declaration's default when
    /// the mechanism is declared, else the unknown state — an absent declaration is no
    /// declaration at all (README §5).
    pub(in crate::provider) fn declared_default(&self, mechanism: Mechanism) -> MechanismSupport {
        self.mechanisms
            .enumerated()
            .into_iter()
            .find(|(candidate, _)| *candidate == mechanism)
            .and_then(|(_, declaration)| declaration)
            .map_or(MechanismSupport::Unknown, Declaration::declared_default)
    }
}

/// The default summary budget of [`CompactionSection::summary_budget`]: the shipped 4096-token
/// summary-output bound.
///
/// # Panics
///
/// Panics if the shipped default is zero, which the constant prevents.
#[expect(
    clippy::unwrap_used,
    reason = "the shipped default 4096 is a nonzero constant"
)]
fn default_summary_budget() -> NonZeroU64 {
    NonZeroU64::new(4096).unwrap()
}

/// The default retention budget of [`CompactionSection::retention_budget`]: the shipped
/// 8192-token retained-input target.
///
/// # Panics
///
/// Panics if the shipped default is zero, which the constant prevents.
#[expect(
    clippy::unwrap_used,
    reason = "the shipped default 8192 is a nonzero constant"
)]
fn default_retention_budget() -> NonZeroU64 {
    NonZeroU64::new(8192).unwrap()
}

/// The compaction strategy a provider definition follows (README §5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompactionStrategy {
    /// Select a compatible configured native operation; otherwise summarize through ordinary
    /// inference on the selected provider/model. A positively classified runtime unsupported
    /// response may also cause one summary fallback.
    #[default]
    NativePreferred,

    /// Always generate a gateway summary, even when native compaction exists — the mode for
    /// portable state across later provider changes.
    GatewaySummary,
}

/// The per-mechanism support declarations of one compaction section: every member optional —
/// an absent member is the unknown state, meaning no declaration at all. A present member
/// carries the definition's default support plus the operation data executing it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct MechanismDeclarations {
    /// The Responses standalone compact endpoint declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) responses_standalone_compact: Option<StandaloneCompactDeclaration>,

    /// The Responses explicit-trigger declaration: the definition's default support of the
    /// mechanism — the trigger rides the ordinary inference connection, so no operation data
    /// exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) responses_explicit_trigger: Option<RideAlongDeclaration>,

    /// The Responses automatic threshold-compaction declaration: the definition's default
    /// support of the mechanism — the controls ride the ordinary inference request, so no
    /// operation data exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) responses_automatic_compaction: Option<RideAlongDeclaration>,

    /// The Messages on-demand compaction declaration: the definition's default support plus
    /// the beta header overlay the operation requires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) messages_on_demand_compaction: Option<OverlayDeclaration>,

    /// The Messages threshold-compaction declaration: the definition's default support plus
    /// the beta header overlay the operation requires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) messages_threshold_compaction: Option<OverlayDeclaration>,
}

impl MechanismDeclarations {
    /// The keyed enumeration of every declaration member in canonical [`Mechanism`] order: one
    /// `(mechanism, declaration)` pair per member, `None` for the members the section leaves
    /// absent (the unknown state). The destructure is exhaustive, so a member added to the
    /// struct cannot compile without joining this walk — the single enumeration the write-time
    /// gating and validation, the declared-default resolution, and the base-URL requirement
    /// predicate share.
    pub(super) fn enumerated(&self) -> [(Mechanism, Option<Declaration<'_>>); 5] {
        let Self {
            responses_standalone_compact,
            responses_explicit_trigger,
            responses_automatic_compaction,
            messages_on_demand_compaction,
            messages_threshold_compaction,
        } = self;

        [
            (
                Mechanism::ResponsesStandaloneCompact,
                responses_standalone_compact
                    .as_ref()
                    .map(Declaration::Standalone),
            ),
            (
                Mechanism::ResponsesExplicitTrigger,
                responses_explicit_trigger
                    .as_ref()
                    .map(Declaration::RideAlong),
            ),
            (
                Mechanism::ResponsesAutomaticCompaction,
                responses_automatic_compaction
                    .as_ref()
                    .map(Declaration::RideAlong),
            ),
            (
                Mechanism::MessagesOnDemandCompaction,
                messages_on_demand_compaction
                    .as_ref()
                    .map(Declaration::Overlay),
            ),
            (
                Mechanism::MessagesThresholdCompaction,
                messages_threshold_compaction
                    .as_ref()
                    .map(Declaration::Overlay),
            ),
        ]
    }
}

/// The declaration borrow of one declared mechanism, keyed by [`Mechanism`]: the wire shape the
/// member parses into. Mechanism-keyed consumers match the variant carrying the operation data
/// they need.
#[derive(Debug, Clone, Copy)]
pub(super) enum Declaration<'a> {
    /// The standalone compact endpoint declaration with its operation data.
    Standalone(&'a StandaloneCompactDeclaration),

    /// A ride-along declaration (the explicit trigger or the automatic threshold compaction)
    /// carrying no operation data.
    RideAlong(&'a RideAlongDeclaration),

    /// An overlay declaration (the Messages on-demand or threshold compaction) carrying overlay
    /// headers.
    Overlay(&'a OverlayDeclaration),
}

impl Declaration<'_> {
    /// The declaration's default support of its mechanism.
    fn declared_default(self) -> MechanismSupport {
        match self {
            Declaration::Standalone(declaration) => declaration.default,
            Declaration::RideAlong(declaration) => declaration.default,
            Declaration::Overlay(declaration) => declaration.default,
        }
    }
}

/// The Responses standalone-compact declaration: the definition's default support plus the
/// standalone operation executing it (README §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct StandaloneCompactDeclaration {
    /// The definition's default support of the mechanism; supported requires the declaration's
    /// operation (write-time enforced).
    #[serde(default)]
    pub(super) default: MechanismSupport,

    /// The standalone compact connection, independent of the inference descriptor's URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) operation: Option<StandaloneCompactOperation>,

    /// The typed positive unsupported-response classifier consulted when a dispatched native
    /// compact call reports a rejection this definition recognizes as a mechanism or model
    /// boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unsupported: Option<UnsupportedClassifier>,
}

/// The ride-along declaration shape shared by the Responses explicit-trigger and automatic
/// threshold-compaction declarations (README §5): the definition's default support of the
/// mechanism, and the unsupported classifier — the trigger and the automatic controls ride the
/// ordinary inference request, so no operation data exists. The wire JSON is this shape for
/// both mechanisms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct RideAlongDeclaration {
    /// The definition's default support of the mechanism.
    #[serde(default)]
    default: MechanismSupport,

    /// The typed positive unsupported-response classifier of the mechanism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unsupported: Option<UnsupportedClassifier>,
}

/// The overlay declaration shape shared by the Messages on-demand and threshold-compaction
/// declarations (README §5): the definition's default support, the beta header overlay the
/// operation requires, and the unsupported classifier. The headers overlay the ordinary
/// inference connection — the replay of a returned compaction block needs them too, wherever
/// the block later travels. The wire JSON is this shape for both mechanisms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct OverlayDeclaration {
    /// The definition's default support of the mechanism; supported requires non-empty overlay
    /// headers (write-time enforced).
    #[serde(default)]
    pub(super) default: MechanismSupport,

    /// The header rows overlaid onto the ordinary inference connection while the mechanism's
    /// beta applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) overlay_headers: Option<Vec<ParamRow>>,

    /// The typed positive unsupported-response classifier of the mechanism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unsupported: Option<UnsupportedClassifier>,
}

/// The standalone compact connection of a Responses definition (README §5): URL, headers, body
/// envelope, and response unwrapping, described independently from the normal generation
/// connection — never derived by appending to the inference URL. The operation frames its
/// upstream parsing and body stream flags itself: an inference descriptor that forces streaming
/// does not force it here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct StandaloneCompactOperation {
    /// The evaluated compact request URL.
    pub(crate) url: StringExpr,

    /// The evaluated request headers, rendered in the carried order. This list subsumes the
    /// auth style, exactly as the inference descriptor's headers do.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<ParamRow>,

    /// The optional request body envelope: a typed JSON tree whose [`TemplateNode::Body`] —
    /// appearing exactly once — is the codec-rendered compact body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<TemplateNode>,

    /// The optional object-member name the gateway unwraps before ingesting a compact response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) response_unwrap: Option<String>,

    /// The typed positive unsupported-response classifier of the operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unsupported: Option<UnsupportedClassifier>,
}

/// The native token-count operation of a Messages definition (README §5): the connection
/// primitives of the format's concrete count request. Counter calls are opt-in definition
/// data; text-only threshold estimation works without them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct CountTokensOperation {
    /// The evaluated count request URL — never guessed from the inference URL.
    pub(crate) url: StringExpr,

    /// The evaluated request headers, rendered in the carried order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<ParamRow>,

    /// The optional request body envelope: a typed JSON tree whose [`TemplateNode::Body`] —
    /// appearing exactly once — is the codec-rendered count body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<TemplateNode>,

    /// The optional object-member name the gateway unwraps before ingesting a count response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) response_unwrap: Option<String>,
}

/// The typed positive unsupported-response classifier of one native operation (README §5): a
/// dispatched native call whose response matches any listed criterion is a positive
/// mechanism-or-model boundary, the one runtime evidence that permits the single summary
/// fallback. An empty classifier never positively classifies; free-form error-message
/// substring matching does not exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct UnsupportedClassifier {
    /// The upstream HTTP statuses matching the boundary: [`RejectedStatus`] admits only
    /// non-success statuses, so the classifier can never list a criterion a success response
    /// carries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) statuses: Vec<RejectedStatus>,

    /// The upstream error `code` members matching the boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) codes: Vec<String>,

    /// The upstream error `param` members matching the boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) parameters: Vec<String>,
}
