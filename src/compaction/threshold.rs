//! Context measurement and gateway threshold defaults (`docs/design/compaction.md`): the
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::{
        MESSAGES_DEFAULT_THRESHOLD, estimate_context_tokens, estimate_message_tokens,
        responses_gateway_default_threshold,
    };
    use crate::{
        compaction::error::CompactionError,
        ir::{IrImage, IrMessage, IrPart, IrRequest, IrRole, IrTool, StateFamily, StateItem},
    };
    use std::error::Error as StdError;

    /// The gateway default threshold is exactly `floor(0.8 × capacity)` — including the
    /// remainder decomposition cases the integer arithmetic exists for — and a model without a
    /// declared effective capacity is rejected, never given an invented window.
    #[test]
    fn gateway_default_threshold_floors_exactly_or_fails() -> Result<(), Box<dyn StdError>> {
        assert_eq!(
            responses_gateway_default_threshold("m", Some(10)).unwrap_or_default(),
            8
        );

        assert_eq!(
            responses_gateway_default_threshold("m", Some(5)).unwrap_or_default(),
            4
        );

        assert_eq!(
            responses_gateway_default_threshold("m", Some(3)).unwrap_or_default(),
            2
        );

        assert_eq!(
            responses_gateway_default_threshold("m", Some(1)).unwrap_or_default(),
            0
        );

        assert_eq!(
            responses_gateway_default_threshold("m", Some(u64::MAX)).unwrap_or_default(),
            14_757_395_258_967_641_292,
            "the largest capacity floors without overflow"
        );

        match responses_gateway_default_threshold("public-model", None) {
            Err(CompactionError::UnknownCapacity { model }) => {
                assert_eq!(model, "public-model");
            }

            other => {
                return Err(format!("a missing capacity fails explicitly, got {other:?}").into());
            }
        }

        Ok(())
    }

    /// The documented estimator counts `ceil(utf8 bytes / 3)` over instructions, message text,
    /// tool-call identity bytes, tool-output content, reasoning text, and the canonical tool JSON.
    #[test]
    fn estimation_counts_model_visible_bytes() -> Result<(), Box<dyn StdError>> {
        let empty = estimate_context_tokens(&IrRequest::default())?;

        assert_eq!(
            empty, 1,
            "an empty request still serializes an empty tool list"
        );

        let mut ir = IrRequest {
            instructions: Some(String::from("abcdef")),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: String::from("abcdef"),
                }],
            }],
            ..IrRequest::default()
        };

        // 12 model-visible bytes plus the serialized empty tool list.
        assert_eq!(estimate_context_tokens(&ir)?, 14_u64.div_ceil(3));

        ir.tools = vec![IrTool {
            name: String::from("lookup"),
            description: None,
            parameters: Some(serde_json::json!({})),
        }];

        let tool_bytes = u64::try_from(serde_json::to_vec(&ir.tools)?.len())?;

        assert_eq!(
            estimate_context_tokens(&ir)?,
            (12 + tool_bytes).div_ceil(3),
            "tool definitions count through their canonical JSON"
        );

        ir.messages.push(IrMessage {
            role: IrRole::Assistant,
            parts: vec![
                IrPart::ToolCall {
                    call_id: String::from("aa"),
                    name: String::from("b"),
                    arguments: String::from("{}"),
                    thought_signature: None,
                },
                IrPart::Reasoning {
                    text: String::from("abcd"),
                    signature: None,
                    opaque: serde_json::Value::Null,
                },
            ],
        });

        ir.messages.push(IrMessage {
            role: IrRole::Tool,
            parts: vec![IrPart::ToolOutput {
                call_id: String::from("aa"),
                content: String::from("result"),
                is_error: false,
            }],
        });

        // 12 instruction/text bytes + (2+1+2) call bytes + 4 reasoning bytes + 6 output bytes,
        // the tool list no longer empty.
        assert_eq!(estimate_context_tokens(&ir)?, (27 + tool_bytes).div_ceil(3));

        Ok(())
    }

    /// Content the estimate cannot represent faithfully — image parts and opaque state items —
    /// fails the measurement instead of counting transport bytes.
    #[test]
    fn unrepresentable_content_fails_the_measurement() {
        let image = IrRequest {
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Image {
                    image: IrImage::Url(String::from("https://upstream.test/image.png")),
                }],
            }],
            ..IrRequest::default()
        };

        assert!(matches!(
            estimate_context_tokens(&image),
            Err(CompactionError::UnsupportedMeasurement)
        ));

        let stateful = IrRequest {
            state_items: vec![StateItem {
                offset: 0,
                item: serde_json::json!({"type": "compaction"}),
                is_gateway: false,
                is_compaction: true,
                family: StateFamily::Responses,
            }],
            ..IrRequest::default()
        };

        assert!(matches!(
            estimate_context_tokens(&stateful),
            Err(CompactionError::UnsupportedMeasurement),
        ));
    }

    /// The shared unit estimator applies the same rules to a message run: text counts, images
    /// fail, and the `ceil(bytes / 3)` rounding holds per run, not per message.
    #[test]
    fn message_estimation_rounds_per_run() -> Result<(), Box<dyn StdError>> {
        let messages = vec![
            IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: String::from("ab"),
                }],
            },
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Text {
                    text: String::from("c"),
                }],
            },
        ];

        assert_eq!(
            estimate_message_tokens(&messages)?,
            1,
            "3 bytes round to one token"
        );

        let image_run = vec![IrMessage {
            role: IrRole::User,
            parts: vec![IrPart::Image {
                image: IrImage::Inline {
                    mime_type: String::from("image/png"),
                    data_base64: String::from("aXY"),
                },
            }],
        }];

        assert!(matches!(
            estimate_message_tokens(&image_run),
            Err(CompactionError::UnsupportedMeasurement)
        ));

        Ok(())
    }

    /// The protocol's default Messages threshold is the documented 150,000 input tokens.
    #[test]
    fn messages_default_threshold_is_the_documented_value() {
        assert_eq!(MESSAGES_DEFAULT_THRESHOLD, 150_000);
    }
}
