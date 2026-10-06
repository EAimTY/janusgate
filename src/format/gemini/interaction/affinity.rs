//! Incremental actual Interactions render-point projection, ordered by outward step indexes.

use crate::affinity::fingerprint::{
    Identity, PreparedCall, PreparedReasoning, PreparedText, Projector, Role,
};
use std::collections::BTreeMap;

/// Pending invocation fields, released when its outward step stops.
struct Call {
    /// Actual wire ID from the starting frame, or the done-stamped ID when the opening
    /// carried none.
    id: String,
    /// Actual emitted function name, or the done-stamped name when the opening carried none.
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

    /// Opens an output step at its actual outward position.
    pub(super) fn start_output(&mut self, index: u64) {
        self.start_text(index);
    }

    /// Opens a reasoning block retaining its closing signature and streamed summary bytes.
    pub(super) fn start_thought(&mut self, index: u64) {
        self.slots.insert(
            index,
            Slot::Thought {
                signature: None,
                text: None,
            },
        );
    }

    /// Opens an invocation with its actual emitted identity, either member possibly still empty.
    pub(super) fn start_call(&mut self, index: u64, id: &str, name: &str) {
        self.slots.insert(
            index,
            Slot::Call(Call {
                id: id.to_string(),
                name: name.to_string(),
                arguments: String::new(),
            }),
        );
    }

    /// Extends an open output step's actual text bytes.
    ///
    /// # Panics
    ///
    /// Panics if the renderer emits a text delta inconsistent with its preceding opening step.
    pub(super) fn text_delta(&mut self, index: u64, text: &str) {
        #[expect(
            clippy::expect_used,
            reason = "own text deltas follow their opening step"
        )]
        match self.slots.get_mut(&index).expect("opened output step") {
            Slot::Text(field) => field.update(text.as_bytes()),
            #[expect(clippy::unreachable, reason = "own text deltas belong to output steps")]
            _ => unreachable!("text delta follows an output step"),
        }
    }

    /// Extends an open invocation's actual argument bytes.
    ///
    /// # Panics
    ///
    /// Panics if the renderer emits an argument delta inconsistent with its opening step.
    pub(super) fn arguments_delta(&mut self, index: u64, arguments: &str) {
        #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
        match self.slots.get_mut(&index).expect("opened invocation step") {
            Slot::Call(call) => call.arguments.push_str(arguments),
            #[expect(clippy::unreachable, reason = "own argument deltas belong to calls")]
            _ => unreachable!("argument delta follows an invocation step"),
        }
    }

    /// Stamps an open thought's actual closing signature.
    ///
    /// # Panics
    ///
    /// Panics if the renderer emits a signature delta inconsistent with its opening step.
    pub(super) fn thought_signature_delta(&mut self, index: u64, signature: &str) {
        #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
        match self.slots.get_mut(&index).expect("opened thought step") {
            Slot::Thought {
                signature: field, ..
            } => *field = Some(signature.to_string()),
            Slot::Unreplayable => {}
            #[expect(
                clippy::unreachable,
                reason = "own signature deltas belong to thoughts"
            )]
            _ => unreachable!("signature delta follows a thought step"),
        }
    }

    /// Extends an open thought's streamed summary text.
    ///
    /// # Panics
    ///
    /// Panics if the renderer emits a summary delta inconsistent with its opening step.
    pub(super) fn thought_summary_delta(&mut self, index: u64, text: &str) {
        #[expect(clippy::expect_used, reason = "own deltas follow their opening step")]
        match self.slots.get_mut(&index).expect("opened thought step") {
            Slot::Thought { text: field, .. } => {
                field
                    .get_or_insert_with(|| self.projector.prepare_text(Role::Assistant, false))
                    .update(text.as_bytes());
            }
            #[expect(clippy::unreachable, reason = "own summary deltas belong to thoughts")]
            _ => unreachable!("summary delta follows a thought step"),
        }
    }

    /// Closes the step at its outward position: an invocation releases its raw fields into final
    /// digests, and a thought releases its emitted summary bytes and signature.
    ///
    /// The closing done event's identity is authoritative where the streamed opening left the
    /// member empty: a call whose opening carried no ID or name — a supported upstream shape
    /// whose deltas stream arguments before identity — completes under the done-stamped identity
    /// the JSON projection of the same response registers, so both transports of one response
    /// fingerprint one history. A non-empty emitted identity stands, so upstream-only done
    /// changes stay excluded.
    pub(super) fn stop(&mut self, index: u64, done: Option<(&str, &str)>) {
        self.complete_call(index, done);
        self.complete_thought(index);
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
    /// The done-stamped identity applies only where the streamed opening left the member empty.
    fn complete_call(&mut self, index: u64, done: Option<(&str, &str)>) {
        let Some(Slot::Call(call)) = self.slots.get_mut(&index) else {
            return;
        };

        if let Some((call_id, name)) = done {
            if call.id.is_empty() {
                call.id = call_id.to_string();
            }

            if call.name.is_empty() {
                call.name = name.to_string();
            }
        }

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
