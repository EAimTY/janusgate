//! Client-response rendering for the Gemini GenerateContent codec: renders IR events as native
//! GenerateContent SSE chunks or an aggregated non-streaming body.

use crate::{
    format::{ClientSseRenderer, arguments_to_object_or_empty},
    ir::{IrAggregation, IrEvent, IrFinishReason, IrUsage},
};
use serde::Serialize;
use serde_json::{Map, Value};

/// Aggregated non-streaming GenerateContent success body: one candidate plus usage.
#[derive(Debug, Serialize)]
pub(crate) struct JsonResponse {
    /// Candidate answers; the gateway serves exactly one.
    pub(crate) candidates: Vec<CandidateOut>,

    /// Token accounting in the Gemini shape.
    #[serde(rename = "usageMetadata")]
    pub(crate) usage_metadata: UsageMetadataOut,

    /// Public model id the client addressed, echoed as the upstream model version.
    #[serde(rename = "modelVersion")]
    pub(crate) model_version: String,

    /// Gateway-minted response identifier.
    #[serde(rename = "responseId")]
    pub(crate) response_id: String,
}

/// One rendered candidate answer.
#[derive(Debug, Serialize)]
pub(crate) struct CandidateOut {
    /// Model content of the candidate.
    pub(crate) content: ContentOut,

    /// Canonical finish reason in the Gemini vocabulary.
    #[serde(rename = "finishReason")]
    pub(crate) finish_reason: &'static str,

    /// Candidate index; the gateway serves exactly one.
    pub(crate) index: u32,
}

/// Model content of one candidate.
#[derive(Debug, Serialize)]
pub(crate) struct ContentOut {
    /// Always `model` for outbound candidates.
    pub(crate) role: &'static str,

    /// Ordered parts of the candidate.
    pub(crate) parts: Vec<PartOut>,
}

/// One outbound part: text (or thought) or a function call.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum PartOut {
    /// Plain text or reasoning text (`thought` set for reasoning).
    Text {
        /// Text content.
        text: String,

        /// Set exactly for reasoning text; omitted for answer text.
        #[serde(skip_serializing_if = "Option::is_none")]
        thought: Option<bool>,
    },

    /// Assistant tool invocation with its replay signature.
    FunctionCall {
        /// Invocation payload.
        #[serde(rename = "functionCall")]
        function_call: FunctionCallOut,

        /// Upstream thought signature replayed across turns.
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
}

/// Outbound function invocation payload.
#[derive(Debug, Serialize)]
pub(crate) struct FunctionCallOut {
    /// Invoked function name.
    pub(crate) name: String,

    /// Arguments object.
    pub(crate) args: Map<String, Value>,

    /// Invocation identifier, the cross-turn pairing channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
}

/// Token accounting in the Gemini shape.
#[derive(Debug, Serialize)]
pub(crate) struct UsageMetadataOut {
    /// Tokens consumed by the prompt.
    #[serde(rename = "promptTokenCount")]
    pub(crate) prompt_token_count: u64,

    /// Tokens produced by candidates, excluding reasoning.
    #[serde(rename = "candidatesTokenCount")]
    pub(crate) candidates_token_count: u64,

    /// Tokens spent on reasoning, omitted when unreported.
    #[serde(rename = "thoughtsTokenCount", skip_serializing_if = "Option::is_none")]
    pub(crate) thoughts_token_count: Option<u64>,

    /// Prompt tokens served from cache, omitted when unreported.
    #[serde(
        rename = "cachedContentTokenCount",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) cached_content_token_count: Option<u64>,
}

/// Streaming chunk: a partial GenerateContentResponse.
#[derive(Debug, Serialize)]
struct StreamChunk {
    /// Partial candidates of this chunk.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<StreamCandidate>,

    /// Cumulative usage, carried only by the terminal usage chunk.
    #[serde(rename = "usageMetadata", skip_serializing_if = "Option::is_none")]
    usage_metadata: Option<UsageMetadataOut>,

    /// Public model id echoed as the upstream model version.
    #[serde(rename = "modelVersion")]
    model_version: String,

    /// Gateway-minted response identifier, stable across the stream.
    #[serde(rename = "responseId")]
    response_id: String,
}

/// One streaming candidate: partial content with an optional terminal reason.
#[derive(Debug, Serialize)]
struct StreamCandidate {
    /// Partial model content.
    content: ContentOut,

    /// Terminal reason, present only on the finish chunk.
    #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
    finish_reason: Option<&'static str>,

    /// Always `0`: the gateway serves one candidate.
    index: u32,
}

