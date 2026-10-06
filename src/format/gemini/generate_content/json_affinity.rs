//! Aggregated-body projection of the actual GenerateContent response fields.

use super::{affinity::project_part, client_render::JsonResponse};
use crate::affinity::fingerprint::Projector;

/// Projects actual candidate parts, following accepted thought replay and the emitted invocation
/// IDs.
pub(super) fn project_json_response(response: &JsonResponse, projector: &mut Projector) -> bool {
    for candidate in &response.candidates {
        for part in &candidate.content.parts {
            project_part(part, projector);
        }
    }
    true
}
