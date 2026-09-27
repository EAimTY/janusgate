//! Client-response rendering for the Gemini Interactions codec: renders IR events as native
//! Interactions SSE frames or the aggregated non-streaming Interaction resource, speaking the
//! official wire vocabulary the parent module pins.
//!
//! # Surface contract
//!
//! - Every streamed frame carries its event name twice: the SSE `event:` line and the official
//!   `event_type` JSON member, so receivers may discriminate either way. The events are
//!   `interaction.created` (opening, carrying the partial Interaction resource under the
//!   `interaction` member with `status: "in_progress"`), `step.start` (`index`, `step`),
//!   `step.delta` (`index`, `delta`), `step.stop` (`index` — the official stop carries no step, so
//!   the streamed deltas are the sole content), and the terminal `interaction.completed` carrying
//!   the terminal partial resource: the mapped status and the usage when one was reported.
//!   `interaction.status_update` is never emitted: the gateway streams no intermediate statuses.
//! - Tool-call completions reconcile with their streamed deltas the Chat Completions way: a done
//!   for an open call streams the argument bytes its deltas did not already deliver (none when they
//!   delivered all of them), and a done for a never-opened call opens the step and streams the
//!   complete arguments as one delta.

use super::{
    RenderStep, STATUS_COMPLETED, STATUS_IN_PROGRESS, STATUS_INCOMPLETE, STATUS_REQUIRES_ACTION,
};
use crate::{
    format::{self, ClientSseRenderer, gemini::split_gemini_usage},
    ids,
    ir::{IrAggregation, IrFinishReason, IrUsage},
    sse,
};
use serde::Serialize;
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

    /// Response steps: thought steps, then the output step, then function calls.
    steps: Vec<RenderStep>,

    /// Token accounting, absent when the upstream reported no usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OutUsage>,
}

/// Token accounting in the official Interactions usage shape: the inverse of the upstream usage
/// mapping — the IR output total (which includes reasoning) splits back into the output and
/// thought members, so a same-family round-trip restores the reported numbers.
#[derive(Debug, Serialize)]
struct OutUsage {
    /// Tokens consumed by the input.
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    total_output_tokens: u64,

    /// Total tokens of the interaction: the sum of the input, output, and thought members —
    /// the official member's documented meaning (`ai.google.dev/api/interactions-api`).
    total_tokens: u64,

    /// Tokens spent on model reasoning, omitted when the upstream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache, omitted when the upstream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cached_tokens: Option<u64>,
}

impl OutUsage {
    /// Splits an IR usage report back into the Interactions members through the shared Gemini
    /// split, the total rendered as the sum of the members the gateway emits.
    fn from_usage(usage: &IrUsage) -> Self {
        let (input, output, thought, cached) = split_gemini_usage(usage);

        Self {
            total_input_tokens: input,
            total_output_tokens: output,
            total_tokens: input + output + thought.unwrap_or(0),
            total_thought_tokens: thought,
            total_cached_tokens: cached,
        }
    }
}

/// The partial Interaction resource the created and completed frames carry under the official
/// `interaction` member. Streaming lifecycle payloads may omit any field of the full resource, so
/// the created frame rides its `model` and the completed frame its `usage`.
#[derive(Debug, Serialize)]
struct InteractionRef<'a> {
    /// Renderer-minted interaction identifier.
    id: &'a str,

    /// Public routing id the client addressed; the created frame only.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,

    /// The interaction status: `in_progress` while streaming, the terminal mapping on the
    /// completed frame.
    status: &'static str,

    /// Token accounting; the completed frame only.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OutUsage>,
}

