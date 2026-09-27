//! Client-response rendering for the Gemini GenerateContent codec: renders IR events as native
//! GenerateContent SSE chunks or an aggregated non-streaming body.

use crate::{
    format::{ClientSseRenderer, arguments_to_object_or_empty, gemini::split_gemini_usage},
    ir::{IrAggregation, IrEvent, IrFinishReason, IrUsage},
};
use serde::Serialize;
use serde_json::{Map, Value};

/// Aggregated non-streaming GenerateContent success body: one candidate plus usage — or, for a
/// prompt-level block, the native blocked shape of `promptFeedback` without candidates.
#[derive(Debug, Serialize)]
pub(crate) struct JsonResponse {
    /// Candidate answers; the gateway serves exactly one — none only for a prompt-level
    /// block, whose native blocked shape carries no candidates at all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) candidates: Vec<CandidateOut>,

    /// Prompt-level feedback, present exactly when the prompt was blocked and no candidate
    /// was generated — the native blocked shape.
    #[serde(rename = "promptFeedback", skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_feedback: Option<PromptFeedbackOut>,

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

/// Prompt-level feedback of a blocked prompt, the native blocked shape.
#[derive(Debug, Serialize)]
pub(crate) struct PromptFeedbackOut {
    /// Why the prompt was blocked, the verbatim upstream spelling.
    #[serde(rename = "blockReason")]
    pub(crate) block_reason: String,
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

    /// Total tokens of the generation: the sum of the prompt, candidate, and thought members
    /// below — the official member's documented arithmetic (`ai.google.dev/api/generate-content`).
    #[serde(rename = "totalTokenCount")]
    pub(crate) total_token_count: u64,

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

