//! Boundary preparation (README §6.8): target-independent authentication of
//! the request's compaction boundaries and latest usable boundary selection,
//! the content-free client-visible routing projection, and the authenticated
//! native producer requirement.
//!
//! Preparation performs no account lookup, no upstream I/O, and no history
//! materialization; it runs before account selection and its proof is retained
//! through compatibility, restoration, and same-account fallback. Preparation
//! applies boundary semantics before role normalization: the latest
//! authenticated usable checkpoint supersedes the covered history —
//! conversational entries wholesale, instruction entries by proven identity
//! overlap — while an unmatched current instruction survives. A null/no-op
//! native block does not establish a boundary. A recognizable gateway carrier
//! is decoded and verified or fails the request explicitly; a damaged marker is
//! never forwarded upstream as native state and never summarized.

use crate::{
    affinity::{Digest, fingerprint::Projector},
    compaction_model::{
        envelope::{EnvelopePayload, EnvelopeV1, NativeWindowPayload},
        identity::hex_sha256,
        sealing,
    },
    format::{CompactionBoundary, NativeArtifact, ParsedCompaction},
};
use thiserror::Error;

/// A boundary-preparation failure, raised before account selection. Each
/// variant carries the same stable message code as the executor-level failure
/// it feeds — declared once as this type's code constants and consumed by the
/// executor-level variants' rendering too — and the mapping onto every outward
/// error taxonomy preserves the variant semantics: client-state failures render
/// as invalid requests, the configuration gap as an internal failure.
#[derive(Debug, Error)]
pub(crate) enum BoundaryError {
    /// No sealing keys are configured, so no gateway artifact can be produced
    /// or consumed. A server configuration gap, not a client fault.
    #[error("{}: {message}", Self::CONFIGURATION_MISSING_CODE)]
    ConfigurationMissing {
        /// Human-readable failure detail.
        message: String,
    },

    /// A gateway carrier failed verification or a boundary is structurally
    /// impossible: unknown key, malformed carrier, failed authentication, a
    /// mismatched visible-content binding, or an active boundary beyond the
    /// request's history. A recognizable but damaged marker is an explicit
    /// client-state error, never reclassified as native state and never
    /// summarized.
    #[error("{}: {message}", Self::INVALID_GATEWAY_STATE_CODE)]
    InvalidGatewayState {
        /// Human-readable failure detail.
        message: String,
    },

    /// Native state no candidate target can access: the authenticated native
    /// window's producer requirement matches no available producer. Inaccessible
    /// state is never silently discarded.
    #[error("{}: {message}", Self::INACCESSIBLE_STATE_CODE)]
    InaccessibleNativeState {
        /// Human-readable failure detail.
        message: String,
    },
}

impl BoundaryError {
    /// Stable failure code of the missing sealing configuration (README §6.8), shared with the
    /// executor-level failure vocabulary.
    pub(crate) const CONFIGURATION_MISSING_CODE: &'static str = "compaction_configuration_missing";

    /// Stable failure code of a failed gateway-carrier verification or an impossible boundary
    /// (README §6.8), shared with the executor-level failure vocabulary.
    pub(crate) const INVALID_GATEWAY_STATE_CODE: &'static str = "compaction_invalid_gateway_state";

    /// Stable failure code of native state no candidate target can access (README §6.8), shared
    /// with the executor-level failure vocabulary.
    pub(crate) const INACCESSIBLE_STATE_CODE: &'static str = "compaction_inaccessible_state";

    /// Builds the invalid-gateway-state failure.
    fn invalid_gateway_state(message: impl Into<String>) -> Self {
        Self::InvalidGatewayState {
            message: message.into(),
        }
    }

    /// Builds the inaccessible-native-state failure.
    pub(crate) fn inaccessible_native_state(message: impl Into<String>) -> Self {
        Self::InaccessibleNativeState {
            message: message.into(),
        }
    }
}

impl From<sealing::UnsealError> for BoundaryError {
    fn from(error: sealing::UnsealError) -> Self {
        match error {
            sealing::UnsealError::ConfigurationMissing => Self::ConfigurationMissing {
                message: String::from(
                    "no compaction sealing keys are configured, so the gateway \
                     carrier cannot be consumed",
                ),
            },

            // Every other unseal failure is carrier state the client presented:
            // an unknown key, a malformed carrier, or failed authentication.
            other => Self::invalid_gateway_state(other.to_string()),
        }
    }
}

/// Whether a history boundary is the native null/no-op block: it retains its
/// documented no-op meaning and does not establish a new boundary.
const fn is_null_no_op(boundary: &CompactionBoundary) -> bool {
    matches!(
        boundary,
        CompactionBoundary::Native {
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: true,
                ..
            },
            ..
        }
    )
}

