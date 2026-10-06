//! Streaming projection of the actual emitted GenerateContent parts, shared with the aggregated
//! body's replay of the same parts.

use super::client_render::PartOut;
use crate::affinity::fingerprint::Projector;

/// Projects one actual emitted part with the same identity semantics as GenerateContent replay.
/// Empty emitted IDs are id-less, and their fresh resolver IDs never enter canonical units.
///
/// # Panics
///
/// Panics if a gateway-rendered invocation lacks its mandatory emitted ID field.
pub(super) fn project_part(part: &PartOut, projector: &mut Projector) {
    use crate::affinity::fingerprint::{Identity, Role};
    match part {
        // The accepted thought flag does not change this parser's plain-text replay semantics.
        PartOut::Text { text, .. } => projector.text(Role::Assistant, text.as_bytes()),
        PartOut::FunctionCall {
            function_call,
            thought_signature,
        } => {
            #[expect(
                clippy::expect_used,
                reason = "the gateway emits every invocation's ID field"
            )]
            let id = function_call
                .id
                .as_deref()
                .expect("emitted invocation has an ID field");
            let synthesized;
            let identity = if id.is_empty() {
                synthesized = crate::ids::new_uuid();
                Identity::Synthetic(&synthesized)
            } else {
                Identity::Wire(id)
            };
            projector.call(
                Role::Assistant,
                identity,
                &function_call.name,
                &serde_json::Value::Object(function_call.args.clone()),
                thought_signature.as_deref(),
            );
        }
    }
}
