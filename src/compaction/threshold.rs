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
#[must_use]
pub(crate) const fn messages_default_threshold() -> u64 {
    150_000
}

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
pub(crate) fn responses_gateway_default_threshold(
    public_model: &str,
    effective_max_input_tokens: Option<u64>,
) -> Result<u64, CompactionError> {
    let capacity = effective_max_input_tokens.ok_or_else(|| CompactionError::UnknownCapacity {
        model: public_model.to_string(),
    })?;

    // Floor(0.8 * capacity) in integer arithmetic: 4/5 of the capacity,
    // rounding down.
    Ok(capacity / 5 * 4)
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
pub(crate) fn estimate_context_tokens(ir: &IrRequest) -> Result<u64, CompactionError> {
    let bytes = estimate_context_bytes(ir)?;

    Ok(bytes.div_ceil(3))
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
pub(crate) fn estimate_message_tokens(messages: &[IrMessage]) -> Result<u64, CompactionError> {
    let mut bytes = 0usize;

    for message in messages {
        for part in &message.parts {
            match part {
                IrPart::Text { text } => bytes += text.len(),

                IrPart::Image { .. } => return Err(CompactionError::UnsupportedMeasurement),

                IrPart::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    bytes += call_id.len();
                    bytes += name.len();
                    bytes += arguments.len();
                }

                IrPart::ToolOutput { content, .. } => bytes += content.len(),

                IrPart::Reasoning { text, .. } => bytes += text.len(),
            }
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
            match part {
                IrPart::Text { text } => bytes += text.len(),

                IrPart::Image { .. } => return Err(CompactionError::UnsupportedMeasurement),

                IrPart::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    bytes += call_id.len();
                    bytes += name.len();
                    bytes += arguments.len();
                }

                IrPart::ToolOutput { content, .. } => bytes += content.len(),

                IrPart::Reasoning { text, .. } => bytes += text.len(),
            }
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

    use super::{
        estimate_context_tokens, messages_default_threshold, responses_gateway_default_threshold,
    };
    use crate::{
        compaction::error::CompactionError,
        ir::{IrImage, IrMessage, IrPart, IrRequest, IrRole, IrTool, StateItem},
    };
    use serde_json::json;

    /// Builds an IR request from its parts.
    fn request(
        instructions: Option<&str>,
        messages: Vec<IrMessage>,
        tools: Vec<IrTool>,
        state_items: Vec<StateItem>,
    ) -> IrRequest {
        IrRequest {
            model: String::new(),
            instructions: instructions.map(String::from),
            messages,
            tools,
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            response_format: None,
            reasoning_effort: None,
            state_items,
        }
    }

    /// A text message.
    fn text(role: IrRole, text: &str) -> IrMessage {
        IrMessage {
            role,
            parts: vec![IrPart::Text {
                text: String::from(text),
            }],
        }
    }

    /// The estimator counts instructions, message text, tool-call fields,
    /// tool-output content, reasoning text, and tool-definition JSON, in
    /// ceil(bytes / 3) tokens.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn estimates_bytes_over_the_model_visible_context() {
        let ir = request(
            Some("be terse"),
            vec![
                text(IrRole::User, "hello"), // 5 bytes
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::ToolCall {
                            call_id: String::from("c1"),
                            name: String::from("search"),
                            arguments: String::from("{\"q\":\"x\"}"),
                            thought_signature: None,
                        },
                        IrPart::Reasoning {
                            text: String::from("thinking"),
                            signature: None,
                            opaque: json!({"opaque": "state"}),
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: String::from("c1"),
                        content: String::from("result"),
                        is_error: false,
                    }],
                },
            ],
            vec![IrTool {
                name: String::from("search"),
                description: Some(String::from("Search the web.")),
                parameters: None,
            }],
            Vec::new(),
        );

        // Counted bytes: instructions `be terse` (8) + text `hello` (5) +
        // call id `c1` (2) + name `search` (6) + arguments `{"q":"x"}` (9) +
        // reasoning text `thinking` (8) + tool output `result` (6) = 44, plus
        // the tool definitions' canonical JSON; the opaque reasoning payload is
        // excluded, its text is not. The tool JSON length is derived here so the
        // assertion pins the composition and the ceiling, not serde itself.
        #[expect(clippy::expect_used, reason = "the fixture carries no images")]
        let tokens = estimate_context_tokens(&ir).expect("the fixture is measurable");

        #[expect(clippy::expect_used, reason = "the fixture tools serialize")]
        let tools_bytes = serde_json::to_vec(&ir.tools).expect("the fixture tools serialize");

        assert_eq!(
            tokens,
            u64::try_from(44 + tools_bytes.len())
                .unwrap_or(u64::MAX)
                .div_ceil(3)
        );
    }

    /// Image content and opaque state items fail the measurement explicitly.
    ///
    /// # Panics
    ///
    /// Panics if a measurable fixture measures or an unmeasurable one does not.
    #[test]
    fn unrepresentable_content_fails_the_measurement() {
        let image = request(
            None,
            vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Image {
                    image: IrImage::Url(String::from("https://upstream.test/a.png")),
                }],
            }],
            Vec::new(),
            Vec::new(),
        );

        assert!(matches!(
            estimate_context_tokens(&image),
            Err(CompactionError::UnsupportedMeasurement)
        ));

        let opaque = request(
            None,
            vec![text(IrRole::User, "hello")],
            Vec::new(),
            vec![StateItem {
                offset: 0,
                item: json!({"type": "compaction"}),
                is_gateway: false,
            }],
        );

        assert!(matches!(
            estimate_context_tokens(&opaque),
            Err(CompactionError::UnsupportedMeasurement)
        ));
    }

    /// The Messages default trigger is the protocol's 150,000 tokens, and the
    /// Responses gateway default floors 0.8 over the declared capacity, whose
    /// absence is a rejection.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn default_thresholds_follow_the_documented_rules() {
        assert_eq!(messages_default_threshold(), 150_000);

        assert_eq!(
            responses_gateway_default_threshold("m", Some(1_000_000)).map_err(|_| ()),
            Ok(800_000)
        );

        // Floor semantics: 0.8 * 101 = 80.8 floors to 80.
        assert_eq!(
            responses_gateway_default_threshold("m", Some(101)).map_err(|_| ()),
            Ok(80)
        );

        assert!(matches!(
            responses_gateway_default_threshold("m", None),
            Err(CompactionError::UnknownCapacity { .. })
        ));
    }
}