/// Target-independent authentication and latest usable boundary selection for one request.
/// It performs no account lookup, upstream I/O, or history materialization.
///
/// Supersession is positional: a boundary occupies its message offset, and its
/// own state-item slot orders it within that offset — state items the
/// boundary's encounter order precedes (at an earlier offset, or at the same
/// offset with an earlier vector index) are covered and superseded by it, while
/// the items that follow it are the fresh suffix state the restored window
/// renders. The routing projection filters state by the same ordering.
#[derive(Default)]
pub(crate) struct BoundaryPreparation {
    /// Latest authenticated non-no-op boundary; superseded decoded content is discarded.
    active: Option<PreparedBoundary>,
    /// Original boundary position, distinguishing multiple artifacts at one message offset.
    active_index: Option<usize>,
}

/// Content-free client-visible routing evidence prepared before account selection.
pub(crate) struct RoutingPreparation {
    /// Accepted active history, retaining its open tail for actual outward rendering.
    pub(crate) projector: Projector,
    /// Latest authenticated carrier identity or exact canonical raw artifact identity.
    pub(crate) artifact: Option<Digest>,
}

/// One validated active boundary, retaining only what subsequent restoration needs.
#[allow(
    clippy::large_enum_variant,
    reason = "one transient boundary is held per request; boxing adds a carrier allocation"
)]
enum PreparedBoundary {
    /// Authenticated gateway checkpoint at its original conversation offset.
    Gateway {
        /// Number of accepted conversation messages before this carrier.
        offset: usize,
        /// Already authenticated envelope, never unsealed again during this request.
        envelope: EnvelopeV1,
    },
    /// Raw producer-native state whose compatibility remains target-dependent.
    Native {
        /// Original conversation offset.
        offset: usize,
        /// Parsed native placement and no-op facts.
        artifact: NativeArtifact,
    },
}

/// The authenticated active boundary, as the restoration and target-compatibility
/// consumers see it: the boundary's conversation offset beside its decoded
/// envelope or parsed native artifact.
pub(crate) enum ActiveBoundary<'a> {
    /// An authenticated gateway checkpoint at its original conversation offset.
    Gateway {
        /// Number of accepted conversation messages before this carrier.
        offset: usize,
        /// The already authenticated envelope.
        envelope: &'a EnvelopeV1,
    },

    /// Raw producer-native state whose compatibility remains target-dependent.
    Native {
        /// Original conversation offset.
        offset: usize,
        /// Parsed native placement and no-op facts.
        artifact: NativeArtifact,
    },
}

/// The state-item vector indices of the request's compaction artifacts, in
/// boundary order: the i-th compaction-typed state item is the i-th parsed
/// boundary, both pushed in encounter order by the codecs and paired by the
/// state item's carried `is_compaction` fact — never re-derived from the
/// vendor JSON.
fn compaction_state_slots(ir: &crate::ir::IrRequest) -> Vec<usize> {
    ir.state_items
        .iter()
        .enumerate()
        .filter_map(|(index, state)| state.is_compaction.then_some(index))
        .collect()
}

