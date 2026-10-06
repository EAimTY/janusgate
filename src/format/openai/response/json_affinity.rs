//! Aggregated-body projection of the actual Responses output fields.

use super::client_render::{OutputItem, OutputTextPart, RenderedOutputItem, ResponseObject};
use crate::affinity::fingerprint::Projector;

/// Projects actual ordered output items, excluding unencrypted reasoning dropped on replay.
pub(super) fn project_json_response(response: &ResponseObject, projector: &mut Projector) -> bool {
    project_json_after_prefix(response, projector, 0)
}

/// Projects actual answer fields after the already normalized prepared checkpoint prefix.
pub(super) fn project_json_after_prefix(
    response: &ResponseObject,
    projector: &mut Projector,
    prefix_count: usize,
) -> bool {
    for item in response.output.iter().skip(prefix_count) {
        match item {
            OutputItem::Rendered(RenderedOutputItem::Message { content, .. }) => {
                for OutputTextPart::OutputText { text, .. } in content {
                    projector.text(
                        crate::affinity::fingerprint::Role::Assistant,
                        text.as_bytes(),
                    );
                }
            }
            OutputItem::Rendered(RenderedOutputItem::FunctionCall(call)) => {
                if !crate::format::affinity::project_call(
                    projector,
                    &call.call_id,
                    &call.name,
                    &call.arguments,
                    call.thought_signature.as_deref(),
                ) {
                    return false;
                }
            }
            OutputItem::Rendered(RenderedOutputItem::Reasoning { .. }) => {}
            OutputItem::State(crate::format::compaction_wire::Checkpoint::Native(value)) => {
                let Some(state) = super::window_affinity::prepare_native_state(value, projector)
                else {
                    return false;
                };
                if super::compaction::native_artifact_anchor(value, projector).is_some() {
                    *projector = crate::affinity::fingerprint::Projector::new();
                }
                projector.state_prepared(state);
            }
            OutputItem::State(_) => return false,
        }
    }
    true
}

/// Identifies actual complete raw native checkpoints without treating reflected markers as ours.
pub(super) fn completed_json_artifacts(response: &ResponseObject) -> Vec<crate::affinity::Anchor> {
    let projector = crate::affinity::fingerprint::Projector::new();
    response
        .output
        .iter()
        .flat_map(|item| match item {
            OutputItem::State(crate::format::compaction_wire::Checkpoint::Native(value)) => {
                super::window_affinity::prepare_native_checkpoint(value, &projector)
                    .map_or_else(Vec::new, |(_, anchors)| anchors)
            }
            OutputItem::Rendered(_) | OutputItem::State(_) => Vec::new(),
        })
        .collect()
}
