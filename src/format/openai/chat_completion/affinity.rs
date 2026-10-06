//! Incremental projection of actual Chat Completions render points, independent of upstream IR
//! identities.

use crate::affinity::fingerprint::{Identity, PreparedCall, Projector, Role};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

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

    /// Hashes the assistant text bytes of an actual rendered content delta at its typed render
    /// point. The leading role chunk carries the empty string only, which the projector ignores.
    pub(super) fn text(&mut self, text: &str) {
        if !self.is_fingerprintable {
            return;
        }
        self.projector.text(Role::Assistant, text.as_bytes());
    }

    /// Observes the actual wire fragments of one rendered tool-call delta chunk at its outward
    /// ordinal: the identity and name fragments the chunk revealed (absent members carry
    /// nothing), the argument bytes it delivered, and the chunk's replay signature when present.
    pub(super) fn call_delta(
        &mut self,
        index: usize,
        id: Option<&str>,
        name: Option<&str>,
        arguments: &str,
        thought_signature: Option<&str>,
    ) {
        if !self.is_fingerprintable {
            return;
        }
        let slot = self
            .calls
            .entry(index)
            .or_insert_with(|| Slot::Open(Call::default()));
        let Slot::Open(call) = slot else {
            self.is_fingerprintable = false;
            return;
        };
        if let Some(id) = id {
            call.id.get_or_insert_with(String::new).push_str(id);
        }
        if let Some(name) = name {
            call.name.get_or_insert_with(String::new).push_str(name);
        }
        call.arguments.push_str(arguments);
        if let Some(signature) = thought_signature {
            call.signature = Some(signature.to_string());
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
