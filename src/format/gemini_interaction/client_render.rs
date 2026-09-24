//! Client-response rendering for the Gemini Interactions codec: renders IR events as native
//! Interactions SSE frames (`event:`-named family events) or the aggregated non-streaming
//! Interaction resource, speaking the wire vocabulary the parent module fixes.
//!
//! # Surface contract
//!
//! - Streaming responses reuse the family's own event vocabulary (`interaction.created`,
//!   `step.start`, `step.delta`, `step.stop`, `interaction.complete`) with `event:` names; the
//!   aggregated response is the Interaction resource (`id`, `model`, `status`, `steps`, `usage`). A
//!   `step.stop` always carries the complete step, so receivers replace — never append —
//!   accumulated delta state. Steps render in a fixed order the aggregation can guarantee: thought
//!   steps first, then the text step, then function calls.

use super::{RenderStep, STATUS_COMPLETED, STATUS_INCOMPLETE, STATUS_REQUIRES_ACTION};
use crate::{
    format::{self, ClientSseRenderer},
    ids,
    ir::{IrAggregation, IrFinishReason, IrUsage},
    sse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The non-streaming success body: the Interaction resource over the aggregated events.
#[derive(Debug, Serialize)]
pub(crate) struct JsonResponse {
    /// Renderer-minted interaction identifier.
    id: String,

    /// Public routing id the client addressed.
    model: String,

    /// Terminal status derived from the aggregated finish.
    status: &'static str,

    /// Response steps: thought steps, then the text step, then function calls.
    steps: Vec<RenderStep>,

    /// Token accounting, absent when the upstream reported no usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OutUsage>,
}

/// Token accounting in the Interactions usage shape: the inverse of the upstream usage mapping —
/// the IR output total (which includes reasoning) splits back into the output and thought
/// members, so a same-family round-trip restores the reported numbers.
#[derive(Debug, Serialize)]
struct OutUsage {
    /// Tokens consumed by the input.
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    total_output_tokens: u64,

    /// Tokens spent on model reasoning, omitted when the upstream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache, omitted when the upstream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cached_tokens: Option<u64>,
}

impl OutUsage {
    /// Splits an IR usage report back into the Interactions members.
    fn from_usage(usage: &IrUsage) -> Self {
        Self {
            total_input_tokens: usage.input_tokens,
            total_output_tokens: usage
                .output_tokens
                .saturating_sub(usage.reasoning_tokens.unwrap_or(0)),
            total_thought_tokens: usage.reasoning_tokens,
            total_cached_tokens: usage.cached_tokens,
        }
    }
}

/// Renders an aggregated completion as the non-streaming Interaction resource. Steps render in the
/// one order the aggregation can guarantee — thought steps in block order, then the text step,
/// then function calls — because the fold does not preserve the original interleaving.
///
/// # Panics
///
/// Panics if serializing a stored opaque step fails, which cannot occur for JSON values.
pub(super) fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> JsonResponse {
    let mut steps: Vec<RenderStep> = Vec::new();

    for block in &aggregation.reasoning_blocks {
        steps.push(super::render_thought(
            &block.text,
            block.signature.clone(),
            &block.opaque,
        ));
    }

    if !aggregation.text.is_empty() {
        steps.push(RenderStep::Typed(super::TypedStep::OutputText {
            text: aggregation.text.clone(),
        }));
    }

    for call in &aggregation.tool_calls {
        steps.push(RenderStep::Typed(super::TypedStep::FunctionCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            signature: call.thought_signature.clone(),
        }));
    }

    JsonResponse {
        id: format!("int_{}", ids::new_uuid()),
        model: public_id.to_string(),
        status: finish_status(aggregation.finish),
        steps,
        usage: aggregation.usage.as_ref().map(OutUsage::from_usage),
    }
}

