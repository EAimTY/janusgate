//! Compaction-specific pieces of the Anthropic Messages codec: the outbound gateway carrier
//! block and the tolerant typed view of a vendor compaction block.
//!
//! The Messages compaction wire (request parsing, upstream render extensions, upstream capture,
//! client rendering) lives in the family's concern modules beside this one; this module holds
//! only the pieces those directions share.

use serde::Deserialize;

#[cfg(test)]
mod tests;

/// The fixed carrier description rendered as a gateway compaction block's `content` when the
/// sealed envelope carries no readable summary — wrapped native state with no exported text. It
/// is a transport marker, never text sent to a model and never a substitute summary.
pub(crate) const GATEWAY_CARRIER_DESCRIPTION: &str =
    "JanusGate compacted conversation state (encrypted); replay through JanusGate.";

/// The tolerant typed view of a vendor `compaction` content block: every member optional, extra
/// vendor members ignored. The block's shape is the vendor's to define, so upstream parsing
/// captures the raw vendor fragment beside this view for replay fidelity; this struct only decides
/// which members the gateway reads.
#[derive(Debug, Default, Clone, Deserialize)]
pub(super) struct CompactionBlock {
    /// The readable summary content; `None` marks the native null-content no-op block.
    #[serde(default)]
    pub(super) content: Option<String>,
}

/// Builds a concrete Messages gateway carrier with readable content or the fixed marker.
pub(crate) fn gateway_carrier_block(
    content: Option<&str>,
    signature: &str,
) -> crate::format::compaction_wire::Checkpoint {
    crate::format::compaction_wire::Checkpoint::Messages(
        crate::format::compaction_wire::MessagesCheckpoint {
            content: content.unwrap_or(GATEWAY_CARRIER_DESCRIPTION).to_owned(),
            signature: signature.to_owned(),
        },
    )
}
