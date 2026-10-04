//! Digest-only projection of actual sequential Messages SSE content blocks.

use crate::{
    affinity::fingerprint::{Identity, PreparedText, Projector, Role},
    ir::IrEvent,
};
use serde::{Deserialize, Serialize};

/// Conversation-bearing fields of the renderer's own concrete SSE payloads.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    /// Opens one actual outward content block.
    ContentBlockStart {
        /// Actual starting block fields, independent of upstream IR identity.
        content_block: Block,
    },
    /// Extends an opened outward block.
    ContentBlockDelta {
        /// Actual emitted field bytes.
        delta: Delta,
    },
    /// Completes the currently opened block.
    ContentBlockStop,
    /// Content-free envelope, usage, or terminal metadata.
    #[serde(other)]
    Other,
}

/// Actual replay-bearing starting blocks supported by ordinary projection.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    /// Plain assistant text.
    Text {
        /// Starting text, normally empty on the first-party stream.
        text: String,
    },
    /// A whole invocation whose argument deltas follow immediately.
    ToolUse {
        /// Real outward wire identity.
        id: String,
        /// Actual emitted function name.
        name: String,
        /// Starting foreign argument object.
        input: serde_json::Map<String, serde_json::Value>,
    },
    /// Actual replayable thinking fields, initially empty on the first-party stream.
    Thinking {
        /// Initial visible text.
        thinking: String,
        /// Optional initial signature.
        signature: Option<String>,
    },
    /// Unsupported fields or compaction awaiting authenticated preparation.
    #[serde(other)]
    Other,
}

/// Actual field deltas, excluding content-free metadata.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    /// Assistant text bytes.
    #[serde(rename = "text_delta")]
    Text {
        /// Exact emitted UTF-8 text.
        text: String,
    },
    /// Invocation argument bytes, assembled only until the block completes.
    #[serde(rename = "input_json_delta")]
    InputJson {
        /// Exact emitted foreign JSON fragment.
        partial_json: String,
    },
    /// Exact visible reasoning bytes.
    #[serde(rename = "thinking_delta")]
    Thinking {
        /// Actual emitted reasoning chunk.
        thinking: String,
    },
    /// Producer signature bytes exposed to the client.
    #[serde(rename = "signature_delta")]
    Signature {
        /// Actual emitted signature chunk.
        signature: String,
    },
    /// Replay-bearing reasoning/state whose projection is not yet connected.
    #[serde(other)]
    Other,
}

/// One unfinished tool block; its raw fields disappear as soon as it completes.
struct Call {
    /// Actual emitted call identity.
    id: String,
    /// Actual emitted function name.
    name: String,
    /// Initial argument object, used when no delta replaces it.
    input: serde_json::Map<String, serde_json::Value>,
    /// Pending argument JSON, never retained after completed field hashing.
    arguments: String,
}

/// One thinking block retaining only a field digest and its emitted signature metadata.
struct Thinking {
    /// Incremental actual output text, independent of an adjacent ordinary text field.
    text: PreparedText,
    /// Emitted signature metadata, hashed independently when the block completes.
    signature: Option<String>,
}

/// Sequential outward history projection owned by the rendering task.
pub(super) struct Output {
    /// Accepted input tail and output field digests.
    projector: Projector,
    /// At most one outward tool block is open on the Messages surface.
    call: Option<Call>,
    /// At most one outward thinking block; no generated text buffer is retained.
    thinking: Option<Thinking>,
    /// Whether the whole completed result has an implemented replay projection.
    is_fingerprintable: bool,
}

impl Output {
    /// Starts alongside actual rendering, retaining no generated text.
    pub(super) fn new(projector: Projector) -> Self {
        Self {
            projector,
            call: None,
            thinking: None,
            is_fingerprintable: true,
        }
    }

    /// A complete actual checkpoint supersedes every covered conversation part and relationship.
    pub(super) fn replace_window(&mut self, projector: Projector) {
        self.projector = projector;
        self.call = None;
        self.thinking = None;
        self.is_fingerprintable = true;
    }

