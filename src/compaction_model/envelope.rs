//! The versioned gateway envelope: the authenticated state a gateway
//! compaction artifact carries.
//!
//! One envelope version exists. Its payload is either a gateway summary — a
//! readable summary plus the exact retained history and instruction context
//! needed for continuation — or a wrapped native window: the exact native
//! block or whole next window of a foreign-protocol native compaction result,
//! together with the provenance needed to replay it against its producer.
//!
//! Envelopes never carry credentials, access tokens, URLs containing secrets,
//! or the entire uncompressed conversation. Wrapped native ciphertext remains
//! native ciphertext; it is not a gateway-readable summary.

use crate::{
    compaction_model::{
        identity::{InstructionRole, ItemIdentity},
        mechanism::Mechanism,
    },
    format::UpstreamFormat,
    ir::{IrMessage, IrPart, IrRole},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The gateway summary payload: everything continuation needs, restored once
/// at the checkpoint boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GatewaySummaryPayload {
    /// The readable summary text of the covered older history.
    pub(crate) summary_text: String,

    /// Instruction items preserved across the checkpoint, in their original
    /// order. Past conversation never appears here: instruction roles stay
    /// separate from the conversation summary.
    pub(crate) preserved_instructions: Vec<PreservedInstruction>,

    /// Indivisible retained interaction units, in conversation order. Each
    /// unit carries whole tool exchanges; no unit is ever partially restored.
    pub(crate) retained_units: Vec<RetainedUnit>,

    /// Exact native state retained beside the messages, with restored-window offsets.
    pub(crate) retained_state_items: Vec<crate::ir::StateItem>,

    /// The identities of the preserved instruction items the checkpoint
    /// covers, so replay can distinguish proven instruction overlap from new
    /// entries — the conversation prefix is superseded wholesale at the
    /// boundary offset and never matched by identity. Bounded: the covered
    /// conversational content itself is never copied here.
    pub(crate) overlap_identities: Vec<ItemIdentity>,
}

/// One instruction item preserved across a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreservedInstruction {
    /// The item's stable identity.
    pub(crate) identity: ItemIdentity,

    /// The instruction's role.
    pub(crate) role: InstructionRole,

    /// The instruction text.
    pub(crate) text: String,
}

/// One indivisible retained interaction unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedUnit {
    /// The unit's messages in order, in the protocol-neutral IR form every
    /// format renders. The conversation prefix supersedes wholesale at the
    /// checkpoint boundary, so the unit carries no identity of its own.
    pub(crate) messages: Vec<IrMessage>,
}

/// A wrapped native compaction state: the exact native artifact produced by an
/// upstream, plus the provenance needed to replay it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeWindowPayload {
    /// Stable producer identity: the originating provider instance, spelled by
    /// [`NativeWindowPayload::producer_instance`] — `provider:{providers.id}`.
    /// A credential refresh is not a new producer; the endpoint/account/model
    /// the row points at is the compatibility surface this identifier names.
    pub(crate) producer_instance_id: String,

    /// The originating upstream model identifier the native state was produced
    /// for.
    pub(crate) upstream_model_id: String,

    /// The originating native mechanism.
    pub(crate) mechanism: Mechanism,

    /// The wire format the native state speaks.
    pub(crate) native_format: UpstreamFormat,

    /// Prefix requirements the native replay must honor, when the producer
    /// binds its state to a system/tool prefix.
    pub(crate) required_prefix_identity: Option<PrefixIdentity>,

    /// Whole active-turn messages preserved after the native compacted prefix.
    pub(crate) retained_messages: Vec<IrMessage>,

    /// Opaque retained state, with offsets relative to the retained messages.
    pub(crate) retained_state_items: Vec<crate::ir::StateItem>,

    /// The exact native replay window.
    pub(crate) window: NativeReplayWindow,
}

impl NativeWindowPayload {
    /// The wire spelling of one provider instance's producer identity:
    /// `provider:{providers.id}`. The one constructor of the spelling every
    /// producer comparison shares — a hand-derived copy that drifts would
    /// match no envelope and fail replay as inaccessible state.
    #[must_use]
    pub(crate) fn producer_instance(provider_id: i64) -> String {
        format!("provider:{provider_id}")
    }
}

/// Prefix requirements of signed native state: digests of the exact system
/// instruction and tool-definition prefix the producer's signature verifies
/// against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrefixIdentity {
    /// Digest of the required system instruction prefix, when bound.
    pub(crate) instructions_digest: Option<String>,

    /// Digest of the required tool-definition prefix, when bound.
    pub(crate) tools_digest: Option<String>,
}

