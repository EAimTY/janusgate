//! Incremental digest-only Responses output ordered by actual outward item position.

use super::client_render::FunctionCallItem;
use crate::affinity::fingerprint::{Identity, PreparedCall, PreparedText, Projector, Role};
use std::collections::BTreeMap;

/// Replayable output fields beside the accepted input's open canonical tail.
pub(super) struct Output {
    /// Accepted input and request-local relationship metadata.
    projector: Projector,
    /// Output positions assigned by the ordinary renderer, excluding ignored reasoning.
    slots: BTreeMap<usize, Slot>,
    /// Latest actual completed checkpoint position; earlier late done fields remain covered.
    checkpoint_index: Option<usize>,
    /// Whether every completed item has ordinary replay semantics.
    is_fingerprintable: bool,
}

/// One content-free item awaiting final output ordering.
enum Slot {
    /// Incrementally hashed actual text, seeded with an adjacent preceding field.
    Text(PreparedText),
    /// Actual completed call fields; `None` reserves an unfinished outward position.
    Call(Option<PreparedCall>),
    /// Completed native state, normalized without retaining its payload.
    State(crate::affinity::fingerprint::PreparedState),
}

impl Output {
    /// Starts alongside a renderer without copying generated text or argument buffers.
    pub(super) fn new(projector: Projector) -> Self {
        Self {
            projector,
            slots: BTreeMap::new(),
            checkpoint_index: None,
            is_fingerprintable: true,
        }
    }

    /// Hashes bytes from an actual rendered text delta at its outward message position.
    pub(super) fn text(&mut self, index: usize, bytes: &[u8]) {
        if self.is_covered(index) {
            return;
        }
        if !self.slots.contains_key(&index) {
            let text = match self.slots.last_key_value() {
                Some((_, Slot::Text(text))) => text.clone(),
                Some((_, Slot::Call(_) | Slot::State(_))) => {
                    self.projector.prepare_text(Role::Assistant, false)
                }
                None => self.projector.prepare_text(Role::Assistant, true),
            };
            self.slots.insert(index, Slot::Text(text));
        }
        if let Some(Slot::Text(text)) = self.slots.get_mut(&index) {
            text.update(bytes);
        }
    }

    /// Reserves the renderer's first-seen call position before its arguments are complete.
    pub(super) fn reserve_call(&mut self, index: usize) {
        if self.is_covered(index) {
            return;
        }
        self.slots.entry(index).or_insert(Slot::Call(None));
    }

    /// Digests the actual done item's authoritative fields, ignoring provisional deltas.
    pub(super) fn complete_call(&mut self, index: usize, call: &FunctionCallItem) {
        if self.is_covered(index) {
            return;
        }
        let Ok(arguments) = crate::format::arguments_as_object(&call.arguments) else {
            self.is_fingerprintable = false;
            return;
        };
        let call = self.projector.prepare_call(
            Identity::Wire(&call.call_id),
            &call.name,
            &serde_json::Value::Object(arguments),
            call.thought_signature.as_deref(),
        );
        self.slots.insert(index, Slot::Call(Some(call)));
    }

    /// Defers state-bearing output to the authenticated state planner.
    pub(super) fn observe_state(&mut self) {
        self.is_fingerprintable = false;
    }

    /// Whether an actual output position lies before the latest completed checkpoint.
    fn is_covered(&self, index: usize) -> bool {
        self.checkpoint_index
            .is_some_and(|checkpoint| index < checkpoint)
    }

    /// Completes actual state fields; a native checkpoint supersedes the covered input/output
    /// and returns digest-only anchors for its own outward done-frame boundary.
    pub(super) fn complete_state(
        &mut self,
        index: usize,
        value: &serde_json::Value,
    ) -> Option<Vec<crate::affinity::Anchor>> {
        if self.is_covered(index) {
            return None;
        }
        if let Some((state, anchors)) =
            super::window_affinity::prepare_native_checkpoint(value, &self.projector)
        {
            self.projector = Projector::new();
            self.slots.clear();
            self.checkpoint_index = Some(index);
            self.is_fingerprintable = true;
            self.slots.insert(index, Slot::State(state));
            Some(anchors)
        } else {
            if let Some(state) =
                super::window_affinity::prepare_native_state(value, &self.projector)
            {
                self.slots.insert(index, Slot::State(state));
            } else {
                self.is_fingerprintable = false;
            }
            None
        }
    }

    /// Finalizes ordered fields only when every outward invocation is complete and replayable.
    pub(super) fn finish(mut self) -> Option<Projector> {
        if !self.is_fingerprintable {
            return None;
        }
        for slot in self.slots.into_values() {
            match slot {
                Slot::Text(text) => self.projector.text_prepared(Role::Assistant, text),
                Slot::Call(Some(call)) => self.projector.call_prepared(Role::Assistant, call),
                Slot::Call(None) => return None,
                Slot::State(state) => self.projector.state_prepared(state),
            }
        }
        Some(self.projector)
    }
}