/// Maps the canonical finish reason to the Gemini vocabulary.
///
/// Gemini ends function-call turns with the plain `STOP` reason — its vocabulary has no
/// tool-call value — so a turn that delivered calls still reports `STOP`, matching the
/// upstream parser's inverse mapping.
fn finish_reason_to_gemini(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop | IrFinishReason::ToolCall => "STOP",
        IrFinishReason::Length => "MAX_TOKENS",
        IrFinishReason::ContentFilter => "SAFETY",
    }
}

/// Renders IR usage into the Gemini usage shape: the output total keeps including reasoning
/// (matching the upstream codec), so the candidates count is the output minus reasoning.
fn usage_to_gemini(usage: IrUsage) -> UsageMetadataOut {
    let reasoning = usage.reasoning_tokens.unwrap_or(0);

    UsageMetadataOut {
        prompt_token_count: usage.input_tokens,
        candidates_token_count: usage.output_tokens.saturating_sub(reasoning),
        thoughts_token_count: usage.reasoning_tokens,
        cached_content_token_count: usage.cached_tokens,
    }
}

/// Renders an aggregated completion as a non-streaming GenerateContent response.
pub(super) fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> JsonResponse {
    let mut parts: Vec<PartOut> = Vec::new();

    if !aggregation.reasoning.is_empty() {
        parts.push(PartOut::Text {
            text: aggregation.reasoning.clone(),
            thought: Some(true),
        });
    }

    if !aggregation.text.is_empty() {
        parts.push(PartOut::Text {
            text: aggregation.text.clone(),
            thought: None,
        });
    }

    for call in &aggregation.tool_calls {
        parts.push(PartOut::FunctionCall {
            function_call: FunctionCallOut {
                name: call.name.clone(),
                args: arguments_to_object_or_empty(&call.arguments),
                id: Some(call.call_id.clone()),
            },
            thought_signature: call.thought_signature.clone(),
        });
    }

    let usage = aggregation.usage.unwrap_or_default();

    JsonResponse {
        candidates: vec![CandidateOut {
            content: ContentOut {
                role: "model",
                parts,
            },
            finish_reason: finish_reason_to_gemini(
                aggregation.finish.unwrap_or(IrFinishReason::Stop),
            ),
            index: 0,
        }],
        usage_metadata: usage_to_gemini(usage),
        model_version: public_id.to_string(),
        response_id: format!("gen-{}", crate::ids::new_uuid()),
    }
}

/// Streaming renderer producing native GenerateContent SSE frames.
#[derive(Debug)]
pub(super) struct SseRenderer {
    /// Public model id echoed as `modelVersion`.
    model: String,

    /// Gateway-minted response identifier, stable across the stream.
    response_id: String,

    /// Whether any reasoning delta was already rendered; a non-empty done renders only when
    /// none preceded it (parsers emit either deltas plus an empty done, or a done carrying the
    /// full text with no deltas — never both with overlapping text).
    is_reasoning_streamed: bool,

    /// Latest usage observed; rendered once terminally.
    usage: Option<IrUsage>,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(super) fn new(model: String) -> Self {
        Self {
            model,
            response_id: format!("gen-{}", crate::ids::new_uuid()),
            is_reasoning_streamed: false,
            usage: None,
        }
    }

    /// Formats one streaming chunk as a bare `data:` frame.
    fn chunk_frame(
        &self,
        candidates: Vec<StreamCandidate>,
        usage: Option<UsageMetadataOut>,
    ) -> String {
        let chunk = StreamChunk {
            candidates,
            usage_metadata: usage,
            model_version: self.model.clone(),
            response_id: self.response_id.clone(),
        };

        crate::sse::format_data_frame(&crate::format::render_json(&chunk))
    }