/// Maps the canonical finish onto the Interactions status vocabulary. The vocabulary has no
/// content-filter member: a filtered turn was cut off, so it reports `incomplete` — the same
/// bucket the Responses surface renders it into.
fn finish_status(finish: Option<IrFinishReason>) -> &'static str {
    match finish {
        None | Some(IrFinishReason::Stop) => STATUS_COMPLETED,
        Some(IrFinishReason::ToolCall) => STATUS_REQUIRES_ACTION,
        Some(IrFinishReason::Length) | Some(IrFinishReason::ContentFilter) => STATUS_INCOMPLETE,
    }
}

/// Streaming renderer emitting the family's own event vocabulary with `event:` names.
#[derive(Debug)]
pub(super) struct SseRenderer {
    /// Public routing id the client addressed, echoed in the created frame.
    public_id: String,

    /// Renderer-minted interaction identifier.
    interaction_id: String,

    /// Next upstream step index to mint for an opened step.
    next_step_index: u64,

    /// Whether the created frame was emitted.
    is_created: bool,

    /// Open model text step, accumulating its text.
    open_text: Option<OpenTextStep>,

    /// Open model reasoning block, accumulating its summary.
    open_thought: Option<OpenThoughtStep>,

    /// Open model tool invocations by call index.
    open_calls: Vec<OpenCallStep>,

    /// Terminal finish buffered for the complete frame.
    finish: Option<IrFinishReason>,

    /// Terminal usage buffered for the complete frame.
    usage: Option<IrUsage>,

    /// Whether the complete frame was emitted; the gateway sends `Done` exactly once, so this is
    /// a defensive guard, not a second terminal path.
    is_complete: bool,
}

/// An open model text step of the client stream.
#[derive(Debug)]
struct OpenTextStep {
    /// Minted step index.
    index: u64,

    /// Text streamed so far.
    text: String,
}

/// An open model reasoning block of the client stream.
#[derive(Debug)]
struct OpenThoughtStep {
    /// Minted step index.
    index: u64,

    /// Minted step identifier.
    id: String,

    /// Summary streamed so far.
    summary: String,
}

/// An open model tool invocation of the client stream.
#[derive(Debug)]
struct OpenCallStep {
    /// Stable tool-call index within the response.
    call_index: usize,

    /// Minted step index.
    step_index: u64,

    /// Identifier of the invocation, filled in as deltas reveal it.
    id: String,

    /// Name of the invoked function, filled in as deltas reveal it.
    name: String,
}

/// The `interaction.created` frame payload.
#[derive(Debug, Serialize)]
struct CreatedPayload<'a> {
    /// Renderer-minted interaction identifier.
    id: &'a str,

    /// Public routing id the client addressed.
    model: &'a str,
}

/// The `step.start` frame payload.
#[derive(Debug, Serialize)]
struct StartPayload<'a> {
    /// Step index the frame opens.
    step_index: u64,

    /// The opening shape of the step.
    step: StartStep<'a>,
}

/// The opening shape of a streamed step: its kind, with the id and name when already known.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum StartStep<'a> {
    /// An opening text step.
    #[serde(rename = "output_text")]
    OutputText {},

    /// An opening reasoning block.
    #[serde(rename = "thought")]
    Thought {
        /// Step identifier.
        id: &'a str,
    },

    /// An opening tool invocation.
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Identifier of the invocation, empty until deltas reveal it.
        id: &'a str,

        /// Name of the invoked function, empty until deltas reveal it.
        name: &'a str,
    },
}

/// The `step.delta` frame payload.
#[derive(Debug, Serialize)]
struct DeltaPayload<'a> {
    /// Step index the delta belongs to.
    step_index: u64,

    /// The incremental delta.
    delta: OutDelta<'a>,
}

/// One incremental delta of the client stream, tagged by its kind.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum OutDelta<'a> {
    /// Model text bytes.
    #[serde(rename = "text")]
    Text {
        /// The text bytes.
        text: &'a str,
    },

    /// Reasoning summary bytes.
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        /// The summary bytes.
        summary: &'a str,
    },

    /// The signature terminating a reasoning block.
    #[serde(rename = "thought_signature")]
    ThoughtSignature {
        /// The signature text.
        signature: &'a str,
    },

    /// Tool argument bytes.
    #[serde(rename = "tool_arguments")]
    ToolArguments {
        /// The argument bytes.
        arguments: &'a str,
    },
}