/// Renders an aggregated completion as the non-streaming Interaction resource. Steps render in the
/// one order the aggregation can guarantee — thought steps in block order, then the output step,
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
        steps.push(RenderStep::Typed(super::TypedStep::ModelOutput {
            content: vec![super::ContentBlock::Text {
                text: aggregation.text.clone(),
            }],
        }));
    }

    for call in &aggregation.tool_calls {
        // The official `function_call` step carries no signature member (README §5.7), so a
        // tool call carrying one — a GenerateContent upstream reports the replayable
        // signature on the `functionCall` part — synthesizes the signature-bearing thought
        // step beside its call, the step the official wire designates for replayable
        // signatures. `summary` is optional on the official ThoughtStep
        // (`ai.google.dev/api/interactions-api`), so the honest minimal form carries the
        // signature alone.
        if let Some(signature) = &call.thought_signature {
            steps.push(RenderStep::Typed(super::TypedStep::Thought {
                summary: Vec::new(),
                signature: Some(signature.clone()),
            }));
        }

        steps.push(RenderStep::Typed(super::TypedStep::FunctionCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            // The degraded remainder carries no inventable shape: a non-object arguments text
            // renders the empty object, never failing a healthy stream.
            arguments: Value::Object(format::arguments_to_object_or_empty(&call.arguments)),
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

/// Streaming renderer emitting the official event vocabulary with `event:` lines and `event_type`
/// members.
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

    /// Open model output step, accumulating its text.
    open_output: Option<OpenOutputStep>,

    /// Open model reasoning block, accumulating its summary.
    open_thought: Option<OpenThoughtStep>,

    /// Open model tool invocations by call index.
    open_calls: Vec<OpenCallStep>,

    /// Terminal finish buffered for the completed frame.
    finish: Option<IrFinishReason>,

    /// Terminal usage buffered for the completed frame.
    usage: Option<IrUsage>,

    /// Whether the completed frame was emitted; the gateway sends `Done` exactly once, so this is
    /// a defensive guard, not a second terminal path.
    is_completed: bool,
}

/// An open model output step of the client stream.
#[derive(Debug)]
struct OpenOutputStep {
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

    /// Argument bytes streamed so far.
    arguments: String,
}

/// The `interaction.created` frame payload.
#[derive(Debug, Serialize)]
struct CreatedPayload<'a> {
    /// The official event discriminator, emitted beside the SSE `event:` line.
    event_type: &'static str,

    /// The partial interaction resource.
    interaction: InteractionRef<'a>,
}

/// The `step.start` frame payload.
#[derive(Debug, Serialize)]
struct StartPayload<'a> {
    /// The official event discriminator, emitted beside the SSE `event:` line.
    event_type: &'static str,

    /// Step index the frame opens.
    index: u64,

    /// The opening shape of the step.
    step: StartStep<'a>,
}

/// The opening shape of a streamed step: its kind, with the call's id and name when already
/// known. How partial the official opening object may be is unpinned, so the thought and output
/// openings carry their kind alone.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum StartStep<'a> {
    /// An opening output step.
    #[serde(rename = "model_output")]
    ModelOutput {},

    /// An opening reasoning block.
    #[serde(rename = "thought")]
    Thought {},

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
    /// The official event discriminator, emitted beside the SSE `event:` line.
    event_type: &'static str,

    /// Step index the delta belongs to.
    index: u64,

    /// The incremental delta.
    delta: OutDelta<'a>,
}

/// One incremental delta of the client stream, tagged by its official kind.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum OutDelta<'a> {
    /// Model text bytes.
    #[serde(rename = "text")]
    Text {
        /// The text bytes.
        text: &'a str,
    },

    /// Reasoning summary content; the official delta carries one content block.
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        /// The summary content block.
        content: SummaryContent<'a>,
    },

    /// The signature terminating a reasoning block.
    #[serde(rename = "thought_signature")]
    ThoughtSignature {
        /// The signature text.
        signature: &'a str,
    },

    /// Tool argument bytes.
    #[serde(rename = "arguments_delta")]
    ArgumentsDelta {
        /// The argument bytes.
        arguments: &'a str,
    },
}

/// The official summary content block: the gateway emits text blocks only.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum SummaryContent<'a> {
    /// A text block.
    #[serde(rename = "text")]
    Text {
        /// The summary bytes.
        text: &'a str,
    },
}

/// The `step.stop` frame payload: officially the index alone — the streamed deltas are the sole
/// content, so receivers append them under the opened step.
#[derive(Debug, Serialize)]
struct StopPayload {
    /// The official event discriminator, emitted beside the SSE `event:` line.
    event_type: &'static str,

    /// Step index the frame closes.
    index: u64,
}

/// The `interaction.completed` frame payload.
#[derive(Debug, Serialize)]
struct CompletedPayload<'a> {
    /// The official event discriminator, emitted beside the SSE `event:` line.
    event_type: &'static str,

    /// The terminal partial interaction resource.
    interaction: InteractionRef<'a>,
}