/// The exact native replay window of a wrapped artifact, typed per producing
/// family. The vendor-shaped members are third-party data the gateway does not
/// define; they are preserved verbatim, optional-field presence included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NativeReplayWindow {
    /// The complete canonical output window of an OpenAI-family standalone
    /// compact: the input items to replay verbatim, retained messages and
    /// reasoning included, never reduced to the encrypted item.
    ResponsesWindow {
        /// The vendor-shaped input items, in order.
        items: Vec<Value>,
    },

    /// A single Anthropic compaction block, restored with the leading
    /// placement its replay contract demands.
    AnthropicOnDemandBlock {
        /// The vendor-shaped block, verbatim: content, signature,
        /// encrypted_content, and tool_changes preserved as the producer
        /// emitted them.
        block: Value,
    },
}

/// How a caller's window is consumed at the checkpoint boundary. This is the
/// outward source placement; the payload's mechanism separately determines how
/// a wrapped native window is restored for its producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReplacementMode {
    /// The artifact is the complete represented next window: after the caller
    /// discards the entire original input, the artifact alone sustains
    /// continuation.
    CanonicalWindow,

    /// The artifact restores with the leading placement its protocol demands
    /// (an on-demand compaction block), followed by fresh turns.
    LeadingWindow,

    /// The artifact sits inside the history at its covered boundary: entries
    /// before it are the superseded prefix, entries after it the fresh suffix.
    HistoricalCheckpoint,
}

/// The versioned gateway envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvelopeV1 {
    /// Envelope schema version; always `1` for [`EnvelopeV1`].
    pub(super) version: u8,

    /// The gateway-minted artifact identifier, stable across SSE and JSON
    /// representations.
    pub(crate) artifact_id: String,

    /// How the caller's window is consumed at the checkpoint boundary.
    replacement: ReplacementMode,

    /// The envelope's payload.
    pub(crate) payload: EnvelopePayload,

    /// Digest of the visible content the carrier presented alongside the
    /// envelope, when the carrier binds one (the Messages carrier binds its
    /// block's visible content). Verified on every unseal.
    pub(super) visible_content_digest: Option<String>,
}

/// The envelope payload, discriminated by type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum EnvelopePayload {
    /// A gateway-generated summary checkpoint.
    GatewaySummary(GatewaySummaryPayload),

    /// A wrapped native compaction state.
    NativeWindow(NativeWindowPayload),
}

impl EnvelopeV1 {
    /// The envelope schema version this type decodes.
    pub(super) const VERSION: u8 = 1;

    /// Builds a version-1 envelope.
    #[must_use]
    pub(crate) fn new(
        artifact_id: String,
        replacement: ReplacementMode,
        payload: EnvelopePayload,
        visible_content_digest: Option<String>,
    ) -> Self {
        Self {
            version: Self::VERSION,
            artifact_id,
            replacement,
            payload,
            visible_content_digest,
        }
    }
}

/// Builds the user-role message the gateway summary's readable text restores
/// as: the fixed prefix line followed by the stored summary text. The summary
/// is conversation context, never instruction authority.
#[must_use]
pub(crate) fn summary_message(summary_text: &str) -> IrMessage {
    IrMessage {
        role: IrRole::User,
        parts: vec![IrPart::Text {
            text: format!("{SUMMARY_RESTORE_PREFIX}\n\n{summary_text}"),
        }],
    }
}