    /// Prompt-level block feedback, carried only by the terminal chunk of a blocked stream.
    #[serde(rename = "promptFeedback", skip_serializing_if = "Option::is_none")]
    prompt_feedback: Option<PromptFeedbackOut>,

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
/// (matching the upstream codec), so the members come from the shared inverse split — the
/// candidates count is the output minus reasoning — and the official total member is the sum of
/// the members rendered here.
fn usage_to_gemini(usage: IrUsage) -> UsageMetadataOut {
    let (input, candidates, reasoning, cached) = split_gemini_usage(&usage);

    UsageMetadataOut {
        prompt_token_count: input,
        candidates_token_count: candidates,
        total_token_count: input + candidates + reasoning.unwrap_or(0),
        thoughts_token_count: reasoning,
        cached_content_token_count: cached,
    }
}

/// Renders an aggregated completion as a non-streaming GenerateContent response.
pub(super) fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> JsonResponse {
    // A prompt-level block renders the native blocked shape: `promptFeedback` with the
    // verbatim upstream block reason and no candidates (README §6.3). The detail's presence
    // is the block flag — the parser sets it exactly on the prompt-block finish.
    if let Some(block_reason) = &aggregation.prompt_block_reason {
        return JsonResponse {
            candidates: Vec::new(),
            prompt_feedback: Some(PromptFeedbackOut {
                block_reason: block_reason.clone(),
            }),
            usage_metadata: usage_to_gemini(aggregation.usage.unwrap_or_default()),
            model_version: public_id.to_string(),
            response_id: format!("gen-{}", crate::ids::new_uuid()),
        };
    }

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
        prompt_feedback: None,
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

    /// Verbatim upstream prompt-block reason of a prompt-level block finish; rendered on the
    /// terminal chunk (README §6.3).
    prompt_block_reason: Option<String>,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(super) fn new(model: String) -> Self {
        Self {
            model,
            response_id: format!("gen-{}", crate::ids::new_uuid()),
            is_reasoning_streamed: false,
            usage: None,
            prompt_block_reason: None,
        }
    }

    /// Formats one streaming chunk as a bare `data:` frame.
    fn chunk_frame(
        &self,
        candidates: Vec<StreamCandidate>,
        usage: Option<UsageMetadataOut>,
        prompt_feedback: Option<PromptFeedbackOut>,
    ) -> String {
        let chunk = StreamChunk {
            candidates,
            usage_metadata: usage,
            prompt_feedback,
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

            // A prompt-level block renders no candidate at all — the native blocked shape
            // mirrors `promptFeedback` on the terminal chunk instead (README §6.3) — so the
            // reason buffers here and the empty-candidate finish chunk renders only for a
            // candidate-level filter.
            IrEvent::Finish {
                reason,
                prompt_block_reason,
            } => {
                if let Some(block_reason) = prompt_block_reason {
                    self.prompt_block_reason = Some(block_reason.clone());

                    Vec::new()
                } else {
                    vec![self.chunk_frame(
                        vec![StreamCandidate {
                            content: ContentOut {
                                role: "model",
                                parts: Vec::new(),
                            },
                            finish_reason: Some(finish_reason_to_gemini(*reason)),
                            index: 0,
                        }],
                        None,
                        None,
                    )]
                }
            }

            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            // The terminal chunk renders whenever there is something terminal to carry: the
            // cumulative usage, or a prompt-level block's feedback when the upstream
            // reported none.
            IrEvent::Done => {
                let usage = self.usage.take();

                if usage.is_some() || self.prompt_block_reason.is_some() {
                    let prompt_feedback = self
                        .prompt_block_reason
                        .take()
                        .map(|block_reason| PromptFeedbackOut { block_reason });

                    vec![self.chunk_frame(Vec::new(), usage.map(usage_to_gemini), prompt_feedback)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Renders a mid-stream error as a terminal SSE frame carrying the Gemini error envelope.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let (_, envelope) = crate::format::gemini::error::render(error);

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
            prompt_block_reason: None,
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
            value.pointer("/usageMetadata/totalTokenCount"),
            Some(&serde_json::json!(16))
        );
        assert_eq!(
            value.pointer("/modelVersion"),
            Some(&serde_json::json!("m"))
        );
    }

    /// A prompt-level block round-trips into the native blocked shape: the parsed
    /// `promptFeedback.blockReason` rides the IR Finish event to the aggregated body
    /// verbatim — unmapped — and the body carries no candidates, while the usage still
    /// renders (README §6.3).
    ///
    /// # Panics
    ///
    /// Panics if the blocked turn renders as a candidate success or drops the block reason.
    #[test]
    fn prompt_block_round_trips_to_prompt_feedback_without_candidates() {
        use crate::{
            format::{UpstreamEventParser, gemini::generate_content::UpstreamParser},
            ir::Aggregator,
        };

        let body = serde_json::json!({
            "promptFeedback": {"blockReason": "BLOCKLIST"},
            "usageMetadata": {"promptTokenCount": 3},
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the prompt-blocked body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a prompt-blocked body parses");

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let rendered = render_json_response("m", &aggregator.finish());

        #[expect(clippy::expect_used, reason = "the rendered body always serializes")]
        let value = serde_json::to_value(&rendered).expect("the body serializes");

        assert_eq!(
            value.pointer("/promptFeedback/blockReason"),
            Some(&serde_json::json!("BLOCKLIST"))
        );
        assert!(value.get("candidates").is_none());
        assert_eq!(
            value.pointer("/usageMetadata/totalTokenCount"),
            Some(&serde_json::json!(3))
        );
    }

    /// A candidate-level content filter keeps the empty-candidate render: one candidate
    /// with no parts and the `SAFETY` finish reason, and no `promptFeedback` member.
    ///
    /// # Panics
    ///
    /// Panics if the filtered candidate render diverges from the native shape.
    #[test]
    fn candidate_content_filter_still_renders_empty_candidate() {
        let aggregation = IrAggregation {
            text: String::new(),
            reasoning: String::new(),
            reasoning_blocks: Vec::new(),
            tool_calls: Vec::new(),
            finish: Some(IrFinishReason::ContentFilter),
            usage: None,
            prompt_block_reason: None,
        };

        let rendered = render_json_response("m", &aggregation);

        #[expect(clippy::expect_used, reason = "the rendered body always serializes")]
        let value = serde_json::to_value(&rendered).expect("the body serializes");

        assert_eq!(
            value.pointer("/candidates/0/finishReason"),
            Some(&serde_json::json!("SAFETY"))
        );
        assert_eq!(
            value.pointer("/candidates/0/content/parts"),
            Some(&serde_json::json!([]))
        );
        assert!(value.get("promptFeedback").is_none());
    }

    /// A streamed prompt-level block renders no candidate chunk at all — not even the
    /// empty `SAFETY` one — and the terminal chunk mirrors `promptFeedback` beside the
    /// cumulative usage; with no usage reported the terminal chunk carries the feedback
    /// alone so the block always surfaces (README §6.3).
    ///
    /// # Panics
    ///
    /// Panics if any candidate or finish chunk renders for a blocked stream or the
    /// terminal chunk drops the feedback.
    #[test]
    fn streamed_prompt_block_carries_prompt_feedback_without_candidates() {
        let mut renderer = SseRenderer::new("m".to_string());

        let finish = renderer.render(&IrEvent::Finish {
            reason: IrFinishReason::ContentFilter,
            prompt_block_reason: Some("SAFETY".to_string()),
        });

        assert!(
            finish.is_empty(),
            "a prompt block renders no candidate chunk"
        );

        renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 4,
            output_tokens: 0,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        }));

        let frames = renderer.render(&IrEvent::Done);

        assert_eq!(frames.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let frame = &frames[0];
        assert!(frame.contains(r#""promptFeedback":{"blockReason":"SAFETY""#));
        assert!(frame.contains(r#""promptTokenCount":4"#));
        assert!(!frame.contains(r#""candidates""#));
        assert!(!frame.contains("finishReason"));

        // A blocked stream whose upstream reported no usage still terminates with the
        // feedback frame instead of vanishing.
        let mut blocked = SseRenderer::new("m".to_string());

        blocked.render(&IrEvent::Finish {
            reason: IrFinishReason::ContentFilter,
            prompt_block_reason: Some("SAFETY".to_string()),
        });

        let frames = blocked.render(&IrEvent::Done);

        assert_eq!(frames.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let frame = &frames[0];
        assert!(frame.contains(r#""promptFeedback":{"blockReason":"SAFETY""#));
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

        // The terminal usage chunk carries the full usage shape, total included: the sum of the
        // prompt, candidate, and thought counts.
        let mut with_usage = SseRenderer::new("m".to_string());

        with_usage.render(&IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 6,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: Some(1),
        }));

        let usage_frames = with_usage.render(&IrEvent::Done);

        assert_eq!(usage_frames.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let usage_frame = &usage_frames[0];
        assert!(usage_frame.contains(r#""promptTokenCount":10"#));
        assert!(usage_frame.contains(r#""candidatesTokenCount":5"#));
        assert!(usage_frame.contains(r#""totalTokenCount":16"#));
    }
}