impl SseRenderer {
    /// Boxes a renderer for one parsed request over the public routing id the client addressed.
    pub(super) fn new(public_id: String) -> Self {
        Self {
            public_id,
            interaction_id: format!("int_{}", ids::new_uuid()),
            next_step_index: 0,
            is_created: false,
            open_output: None,
            open_thought: None,
            open_calls: Vec::new(),
            finish: None,
            usage: None,
            is_completed: false,
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
                event_type: super::EVENT_CREATED,
                interaction: InteractionRef {
                    id: &self.interaction_id,
                    model: Some(&self.public_id),
                    status: STATUS_IN_PROGRESS,
                    usage: None,
                },
            }),
        )]
    }

    /// Allocates the next minted step index.
    fn next_index(&mut self) -> u64 {
        let index = self.next_step_index;

        self.next_step_index += 1;

        index
    }

    /// Opens the output step when none is open, returning its index.
    fn open_output_step(&mut self, frames: &mut Vec<String>) -> u64 {
        if let Some(open) = &self.open_output {
            return open.index;
        }

        let index = self.next_index();

        self.open_output = Some(OpenOutputStep {
            index,
            text: String::new(),
        });

        frames.push(start_frame(index, StartStep::ModelOutput {}));

        index
    }

    /// Opens the reasoning block when none is open, returning its index.
    fn open_thought_step(&mut self, frames: &mut Vec<String>) -> u64 {
        if let Some(open) = &self.open_thought {
            return open.index;
        }

        let index = self.next_index();

        frames.push(start_frame(index, StartStep::Thought {}));

        self.open_thought = Some(OpenThoughtStep {
            index,
            summary: String::new(),
        });

        index
    }

    /// Closes the open output step, when one is open.
    fn close_output_step(&mut self, frames: &mut Vec<String>) {
        if let Some(open) = self.open_output.take() {
            frames.push(stop_frame(open.index));
        }
    }

    /// Closes every open step and emits the terminal completed frame exactly once.
    fn complete(&mut self) -> Vec<String> {
        if self.is_completed {
            return Vec::new();
        }

        self.is_completed = true;

        let mut frames = self.ensure_created();

        self.close_output_step(&mut frames);

        if let Some(open) = self.open_thought.take() {
            // Conformant streams pair a thought block's deltas with its done; this defensive
            // close streams the buffered summary so a closed block never loses its text.
            if !open.summary.is_empty() {
                frames.push(delta_frame(
                    open.index,
                    OutDelta::ThoughtSummary {
                        content: SummaryContent::Text {
                            text: open.summary.as_str(),
                        },
                    },
                ));
            }

            frames.push(stop_frame(open.index));
        }

        for open in std::mem::take(&mut self.open_calls) {
            frames.push(stop_frame(open.step_index));
        }

        frames.push(sse::format_frame(
            Some(super::EVENT_COMPLETED),
            &format::render_json(&CompletedPayload {
                event_type: super::EVENT_COMPLETED,
                interaction: InteractionRef {
                    id: &self.interaction_id,
                    model: None,
                    status: finish_status(self.finish),
                    usage: self.usage.as_ref().map(OutUsage::from_usage),
                },
            }),
        ));

        frames
    }
}

/// Renders a `step.start` frame for one opening step: a free function because it borrows no
/// renderer state — the payload serializes immediately into the owned frame string.
fn start_frame(index: u64, step: StartStep) -> String {
    sse::format_frame(
        Some(super::EVENT_STEP_START),
        &format::render_json(&StartPayload {
            event_type: super::EVENT_STEP_START,
            index,
            step,
        }),
    )
}

/// Renders a `step.delta` frame for one incremental delta.
fn delta_frame<'a>(index: u64, delta: OutDelta<'a>) -> String {
    sse::format_frame(
        Some(super::EVENT_STEP_DELTA),
        &format::render_json(&DeltaPayload {
            event_type: super::EVENT_STEP_DELTA,
            index,
            delta,
        }),
    )
}