/// The `step.stop` frame payload: the index beside the complete step, so receivers replace —
/// never append — accumulated delta state.
#[derive(Debug, Serialize)]
struct StopPayload<'a> {
    /// Step index the frame closes.
    step_index: u64,

    /// The complete step.
    step: &'a RenderStep,
}

/// The `interaction.complete` frame payload.
#[derive(Debug, Serialize)]
struct CompletePayload<'a> {
    /// Terminal status derived from the finish.
    status: &'static str,

    /// Token accounting, absent when the stream reported no usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OutUsageRef<'a>>,
}

/// Token accounting borrowed for the complete frame.
#[derive(Debug, Serialize)]
struct OutUsageRef<'a> {
    /// Tokens consumed by the input.
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    total_output_tokens: u64,

    /// Tokens spent on model reasoning, omitted when the stream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache, omitted when the stream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cached_tokens: Option<&'a u64>,
}

impl SseRenderer {
    /// Boxes a renderer for one parsed request over the public routing id the client addressed.
    pub(super) fn new(public_id: String) -> Self {
        Self {
            public_id,
            interaction_id: format!("int_{}", ids::new_uuid()),
            next_step_index: 0,
            is_created: false,
            open_text: None,
            open_thought: None,
            open_calls: Vec::new(),
            finish: None,
            usage: None,
            is_complete: false,
        }
    }

    /// Emits the created frame exactly once, first, ahead of any other frame.
    fn ensure_created(&mut self) -> Vec<String> {
        if self.is_created {
            return Vec::new();
        }

        self.is_created = true;

        vec![sse::format_frame(
            Some(super::EVENT_CREATED),
            &format::render_json(&CreatedPayload {
                id: &self.interaction_id,
                model: &self.public_id,
            }),
        )]
    }

    /// Opens the text step when none is open, returning its index.
    fn open_text_step(&mut self, frames: &mut Vec<String>) -> u64 {
        if let Some(open) = &self.open_text {
            return open.index;
        }

        let index = self.next_step_index;

        self.next_step_index += 1;
        self.open_text = Some(OpenTextStep {
            index,
            text: String::new(),
        });

        frames.push(sse::format_frame(
            Some(super::EVENT_STEP_START),
            &format::render_json(&StartPayload {
                step_index: index,
                step: StartStep::OutputText {},
            }),
        ));

        index
    }

    /// Opens the reasoning block under the given id when none is open, returning its index.
    /// Callers pass the stored id for done-only blocks and a minted one for live deltas, so the
    /// opened step always carries the id its stop will close with.
    fn open_thought_step(&mut self, frames: &mut Vec<String>, id: String) -> u64 {
        if let Some(open) = &self.open_thought {
            return open.index;
        }

        let index = self.next_step_index;

        self.next_step_index += 1;

        frames.push(sse::format_frame(
            Some(super::EVENT_STEP_START),
            &format::render_json(&StartPayload {
                step_index: index,
                step: StartStep::Thought { id: &id },
            }),
        ));

        self.open_thought = Some(OpenThoughtStep {
            index,
            id,
            summary: String::new(),
        });

        index
    }

    /// Mints a step identifier for a freshly opened reasoning block.
    fn minted_thought_id() -> String {
        format!("thought_{}", ids::new_uuid())
    }

    /// Closes the open text step with its complete step, when one is open.
    fn close_text_step(&mut self, frames: &mut Vec<String>) {
        if let Some(open) = self.open_text.take() {
            let step = RenderStep::Typed(super::TypedStep::OutputText { text: open.text });

            frames.push(stop_frame(open.index, &step));
        }
    }

