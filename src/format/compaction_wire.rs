//! The gateway-generated compaction checkpoints: the outward carrier values the compaction
//! workflows splice into client payloads (README §6.8).
//!
//! Checkpoints keep their concrete types until the HTTP/SSE serialization boundary; only
//! producer-owned native state retains arbitrary JSON. The iteration reports and native control
//! entries beside them are the IR's compaction facts, living in
//! [`crate::ir::compaction`](crate::ir::compaction).

use serde::Serialize;
use serde_json::Value;

/// A completed checkpoint rendered before an answer or as a compact-only result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum Checkpoint {
    /// A gateway-generated Responses carrier.
    Responses(ResponsesCheckpoint),
    /// A gateway-generated Messages carrier.
    Messages(MessagesCheckpoint),
    /// Complete producer-owned state or a native canonical window item, preserved verbatim.
    Native(Value),
}

impl From<Value> for Checkpoint {
    fn from(value: Value) -> Self {
        Self::Native(value)
    }
}

/// The gateway-generated Responses compaction item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename = "compaction")]
pub(crate) struct ResponsesCheckpoint {
    /// Gateway-issued item identifier.
    pub(super) id: String,
    /// Authenticated gateway state carrier.
    pub(super) encrypted_content: String,
    /// Optional authoring provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) created_by: Option<String>,
}

/// The gateway-generated Messages compaction block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename = "compaction")]
pub(crate) struct MessagesCheckpoint {
    /// Readable summary or the fixed encrypted-state transport description.
    pub(super) content: String,
    /// Authenticated gateway carrier bound to the visible content.
    pub(super) signature: String,
}