    /// Observes emitted frames only, never upstream-only done metadata or buffered deltas.
    ///
    /// # Panics
    ///
    /// Panics if the first-party renderer emits malformed frames or argument deltas without
    /// opening their tool block.
    pub(super) fn observe(&mut self, _event: &IrEvent, frames: &[String]) {
        for frame in frames {
            #[expect(
                clippy::expect_used,
                reason = "every first-party SSE frame has a data line"
            )]
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .expect("rendered Messages frame has data");
            #[expect(
                clippy::expect_used,
                reason = "the own renderer emits concrete valid JSON"
            )]
            let frame: Frame = serde_json::from_str(data).expect("rendered Messages JSON");
            match frame {
                Frame::ContentBlockStart {
                    content_block: Block::Text { text },
                }
                | Frame::ContentBlockDelta {
                    delta: Delta::Text { text },
                } => {
                    self.projector.text(Role::Assistant, text.as_bytes());
                }
                Frame::ContentBlockStart {
                    content_block: Block::ToolUse { id, name, input },
                } => {
                    self.call = Some(Call {
                        id,
                        name,
                        input,
                        arguments: String::new(),
                    });
                }
                Frame::ContentBlockStart {
                    content_block:
                        Block::Thinking {
                            thinking,
                            signature,
                        },
                } => {
                    let mut text = self.projector.prepare_text(Role::Assistant, false);
                    text.update(thinking.as_bytes());
                    self.thinking = Some(Thinking { text, signature });
                }
                Frame::ContentBlockDelta {
                    delta: Delta::Thinking { thinking },
                } => {
                    #[expect(
                        clippy::expect_used,
                        reason = "own thinking deltas follow their start"
                    )]
                    let field = self.thinking.as_mut().expect("opened thinking block");
                    field.text.update(thinking.as_bytes());
                }
                Frame::ContentBlockDelta {
                    delta: Delta::Signature { signature },
                } => {
                    #[expect(
                        clippy::expect_used,
                        reason = "own signature deltas follow their start"
                    )]
                    self.thinking
                        .as_mut()
                        .expect("opened thinking block")
                        .signature
                        .get_or_insert_default()
                        .push_str(&signature);
                }
                Frame::ContentBlockDelta {
                    delta: Delta::InputJson { partial_json },
                } => {
                    #[expect(clippy::expect_used, reason = "own tool deltas follow their start")]
                    self.call
                        .as_mut()
                        .expect("opened tool block")
                        .arguments
                        .push_str(&partial_json);
                }
                Frame::ContentBlockStop => {
                    self.complete_call();
                    self.complete_thinking();
                }
                Frame::ContentBlockStart {
                    content_block: Block::Other,
                }
                | Frame::ContentBlockDelta {
                    delta: Delta::Other,
                } => self.is_fingerprintable = false,
                Frame::Other => {}
            }
        }
    }

    /// Completes emitted thinking from independent field digests, including late signatures.
    fn complete_thinking(&mut self) {
        let Some(thinking) = self.thinking.take() else {
            return;
        };
        let reasoning = super::json_affinity::prepare_stream_thinking(
            &self.projector,
            &thinking.text,
            thinking.signature.as_deref(),
        );
        self.projector
            .reasoning_prepared(Role::Assistant, reasoning);
    }

    /// Finalizes actual emitted argument objects without making malformed foreign JSON an error.
    fn complete_call(&mut self) {
        let Some(call) = self.call.take() else {
            return;
        };
        let input = if call.arguments.is_empty() {
            call.input
        } else {
            let Ok(input) =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&call.arguments)
            else {
                self.is_fingerprintable = false;
                return;
            };
            input
        };
        self.projector.call(
            Role::Assistant,
            Identity::Wire(&call.id),
            &call.name,
            &serde_json::Value::Object(input),
            None,
        );
    }

    /// Exposes a completed continuation only when every emitted block is replayable.
    pub(super) fn finish(self) -> Option<Projector> {
        (self.is_fingerprintable && self.call.is_none() && self.thinking.is_none())
            .then_some(self.projector)
    }
}