    /// Closes every open step and emits the terminal complete frame exactly once.
    fn complete(&mut self) -> Vec<String> {
        if self.is_complete {
            return Vec::new();
        }

        self.is_complete = true;

        let mut frames = self.ensure_created();

        self.close_text_step(&mut frames);

        if let Some(open) = self.open_thought.take() {
            let step = RenderStep::Typed(super::TypedStep::Thought {
                id: open.id,
                summary: (!open.summary.is_empty()).then_some(open.summary),
                signature: None,
            });

            frames.push(stop_frame(open.index, &step));
        }

        for open in std::mem::take(&mut self.open_calls) {
            let step = RenderStep::Typed(super::TypedStep::FunctionCall {
                id: open.id,
                name: open.name,
                arguments: String::new(),
                signature: None,
            });

            frames.push(stop_frame(open.step_index, &step));
        }

        let usage = self.usage.as_ref().map(|usage| OutUsageRef {
            total_input_tokens: usage.input_tokens,
            total_output_tokens: usage
                .output_tokens
                .saturating_sub(usage.reasoning_tokens.unwrap_or(0)),
            total_thought_tokens: usage.reasoning_tokens,
            total_cached_tokens: usage.cached_tokens.as_ref(),
        });

        frames.push(sse::format_frame(
            Some(super::EVENT_COMPLETE),
            &format::render_json(&CompletePayload {
                status: finish_status(self.finish),
                usage,
            }),
        ));

        frames
    }
}

/// Renders a `step.stop` frame for one complete step: a free function because it borrows no
/// renderer state — the step serializes immediately into the owned frame string.
fn stop_frame(step_index: u64, step: &RenderStep) -> String {
    sse::format_frame(
        Some(super::EVENT_STEP_STOP),
        &format::render_json(&StopPayload { step_index, step }),
    )
}

/// Minimal probe reading a stored thought step's identifier.
#[derive(Debug, Deserialize)]
struct OpaqueIdProbe {
    /// Step type tag.
    #[serde(rename = "type")]
    step_type: String,

    /// Step identifier, when the step carries one.
    #[serde(default)]
    id: Option<String>,
}

