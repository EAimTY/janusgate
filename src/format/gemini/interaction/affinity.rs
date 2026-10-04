//! Incremental actual Interactions SSE projection, ordered by outward step indexes.

use crate::affinity::fingerprint::{
    Identity, PreparedCall, PreparedReasoning, PreparedText, Projector, Role,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Conversation-bearing fields of the renderer's own concrete SSE frames.
#[derive(Serialize, Deserialize)]
#[serde(tag = "event_type")]
enum Frame {
    /// Opens a step at its actual outward position.
    #[serde(rename = "step.start")]
    Start {
        /// Actual outward position, independent of IR call indexes.
        index: u64,
        /// Actual emitted opening fields.
        step: Step,
    },
    /// Extends an existing outward step.
    #[serde(rename = "step.delta")]
    Delta {
        /// Outward position being extended.
        index: u64,
        /// Actual emitted field bytes.
        delta: Delta,
    },
    /// Completes the step's emitted fields.
    #[serde(rename = "step.stop")]
    Stop {
        /// Outward position being finalized.
        index: u64,
    },
    /// Content-free creation, usage, or terminal resource metadata.
    #[serde(other)]
    Other,
}

/// Starting steps supported by ordinary conversation projection.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Step {
    /// A plain assistant text output, initially empty.
    ModelOutput,
    /// Native reasoning, initially without summary or signature fields.
    Thought,
    /// A function invocation with its actual emitted identity.
    FunctionCall {
        /// Actual starting wire ID; later upstream-only changes are excluded.
        id: String,
        /// Actual starting function name.
        name: String,
    },
    /// Native thought projection awaits canonical opaque assembly.
    #[serde(other)]
    Other,
}

/// Actual streamed field extensions.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    /// Plain assistant UTF-8 bytes.
    Text {
        /// Actual emitted text chunk.
        text: String,
    },
    /// Foreign invocation argument fragments.
    #[serde(rename = "arguments_delta")]
    Arguments {
        /// Actual emitted JSON bytes, including only proven residuals.
        arguments: String,
    },
    /// Actual native signature of an otherwise empty thought.
    ThoughtSignature {
        /// Actual emitted signature bytes.
        signature: String,
    },
    /// Text extending the thought's single streamed summary block.
    ThoughtSummary {
        /// Actual emitted summary content.
        content: Summary,
    },
    /// Native thought fields awaiting their canonical projection.
    #[serde(other)]
    Other,
}

/// Concrete summary content emitted by this renderer.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Summary {
    /// Actual UTF-8 summary bytes.
    Text {
        /// Emitted chunk, never retained after hashing.
        text: String,
    },
}

/// Pending invocation fields, released when its outward step stops.
struct Call {
    /// Actual wire ID from the starting frame.
    id: String,
    /// Actual emitted function name.
    name: String,
    /// Temporary argument JSON assembly, never completed cached content.
    arguments: String,
}

/// One output position retaining digest metadata or an unfinished invocation.
enum Slot {
    /// Incremental text state seeded with the adjacent canonical predecessor.
    Text(PreparedText),
    /// An invocation awaiting its final emitted argument bytes.
    Call(Call),
    /// Completed invocation fields with their content discarded.
    CompletedCall(PreparedCall),
    /// A thought retaining only its emitted field digests and closing signature metadata.
    Thought {
        /// Actual signature, released into a digest when the step stops.
        signature: Option<String>,
        /// Incremental actual summary bytes; absence differs from an emitted empty summary.
        text: Option<PreparedText>,
    },
    /// Completed native reasoning with its variable fields discarded.
    CompletedThought(PreparedReasoning),
    /// Native reasoning or malformed foreign arguments whose projection is unavailable.
    Unreplayable,
}

/// Ordered replay projection beside the normal client renderer.
pub(super) struct Output {
    /// Accepted input tail and request-local relationship bookkeeping.
    projector: Projector,
    /// Actual first-seen step positions, independent of completion order.
    slots: BTreeMap<u64, Slot>,
}

impl Output {
    /// Starts a digest-only output adapter without copying generated text.
    pub(super) fn new(projector: Projector) -> Self {
        Self {
            projector,
            slots: BTreeMap::new(),
        }
    }

    /// Starts a text field with exactly the state preceding its actual outward position.
    fn start_text(&mut self, index: u64) {
        let text = match self.slots.last_key_value() {
            Some((_, Slot::Text(text))) => text.clone(),
            Some(_) => self.projector.prepare_text(Role::Assistant, false),
            None => self.projector.prepare_text(Role::Assistant, true),
        };
        self.slots.insert(index, Slot::Text(text));
    }