impl BoundaryPreparation {
    /// The active boundary's own state-item slot: the vector index of the
    /// state item the boundary was parsed from, ordering it among the state
    /// items that share its message offset. Restoration supersedes
    /// same-offset state by this slot, matching the routing projection's
    /// vector filter.
    ///
    /// # Panics
    ///
    /// Panics if the active boundary has no recorded state slot — the parser
    /// records each compaction state item its boundary was parsed from.
    pub(crate) fn active_state_slot(&self, ir: &crate::ir::IrRequest) -> usize {
        let slots = compaction_state_slots(ir);

        #[expect(
            clippy::expect_used,
            reason = "restoration runs only against an active boundary, which authenticate \
                      always pairs with its parsed boundary"
        )]
        let boundary_index = self
            .active_index
            .expect("the active boundary has a parsed boundary");

        #[expect(
            clippy::expect_used,
            reason = "the parser records each compaction state slot"
        )]
        *slots
            .get(boundary_index)
            .expect("the active boundary has a state slot")
    }

    /// The latest authenticated usable boundary, when one stands.
    pub(crate) fn active(&self) -> Option<ActiveBoundary<'_>> {
        Some(match self.active.as_ref()? {
            PreparedBoundary::Gateway { offset, envelope } => ActiveBoundary::Gateway {
                offset: *offset,
                envelope,
            },
            PreparedBoundary::Native { offset, artifact } => ActiveBoundary::Native {
                offset: *offset,
                artifact: *artifact,
            },
        })
    }

    /// Projects the validated client-visible active replay window without restoring hidden text.
    /// The latest carrier supersedes covered messages/state; no-op blocks establish no history
    /// unit or artifact hint. Original source records remain aligned with uncovered tool parts.
    ///
    /// # Errors
    ///
    /// Returns invalid gateway state if an active boundary sits beyond the accepted history.
    ///
    /// # Panics
    ///
    /// Panics if parser state slots disagree with their recorded boundary facts.
    pub(crate) fn routing_preparation(
        &self,
        parsed: &crate::format::ParsedRequest,
    ) -> Result<RoutingPreparation, BoundaryError> {
        let mut projector = Projector::new();
        let compaction = parsed.compaction.as_ref();
        let slots = compaction_state_slots(&parsed.ir);
        let first_message = match self.active.as_ref() {
            Some(
                PreparedBoundary::Gateway { offset, .. } | PreparedBoundary::Native { offset, .. },
            ) => *offset,
            None => 0,
        };
        if first_message > parsed.ir.messages.len() {
            return Err(BoundaryError::invalid_gateway_state(
                "the active checkpoint's boundary sits beyond the request's history",
            ));
        }
        let active_slot = self.active_index.map(|index| {
            #[expect(
                clippy::expect_used,
                reason = "the parser records each compaction state slot"
            )]
            *slots
                .get(index)
                .expect("the active boundary has a state slot")
        });
        let artifact = match self.active.as_ref() {
            Some(PreparedBoundary::Gateway { envelope, .. }) => {
                #[expect(
                    clippy::expect_used,
                    reason = "an active gateway boundary was authenticated from this request"
                )]
                let boundary = compaction
                    .and_then(|parsed| {
                        self.active_index
                            .and_then(|index| parsed.boundaries.get(index))
                    })
                    .expect("the authenticated gateway boundary exists");
                #[expect(
                    clippy::unreachable,
                    reason = "the retained preparation matches its original boundary"
                )]
                let CompactionBoundary::Gateway { carrier, .. } = boundary else {
                    unreachable!("the active gateway preparation has a gateway carrier");
                };
                #[expect(
                    clippy::expect_used,
                    reason = "only Responses and Messages parse gateway carriers"
                )]
                let family =
                    crate::format::affinity::replay_family(parsed.affinity.client_format())
                        .expect("gateway carriers belong to a state-bearing client codec");

                projector.gateway_state(family, carrier);
                Some(projector.gateway_artifact(&envelope.artifact_id))
            }
            Some(PreparedBoundary::Native { .. }) => {
                #[expect(
                    clippy::expect_used,
                    reason = "the active native boundary has a recorded state slot"
                )]
                let state = active_slot
                    .and_then(|index| parsed.ir.state_items.get(index))
                    .expect("native state exists");
                Some(projector.native_artifact(&state.item))
            }
            None => None,
        };
        let states = parsed
            .ir
            .state_items
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                if active_slot.is_some_and(|active| index < active) || state.is_gateway {
                    return None;
                }
                if let Ok(boundary_index) = slots.binary_search(&index)
                    && compaction
                        .and_then(|parsed| parsed.boundaries.get(boundary_index))
                        .is_some_and(is_null_no_op)
                {
                    return None;
                }
                Some(state)
            });
        parsed
            .affinity
            .append_window(&parsed.ir.messages, states, first_message, &mut projector);
        Ok(RoutingPreparation {
            projector,
            artifact,
        })
    }

    /// Returns the authenticated producer requirement of the latest active native window.
    /// Portable summaries and unverified raw native artifacts impose no authenticated account.
    pub(crate) fn native_requirement(&self) -> Option<&NativeWindowPayload> {
        match self.active.as_ref()? {
            PreparedBoundary::Gateway { envelope, .. } => match &envelope.payload {
                EnvelopePayload::NativeWindow(payload) => Some(payload),
                EnvelopePayload::GatewaySummary(_) => None,
            },
            PreparedBoundary::Native { .. } => None,
        }
    }

    /// Authenticates every recognizable carrier in history order before choosing a target.
    /// A later usable checkpoint supersedes older state; native null blocks create no boundary.
    /// Messages visible content must exactly match its authenticated envelope binding.
    ///
    /// # Errors
    ///
    /// Returns a carrier authentication, unknown-key, or visible-content binding failure even
    /// when a later checkpoint would supersede the damaged carrier.
    pub(crate) fn authenticate(parsed: &ParsedCompaction) -> Result<Self, BoundaryError> {
        let mut preparation = Self::default();
        for (index, boundary) in parsed.boundaries.iter().enumerate() {
            match boundary {
                CompactionBoundary::Gateway {
                    offset,
                    carrier,
                    visible_content,
                } => {
                    let binding = visible_content
                        .as_deref()
                        .map(|content| hex_sha256(content.as_bytes()));
                    let envelope = sealing::unseal_with_live_config(
                        carrier,
                        binding.as_deref().map(str::as_bytes),
                    )?;
                    match (&envelope.visible_content_digest, &binding) {
                        (Some(envelope_digest), Some(carrier_digest))
                            if envelope_digest == carrier_digest => {}
                        (None, None) => {}
                        _ => {
                            return Err(BoundaryError::invalid_gateway_state(
                                "the gateway carrier's visible content does not match its binding",
                            ));
                        }
                    }
                    preparation.active_index = Some(index);
                    preparation.active = Some(PreparedBoundary::Gateway {
                        offset: *offset,
                        envelope,
                    });
                }
                CompactionBoundary::Native { offset, artifact } if !is_null_no_op(boundary) => {
                    preparation.active_index = Some(index);
                    preparation.active = Some(PreparedBoundary::Native {
                        offset: *offset,
                        artifact: *artifact,
                    });
                }
                CompactionBoundary::Native { .. } => {}
            }
        }
        Ok(preparation)
    }
}