/// Returns the identifier a same-family opaque step carries, so a done-only block reopens under
/// the stored id instead of a fresh one; anything else yields no hint.
fn opaque_step_id(opaque: &Value) -> Option<String> {
    serde_json::from_value::<OpaqueIdProbe>(opaque.clone())
        .ok()
        .and_then(|probe| {
            (probe.step_type == super::STEP_THOUGHT)
                .then_some(probe.id)
                .flatten()
        })
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more native SSE frames.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        use crate::ir::IrEvent;

        match event {
            IrEvent::MessageStart { .. } => self.ensure_created(),

            IrEvent::TextDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_text_step(&mut frames);

                if let Some(open) = &mut self.open_text {
                    open.text.push_str(delta);
                }

                frames.push(sse::format_frame(
                    Some(super::EVENT_STEP_DELTA),
                    &format::render_json(&DeltaPayload {
                        step_index: index,
                        delta: OutDelta::Text {
                            text: delta.as_str(),
                        },
                    }),
                ));

                frames
            }

            IrEvent::ReasoningDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_thought_step(&mut frames, Self::minted_thought_id());

                if let Some(open) = &mut self.open_thought {
                    open.summary.push_str(delta);
                }

                frames.push(sse::format_frame(
                    Some(super::EVENT_STEP_DELTA),
                    &format::render_json(&DeltaPayload {
                        step_index: index,
                        delta: OutDelta::ThoughtSummary {
                            summary: delta.as_str(),
                        },
                    }),
                ));

                frames
            }

            // A done carrying text opens its block when no deltas preceded it; an empty-text
            // done only closes the block the deltas already delivered. Either way the stop
            // carries the complete step: the stored value verbatim for same-family blocks,
            // synthesized from text and signature otherwise.
            IrEvent::ReasoningDone {
                text,
                signature,
                opaque,
            } => {
                let mut frames = self.ensure_created();

                match self.open_thought.take() {
                    Some(open) => {
                        let index = open.index;

                        if let Some(signature) = signature {
                            frames.push(sse::format_frame(
                                Some(super::EVENT_STEP_DELTA),
                                &format::render_json(&DeltaPayload {
                                    step_index: index,
                                    delta: OutDelta::ThoughtSignature {
                                        signature: signature.as_str(),
                                    },
                                }),
                            ));
                        }

                        let step = if super::is_family_thought_step(opaque) {
                            RenderStep::Verbatim(opaque.clone())
                        } else {
                            RenderStep::Typed(super::TypedStep::Thought {
                                id: open.id,
                                summary: (!open.summary.is_empty()).then_some(open.summary),
                                signature: signature.clone(),
                            })
                        };

                        frames.push(stop_frame(index, &step));
                    }

                    // A done carrying the full text with no prior deltas reopens under the
                    // stored id when the opaque is a same-family step, else under a minted one,
                    // so the stop always closes the id its start opened with.
                    None if !text.is_empty() => {
                        let id = opaque_step_id(opaque).unwrap_or_else(Self::minted_thought_id);
                        let index = self.open_thought_step(&mut frames, id.clone());

                        frames.push(sse::format_frame(
                            Some(super::EVENT_STEP_DELTA),
                            &format::render_json(&DeltaPayload {
                                step_index: index,
                                delta: OutDelta::ThoughtSummary {
                                    summary: text.as_str(),
                                },
                            }),
                        ));

                        if let Some(signature) = signature {
                            frames.push(sse::format_frame(
                                Some(super::EVENT_STEP_DELTA),
                                &format::render_json(&DeltaPayload {
                                    step_index: index,
                                    delta: OutDelta::ThoughtSignature {
                                        signature: signature.as_str(),
                                    },
                                }),
                            ));
                        }

                        self.open_thought = None;

                        let step = if super::is_family_thought_step(opaque) {
                            RenderStep::Verbatim(opaque.clone())
                        } else {
                            RenderStep::Typed(super::TypedStep::Thought {
                                id,
                                summary: Some(text.clone()),
                                signature: signature.clone(),
                            })
                        };

                        frames.push(stop_frame(index, &step));
                    }

                    // An empty done with no open block closes nothing: no deltas delivered text
                    // and no signature needs carrying.
                    _ => {}
                }

                frames
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                let mut frames = self.ensure_created();

                let step_index = match self
                    .open_calls
                    .iter_mut()
                    .find(|open| open.call_index == *index)
                {
                    Some(open) => {
                        if let Some(call_id) = call_id
                            && open.id.is_empty()
                        {
                            open.id = call_id.clone();
                        }

                        if let Some(name) = name
                            && open.name.is_empty()
                        {
                            open.name = name.clone();
                        }

                        open.step_index
                    }

                    None => {
                        let step_index = self.next_step_index;

                        self.next_step_index += 1;

                        frames.push(sse::format_frame(
                            Some(super::EVENT_STEP_START),
                            &format::render_json(&StartPayload {
                                step_index,
                                step: StartStep::FunctionCall {
                                    id: call_id.as_deref().unwrap_or(""),
                                    name: name.as_deref().unwrap_or(""),
                                },
                            }),
                        ));

                        self.open_calls.push(OpenCallStep {
                            call_index: *index,
                            step_index,
                            id: call_id.clone().unwrap_or_default(),
                            name: name.clone().unwrap_or_default(),
                        });

                        step_index
                    }
                };

                // Empty argument keep-alives carry no bytes and render no frame.
                if !arguments_delta.is_empty() {
                    frames.push(sse::format_frame(
                        Some(super::EVENT_STEP_DELTA),
                        &format::render_json(&DeltaPayload {
                            step_index,
                            delta: OutDelta::ToolArguments {
                                arguments: arguments_delta.as_str(),
                            },
                        }),
                    ));
                }

                frames
            }

            // The stop carries the complete call — id, name, full arguments, signature — so the
            // done-only path opens its step here and the streamed path closes the step its
            // deltas opened. Receivers replace accumulated state with the stop's step.
            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let mut frames = self.ensure_created();

                let step_index = match self
                    .open_calls
                    .iter()
                    .position(|open| open.call_index == *index)
                {
                    Some(position) => self.open_calls.remove(position).step_index,

                    None => {
                        let step_index = self.next_step_index;

                        self.next_step_index += 1;

                        frames.push(sse::format_frame(
                            Some(super::EVENT_STEP_START),
                            &format::render_json(&StartPayload {
                                step_index,
                                step: StartStep::FunctionCall {
                                    id: call_id.as_str(),
                                    name: name.as_str(),
                                },
                            }),
                        ));

                        step_index
                    }
                };

                let step = RenderStep::Typed(super::TypedStep::FunctionCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    signature: thought_signature.clone(),
                });

                frames.push(stop_frame(step_index, &step));

                frames
            }

            IrEvent::Finish(reason) => {
                let mut frames = self.ensure_created();

                self.finish = Some(*reason);

                // The finish closes the open text step: its stop carries the complete text the
                // deltas already delivered. An open reasoning block without its done is closed
                // defensively at completion; conformant streams always pair them.
                self.close_text_step(&mut frames);

                frames
            }

            IrEvent::Usage(usage) => {
                // Usage rides the terminal complete frame; the latest observation wins.
                self.usage = Some(*usage);

                self.ensure_created()
            }

            IrEvent::Done => self.complete(),
        }
    }

    /// Renders a mid-stream error as a terminal data frame carrying the Gemini error envelope,
    /// classified once through the shared API error.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let (_, envelope) = crate::format::error_gemini::render(error);

        sse::format_data_frame(&format::render_json(&envelope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        format::{ParsedRequest, RouteContext, gemini_interaction::request_parse::parse_request},
        ir::{InvalidRequest, IrFinishReason, IrPart, IrRole},
    };

    /// Parses one request body given as JSON with an empty route context.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes(), &RouteContext::empty())
    }

    /// The aggregated response renders the Interaction resource: thought steps before the text
    /// step before function calls, same-family opaques replayed verbatim, the finish mapped onto
    /// the status vocabulary, and usage split back into output and thought members.
    ///
    /// # Panics
    ///
    /// Panics if the rendered resource diverges from the contract.
    #[test]
    fn renders_the_aggregated_interaction_resource() {
        use crate::ir::{IrAggregatedToolCall, IrReasoningBlock};

        let stored = serde_json::json!({
            "type": "thought",
            "id": "th_1",
            "summary": "stored",
            "signature": "sig-1",
        });

        let aggregation = IrAggregation {
            text: "hello".to_string(),
            reasoning: "storedfresh".to_string(),
            reasoning_blocks: vec![
                IrReasoningBlock {
                    text: "stored".to_string(),
                    signature: Some("sig-1".to_string()),
                    opaque: stored.clone(),
                },
                IrReasoningBlock::text_only("fresh".to_string()),
            ],
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"hi"}"#.to_string(),
                thought_signature: Some("sig-call".to_string()),
            }],
            finish: Some(IrFinishReason::ToolCall),
            usage: Some(IrUsage {
                input_tokens: 10,
                output_tokens: 28,
                cached_tokens: Some(3),
                cache_write_tokens: None,
                reasoning_tokens: Some(8),
            }),
        };

        #[expect(
            clippy::expect_used,
            reason = "the rendered resource always serializes"
        )]
        let rendered: Value =
            serde_json::to_value(render_json_response("public-model", &aggregation))
                .expect("the aggregated resource serializes");

        assert_eq!(
            rendered.pointer("/model"),
            Some(&serde_json::json!("public-model"))
        );
        assert_eq!(
            rendered.pointer("/status"),
            Some(&serde_json::json!(STATUS_REQUIRES_ACTION))
        );

        assert_eq!(rendered.pointer("/steps/0"), Some(&stored));
        assert_eq!(
            rendered.pointer("/steps/1/type"),
            Some(&serde_json::json!("thought"))
        );
        assert_eq!(
            rendered.pointer("/steps/1/summary"),
            Some(&serde_json::json!("fresh"))
        );
        assert_eq!(
            rendered.pointer("/steps/2"),
            Some(&serde_json::json!({"type": "output_text", "text": "hello"})),
        );
        assert_eq!(
            rendered.pointer("/steps/3"),
            Some(&serde_json::json!({
                "type": "function_call",
                "id": "call_1",
                "name": "search",
                "arguments": r#"{"q":"hi"}"#,
                "signature": "sig-call",
            })),
        );
        assert!(
            rendered.pointer("/steps/4").is_none(),
            "the resource renders exactly four steps",
        );

        assert_eq!(
            rendered.pointer("/usage"),
            Some(&serde_json::json!({
                "total_input_tokens": 10,
                "total_output_tokens": 20,
                "total_thought_tokens": 8,
                "total_cached_tokens": 3,
            })),
        );
    }

    /// The gateway's own rendered steps parse back verbatim: replaying the aggregated resource's
    /// steps as the next request's input — the multi-turn loop — must parse (the history-replay
    /// carve-out). The replayed thought keeps its opaque, the call keeps its signature.
    ///
    /// # Panics
    ///
    /// Panics if the replay diverges from the rendered content.
    #[test]
    fn rendered_steps_replay_verbatim() {
        use crate::ir::{IrAggregatedToolCall, IrReasoningBlock};

        let stored = serde_json::json!({
            "type": "thought",
            "id": "th_1",
            "summary": "stored",
            "signature": "sig-1",
        });

        let aggregation = IrAggregation {
            text: "hello".to_string(),
            reasoning: "stored".to_string(),
            reasoning_blocks: vec![IrReasoningBlock {
                text: "stored".to_string(),
                signature: Some("sig-1".to_string()),
                opaque: stored.clone(),
            }],
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"hi"}"#.to_string(),
                thought_signature: Some("sig-call".to_string()),
            }],
            finish: Some(IrFinishReason::ToolCall),
            usage: None,
        };

        #[expect(
            clippy::expect_used,
            reason = "the rendered resource always serializes"
        )]
        let rendered: Value =
            serde_json::to_value(render_json_response("public-model", &aggregation))
                .expect("the aggregated resource serializes");

        #[expect(clippy::expect_used, reason = "the resource carries a step array")]
        let steps = rendered
            .pointer("/steps")
            .expect("the aggregated resource carries steps")
            .clone();

        #[expect(
            clippy::expect_used,
            reason = "the replayed steps are constructed to parse"
        )]
        let replayed = parse(serde_json::json!({"model": "public-model", "input": steps}))
            .expect("the gateway's own rendered steps must parse back");

        assert_eq!(replayed.ir.messages.len(), 1);

        #[expect(
            clippy::expect_used,
            reason = "the replay carries one assistant message"
        )]
        let message = replayed
            .ir
            .messages
            .first()
            .expect("the replay carries one assistant message");

        assert_eq!(message.role, IrRole::Assistant);
        assert_eq!(message.parts.len(), 3);

        let mut saw_thought = false;
        let mut saw_call = false;
        let mut saw_text = false;
        let mut saw_unexpected = false;

        for part in &message.parts {
            match part {
                IrPart::Reasoning {
                    text,
                    signature,
                    opaque,
                } => {
                    saw_thought = true;
                    assert_eq!(text, "stored");
                    assert_eq!(signature, &Some("sig-1".to_string()));
                    assert_eq!(opaque, &stored);
                }

                IrPart::ToolCall {
                    call_id,
                    name,
                    thought_signature,
                    ..
                } => {
                    saw_call = true;
                    assert_eq!(call_id, "call_1");
                    assert_eq!(name, "search");
                    assert_eq!(thought_signature, &Some("sig-call".to_string()));
                }

                IrPart::Text { text } => {
                    saw_text = true;
                    assert_eq!(text, "hello");
                }

                // Images and tool outputs never appear in a replayed assistant turn.
                IrPart::Image { .. } | IrPart::ToolOutput { .. } => {
                    saw_unexpected = true;
                }
            }
        }

        assert!(
            !saw_unexpected,
            "the replay carries no image or tool-output parts"
        );
        assert!(
            saw_text && saw_thought && saw_call,
            "the replay keeps text, thought, and call"
        );
    }

    /// The streaming renderer speaks the family's event vocabulary end to end: created opens the
    /// stream, text and thought deltas stream under opened steps, an empty-text done closes the
    /// thought with its signature, call deltas assemble and the done closes the call, and the
    /// terminal complete frame carries the mapped status and usage exactly once.
    ///
    /// # Panics
    ///
    /// Panics if the frame sequence diverges from the vocabulary.
    #[test]
    fn renders_the_streaming_vocabulary() {
        use crate::ir::IrEvent;

        let mut renderer = SseRenderer::new("public-model".to_string());

        let mut frames = renderer.render(&IrEvent::MessageStart {
            id: "upstream-id".to_string(),
            model: "upstream-model".to_string(),
        });

        frames.extend(renderer.render(&IrEvent::TextDelta("hi".to_string())));
        frames.extend(renderer.render(&IrEvent::ReasoningDelta("hmm".to_string())));
        frames.extend(renderer.render(&IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("sig-t".to_string()),
            opaque: serde_json::json!({
                "type": "thought",
                "id": "th_0",
                "summary": "hmm",
                "signature": "sig-t",
            }),
        }));
        frames.extend(renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("search".to_string()),
            arguments_delta: r##"{"q":"##.to_string(),
        }));
        frames.extend(renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"hi"}"#.to_string(),
            thought_signature: None,
        }));
        frames.extend(renderer.render(&IrEvent::Finish(IrFinishReason::Stop)));
        frames.extend(renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 5,
            output_tokens: 6,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        })));
        frames.extend(renderer.render(&IrEvent::Done));

        let events: Vec<&str> = frames
            .iter()
            .filter_map(|frame| {
                frame
                    .strip_prefix("event: ")
                    .and_then(|rest| rest.split('\n').next())
            })
            .collect();

        assert_eq!(
            events,
            vec![
                super::super::EVENT_CREATED,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_COMPLETE,
            ],
        );

        #[expect(
            clippy::expect_used,
            reason = "the stream ends with the complete frame"
        )]
        let complete = frames
            .last()
            .expect("the stream ends with the complete frame");

        assert!(complete.contains(STATUS_COMPLETED));
        assert!(complete.contains("total_input_tokens"));
    }

    /// A done carrying the full text with no prior deltas still delivers its block: the renderer
    /// opens the step, streams the text, and closes it — the done-only path mirrors the
    /// delta-then-empty-done path.
    ///
    /// # Panics
    ///
    /// Panics if the done-only block loses its text.
    #[test]
    fn renders_a_done_only_reasoning_block() {
        use crate::ir::IrEvent;

        let mut renderer = SseRenderer::new("public-model".to_string());

        renderer.render(&IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        });

        let frames = renderer.render(&IrEvent::ReasoningDone {
            text: "whole".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        let joined = frames.join("");

        assert!(joined.contains("thought_summary"));
        assert!(joined.contains("whole"));
        assert!(joined.contains(super::super::EVENT_STEP_STOP));
    }

    /// The mid-stream error frame carries the Gemini error envelope in a data-only frame.
    ///
    /// # Panics
    ///
    /// Panics if the frame diverges from the Gemini error shape.
    #[test]
    fn renders_error_frames_as_gemini_envelopes() {
        let mut renderer = SseRenderer::new("public-model".to_string());

        let frame = renderer.error_frame(&crate::api::Error::InvalidApiKey);

        assert!(frame.starts_with("data: "));

        #[expect(
            clippy::expect_used,
            reason = "the error frame is constructed to parse"
        )]
        let payload: Value = serde_json::from_str(
            frame
                .strip_prefix("data: ")
                .expect("the error frame is a data frame")
                .trim(),
        )
        .expect("the error frame carries JSON");

        // The official Status shape (google.aip.dev/193): a numeric HTTP `code` beside the
        // canonical google.rpc.Code-style `status` name.
        assert_eq!(
            payload.pointer("/error/code"),
            Some(&serde_json::json!(401))
        );
        assert_eq!(
            payload.pointer("/error/status"),
            Some(&serde_json::json!("UNAUTHENTICATED"))
        );
    }
}
