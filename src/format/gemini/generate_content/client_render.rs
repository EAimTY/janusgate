//! Client-response rendering for the Gemini GenerateContent codec: renders IR events as native
//! GenerateContent SSE chunks or an aggregated non-streaming body.

use crate::{
    format::{
        ClientSseRenderer, arguments_to_object_or_empty,
        gemini::{error_data_frame, split_gemini_usage},
    },
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
    pub(super) candidates: Vec<CandidateOut>,

    /// Prompt-level feedback, present exactly when the prompt was blocked and no candidate
    /// was generated — the native blocked shape.
    #[serde(rename = "promptFeedback", skip_serializing_if = "Option::is_none")]
    prompt_feedback: Option<PromptFeedbackOut>,

    /// Token accounting in the Gemini shape.
    #[serde(rename = "usageMetadata")]
    usage_metadata: UsageMetadataOut,

    /// Public model id the client addressed, echoed as the upstream model version.
    #[serde(rename = "modelVersion")]
    model_version: String,

    /// Gateway-minted response identifier.
    #[serde(rename = "responseId")]
    response_id: String,
}

/// Prompt-level feedback of a blocked prompt, the native blocked shape.
#[derive(Debug, Serialize)]
struct PromptFeedbackOut {
    /// Why the prompt was blocked, the verbatim upstream spelling.
    #[serde(rename = "blockReason")]
    block_reason: String,
}

/// One rendered candidate answer.
#[derive(Debug, Serialize)]
pub(super) struct CandidateOut {
    /// Model content of the candidate.
    pub(super) content: ContentOut,

    /// Canonical finish reason in the Gemini vocabulary.
    #[serde(rename = "finishReason")]
    finish_reason: &'static str,

    /// Candidate index; the gateway serves exactly one.
    index: u32,
}

/// Model content of one candidate.
#[derive(Debug, Serialize)]
pub(super) struct ContentOut {
    /// Always `model` for outbound candidates.
    role: &'static str,

    /// Ordered parts of the candidate.
    pub(super) parts: Vec<PartOut>,
}

/// One outbound part: text (or thought) or a function call.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum PartOut {
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
pub(super) struct FunctionCallOut {
    /// Invoked function name.
    pub(super) name: String,

    /// Arguments object.
    pub(super) args: Map<String, Value>,

    /// Invocation identifier, the cross-turn pairing channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) id: Option<String>,
}

/// Token accounting in the Gemini shape.
#[derive(Debug, Serialize)]
struct UsageMetadataOut {
    /// Tokens consumed by the prompt.
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: u64,

    /// Tokens produced by candidates, excluding reasoning.
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: u64,

    /// Total tokens of the generation: the sum of the prompt, candidate, and thought members
    /// below — the official member's documented arithmetic (`ai.google.dev/api/generate-content`).
    #[serde(rename = "totalTokenCount")]
    total_token_count: u64,

    /// Tokens spent on reasoning, omitted when unreported.
    #[serde(rename = "thoughtsTokenCount", skip_serializing_if = "Option::is_none")]
    thoughts_token_count: Option<u64>,

    /// Prompt tokens served from cache, omitted when unreported.
    #[serde(
        rename = "cachedContentTokenCount",
        skip_serializing_if = "Option::is_none"
    )]
    cached_content_token_count: Option<u64>,
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
/// the members rendered here, added with `saturating_add` so a third-party total can never
/// panic (debug) or wrap (release), mirroring the fold in [`crate::format::gemini`].
fn usage_to_gemini(usage: IrUsage) -> UsageMetadataOut {
    let (input, candidates, reasoning, cached) = split_gemini_usage(&usage);

    UsageMetadataOut {
        prompt_token_count: input,
        candidates_token_count: candidates,
        total_token_count: input
            .saturating_add(candidates)
            .saturating_add(reasoning.unwrap_or(0)),
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
pub(super) struct SseRenderer {
    /// Public model id echoed as `modelVersion`.
    model: String,

    /// Gateway-minted response identifier, stable across the stream.
    response_id: String,

    /// Whether reasoning deltas were rendered since the last [`IrEvent::ReasoningDone`] —
    /// mirroring the Aggregator's `pending_reasoning`: a done renders only text its own block's
    /// deltas did not already deliver, and the state resets when the block closes so every later
    /// done-only block still renders (parsers emit either deltas plus an empty done, or a done
    /// carrying the full text with no deltas — never both with overlapping text).
    is_reasoning_block_streamed: bool,

    /// Latest usage observed; rendered once terminally.
    usage: Option<IrUsage>,

    /// Verbatim upstream prompt-block reason of a prompt-level block finish; rendered on the
    /// terminal chunk (README §6.3).
    prompt_block_reason: Option<String>,
    /// Actual emitted part continuation, enabled only for accepted pooled streaming results.
    projection: Option<crate::affinity::fingerprint::Projector>,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(super) fn new(model: String) -> Self {
        Self {
            model,
            response_id: format!("gen-{}", crate::ids::new_uuid()),
            is_reasoning_block_streamed: false,
            usage: None,
            prompt_block_reason: None,
            projection: None,
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

    /// Formats one content chunk carrying the given parts; a pooled projection observes the
    /// emitted parts at their typed render point.
    fn parts_frame(&mut self, parts: Vec<PartOut>) -> String {
        if let Some(projector) = self.projection.as_mut() {
            for part in &parts {
                super::affinity::project_part(part, projector);
            }
        }
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
    fn begin_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.projection = Some(projector);
    }

    fn take_projection(&mut self) -> Option<crate::affinity::fingerprint::Projector> {
        self.projection.take()
    }

    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => Vec::new(),

            // Native compaction state is unrepresentable on this surface and
            // unreachable in practice: a caller that cannot express compaction
            // controls can never trigger a native compaction upstream.
            IrEvent::CompactionState { .. } => Vec::new(),

            IrEvent::TextDelta(delta) => vec![self.parts_frame(vec![PartOut::Text {
                text: delta.clone(),
                thought: None,
            }])],

            IrEvent::ReasoningDelta(delta) => {
                self.is_reasoning_block_streamed = true;

                vec![self.parts_frame(vec![PartOut::Text {
                    text: delta.clone(),
                    thought: Some(true),
                }])]
            }

            // A done renders text only when its own block's deltas did not already deliver it;
            // the block closes here either way, so the per-block state resets and a later
            // done-only block still renders. The opaque payload is dropped on this surface.
            IrEvent::ReasoningDone { text, .. } => {
                let is_block_streamed = self.is_reasoning_block_streamed;

                self.is_reasoning_block_streamed = false;

                if text.is_empty() || is_block_streamed {
                    Vec::new()
                } else {
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

            IrEvent::CompactionMetadata { .. } => Vec::new(),
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
        error_data_frame(error)
    }
}