    /// Formats one content chunk carrying the given parts.
    fn parts_frame(&self, parts: Vec<PartOut>) -> String {
        self.chunk_frame(
            vec![StreamCandidate {
                content: ContentOut {
                    role: "model",
                    parts,
                },
                finish_reason: None,
                index: 0,
            }],
            None,
        )
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => Vec::new(),

            IrEvent::TextDelta(delta) => vec![self.parts_frame(vec![PartOut::Text {
                text: delta.clone(),
                thought: None,
            }])],

            IrEvent::ReasoningDelta(delta) => {
                self.is_reasoning_streamed = true;

                vec![self.parts_frame(vec![PartOut::Text {
                    text: delta.clone(),
                    thought: Some(true),
                }])]
            }

            // A done carrying text renders it only when no deltas preceded it; the opaque
            // payload is dropped on this surface.
            IrEvent::ReasoningDone { text, .. } => {
                if text.is_empty() || self.is_reasoning_streamed {
                    Vec::new()
                } else {
                    self.is_reasoning_streamed = true;

                    vec![self.parts_frame(vec![PartOut::Text {
                        text: text.clone(),
                        thought: Some(true),
                    }])]
                }
            }

            // Tool-call deltas buffer upstream: the done carries the complete call, so deltas
            // render nothing and the done renders the full invocation live.
            IrEvent::ToolCallDelta { .. } => Vec::new(),

            IrEvent::ToolCallDone {
                call_id,
                name,
                arguments,
                thought_signature,
                ..
            } => vec![self.parts_frame(vec![PartOut::FunctionCall {
                function_call: FunctionCallOut {
                    name: name.clone(),
                    args: arguments_to_object_or_empty(arguments),
                    id: Some(call_id.clone()),
                },
                thought_signature: thought_signature.clone(),
            }])],

            IrEvent::Finish(reason) => vec![self.chunk_frame(
                vec![StreamCandidate {
                    content: ContentOut {
                        role: "model",
                        parts: Vec::new(),
                    },
                    finish_reason: Some(finish_reason_to_gemini(*reason)),
                    index: 0,
                }],
                None,
            )],

            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => match self.usage.take() {
                Some(usage) => vec![self.chunk_frame(Vec::new(), Some(usage_to_gemini(usage)))],
                None => Vec::new(),
            },
        }
    }

    /// Renders a mid-stream error as a terminal SSE frame carrying the Gemini error envelope.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let (_, envelope) = crate::format::error_gemini::render(error);

        crate::sse::format_data_frame(&crate::format::render_json(&envelope))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies aggregated rendering emits the native shape: one `model` candidate with
    /// text plus function calls, a mapped finish reason, and Gemini usage columns.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the native shape.
    #[test]
    fn renders_aggregated_native_shape() {
        use crate::ir::{IrAggregatedToolCall, IrUsage};

        let aggregation = IrAggregation {
            text: "hi".to_string(),
            reasoning: String::new(),
            reasoning_blocks: Vec::new(),
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "lint".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig".to_string()),
            }],
            finish: Some(IrFinishReason::ToolCall),
            usage: Some(IrUsage {
                input_tokens: 10,
                output_tokens: 6,
                cached_tokens: Some(2),
                cache_write_tokens: None,
                reasoning_tokens: Some(1),
            }),
        };

        let rendered = render_json_response("m", &aggregation);

        #[expect(clippy::expect_used, reason = "the rendered body always serializes")]
        let value = serde_json::to_value(&rendered).expect("the body serializes");

        assert_eq!(
            value.pointer("/candidates/0/content/role"),
            Some(&serde_json::json!("model"))
        );
        assert_eq!(
            value.pointer("/candidates/0/finishReason"),
            Some(&serde_json::json!("STOP"))
        );
        assert_eq!(
            value.pointer("/candidates/0/content/parts/0/text"),
            Some(&serde_json::json!("hi"))
        );
        assert_eq!(
            value.pointer("/candidates/0/content/parts/1/functionCall/name"),
            Some(&serde_json::json!("lint"))
        );
        assert_eq!(
            value.pointer("/usageMetadata/promptTokenCount"),
            Some(&serde_json::json!(10))
        );
        assert_eq!(
            value.pointer("/usageMetadata/candidatesTokenCount"),
            Some(&serde_json::json!(5))
        );
        assert_eq!(
            value.pointer("/usageMetadata/thoughtsTokenCount"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            value.pointer("/modelVersion"),
            Some(&serde_json::json!("m"))
        );
    }

    /// Verifies the streaming renderer delivers text live and renders a done-only
    /// reasoning block exactly once: a non-empty done with no prior deltas emits its
    /// thought part, while a done after deltas emits nothing.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the documented behavior.
    #[test]
    fn streams_deltas_live_and_done_only_reasoning_once() {
        let mut renderer = SseRenderer::new("m".to_string());

        let text = renderer.render(&IrEvent::TextDelta("hi".to_string()));

        assert_eq!(text.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let first_frame = &text[0];
        assert!(first_frame.contains(r#""text":"hi""#));

        let mut reasoning_only = SseRenderer::new("m".to_string());

        let done = reasoning_only.render(&IrEvent::ReasoningDone {
            text: "thought".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        assert_eq!(done.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let done_frame = &done[0];
        assert!(done_frame.contains(r#""thought":true"#));

        let mut with_deltas = SseRenderer::new("m".to_string());

        with_deltas.render(&IrEvent::ReasoningDelta("a".to_string()));

        let done = with_deltas.render(&IrEvent::ReasoningDone {
            text: "a".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        assert!(done.is_empty());
    }
}
