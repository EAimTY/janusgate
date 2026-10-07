//! Borrowed projection of actual Messages JSON blocks through ordinary replay normalization.

use super::{OutBlock, TypedBlock};
use crate::affinity::{
    Digest,
    fingerprint::{
        BorrowedJson, Identity, PreparedReasoning, PreparedText, Projector, ReplayFamily, Role,
    },
};
use serde::{Deserialize, Serialize};

/// Accepted assistant fields; foreign fragments borrow strings and sort object members.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Block<'a> {
    /// Signed reasoning normalized through the same typed payload as input parsing.
    Thinking {
        /// Exact preserved text.
        thinking: &'a str,
        /// Optional signature; explicit null normalizes to absence.
        signature: Option<&'a str>,
    },
    /// Opaque redacted reasoning.
    RedactedThinking {
        /// Foreign data, recursively normalized without copying text.
        #[serde(borrow)]
        data: BorrowedJson<'a>,
    },
}

/// Closed normalized opaque reasoning discriminator.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReasoningKind {
    /// Visible signed reasoning.
    Thinking,
    /// Producer-owned redacted reasoning.
    RedactedThinking,
}

/// Canonical known thinking payload, with independently hashed text and signature.
#[derive(Serialize, Deserialize)]
struct Thinking {
    /// Exact reasoning field identity.
    thinking: Digest,
    /// Normalized first-party type tag.
    #[serde(rename = "type")]
    kind: ReasoningKind,
}

/// Canonical input redacted payload, with lexical member order and sorted foreign data.
#[derive(Serialize, Deserialize)]
struct Redacted<'a> {
    /// Borrowed producer data.
    #[serde(borrow)]
    data: BorrowedJson<'a>,
    /// Normalized first-party type tag.
    #[serde(rename = "type")]
    kind: ReasoningKind,
}

/// Projects a complete actual block without building another JSON text or owned replay block.
/// Unsupported foreign output suppresses its anchor without changing the successful response.
///
/// # Panics
///
/// Panics if accepted canonical fields cannot serialize to the infallible hashing sink or a
/// resident history exhausts its invocation ordinal domain.
pub(super) fn project_block(block: &OutBlock, projector: &mut Projector) -> bool {
    match block {
        OutBlock::Typed(TypedBlock::Text { text }) => {
            projector.text(Role::Assistant, text.as_bytes());
        }
        OutBlock::Typed(TypedBlock::ToolUse { id, name, input }) if input.is_object() => {
            projector.call(Role::Assistant, Identity::Wire(id), name, input, None);
        }
        OutBlock::Typed(TypedBlock::Thinking {
            thinking,
            signature,
        }) => {
            project_thinking(projector, thinking, signature.as_deref());
        }
        OutBlock::Typed(TypedBlock::RedactedThinking { data }) => {
            let Ok(data) = BorrowedJson::deserialize(data) else {
                return false;
            };
            project_redacted(projector, data);
        }
        OutBlock::Typed(_) => return false,
        OutBlock::Checkpoint(crate::format::compaction_wire::Checkpoint::Native(value))
        | OutBlock::Verbatim(value) => {
            if super::compaction_affinity::is_native_no_op(value) {
                return true;
            }
            if let Some((window, _)) = super::compaction_affinity::completed_projection(value, None)
            {
                *projector = window;
                return true;
            }
            let Ok(block) = Block::deserialize(value) else {
                return false;
            };
            match block {
                Block::Thinking {
                    thinking,
                    signature,
                } => {
                    project_thinking(projector, thinking, signature);
                }
                Block::RedactedThinking { data } => project_redacted(projector, data),
            }
        }
        OutBlock::Checkpoint(_) => return false,
    }
    true
}

/// Shares exact input normalization across typed and verbatim signed reasoning outputs.
fn project_thinking(projector: &mut Projector, thinking: &str, signature: Option<&str>) {
    let opaque = Thinking {
        thinking: projector.reasoning_text_digest(thinking),
        kind: ReasoningKind::Thinking,
    };
    projector.reasoning_canonical(
        Role::Assistant,
        ReplayFamily::Messages,
        thinking,
        signature,
        &opaque,
    );
}

/// Completes the actual emitted text/signature independently of upstream completion carriers.
///
/// # Panics
///
/// Panics if the digest-only known payload cannot serialize.
pub(super) fn prepare_stream_thinking(
    projector: &Projector,
    field: &PreparedText,
    signature: Option<&str>,
) -> PreparedReasoning {
    let opaque = Thinking {
        thinking: field.digest(),
        kind: ReasoningKind::Thinking,
    };
    projector.prepare_reasoning_fields(ReplayFamily::Messages, field, signature, &opaque)
}

/// Projects redacted data without adding visible text or a synthesized signature.
fn project_redacted(projector: &mut Projector, data: BorrowedJson<'_>) {
    let opaque = Redacted {
        data,
        kind: ReasoningKind::RedactedThinking,
    };
    projector.reasoning_canonical(Role::Assistant, ReplayFamily::Messages, "", None, &opaque);
}