    /// Observes actual emitted frames, excluding invisible changes to upstream done metadata.
    ///
    /// # Panics
    ///
    /// Panics if the own renderer emits malformed frames or a delta inconsistent with its
    /// preceding opening step.
    pub(super) fn observe(&mut self, _event: &crate::ir::IrEvent, frames: &[String]) {
        for frame in frames {
            #[expect(
                clippy::expect_used,
                reason = "every own SSE frame carries a data line"
            )]
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .expect("rendered Interactions frame has data");
            #[expect(
                clippy::expect_used,
                reason = "own concrete SSE payloads serialize valid JSON"
            )]
            let frame: Frame = serde_json::from_str(data).expect("rendered Interactions JSON");
            match frame {
                Frame::Start {
                    index,
                    step: Step::ModelOutput,
                } => self.start_text(index),
                Frame::Start {
                    index,
                    step: Step::Thought,
                } => {
                    self.slots.insert(
                        index,
                        Slot::Thought {
                            signature: None,
                            text: None,
                        },
                    );
                }
                Frame::Start {
                    index,
                    step: Step::FunctionCall { id, name },
                } => {
                    self.slots.insert(
                        index,
                        Slot::Call(Call {
                            id,
                            name,
                            arguments: String::new(),
                        }),
                    );
                }
                Frame::Delta {
                    index,
                    delta: Delta::Text { text },
                } =>
                {
                    #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
                    match self.slots.get_mut(&index).expect("opened output step") {
                        Slot::Text(field) => field.update(text.as_bytes()),
                        #[expect(
                            clippy::unreachable,
                            reason = "own text deltas belong to output steps"
                        )]
                        _ => unreachable!("text delta follows an output step"),
                    }
                }
                Frame::Delta {
                    index,
                    delta: Delta::Arguments { arguments },
                } =>
                {
                    #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
                    match self.slots.get_mut(&index).expect("opened invocation step") {
                        Slot::Call(call) => call.arguments.push_str(&arguments),
                        #[expect(
                            clippy::unreachable,
                            reason = "own argument deltas belong to calls"
                        )]
                        _ => unreachable!("argument delta follows an invocation step"),
                    }
                }
                Frame::Delta {
                    index,
                    delta: Delta::ThoughtSignature { signature },
                } =>
                {
                    #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
                    match self.slots.get_mut(&index).expect("opened thought step") {
                        Slot::Thought {
                            signature: field, ..
                        } => *field = Some(signature),
                        Slot::Unreplayable => {}
                        #[expect(
                            clippy::unreachable,
                            reason = "own signature deltas belong to thought steps"
                        )]
                        _ => unreachable!("signature delta follows a thought step"),
                    }
                }
                Frame::Delta {
                    index,
                    delta:
                        Delta::ThoughtSummary {
                            content: Summary::Text { text },
                        },
                } =>
                {
                    #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
                    match self.slots.get_mut(&index).expect("opened thought step") {
                        Slot::Thought { text: field, .. } => {
                            field
                                .get_or_insert_with(|| {
                                    self.projector.prepare_text(Role::Assistant, false)
                                })
                                .update(text.as_bytes());
                        }
                        #[expect(
                            clippy::unreachable,
                            reason = "own summary deltas belong to thought steps"
                        )]
                        _ => unreachable!("summary delta follows a thought step"),
                    }
                }
                Frame::Stop { index } => {
                    self.complete_call(index);
                    self.complete_thought(index);
                }
                Frame::Start {
                    index,
                    step: Step::Other,
                }
                | Frame::Delta {
                    index,
                    delta: Delta::Other,
                } => {
                    self.slots.insert(index, Slot::Unreplayable);
                }
                Frame::Other => {}
            }
        }
    }

    /// Completes actual emitted text and signature independently of upstream done carriers.
    fn complete_thought(&mut self, index: u64) {
        let Some(Slot::Thought { signature, text }) = self.slots.get_mut(&index) else {
            return;
        };
        let thought = match text {
            None => super::json_affinity::prepare_signature_thought(
                &self.projector,
                signature.as_deref(),
            ),
            Some(field) => super::json_affinity::prepare_stream_thought(
                &self.projector,
                field,
                signature.as_deref(),
            ),
        };
        self.slots.insert(index, Slot::CompletedThought(thought));
    }

    /// Releases raw invocation fields into final digests, preserving only replayable arguments.
    fn complete_call(&mut self, index: u64) {
        let Some(Slot::Call(call)) = self.slots.get_mut(&index) else {
            return;
        };
        let Ok(arguments) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&call.arguments)
        else {
            self.slots.insert(index, Slot::Unreplayable);
            return;
        };
        let call = self.projector.prepare_call(
            Identity::Wire(&call.id),
            &call.name,
            &serde_json::Value::Object(arguments),
            None,
        );
        self.slots.insert(index, Slot::CompletedCall(call));
    }

    /// Finalizes only a fully replayable actual output, preserving first-seen step order.
    pub(super) fn finish(mut self) -> Option<Projector> {
        for slot in self.slots.into_values() {
            match slot {
                Slot::Text(text) => self.projector.text_prepared(Role::Assistant, text),
                Slot::CompletedCall(call) => self.projector.call_prepared(Role::Assistant, call),
                Slot::CompletedThought(thought) => {
                    self.projector.reasoning_prepared(Role::Assistant, thought)
                }
                Slot::Call(_) | Slot::Thought { .. } | Slot::Unreplayable => return None,
            }
        }
        Some(self.projector)
    }
}