/// Renders a `step.stop` frame for one closed step.
fn stop_frame(index: u64) -> String {
    sse::format_frame(
        Some(super::EVENT_STEP_STOP),
        &format::render_json(&StopPayload {
            event_type: super::EVENT_STEP_STOP,
            index,
        }),
    )
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more native SSE frames.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        use crate::ir::IrEvent;

        match event {
            IrEvent::MessageStart { .. } => self.ensure_created(),

            IrEvent::TextDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_output_step(&mut frames);

                if let Some(open) = &mut self.open_output {
                    open.text.push_str(delta);
                }

                frames.push(delta_frame(index, OutDelta::Text { text: delta }));

                frames
            }

            IrEvent::ReasoningDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_thought_step(&mut frames);

                if let Some(open) = &mut self.open_thought {
                    open.summary.push_str(delta);
                }

                frames.push(delta_frame(
                    index,
                    OutDelta::ThoughtSummary {
                        content: SummaryContent::Text { text: delta },
                    },
                ));

                frames
            }

            // A done carrying text opens its block when no deltas preceded it; an empty-text
            // done only closes the block the deltas already delivered. Either way the stop
            // carries the index alone: the streamed deltas are the content.
            IrEvent::ReasoningDone {
                text,
                signature,
                opaque: _,
            } => {
                let mut frames = self.ensure_created();

                match self.open_thought.take() {
                    Some(open) => {
                        if let Some(signature) = signature {
                            frames.push(delta_frame(
                                open.index,
                                OutDelta::ThoughtSignature { signature },
                            ));
                        }

                        frames.push(stop_frame(open.index));
                    }

                    // A done carrying the full text with no prior deltas reopens the block, so the
                    // text rides the stream before the stop closes it; a signature-only done
                    // reopens it too, the signature being the replayable state that must reach the
                    // stream even when the block carries no summary text.
                    None if !text.is_empty() || signature.is_some() => {
                        let index = self.open_thought_step(&mut frames);

                        if !text.is_empty() {
                            frames.push(delta_frame(
                                index,
                                OutDelta::ThoughtSummary {
                                    content: SummaryContent::Text { text },
                                },
                            ));
                        }

                        if let Some(signature) = signature {
                            frames
                                .push(delta_frame(index, OutDelta::ThoughtSignature { signature }));
                        }

                        self.open_thought = None;

                        frames.push(stop_frame(index));
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
                        let step_index = self.next_index();

                        frames.push(start_frame(
                            step_index,
                            StartStep::FunctionCall {
                                id: call_id.as_deref().unwrap_or(""),
                                name: name.as_deref().unwrap_or(""),
                            },
                        ));

                        self.open_calls.push(OpenCallStep {
                            call_index: *index,
                            step_index,
                            id: call_id.clone().unwrap_or_default(),
                            name: name.clone().unwrap_or_default(),
                            arguments: String::new(),
                        });

                        step_index
                    }
                };

                // Empty argument keep-alives carry no bytes and render no frame.
                if !arguments_delta.is_empty() {
                    if let Some(open) = self
                        .open_calls
                        .iter_mut()
                        .find(|open| open.call_index == *index)
                    {
                        open.arguments.push_str(arguments_delta);
                    }

                    frames.push(delta_frame(
                        step_index,
                        OutDelta::ArgumentsDelta {
                            arguments: arguments_delta,
                        },
                    ));
                }

                frames
            }

            // The done reconciles with the streamed deltas the Chat Completions way: a done for
            // an open call streams the residual argument bytes its deltas did not deliver (a
            // divergent done streams nothing — the streamed bytes stand), and a done-only call
            // opens its step and streams the complete arguments as one delta. Either way the stop
            // closes the call.
            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let mut frames = self.ensure_created();

                // The official function_call step carries no signature member (README §5.7),
                // so a signature-bearing done streams the signature-only thought step ahead of
                // the call's frames — the step the official wire designates for replayable
                // signatures, opened and closed around the signature delta just as a streamed
                // thought step closes.
                if let Some(signature) = thought_signature {
                    let thought_index = self.next_index();

                    frames.push(start_frame(thought_index, StartStep::Thought {}));
                    frames.push(delta_frame(
                        thought_index,
                        OutDelta::ThoughtSignature { signature },
                    ));
                    frames.push(stop_frame(thought_index));
                }

                match self
                    .open_calls
                    .iter()
                    .position(|open| open.call_index == *index)
                {
                    Some(position) => {
                        let open = self.open_calls.remove(position);

                        // The prefix match is what proves the residual slice in bounds.
                        if arguments.starts_with(open.arguments.as_str()) {
                            let residual = &arguments[open.arguments.len()..];

                            if !residual.is_empty() {
                                frames.push(delta_frame(
                                    open.step_index,
                                    OutDelta::ArgumentsDelta {
                                        arguments: residual,
                                    },
                                ));
                            }
                        }

                        frames.push(stop_frame(open.step_index));
                    }

                    None => {
                        let step_index = self.next_index();

                        frames.push(start_frame(
                            step_index,
                            StartStep::FunctionCall {
                                id: call_id.as_str(),
                                name: name.as_str(),
                            },
                        ));

                        if !arguments.is_empty() {
                            frames.push(delta_frame(
                                step_index,
                                OutDelta::ArgumentsDelta { arguments },
                            ));
                        }

                        frames.push(stop_frame(step_index));
                    }
                }

                frames
            }

            IrEvent::Finish { reason, .. } => {
                let mut frames = self.ensure_created();

                self.finish = Some(*reason);

                // The finish closes the open output step. An open reasoning block without its
                // done is closed defensively at completion; conformant streams always pair them.
                self.close_output_step(&mut frames);

                frames
            }

            IrEvent::Usage(usage) => {
                // Usage rides the terminal completed frame; the latest observation wins.
                self.usage = Some(*usage);

                self.ensure_created()
            }

            IrEvent::Done => self.complete(),
        }
    }

    /// Renders a mid-stream error as a terminal data frame carrying the Gemini error envelope,
    /// classified once through the shared API error.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let (_, envelope) = crate::format::gemini::error::render(error);

        sse::format_data_frame(&format::render_json(&envelope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        format::{ParsedRequest, RouteContext, gemini::interaction::request_parse::parse_request},
        ir::{InvalidRequest, IrFinishReason, IrPart, IrRole},
    };

    /// Parses one request body given as JSON with an empty route context.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes(), &RouteContext::empty())
    }

    /// The aggregated response renders the Interaction resource: thought steps before the output
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
            "summary": [{"type": "text", "text": "stored"}],
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
            prompt_block_reason: None,
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
            rendered.pointer("/steps/1"),
            Some(&serde_json::json!({
                "type": "thought",
                "summary": [{"type": "text", "text": "fresh"}],
            })),
        );
        assert_eq!(
            rendered.pointer("/steps/2"),
            Some(&serde_json::json!({
                "type": "model_output",
                "content": [{"type": "text", "text": "hello"}],
            })),
        );
        // The official function_call step carries the arguments object and no signature member:
        // the call's signature renders as the signature-only thought step beside it — the
        // step the official wire designates for replayable signatures (summary optional).
        assert_eq!(
            rendered.pointer("/steps/3"),
            Some(&serde_json::json!({
                "type": "thought",
                "signature": "sig-call",
            })),
        );
        assert_eq!(
            rendered.pointer("/steps/4"),
            Some(&serde_json::json!({
                "type": "function_call",
                "id": "call_1",
                "name": "search",
                "arguments": {"q": "hi"},
            })),
        );
        assert!(
            rendered.pointer("/steps/5").is_none(),
            "the resource renders exactly five steps",
        );

        assert_eq!(
            rendered.pointer("/usage"),
            Some(&serde_json::json!({
                "total_input_tokens": 10,
                "total_output_tokens": 20,
                "total_thought_tokens": 8,
                "total_cached_tokens": 3,
                "total_tokens": 38,
            })),
        );
    }

    /// The gateway's own rendered steps parse back verbatim: replaying the aggregated resource's
    /// steps as the next request's input — the multi-turn loop — must parse (the history-replay
    /// carve-out). The replayed thought keeps its opaque, the call keeps its arguments object.
    ///
    /// # Panics
    ///
    /// Panics if the replay diverges from the rendered content.
    #[test]
    fn rendered_steps_replay_verbatim() {
        use crate::ir::{IrAggregatedToolCall, IrReasoningBlock};

        let stored = serde_json::json!({
            "type": "thought",
            "summary": [{"type": "text", "text": "stored"}],
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
                thought_signature: None,
            }],
            finish: Some(IrFinishReason::ToolCall),
            prompt_block_reason: None,
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
                    arguments,
                    thought_signature,
                } => {
                    saw_call = true;
                    assert_eq!(call_id, "call_1");
                    assert_eq!(name, "search");
                    assert_eq!(arguments, r#"{"q":"hi"}"#);
                    assert_eq!(thought_signature, &None);
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
            "the replay keeps text, thought, and call",
        );
    }

    /// Extracts each frame's event name from its `event:` line.
    fn event_names(frames: &[String]) -> Vec<&str> {
        frames
            .iter()
            .filter_map(|frame| {
                frame
                    .strip_prefix("event: ")
                    .and_then(|rest| rest.split('\n').next())
            })
            .collect()
    }

    /// The streaming renderer speaks the official event vocabulary end to end: created opens the
    /// stream under the `interaction` member, text and thought deltas stream under opened steps,
    /// the done closes the thought with its signature delta, call deltas assemble and the done
    /// closes the call, and the terminal completed frame carries the mapped status and usage —
    /// every frame self-described by both the `event:` line and the `event_type` member.
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
                "summary": [{"type": "text", "text": "hmm"}],
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
        frames.extend(renderer.render(&IrEvent::finish(IrFinishReason::Stop)));
        frames.extend(renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 5,
            output_tokens: 6,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        })));
        frames.extend(renderer.render(&IrEvent::Done));

        assert_eq!(
            event_names(&frames),
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
                // The done reconciles the residual bytes its delta did not deliver.
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                // The finish closes the open output step.
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_COMPLETED,
            ],
        );

        // Every frame is self-described: the created frame's payload carries the official
        // `event_type` beside the SSE line, and the completed frame carries the terminal partial
        // resource with the usage.
        for frame in &frames {
            assert!(
                frame.contains("event_type"),
                "every frame carries the official discriminator: {frame}",
            );
        }

        #[expect(
            clippy::expect_used,
            reason = "the stream ends with the completed frame"
        )]
        let completed = frames.last().expect("the stream ends with the frame");

        assert!(completed.contains(STATUS_COMPLETED));
        assert!(completed.contains("total_input_tokens"));
        // The completed frame's usage carries the official total member: the sum of the input,
        // output, and thought members (no thought reported here, so input + output).
        assert!(completed.contains(r#""total_tokens":11"#));

        #[expect(
            clippy::expect_used,
            reason = "the stream opens with the created frame"
        )]
        let created = frames.first().expect("the stream opens with the frame");

        assert!(created.contains(STATUS_IN_PROGRESS));
        assert!(created.contains("public-model"));

        // The delta payloads carry the official members: the summary rides a content block and
        // the arguments ride the arguments_delta kind.
        let joined = frames.join("");

        assert!(joined.contains(r#""delta":{"type":"thought_summary","content":{"type":"text""#));
        assert!(joined.contains(r#""type":"arguments_delta","arguments""#));
    }

    /// A done carrying the full text with no prior deltas still delivers its block: the renderer
    /// opens the step, streams the text, and closes it — the done-only path mirrors the
    /// delta-then-empty-done path. A done-only tool call streams its complete arguments as one
    /// delta, and a divergent done streams no residual bytes.
    ///
    /// # Panics
    ///
    /// Panics if the done-only paths lose content or stream spurious bytes.
    #[test]
    fn renders_done_only_paths_and_reconciles_residuals() {
        use crate::ir::IrEvent;

        let mut renderer = SseRenderer::new("public-model".to_string());

        renderer.render(&IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        });

        let thought = renderer.render(&IrEvent::ReasoningDone {
            text: "whole".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        let joined = thought.join("");

        assert!(joined.contains("thought_summary"));
        assert!(joined.contains("whole"));
        assert!(joined.contains(super::super::EVENT_STEP_STOP));

        let call = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"hi"}"#.to_string(),
            thought_signature: None,
        });

        let joined = call.join("");

        assert!(joined.contains(super::super::EVENT_STEP_START));
        assert!(joined.contains("arguments_delta"));
        assert!(joined.contains("hi"));
        assert!(joined.contains(super::super::EVENT_STEP_STOP));

        // A streamed call whose done carries the same prefix reconciles to a residual delta plus
        // the closing stop.
        renderer.render(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_2".to_string()),
            name: Some("calc".to_string()),
            arguments_delta: r#"{"a":"#.to_string(),
        });

        let reconciled = renderer.render(&IrEvent::ToolCallDone {
            index: 1,
            call_id: "call_2".to_string(),
            name: "calc".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: None,
        });

        assert_eq!(
            event_names(&reconciled),
            vec![
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
            ],
        );
        assert!(
            reconciled.join("").contains("1}"),
            "the residual bytes the deltas did not deliver must stream",
        );
    }

    /// A GenerateContent upstream's signature state crosses to this surface through its
    /// official home: a thinking part's `thoughtSignature` parses into a reasoning block and
    /// a `functionCall` part's `thoughtSignature` rides the IR tool call, and both render as
    /// signature-bearing `thought` steps — the step the official Interactions wire designates
    /// for replayable signatures, its `summary` being optional
    /// (`ai.google.dev/api/interactions-api`) — before the `function_call` step, which
    /// carries no signature member (README §5.7).
    ///
    /// # Panics
    ///
    /// Panics if the rendered steps diverge from the pinned bridge.
    #[test]
    fn generate_content_signatures_render_as_signature_thought_steps() {
        use crate::{
            format::{UpstreamEventParser, gemini::generate_content::UpstreamParser},
            ir::Aggregator,
        };

        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"text": "hmm", "thought": true, "thoughtSignature": "sig-t"},
                    {"functionCall": {"id": "fc-1", "name": "search", "args": {"q": "hi"}},
                     "thoughtSignature": "sig-call"},
                ]},
                "finishReason": "STOP",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the signed upstream body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("the signed upstream body parses");

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        #[expect(
            clippy::expect_used,
            reason = "the rendered resource always serializes"
        )]
        let rendered: Value =
            serde_json::to_value(render_json_response("public-model", &aggregation))
                .expect("the aggregated resource serializes");

        // The thinking part's block renders its summary text beside its signature.
        assert_eq!(
            rendered.pointer("/steps/0"),
            Some(&serde_json::json!({
                "type": "thought",
                "summary": [{"type": "text", "text": "hmm"}],
                "signature": "sig-t",
            })),
        );

        // The call's signature synthesizes the minimal signature-only thought step, the
        // official `summary` being optional.
        assert_eq!(
            rendered.pointer("/steps/1"),
            Some(&serde_json::json!({
                "type": "thought",
                "signature": "sig-call",
            })),
        );

        assert_eq!(
            rendered.pointer("/steps/2"),
            Some(&serde_json::json!({
                "type": "function_call",
                "id": "fc-1",
                "name": "search",
                "arguments": {"q": "hi"},
            })),
        );
        assert!(
            rendered.pointer("/steps/3").is_none(),
            "the resource renders exactly three steps",
        );
    }

    /// The streaming renderer carries a signature-bearing done the same way: a done carrying
    /// a tool-call signature opens the signature-only thought step and closes it ahead of
    /// the call's frames — an open call streams the thought step before its residual and
    /// stop — and a signature-only reasoning done with no open block still streams its
    /// signature, the state the thinking docs require surviving.
    ///
    /// # Panics
    ///
    /// Panics if the frame sequences diverge from the pinned bridge.
    #[test]
    fn signature_bearing_dones_stream_the_thought_step() {
        use crate::ir::IrEvent;

        let mut renderer = SseRenderer::new("public-model".to_string());

        renderer.render(&IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        });

        let done_only = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"hi"}"#.to_string(),
            thought_signature: Some("sig-call".to_string()),
        });

        assert_eq!(
            event_names(&done_only),
            vec![
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
            ],
            "the thought step streams ahead of the call's frames",
        );
        assert!(
            done_only
                .join("")
                .contains(r#""type":"thought_signature","signature":"sig-call""#),
        );

        // A done for a call already opened by deltas streams the thought step, then the
        // residual bytes and the call's stop.
        renderer.render(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_2".to_string()),
            name: Some("calc".to_string()),
            arguments_delta: r#"{"a":"#.to_string(),
        });

        let reconciled = renderer.render(&IrEvent::ToolCallDone {
            index: 1,
            call_id: "call_2".to_string(),
            name: "calc".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
            thought_signature: Some("sig-2".to_string()),
        });

        assert_eq!(
            event_names(&reconciled),
            vec![
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
            ],
            "the thought step streams before the open call's residual and stop",
        );

        // A signature-only reasoning done reopens a thought step to carry its signature.
        let signature_only = renderer.render(&IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("sig-t".to_string()),
            opaque: Value::Null,
        });

        assert_eq!(
            event_names(&signature_only),
            vec![
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
            ],
        );
        assert!(signature_only.join("").contains(r#""signature":"sig-t""#));
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
