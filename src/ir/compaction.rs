//! The IR's compaction facts: the iteration accounting reports and native control entries the
//! compaction workflows carry beside the IR events and aggregation.
//!
//! The iteration reports are the IR's compaction accounting facts — they ride
//! [`IrEvent::CompactionMetadata`](crate::ir::IrEvent::CompactionMetadata) and the aggregation —
//! and the control entries are the vendor-shaped values the request parsers produce and a native
//! upstream receives verbatim. Only producer-owned iteration entries retain arbitrary JSON; the
//! generated reports and controls keep their concrete types until the HTTP/SSE serialization
//! boundary.

use super::IrUsage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The kind of billed sampling iteration. Render-only: it rides the outward
/// usage rendering and is never parsed back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SamplingKind {
    /// Work producing a compaction checkpoint.
    Compaction,
    /// Work producing the caller's ordinary answer.
    Message,
}

/// Usage generated from one actual dispatch in the gateway ledger. Render-only: the gateway
/// constructs the report and serializes it outward (the native producer reports ride
/// [`UsageIteration::Native`] verbatim), never parsing it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct IterationReport {
    /// Sampling operation kind.
    #[serde(rename = "type")]
    kind: SamplingKind,
    /// Tokens consumed by the input.
    pub(crate) input_tokens: u64,
    /// Tokens produced by the output.
    pub(crate) output_tokens: u64,
    /// Input tokens written to the upstream cache, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_creation_input_tokens: Option<u64>,
    /// Input tokens served from the upstream cache, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read_input_tokens: Option<u64>,
}

/// One iteration report rendered in the Messages usage array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum UsageIteration {
    /// A gateway-generated report with fully known fields.
    Gateway(IterationReport),
    /// An authoritative producer report, including all vendor extensions.
    Native(Value),
}

impl<'de> Deserialize<'de> for UsageIteration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::Native)
    }
}

impl From<Value> for UsageIteration {
    fn from(value: Value) -> Self {
        Self::Native(value)
    }
}

impl UsageIteration {
    /// Renders actual observed usage without inventing absent breakdowns.
    pub(crate) const fn gateway(kind: SamplingKind, usage: IrUsage) -> Self {
        Self::Gateway(IterationReport {
            kind,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_write_tokens,
            cache_read_input_tokens: usage.cached_tokens,
        })
    }
}

/// One Responses automatic `context_management` entry: the parsed control and the native
/// passthrough wire entry in one type — the parse and the upstream render share the single
/// field-identical form (`docs/design/compaction.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "compaction")]
pub(crate) struct AutomaticEntry {
    /// Caller-supplied threshold in input tokens; absence keeps the producer's own default
    /// (native) or the gateway's declared-capacity default (emulated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) compact_threshold: Option<u64>,
}

/// One Messages native threshold edit: the vendor-shaped `compact_20260112` edit object a native
/// threshold upstream receives verbatim (`docs/design/compaction.md`). Converted from the parsed
/// [`crate::format::compaction_control::ThresholdEdit`] at the format layer's request boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename = "compact_20260112")]
pub(crate) struct ThresholdEdit {
    /// Caller-supplied input-token trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trigger: Option<InputTokenTrigger>,
    /// Present only when the caller requested a pause.
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "pause_after_compaction"
    )]
    pub(crate) should_pause_after_compaction: Option<bool>,
    /// Caller-supplied summarization instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) instructions: Option<String>,
}

