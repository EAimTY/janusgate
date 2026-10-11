//! Boundary preparation (`docs/design/compaction.md`): target-independent authentication of
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
    /// Stable failure code of the missing sealing configuration (`docs/design/compaction.md`),
    /// shared with the executor-level failure vocabulary.
    pub(crate) const CONFIGURATION_MISSING_CODE: &'static str = "compaction_configuration_missing";

    /// Stable failure code of a failed gateway-carrier verification or an impossible boundary
    /// (`docs/design/compaction.md`), shared with the executor-level failure vocabulary.
    pub(crate) const INVALID_GATEWAY_STATE_CODE: &'static str = "compaction_invalid_gateway_state";

    /// Stable failure code of native state no candidate target can access
    /// (`docs/design/compaction.md`), shared with the executor-level failure vocabulary.
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::{ActiveBoundary, BoundaryError, BoundaryPreparation};
    use crate::{
        compaction_model::{
            envelope::{
                EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, NativeReplayWindow,
                NativeWindowPayload, ReplacementMode,
            },
            identity::hex_sha256,
            mechanism::Mechanism,
            sealing::{self, UnsealError},
        },
        format::{
            BoundaryPlacement, CompactionBoundary, NativeArtifact, ParsedCompaction, UpstreamFormat,
        },
        ir::{IrRequest, StateFamily, StateItem},
        test_support,
    };
    use std::error::Error as StdError;

    /// Seals the fixture summary envelope under the installed shared test configuration into a
    /// gateway carrier, optionally binding a visible-content digest.
    fn sealed_carrier(visible_content: Option<&str>) -> Result<String, UnsealError> {
        let binding = visible_content.map(|content| hex_sha256(content.as_bytes()));

        let envelope = EnvelopeV1::new(
            String::from("cmp_boundary_fixture"),
            ReplacementMode::CanonicalWindow,
            EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
                summary_text: String::from("Boundary fixture summary."),
                preserved_instructions: Vec::new(),
                retained_units: Vec::new(),
                retained_state_items: Vec::new(),
                overlap_identities: Vec::new(),
            }),
            binding.clone(),
        );

        sealing::seal_with_live_config(
            &envelope,
            binding.as_deref().map(|digest| digest.as_bytes()),
        )
    }

    /// The gateway boundary of one carrier at one message offset.
    fn gateway_boundary(offset: usize, carrier: String) -> CompactionBoundary {
        CompactionBoundary::Gateway {
            offset,
            carrier,
            visible_content: None,
        }
    }

    /// The native non-null Messages compaction block boundary at one message offset.
    fn native_boundary(offset: usize) -> CompactionBoundary {
        CompactionBoundary::Native {
            offset,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: false,
                placement: BoundaryPlacement::Historical,
            },
        }
    }

    /// Authenticates a boundary list under the installed shared test configuration.
    fn prepare(boundaries: Vec<CompactionBoundary>) -> Result<BoundaryPreparation, BoundaryError> {
        let parsed = ParsedCompaction {
            control: None,
            boundaries,
            instruction_items: Vec::new(),
        };

        BoundaryPreparation::authenticate(&parsed)
    }

    /// A history without usable boundaries leaves no active boundary: an empty list, and a native
    /// null/no-op block both establish nothing.
    #[test]
    fn no_usable_boundary_stands_without_one() -> Result<(), Box<dyn StdError>> {
        let _lock = test_support::configuration_lock();
        test_support::install_configuration();

        assert!(prepare(Vec::new())?.active().is_none());

        let no_op = CompactionBoundary::Native {
            offset: 1,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: true,
                placement: crate::format::BoundaryPlacement::Leading,
            },
        };

        assert!(
            prepare(vec![no_op])?.active().is_none(),
            "a null/no-op native block retains its no-op meaning and establishes no boundary"
        );

        Ok(())
    }

    /// The latest usable boundary wins: a later gateway carrier supersedes an earlier one, and a
    /// later native artifact supersedes an authenticated carrier.
    #[test]
    fn the_latest_usable_boundary_wins() -> Result<(), Box<dyn StdError>> {
        let _lock = test_support::configuration_lock();
        test_support::install_configuration();

        let first = sealed_carrier(None)?;
        let second = sealed_carrier(None)?;

        let preparation = prepare(vec![
            gateway_boundary(0, first),
            gateway_boundary(2, second),
        ])?;

        let Some(ActiveBoundary::Gateway { offset, envelope }) = preparation.active() else {
            return Err("the later carrier is the active gateway boundary".into());
        };

        assert_eq!(offset, 2);
        assert_eq!(envelope.artifact_id, "cmp_boundary_fixture");

        let preparation = prepare(vec![
            gateway_boundary(0, sealed_carrier(None)?),
            native_boundary(3),
        ])?;

        assert!(
            matches!(
                preparation.active(),
                Some(ActiveBoundary::Native { offset: 3, .. })
            ),
            "a later native artifact supersedes an authenticated carrier"
        );

        Ok(())
    }

    /// A recognizable but damaged marker fails the request explicitly — even when a later valid
    /// carrier would supersede it — and an unknown key id fails the same way: damaged state is
    /// never reclassified as native data and never skipped.
    #[test]
    fn damaged_markers_fail_even_when_superseded() -> Result<(), Box<dyn StdError>> {
        let _lock = test_support::configuration_lock();
        test_support::install_configuration();

        let damaged = format!("{}damaged.!!!not-base64!!!", sealing::CARRIER_NAMESPACE);

        let outcome = prepare(vec![
            gateway_boundary(0, damaged),
            gateway_boundary(2, sealed_carrier(None)?),
        ]);

        assert!(
            matches!(outcome, Err(BoundaryError::InvalidGatewayState { .. })),
            "a damaged marker before a valid carrier still fails the request"
        );

        let unknown_key = format!(
            "{}rotated-away.{}",
            sealing::CARRIER_NAMESPACE,
            "cGF5bG9hZC1ieXRlcy1iZXlvbmQtdGhlLW5vbmNlLWxlbmd0aA"
        );

        let outcome = prepare(vec![gateway_boundary(0, unknown_key)]);

        assert!(matches!(
            outcome,
            Err(BoundaryError::InvalidGatewayState { .. })
        ));

        Ok(())
    }

    /// The Messages visible-content binding is verified against the envelope's authenticated
    /// digest: matching content authenticates, and any mismatch fails as invalid gateway state.
    #[test]
    fn visible_content_bindings_are_verified() -> Result<(), Box<dyn StdError>> {
        let _lock = test_support::configuration_lock();
        test_support::install_configuration();

        let carrier = sealed_carrier(Some("the visible summary"))?;

        let bound = ParsedCompaction {
            control: None,
            boundaries: vec![CompactionBoundary::Gateway {
                offset: 1,
                carrier: carrier.clone(),
                visible_content: Some(String::from("the visible summary")),
            }],
            instruction_items: Vec::new(),
        };

        assert!(BoundaryPreparation::authenticate(&bound).is_ok());

        let mismatched = ParsedCompaction {
            control: None,
            boundaries: vec![CompactionBoundary::Gateway {
                offset: 1,
                carrier,
                visible_content: Some(String::from("tampered visible summary")),
            }],
            instruction_items: Vec::new(),
        };

        assert!(matches!(
            BoundaryPreparation::authenticate(&mismatched),
            Err(BoundaryError::InvalidGatewayState { .. })
        ));

        Ok(())
    }

    /// A gateway envelope wrapping a native window carries the authenticated producer
    /// requirement; a summary envelope and a raw native boundary impose none.
    #[test]
    fn native_requirements_come_only_from_wrapped_windows() -> Result<(), Box<dyn StdError>> {
        let _lock = test_support::configuration_lock();
        test_support::install_configuration();

        assert!(
            prepare(Vec::new())?.native_requirement().is_none(),
            "no boundary imposes no requirement"
        );

        let summary = sealed_carrier(None)?;

        assert!(
            prepare(vec![gateway_boundary(0, summary)])?
                .native_requirement()
                .is_none(),
            "a gateway summary imposes no producer requirement"
        );

        let producer_instance = NativeWindowPayload::producer_instance(3);

        let native_envelope = EnvelopeV1::new(
            String::from("cmp_native"),
            ReplacementMode::CanonicalWindow,
            EnvelopePayload::NativeWindow(NativeWindowPayload {
                producer_instance_id: producer_instance,
                upstream_model_id: String::from("upstream-model"),
                mechanism: Mechanism::MessagesOnDemandCompaction,
                native_format: UpstreamFormat::AnthropicMessages,
                required_prefix_identity: None,
                retained_messages: Vec::new(),
                retained_state_items: Vec::new(),
                window: NativeReplayWindow::AnthropicOnDemandBlock {
                    block: serde_json::json!({"type": "compaction"}),
                },
            }),
            None,
        );

        let carrier = sealing::seal_with_live_config(&native_envelope, None)?;

        let preparation = prepare(vec![gateway_boundary(0, carrier)])?;

        let requirement = preparation
            .native_requirement()
            .ok_or("a wrapped native window carries its producer requirement")?;

        assert_eq!(requirement.producer_instance_id, "provider:3");

        assert_eq!(requirement.upstream_model_id, "upstream-model");

        Ok(())
    }

    /// The active boundary's own state slot is its compaction state item's vector index — the
    /// ordering fact that sorts same-offset state into covered and fresh.
    #[test]
    fn the_active_state_slot_is_the_boundarys_own_state_item() -> Result<(), Box<dyn StdError>> {
        let _lock = test_support::configuration_lock();
        test_support::install_configuration();

        let first = sealed_carrier(None)?;
        let second = sealed_carrier(None)?;

        let preparation = prepare(vec![
            gateway_boundary(0, first),
            gateway_boundary(1, second),
        ])?;

        let ir = IrRequest {
            state_items: vec![
                StateItem {
                    offset: 0,
                    item: serde_json::json!({"type": "compaction"}),
                    is_gateway: true,
                    is_compaction: true,
                    family: StateFamily::Responses,
                },
                StateItem {
                    offset: 0,
                    item: serde_json::json!({"opaque": "reasoning"}),
                    is_gateway: false,
                    is_compaction: false,
                    family: StateFamily::Responses,
                },
                StateItem {
                    offset: 1,
                    item: serde_json::json!({"type": "compaction"}),
                    is_gateway: true,
                    is_compaction: true,
                    family: StateFamily::Responses,
                },
            ],
            ..IrRequest::default()
        };

        assert_eq!(
            preparation.active_state_slot(&ir),
            2,
            "the second boundary pairs with the second compaction-typed state item"
        );

        Ok(())
    }
}
