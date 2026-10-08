//! Context measurement and gateway threshold defaults (README §6.8): the
//! shared text-only estimator every gateway-emulated threshold evaluates, and
//! the documented default thresholds.
//!
//! The estimator is a documented gateway estimate — `ceil(utf8 bytes / 3)` over
//! the deterministic typed rendering of the model-visible context — not a
//! conservative bound and not the model's actual tokenizer result. Content the
//! estimate cannot represent faithfully — image bytes and opaque native state —
//! is never measured by its transport length: it fails the measurement
//! explicitly, requiring a native counter or a modality-aware counter instead.
//! Estimates never enter the accounting ledger; they are not billed usage.

use super::error::CompactionError;
use crate::ir::{IrMessage, IrPart, IrRequest};

/// The protocol's default Messages threshold trigger: 150,000 input tokens,
/// kept whenever a `compact_20260112` edit states no trigger of its own.
pub(super) const MESSAGES_DEFAULT_THRESHOLD: u64 = 150_000;

/// Resolves the gateway default threshold of an emulated Responses automatic
/// compaction with no client-stated threshold: `floor(0.8 * capacity)` over the
/// model's declared effective maximum input tokens.
///
/// The declared capacity is required: a model with no effective input-tokens
/// fact rejects the request instead of inventing a context window. This is a
/// gateway default, never an OpenAI guarantee.
///
/// # Errors
///
/// Returns [`CompactionError::UnknownCapacity`] when the model declares no
/// effective maximum input tokens.
pub(super) fn responses_gateway_default_threshold(
    public_model: &str,
    effective_max_input_tokens: Option<u64>,
) -> Result<u64, CompactionError> {
    let capacity = effective_max_input_tokens.ok_or_else(|| CompactionError::UnknownCapacity {
        model: public_model.to_string(),
    })?;

    // Floor(0.8 * capacity) exactly, in integer arithmetic: with
    // capacity = 5q + r, floor(4c/5) = 4q + floor(4r/5). The decomposition
    // keeps every factor bounded — 4q <= 4/5 of u64::MAX and 4r <= 16 — so no
    // multiply can overflow, unlike the direct `capacity * 4`.
    let (quotient, remainder) = (capacity / 5, capacity % 5);

    Ok(quotient * 4 + remainder * 4 / 5)
}

/// Estimates the active context size of one request in tokens:
/// `ceil(utf8 bytes / 3)` over the instructions, the text, tool-call, tool-output,
/// and reasoning text of every message, and the canonical JSON of the tool
/// definitions.
///
/// Image content and any opaque state item are unrepresentable in the estimate:
/// encountering one fails the measurement with
/// [`CompactionError::UnsupportedMeasurement`] instead of counting transport
/// bytes (a base64 blob or a URL length is not a context token count).
///
/// # Errors
///
/// Returns [`CompactionError::UnsupportedMeasurement`] when the request carries
/// image content or an opaque state item.
pub(super) fn estimate_context_tokens(ir: &IrRequest) -> Result<u64, CompactionError> {
    let bytes = estimate_context_bytes(ir)?;

    Ok(bytes.div_ceil(3))
}

/// Adds one model-visible part's counted bytes to the running estimate,
/// failing the measurement on image content the estimate cannot represent.
///
/// # Errors
///
/// Returns [`CompactionError::UnsupportedMeasurement`] for image content.
fn part_bytes(part: &IrPart) -> Result<usize, CompactionError> {
    match part {
        IrPart::Text { text } => Ok(text.len()),

        IrPart::Image { .. } => Err(CompactionError::UnsupportedMeasurement),

        IrPart::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => Ok(call_id.len() + name.len() + arguments.len()),

        IrPart::ToolOutput { content, .. } => Ok(content.len()),

        IrPart::Reasoning { text, .. } => Ok(text.len()),
    }
}

/// Estimates the token size of a run of messages — the shared unit estimator
/// the retention selection measures interaction units with. The rules are the
/// message rules of [`estimate_context_tokens`]: image content fails the
/// measurement, opaque reasoning payloads are excluded, text counts.
///
/// # Errors
///
/// Returns [`CompactionError::UnsupportedMeasurement`] when a message carries
/// image content.
pub(super) fn estimate_message_tokens(messages: &[IrMessage]) -> Result<u64, CompactionError> {
    let mut bytes = 0usize;

    for message in messages {
        for part in &message.parts {
            bytes += part_bytes(part)?;
        }
    }

    Ok(u64::try_from(bytes).unwrap_or(u64::MAX).div_ceil(3))
}

/// The byte total [`estimate_context_tokens`] divides by three.
///
/// # Panics
///
/// Panics if the tool definitions fail to serialize — impossible, the IR tools
/// are concrete serde types.
fn estimate_context_bytes(ir: &IrRequest) -> Result<u64, CompactionError> {
    // Opaque state never rides the estimate: the bytes are ciphertext or
    // vendor framing, not model-visible text.
    if !ir.state_items.is_empty() {
        return Err(CompactionError::UnsupportedMeasurement);
    }

    let mut bytes = ir.instructions.as_deref().map_or(0, str::len);

    for message in &ir.messages {
        for part in &message.parts {
            bytes += part_bytes(part)?;
        }
    }

    // Tool definitions enter the model-visible context; their canonical JSON
    // serialization is the deterministic byte rendering.
    #[expect(
        clippy::expect_used,
        reason = "serializing concrete serde types over IR tools cannot fail"
    )]
    let tools_bytes = serde_json::to_vec(&ir.tools).expect("the tool definitions serialize");

    bytes += tools_bytes.len();

    Ok(u64::try_from(bytes).unwrap_or(u64::MAX))
}