/// Input-token trigger carried by a Messages threshold edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename = "input_tokens")]
pub(crate) struct InputTokenTrigger {
    /// Exact caller-supplied threshold.
    pub(crate) value: u64,
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking on its own assertions; a # Panics section on every \
                  test would document nothing its name does not"
    )]

    use super::{
        AutomaticEntry, InputTokenTrigger, IterationReport, SamplingKind, ThresholdEdit,
        UsageIteration,
    };
    use crate::ir::IrUsage;
    use std::error::Error as StdError;

    /// Every deserialized iteration entry is a producer-owned native report — the gateway never
    /// parses its own render back — while the constructor-built gateway report renders with its
    /// sampling kind and omits unreported breakdowns.
    #[test]
    fn iteration_entries_parse_as_native_and_render_gateway_facts() -> Result<(), Box<dyn StdError>>
    {
        let parsed: UsageIteration = serde_json::from_value(serde_json::json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "vendor_extension": true
        }))?;

        assert!(
            matches!(parsed, UsageIteration::Native(_)),
            "a parsed entry is always the producer-owned native report"
        );

        let report = UsageIteration::gateway(
            SamplingKind::Compaction,
            IrUsage {
                input_tokens: 10,
                output_tokens: 4,
                cached_tokens: None,
                cache_write_tokens: Some(3),
                reasoning_tokens: None,
            },
        );

        let rendered = serde_json::to_value(&report)?;

        assert_eq!(
            rendered,
            serde_json::json!({
                "type": "compaction",
                "input_tokens": 10,
                "output_tokens": 4,
                "cache_creation_input_tokens": 3
            }),
            "an unreported cache read stays absent on the wire"
        );

        let message_report = UsageIteration::gateway(
            SamplingKind::Message,
            IrUsage {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: Some(1),
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
        );

        let rendered = serde_json::to_value(&message_report)?;

        assert_eq!(
            crate::test_support::at(&rendered, "/type"),
            &serde_json::json!("message"),
            "the sampling kind rides the wire spelling"
        );

        assert_eq!(
            serde_json::to_value(&IterationReport {
                kind: SamplingKind::Compaction,
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            })?
            .pointer("/cache_read_input_tokens"),
            None,
            "absent breakdowns are skipped, never null"
        );

        Ok(())
    }

    /// The Messages threshold edit keeps its vendor tag and member presence: absent members stay
    /// absent, and the nested trigger carries its own `input_tokens` tag.
    #[test]
    fn threshold_edits_keep_their_vendor_shape() -> Result<(), Box<dyn StdError>> {
        let full = ThresholdEdit {
            trigger: Some(InputTokenTrigger { value: 50_000 }),
            should_pause_after_compaction: Some(true),
            instructions: Some(String::from("Keep decisions.")),
        };

        assert_eq!(
            serde_json::to_value(&full)?,
            serde_json::json!({
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
                "pause_after_compaction": true,
                "instructions": "Keep decisions."
            })
        );

        assert_eq!(
            serde_json::to_value(ThresholdEdit {
                trigger: None,
                should_pause_after_compaction: None,
                instructions: None,
            })?,
            serde_json::json!({"type": "compact_20260112"})
        );

        let parsed: ThresholdEdit = serde_json::from_value(serde_json::json!({
            "type": "compact_20260112",
            "trigger": {"type": "input_tokens", "value": 60_000}
        }))?;

        assert_eq!(parsed.trigger.map(|trigger| trigger.value), Some(60_000));

        assert_eq!(parsed.should_pause_after_compaction, None);

        assert_eq!(parsed.instructions, None);

        Ok(())
    }

    /// The Responses automatic entry parses and renders as the single field-identical wire form:
    /// the `compaction` tag beside the threshold when the caller stated one.
    #[test]
    fn automatic_entries_are_field_identical_on_both_wires() -> Result<(), Box<dyn StdError>> {
        assert_eq!(
            serde_json::to_value(AutomaticEntry {
                compact_threshold: Some(90_000)
            })?,
            serde_json::json!({"type": "compaction", "compact_threshold": 90_000})
        );

        assert_eq!(
            serde_json::to_value(AutomaticEntry {
                compact_threshold: None
            })?,
            serde_json::json!({"type": "compaction"})
        );

        let parsed: AutomaticEntry = serde_json::from_value(serde_json::json!({
            "type": "compaction"
        }))?;

        assert_eq!(parsed.compact_threshold, None);

        Ok(())
    }
}
