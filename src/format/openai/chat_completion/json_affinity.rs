//! Aggregated-body projection of the actual Chat Completions response fields.

use super::client_render::ChatCompletionResponse;
use crate::affinity::fingerprint::Projector;

/// Projects the actual assistant message; reasoning_content is discarded by Chat replay.
pub(super) fn project_json_response(
    response: &ChatCompletionResponse,
    projector: &mut Projector,
) -> bool {
    for choice in &response.choices {
        if let Some(text) = &choice.message.content {
            projector.text(
                crate::affinity::fingerprint::Role::Assistant,
                text.as_bytes(),
            );
        }
        for call in &choice.message.tool_calls {
            if !crate::format::affinity::project_call(
                projector,
                &call.id,
                &call.function.name,
                &call.function.arguments,
                call.thought_signature.as_deref(),
            ) {
                return false;
            }
        }
    }
    true
}
