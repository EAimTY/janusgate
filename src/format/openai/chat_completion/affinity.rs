//! Incremental projection of actual Chat Completions frames, independent of upstream IR identities.

use crate::affinity::fingerprint::{Identity, PreparedCall, Projector, Role};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Only emitted conversation fields participate; resource IDs, reasoning, usage, and finish
/// metadata remain outside these concrete readback shapes.
#[derive(Serialize, Deserialize)]
struct Frame {
    /// One served choice, or an empty terminal usage chunk.
    choices: Vec<Choice>,
}

/// Conversation-bearing portion of one emitted choice.
#[derive(Serialize, Deserialize)]
struct Choice {
    /// Actually rendered incremental message fields.
    delta: Delta,
}

/// Fields that survive Chat Completions client replay.
#[derive(Serialize, Deserialize)]
struct Delta {
    /// Actual text bytes, concatenated independently of interleaved tool chunks.
    content: Option<String>,
    /// Outward-indexed tool deltas, absent on text/role/finish chunks.
    tool_calls: Option<Vec<CallDelta>>,
}

/// Actual wire tool fields, without inferred or authoritative upstream-done replacements.
#[derive(Serialize, Deserialize)]
struct CallDelta {
    /// Dense outward ordinal assigned on first rendered sight.
    index: usize,
    /// Actual emitted ID fragment, absent on continuation chunks.
    id: Option<String>,
    /// Actual function fields, absent on signature-only chunks.
    function: Option<FunctionDelta>,
    /// Complete emitted replay signature, absent except its actual companion chunk.
    thought_signature: Option<String>,
}

/// Actual function delta bytes in their emitted order.
#[derive(Serialize, Deserialize)]
struct FunctionDelta {
    /// Emitted name fragment, absent on argument-only continuation chunks.
    name: Option<String>,
    /// Emitted JSON argument fragment.
    arguments: String,
}

/// One in-flight tool's client-side assembly; finalized calls retain only field digests.
#[derive(Default)]
struct Call {
    /// Actual emitted ID, assembled only from present wire fragments.
    id: Option<String>,
    /// Actual emitted name, assembled only from present wire fragments.
    name: Option<String>,
    /// One foreign argument object's pending bytes, released at its codec completion.
    arguments: String,
    /// Actual emitted final signature, never the upstream's unrendered state.
    signature: Option<String>,
}

impl Call {
    /// Finalizes only a complete replayable actual invocation, without altering inference.
    fn prepare(self, projector: &Projector) -> Option<PreparedCall> {
        let id = self.id?;
        let name = self.name?;
        let arguments = serde_json::from_str::<Map<String, Value>>(&self.arguments).ok()?;
        Some(projector.prepare_call(
            Identity::Wire(&id),
            &name,
            &Value::Object(arguments),
            self.signature.as_deref(),
        ))
    }
}

/// Per-outward-index state, never ordered by upstream identity or completion arrival.
enum Slot {
    /// Emitted fragments awaiting complete foreign JSON/identity.
    Open(Call),
    /// Finalized fields awaiting their actual conversation position.
    Complete(PreparedCall),
}

/// Client-codec projection: answer text is hashed as it streams; finalized calls keep digests.
pub(super) struct Output {
    /// Original accepted history plus the concatenated actual assistant text.
    projector: Projector,
    /// Outward call order, independent of upstream indexes and done-event order.
    calls: BTreeMap<usize, Slot>,
    /// False after malformed actual output or a delta following a completed call.
    is_fingerprintable: bool,
}

impl Output {
    /// Starts an accepted request's actual outward continuation.
    pub(super) fn new(projector: Projector) -> Self {
        Self {
            projector,
            calls: BTreeMap::new(),
            is_fingerprintable: true,
        }
    }

    /// Observes only actual frames after their renderer has reconciled residuals and identities.
    ///
    /// # Panics
    ///
    /// Panics if this codec emits malformed static SSE framing or malformed first-party JSON.
    pub(super) fn observe(&mut self, frames: &[String]) {
        if !self.is_fingerprintable {
            return;
        }
        for frame in frames {
            #[expect(
                clippy::expect_used,
                reason = "this codec's own frames have static data framing"
            )]
            let data = frame
                .strip_prefix("data: ")
                .and_then(|frame| frame.strip_suffix("\n\n"))
                .expect("rendered data frame has its static delimiters");
            if data == "[DONE]" {
                continue;
            }
            #[expect(
                clippy::expect_used,
                reason = "concrete emitted Chat payloads are valid JSON"
            )]
            let frame: Frame = serde_json::from_str(data).expect("rendered Chat frame parses");
            for choice in frame.choices {
                if let Some(text) = choice.delta.content {
                    self.projector.text(Role::Assistant, text.as_bytes());
                }
                for delta in choice.delta.tool_calls.into_iter().flatten() {
                    let slot = self
                        .calls
                        .entry(delta.index)
                        .or_insert_with(|| Slot::Open(Call::default()));
                    let Slot::Open(call) = slot else {
                        self.is_fingerprintable = false;
                        return;
                    };
                    if let Some(id) = delta.id {
                        call.id.get_or_insert_with(String::new).push_str(&id);
                    }
                    if let Some(function) = delta.function {
                        if let Some(name) = function.name {
                            call.name.get_or_insert_with(String::new).push_str(&name);
                        }
                        call.arguments.push_str(&function.arguments);
                    }
                    if let Some(signature) = delta.thought_signature {
                        call.signature = Some(signature);
                    }
                }
            }
        }
    }

    /// Releases completed invocation content immediately, retaining only canonical field digests.
    pub(super) fn complete_call(&mut self, index: usize) {
        if !self.is_fingerprintable {
            return;
        }
        let Some(slot) = self.calls.remove(&index) else {
            return;
        };
        let prepared = match slot {
            Slot::Open(call) => call.prepare(&self.projector),
            Slot::Complete(_) => None,
        };
        if let Some(prepared) = prepared {
            self.calls.insert(index, Slot::Complete(prepared));
        } else {
            self.is_fingerprintable = false;
        }
    }

    /// Appends calls in actual outward order after the completed concatenated assistant text.
    pub(super) fn finish(mut self) -> Option<Projector> {
        if !self.is_fingerprintable {
            return None;
        }
        for slot in self.calls.into_values() {
            let prepared = match slot {
                Slot::Complete(prepared) => prepared,
                Slot::Open(call) => call.prepare(&self.projector)?,
            };
            self.projector.call_prepared(Role::Assistant, prepared);
        }
        Some(self.projector)
    }
}
