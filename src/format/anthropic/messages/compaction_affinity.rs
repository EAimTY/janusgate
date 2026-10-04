//! Borrowed canonical identity of an actual completed Messages compaction block.

use crate::affinity::{
    Anchor,
    fingerprint::{BorrowedJson, Projector, StateFamily, StateKind},
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

/// Closed discriminator of a replayable compaction block.
#[derive(Serialize, Deserialize)]
enum Kind {
    /// Client-held compaction state.
    #[serde(rename = "compaction")]
    Compaction,
}

/// Ordinary replay normalization, in lexical member order. Null optional fields are omitted;
/// caching controls are ignored. Foreign tool changes borrow text and sort object members.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Block<'a> {
    /// Accepted but ignored caching hint.
    #[serde(rename = "cache_control", skip_serializing)]
    _cache_control: Option<IgnoredAny>,
    /// Native summary or gateway-visible carrier text; absence is the native no-op.
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    /// Optional native ciphertext.
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted_content: Option<&'a str>,
    /// Native signature or our newly sealed carrier.
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<&'a str>,
    /// Exact foreign tool changes, normalized through borrowed canonical JSON fragments.
    #[serde(borrow, skip_serializing_if = "Option::is_none")]
    tool_changes: Option<Vec<BorrowedJson<'a>>>,
    /// Canonical ordinary replay discriminator.
    #[serde(rename = "type")]
    kind: Kind,
}

/// Derives artifact and returned-state history from the exact block rendered outward. A gateway
/// identity must come from the sealer; reflected signatures cannot invent that provenance.
///
/// # Panics
///
/// Panics if a sealer-proven gateway block lacks the signature its builder always supplies.
pub(crate) fn completed_anchors(block: &Value, artifact_id: Option<&str>) -> Vec<Anchor> {
    completed_projection(block, artifact_id).map_or_else(Vec::new, |(_, anchors)| anchors)
}

/// Prepares digest-only actual block state and anchors for a checkpoint followed by an answer.
///
/// # Panics
///
/// Panics if a sealer-proven gateway block lacks its required signature.
pub(crate) fn completed_projection(
    block: &Value,
    artifact_id: Option<&str>,
) -> Option<(Projector, Vec<Anchor>)> {
    let Ok(block) = Block::deserialize(block) else {
        return None;
    };
    let mut projector = Projector::new();
    let artifact = if let Some(artifact_id) = artifact_id {
        #[expect(
            clippy::expect_used,
            reason = "the gateway carrier builder supplies its signature"
        )]
        let carrier = block
            .signature
            .expect("a sealed gateway block has its signature");
        projector.gateway_state(StateFamily::Messages, carrier);
        projector.gateway_artifact(artifact_id)
    } else {
        if block.content.is_none()
            || block
                .signature
                .is_some_and(crate::compaction::sealing::is_gateway_marker)
        {
            return None;
        }
        projector.state_canonical(StateFamily::Messages, StateKind::Native, &block);
        projector.canonical_artifact(&block)
    };
    let mut anchors = projector.completed_anchors();
    anchors.push(Anchor::Artifact(artifact));
    Some((projector, anchors))
}

/// Whether an actual strictly replayable block is native null/absent-content state. Gateway
/// markers still require authentication and cannot acquire no-op semantics from missing text.
pub(super) fn is_native_no_op(value: &Value) -> bool {
    Block::deserialize(value).is_ok_and(|block| {
        block.content.is_none()
            && !block
                .signature
                .is_some_and(crate::compaction::sealing::is_gateway_marker)
    })
}
