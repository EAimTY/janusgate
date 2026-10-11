//! The gateway-generated compaction checkpoints: the outward carrier values the compaction
//! workflows splice into client payloads (`docs/design/compaction.md`).
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

#[cfg(test)]
mod tests {
    //! Tests for the outward carrier framing: every checkpoint kind serializes under the
    //! vendor's own wire tag, the Responses provenance rides only when present, and native state
    //! is preserved verbatim.

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{Checkpoint, MessagesCheckpoint, ResponsesCheckpoint};
    use serde_json::json;
    use std::error::Error as StdError;

    /// The Responses carrier renders under the vendor `compaction` tag and omits absent
    /// provenance.
    #[test]
    fn responses_checkpoint_wire_shape() -> Result<(), Box<dyn StdError>> {
        let bare = Checkpoint::Responses(ResponsesCheckpoint {
            id: "cmp_1".to_string(),
            encrypted_content: "sealed".to_string(),
            created_by: None,
        });

        assert_eq!(
            serde_json::to_value(&bare)?,
            json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "sealed"})
        );

        let attributed = Checkpoint::Responses(ResponsesCheckpoint {
            id: "cmp_2".to_string(),
            encrypted_content: "sealed".to_string(),
            created_by: Some("native-producer".to_string()),
        });

        assert_eq!(
            serde_json::to_value(&attributed)?,
            json!({
                "type": "compaction",
                "id": "cmp_2",
                "encrypted_content": "sealed",
                "created_by": "native-producer"
            })
        );

        Ok(())
    }

    /// The Messages carrier renders under the vendor `compaction` tag with its bound visible
    /// content.
    #[test]
    fn messages_checkpoint_wire_shape() -> Result<(), Box<dyn StdError>> {
        let checkpoint = Checkpoint::Messages(MessagesCheckpoint {
            content: "Summary of the covered turns.".to_string(),
            signature: "sealed".to_string(),
        });

        assert_eq!(
            serde_json::to_value(&checkpoint)?,
            json!({
                "type": "compaction",
                "content": "Summary of the covered turns.",
                "signature": "sealed"
            })
        );

        Ok(())
    }

    /// Native state is preserved verbatim and the `From<Value>` conversion tags nothing.
    #[test]
    fn native_checkpoint_preserved_verbatim() -> Result<(), Box<dyn StdError>> {
        let native = json!({
            "type": "compaction",
            "id": null,
            "encrypted_content": "producer-cipher",
            "vendor_extension": {"nested": [1, 2]}
        });

        let checkpoint = Checkpoint::from(native.clone());

        assert_eq!(checkpoint, Checkpoint::Native(native.clone()));
        assert_eq!(serde_json::to_value(&checkpoint)?, native);

        Ok(())
    }

    /// Distinct carrier kinds never compare equal even when their wire bytes coincide: the outer
    /// enum is untagged, so a native value carrying the same JSON as a Responses carrier is
    /// wire-identical and the carrier namespace — not this framing — decides recognition.
    #[test]
    fn checkpoint_kinds_stay_distinct() -> Result<(), Box<dyn StdError>> {
        let responses = Checkpoint::Responses(ResponsesCheckpoint {
            id: "cmp_1".to_string(),
            encrypted_content: "sealed".to_string(),
            created_by: None,
        });

        let native_same_fields = Checkpoint::Native(serde_json::to_value(&responses)?);

        assert_ne!(responses, native_same_fields);
        assert_eq!(
            serde_json::to_value(&responses)?,
            serde_json::to_value(&native_same_fields)?
        );

        Ok(())
    }
}
