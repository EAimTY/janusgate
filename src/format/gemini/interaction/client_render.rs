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
    format::{
        self, ClientSseRenderer,
        gemini::{error_data_frame, split_gemini_usage},
    },
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

impl JsonResponse {
    /// The rendered steps, for the aggregated-body projection.
    pub(super) fn steps(&self) -> &[RenderStep] {
        &self.steps
    }
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
    /// split, the total rendered as the sum of the members the gateway emits, added with
    /// `saturating_add` so a third-party total can never panic (debug) or wrap (release),
    /// mirroring the fold in [`crate::format::gemini`].
    fn from_usage(usage: &IrUsage) -> Self {
        let (input, output, thought, cached) = split_gemini_usage(usage);

        Self {
            total_input_tokens: input,
            total_output_tokens: output,
            total_tokens: input
                .saturating_add(output)
                .saturating_add(thought.unwrap_or(0)),
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
        steps.push(RenderStep::Typed(super::TypedStep::FunctionCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            // The degraded remainder carries no inventable shape: a non-object arguments text
            // renders the empty object, never failing a healthy stream.
            arguments: Value::Object(format::arguments_to_object_or_empty(&call.arguments)),
        }));

        // The official `function_call` step carries no signature member
        // (`docs/design/providers.md`), so a tool call carrying one — a GenerateContent
        // upstream reports the replayable signature on the `functionCall` part —
        // synthesizes the signature-bearing thought step beside its call, the step the
        // official wire designates for replayable signatures. `summary` is optional on the
        // official ThoughtStep (`ai.google.dev/api/interactions-api`), so the honest
        // minimal form carries the signature alone. The step follows the call so the
        // aggregated order matches the streamed transport's first-seen step order — the
        // call opens before its done-carried signature streams — and both transports of
        // one response fingerprint one history (`docs/reference/inference.md`).
        if let Some(signature) = &call.thought_signature {
            steps.push(RenderStep::Typed(super::TypedStep::Thought {
                summary: Vec::new(),
                signature: Some(signature.clone()),
            }));
        }
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
pub(super) struct SseRenderer {
    /// Digest-only projection of actual outward step fields.
    projection: Option<super::affinity::Output>,

    /// Public routing id the client addressed, echoed in the created frame.
    public_id: String,

    /// Renderer-minted interaction identifier.
    interaction_id: String,

    /// Next upstream step index to mint for an opened step.
    next_step_index: u64,

    /// Whether the created frame was emitted.
    is_created: bool,

    /// Open model output position; text is delivered immediately in deltas.
    open_output: Option<OpenOutputStep>,

    /// Open model reasoning position; summary is delivered immediately in deltas.
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
}

/// An open model reasoning block of the client stream.
#[derive(Debug)]
struct OpenThoughtStep {
    /// Minted step index.
    index: u64,
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
            projection: None,
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

        self.open_output = Some(OpenOutputStep { index });

        self.emit_start(frames, index, StartStep::ModelOutput {});

        index
    }

    /// Opens the reasoning block when none is open, returning its index.
    fn open_thought_step(&mut self, frames: &mut Vec<String>) -> u64 {
        if let Some(open) = &self.open_thought {
            return open.index;
        }

        let index = self.next_index();

        self.emit_start(frames, index, StartStep::Thought {});

        self.open_thought = Some(OpenThoughtStep { index });

        index
    }

    /// Closes the open output step, when one is open.
    fn close_output_step(&mut self, frames: &mut Vec<String>) {
        if let Some(open) = self.open_output.take() {
            self.emit_stop(frames, open.index, None);
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
            // The deltas already delivered every summary byte; a terminal close emits only
            // the boundary, including upstreams that expose reasoning without a done event.
            self.emit_stop(&mut frames, open.index, None);
        }

        for open in std::mem::take(&mut self.open_calls) {
            self.emit_stop(&mut frames, open.step_index, None);
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

    /// Renders a `step.start` frame; a pooled projection observes the opening at its typed
    /// render point instead of re-parsing the rendered frame.
    fn emit_start(&mut self, frames: &mut Vec<String>, index: u64, step: StartStep<'_>) {
        if let Some(projection) = self.projection.as_mut() {
            match &step {
                StartStep::ModelOutput {} => projection.start_output(index),
                StartStep::Thought {} => projection.start_thought(index),
                StartStep::FunctionCall { id, name } => projection.start_call(index, id, name),
            }
        }

        frames.push(start_frame(index, step));
    }

    /// Renders a `step.delta` frame; a pooled projection observes the same field bytes at the
    /// typed render point.
    fn emit_delta(&mut self, frames: &mut Vec<String>, index: u64, delta: OutDelta<'_>) {
        if let Some(projection) = self.projection.as_mut() {
            match &delta {
                OutDelta::Text { text } => projection.text_delta(index, text),
                OutDelta::ThoughtSummary {
                    content: SummaryContent::Text { text },
                } => projection.thought_summary_delta(index, text),
                OutDelta::ThoughtSignature { signature } => {
                    projection.thought_signature_delta(index, signature);
                }
                OutDelta::ArgumentsDelta { arguments } => {
                    projection.arguments_delta(index, arguments);
                }
            }
        }

        frames.push(delta_frame(index, delta));
    }

    /// Renders a `step.stop` frame; a pooled projection closes the step, an invocation completing
    /// under the done event's authoritative identity when the closing event carries one.
    fn emit_stop(&mut self, frames: &mut Vec<String>, index: u64, done: Option<(&str, &str)>) {
        frames.push(stop_frame(index));

        if let Some(projection) = self.projection.as_mut() {
            projection.stop(index, done);
        }
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
    fn begin_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.projection = Some(super::affinity::Output::new(projector));
    }

    fn take_projection(&mut self) -> Option<crate::affinity::fingerprint::Projector> {
        self.projection
            .take()
            .and_then(super::affinity::Output::finish)
    }

    /// Renders one IR event into zero or more native SSE frames. The projection observes every
    /// emitted frame's conversation fields at its typed render point, in emission order.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        use crate::ir::IrEvent;

        match event {
            IrEvent::MessageStart { .. } => self.ensure_created(),

            // Native compaction state is unrepresentable on this surface and
            // unreachable in practice: a caller that cannot express compaction
            // controls can never trigger a native compaction upstream.
            IrEvent::CompactionState { .. } => Vec::new(),

            IrEvent::TextDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_output_step(&mut frames);

                self.emit_delta(&mut frames, index, OutDelta::Text { text: delta });

                frames
            }

            IrEvent::ReasoningDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_thought_step(&mut frames);

                self.emit_delta(
                    &mut frames,
                    index,
                    OutDelta::ThoughtSummary {
                        content: SummaryContent::Text { text: delta },
                    },
                );

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
                            self.emit_delta(
                                &mut frames,
                                open.index,
                                OutDelta::ThoughtSignature { signature },
                            );
                        }

                        self.emit_stop(&mut frames, open.index, None);
                    }

                    // A done carrying the full text with no prior deltas reopens the block, so the
                    // text rides the stream before the stop closes it; a signature-only done
                    // reopens it too, the signature being the replayable state that must reach the
                    // stream even when the block carries no summary text.
                    None if !text.is_empty() || signature.is_some() => {
                        let index = self.open_thought_step(&mut frames);

                        if !text.is_empty() {
                            self.emit_delta(
                                &mut frames,
                                index,
                                OutDelta::ThoughtSummary {
                                    content: SummaryContent::Text { text },
                                },
                            );
                        }

                        if let Some(signature) = signature {
                            self.emit_delta(
                                &mut frames,
                                index,
                                OutDelta::ThoughtSignature { signature },
                            );
                        }

                        self.open_thought = None;

                        self.emit_stop(&mut frames, index, None);
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

                        self.emit_start(
                            &mut frames,
                            step_index,
                            StartStep::FunctionCall {
                                id: call_id.as_deref().unwrap_or(""),
                                name: name.as_deref().unwrap_or(""),
                            },
                        );

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

                    self.emit_delta(
                        &mut frames,
                        step_index,
                        OutDelta::ArgumentsDelta {
                            arguments: arguments_delta,
                        },
                    );
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

                // The official function_call step carries no signature member
                // (`docs/design/providers.md`), so a signature-bearing done streams
                // the signature-only thought step ahead of the call's frames — the
                // step the official wire designates for replayable signatures,
                // opened and closed around the signature delta just as a streamed
                // thought step closes.
                if let Some(signature) = thought_signature {
                    let thought_index = self.next_index();

                    self.emit_start(&mut frames, thought_index, StartStep::Thought {});
                    self.emit_delta(
                        &mut frames,
                        thought_index,
                        OutDelta::ThoughtSignature {
                            signature: signature.as_str(),
                        },
                    );
                    self.emit_stop(
                        &mut frames,
                        thought_index,
                        Some((call_id.as_str(), name.as_str())),
                    );
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
                                self.emit_delta(
                                    &mut frames,
                                    open.step_index,
                                    OutDelta::ArgumentsDelta {
                                        arguments: residual,
                                    },
                                );
                            }
                        }

                        self.emit_stop(
                            &mut frames,
                            open.step_index,
                            Some((call_id.as_str(), name.as_str())),
                        );
                    }

                    None => {
                        let step_index = self.next_index();

                        self.emit_start(
                            &mut frames,
                            step_index,
                            StartStep::FunctionCall {
                                id: call_id.as_str(),
                                name: name.as_str(),
                            },
                        );

                        if !arguments.is_empty() {
                            self.emit_delta(
                                &mut frames,
                                step_index,
                                OutDelta::ArgumentsDelta { arguments },
                            );
                        }

                        self.emit_stop(
                            &mut frames,
                            step_index,
                            Some((call_id.as_str(), name.as_str())),
                        );
                    }
                }

                frames
            }

            IrEvent::Finish { reason, .. } => {
                let mut frames = self.ensure_created();

                self.finish = Some(*reason);

                // Completion closes any delta-only thought without replaying delivered bytes.
                self.close_output_step(&mut frames);

                frames
            }

            IrEvent::CompactionMetadata { .. } => Vec::new(),
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
        error_data_frame(error)
    }
}