/// The fixed first line of a restored gateway summary message.
const SUMMARY_RESTORE_PREFIX: &str = "The following is a summary of the earlier conversation:";

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking on its own assertions; a # Panics section on every \
                  test would document nothing its name does not"
    )]

    use super::{
        EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, NativeReplayWindow,
        NativeWindowPayload, PrefixIdentity, ReplacementMode, RetainedUnit, summary_message,
    };
    use crate::{
        compaction_model::mechanism::Mechanism,
        format::UpstreamFormat,
        ir::{IrMessage, IrPart, IrRole},
    };
    use std::error::Error as StdError;

    /// The restored summary message is a user-role text message: the fixed context line, a blank
    /// line, then the stored summary text — conversation context, never instruction authority.
    #[test]
    fn summary_message_is_a_prefixed_user_text_message() -> Result<(), Box<dyn StdError>> {
        let message = summary_message("The earlier work.");

        assert_eq!(message.role, IrRole::User);

        let [IrPart::Text { text }] = message.parts.as_slice() else {
            return Err("the summary message carries exactly one text part".into());
        };

        assert_eq!(
            text,
            "The following is a summary of the earlier conversation:\n\nThe earlier work."
        );

        Ok(())
    }

    /// The producer identity spelling is `provider:{id}` — the one constructor every producer
    /// comparison shares; a hand-derived copy that drifts would match no envelope.
    #[test]
    fn producer_instance_spells_the_provider_prefix() {
        assert_eq!(
            NativeWindowPayload::producer_instance(42),
            String::from("provider:42")
        );
    }

    /// A version-1 envelope round-trips through its serde form: the version pins to 1, the
    /// replacement mode and payload discriminate by their canonical snake-case tags, and both
    /// payload kinds decode back to the exact value sealed.
    #[test]
    fn envelope_round_trips_with_version_and_tags() -> Result<(), Box<dyn StdError>> {
        let summary_payload = GatewaySummaryPayload {
            summary_text: String::from("Summary."),
            preserved_instructions: Vec::new(),
            retained_units: vec![RetainedUnit {
                messages: vec![IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("kept"),
                    }],
                }],
            }],
            retained_state_items: Vec::new(),
            overlap_identities: Vec::new(),
        };

        let summary = EnvelopeV1::new(
            String::from("cmp_1"),
            ReplacementMode::HistoricalCheckpoint,
            EnvelopePayload::GatewaySummary(summary_payload.clone()),
            Some(String::from("digest")),
        );

        let body = serde_json::to_value(&summary)?;

        assert_eq!(
            crate::test_support::at(&body, "/version"),
            &serde_json::json!(1)
        );
        assert_eq!(
            crate::test_support::at(&body, "/replacement"),
            &serde_json::json!("historical_checkpoint")
        );
        assert_eq!(
            crate::test_support::at(&body, "/payload/type"),
            &serde_json::json!("gateway_summary")
        );

        assert_eq!(serde_json::from_value::<EnvelopeV1>(body)?, summary);

        let native = EnvelopeV1::new(
            String::from("cmp_2"),
            ReplacementMode::CanonicalWindow,
            EnvelopePayload::NativeWindow(NativeWindowPayload {
                producer_instance_id: String::from("provider:7"),
                upstream_model_id: String::from("upstream-model"),
                mechanism: Mechanism::ResponsesStandaloneCompact,
                native_format: UpstreamFormat::OpenAiResponse,
                required_prefix_identity: Some(PrefixIdentity {
                    instructions_digest: Some(String::from("abc")),
                    tools_digest: None,
                }),
                retained_messages: Vec::new(),
                retained_state_items: Vec::new(),
                window: NativeReplayWindow::ResponsesWindow { items: Vec::new() },
            }),
            None,
        );

        let body = serde_json::to_value(&native)?;

        assert_eq!(
            crate::test_support::at(&body, "/payload/type"),
            &serde_json::json!("native_window")
        );
        assert_eq!(
            crate::test_support::at(&body, "/replacement"),
            &serde_json::json!("canonical_window")
        );

        assert_eq!(serde_json::from_value::<EnvelopeV1>(body)?, native);

        Ok(())
    }

    /// The envelope and its payloads reject unknown members: a member outside the closed schema
    /// fails decoding instead of being silently dropped.
    #[test]
    fn envelope_rejects_unknown_members() {
        let body = serde_json::json!({
            "version": 1,
            "artifact_id": "cmp_1",
            "replacement": "canonical_window",
            "payload": {
                "type": "gateway_summary",
                "summary_text": "s",
                "preserved_instructions": [],
                "retained_units": [],
                "retained_state_items": [],
                "overlap_identities": []
            },
            "unexpected_member": true
        });

        assert!(serde_json::from_value::<EnvelopeV1>(body).is_err());

        let complete = serde_json::json!({
            "version": 1,
            "artifact_id": "cmp_1",
            "replacement": "canonical_window",
            "payload": {
                "type": "gateway_summary",
                "summary_text": "s",
                "preserved_instructions": [],
                "retained_units": [],
                "retained_state_items": [],
                "overlap_identities": []
            }
        });

        assert!(
            serde_json::from_value::<EnvelopeV1>(complete).is_ok(),
            "the same body without the unknown member decodes"
        );
    }
}
