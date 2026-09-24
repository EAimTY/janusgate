//! Upstream-event parsing for the Gemini Interactions codec: parses upstream SSE streams or a
//! complete Interaction body into IR events. Streams are discriminated by the `event:` line when
//! the frame carries one, else by the payload's official `event_type` member.

use super::{
    EVENT_COMPLETED, EVENT_CREATED, EVENT_STATUS_UPDATE, EVENT_STEP_DELTA, EVENT_STEP_START,
    EVENT_STEP_STOP, STATUS_BUDGET_EXCEEDED, STATUS_CANCELLED, STATUS_FAILED, STATUS_INCOMPLETE,
    STATUS_REQUIRES_ACTION, STEP_FUNCTION_CALL, STEP_MODEL_OUTPUT, STEP_THOUGHT, TypedStep,
};
use crate::{
    format::{self, PayloadError, UpstreamError, UpstreamEventParser},
    ids,
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Parses upstream Gemini Interactions responses — SSE frames or a full Interaction body — into IR
/// events. Streams are discriminated by the `event:` line when the frame carries one, else by the
/// payload's official `event_type` member.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether a finish was already emitted; a clean end-of-stream without one degrades to
    /// [`IrFinishReason::Stop`] in the finish hook.
    is_finished: bool,

    /// Count of tool calls opened in this response; the stable per-call index source.
    call_counter: usize,

    /// Steps opened by `step.start` (or implicitly by a first delta) and not yet closed by
    /// `step.stop`, keyed by the upstream step index.
    open_steps: Vec<OpenStep>,
}

/// One upstream step tracked across stream frames. The official `step.stop` carries only the index,
/// so a step's closing event is assembled entirely from the state its deltas streamed.
#[derive(Debug)]
enum OpenStep {
    /// An open model output step.
    Output {
        /// Upstream step index.
        index: u64,
    },

    /// An open model reasoning block, accumulating its summary text and signature. The closing
    /// done's opaque replay payload is the synthesized official step — summary blocks beside the
    /// signature — so a same-family replay stays byte-exact.
    Thought {
        /// Upstream step index.
        index: u64,

        /// Summary text streamed so far.
        summary: String,

        /// Signature streamed so far, when the upstream sent one mid-step.
        signature: Option<String>,
    },

    /// An open model tool invocation, accumulating its argument bytes.
    Call {
        /// Upstream step index.
        index: u64,

        /// Stable tool-call index within the response.
        call_index: usize,

        /// Identifier of the invocation.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// Argument bytes streamed so far.
        arguments: String,
    },
}

impl OpenStep {
    /// The upstream step index the tracked step was opened under.
    fn index(&self) -> u64 {
        match self {
            Self::Output { index } | Self::Thought { index, .. } | Self::Call { index, .. } => {
                *index
            }
        }
    }
}

/// One upstream Interaction body or the partial resource an
/// `interaction.created`/`interaction.completed` frame carries under its `interaction` member.
///
/// Every member is optional and unmodeled members are ignored: upstreams are third-party. Steps are
/// raw values — fragments whose per-kind shape the parser models separately — so a thought step's
/// complete JSON survives as the block's opaque replay payload. The streaming lifecycle payloads
/// may omit any field of the full resource ("Streaming lifecycle payloads may omit fields that are
/// only available on full non-streaming Interaction responses"), and the terminal
/// `interaction.completed` resource deliberately shrinks its steps away, so steps are ingested from
/// bodies only.
#[derive(Debug, Default, Deserialize)]
struct InteractionBody {
    /// Upstream interaction identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,

    /// Terminal status string, deciding the finish mapping.
    #[serde(default)]
    status: Option<String>,

    /// The interaction's steps in order; a body member only — the streaming terminal event shrinks
    /// its steps away, and the live step events are the sole streaming content source.
    #[serde(default)]
    steps: Vec<Value>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UsageReport>,

    /// Failure the upstream may embed in a 2xx body.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Minimal probe reading a frame's official `event_type` member and any failure member the
/// upstream may embed, so the one payload parse both discriminates the frame and vetoes failures —
/// recognized event or not.
#[derive(Debug, Deserialize)]
struct FrameProbe {
    /// The official event discriminator; absent, null, or empty reads as no self-description, so
    /// the frame is ignored as a heartbeat unless the `event:` line discriminates, and a
    /// type-invalid value makes the payload malformed.
    #[serde(rename = "event_type", default)]
    event_type: Option<String>,

    /// Failure the upstream attached, absent on success.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Minimal probe reading a step's type tag; the per-kind payloads are modelled separately.
#[derive(Debug, Deserialize)]
struct StepProbe {
    /// Step type tag, empty when the step carries none.
    #[serde(rename = "type", default)]
    step_type: String,
}

/// One content block of a body step, reduced to the members the IR can carry: only text survives.
#[derive(Debug, Default, Deserialize)]
struct BodyBlock {
    /// Block kind tag; non-text blocks carry no IR-representable content.
    #[serde(rename = "type", default)]
    block_type: String,

    /// The text content, empty on non-text blocks.
    #[serde(default)]
    text: String,
}

impl BodyBlock {
    /// The block's text, empty for every non-text block.
    fn into_text(self) -> String {
        if self.block_type == "text" {
            self.text
        } else {
            String::new()
        }
    }
}

/// A `model_output` step of an Interaction body: the content blocks, text ones only.
#[derive(Debug, Default, Deserialize)]
struct ModelOutputStep {
    /// The content blocks; every non-text block is dropped (the IR output channel is text).
    #[serde(default)]
    content: Vec<BodyBlock>,
}

/// A `thought` step of an Interaction body or a complete step value: the summary blocks and the
/// terminating signature. The step identifier is deliberately unmodelled — it rides inside the
/// opaque fragment untouched, so the codec never reads it.
///
/// No unknown-field rejection: upstream payloads are third-party, and every member beyond the
/// modelled ones rides the opaque fragment untouched.
#[derive(Debug, Default, Deserialize)]
struct ThoughtStep {
    /// Summary content blocks; only text blocks carry IR-representable text.
    #[serde(default)]
    summary: Option<Vec<BodyBlock>>,

    /// Upstream signature terminating the block, absent when the upstream sent none.
    #[serde(default)]
    signature: Option<String>,
}

/// A `function_call` step of an Interaction body.
#[derive(Debug, Default, Deserialize)]
struct FunctionCallStep {
    /// Identifier of the invocation, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Name of the invoked function, empty when the upstream omits it.
    #[serde(default)]
    name: String,

    /// The arguments as the official JSON object; the old JSON-encoded-string spelling is a kind
    /// contradiction and fails the payload.
    #[serde(default)]
    arguments: Option<Map<String, Value>>,
}

/// A partial step carried by `step.start`: the opening shape of a step, with only the members an
/// opening needs. How partial the official opening object may be is unpinned, so every member
/// defaults.
#[derive(Debug, Default, Deserialize)]
struct PartialStep {
    /// Step type tag, empty when the frame carries none.
    #[serde(rename = "type", default)]
    step_type: String,

    /// Step identifier of a function-call step.
    #[serde(default)]
    id: Option<String>,

    /// Function name of a function-call step.
    #[serde(default)]
    name: Option<String>,
}

/// One incremental delta carried by `step.delta`, tagged by its official `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StepDelta {
    /// Model text bytes.
    #[serde(rename = "text")]
    Text {
        /// The text bytes.
        text: String,
    },

    /// Reasoning summary content; the official delta's `content` member is optional, so an
    /// absent member degrades to a no-op through the default non-text block.
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        /// The summary content block, absent when the delta carries none.
        #[serde(default)]
        content: DeltaBlock,
    },

    /// The signature terminating a reasoning block; the official delta's `signature` member is
    /// optional, so an absent member is a no-op that never clears an already-streamed signature.
    #[serde(rename = "thought_signature")]
    ThoughtSignature {
        /// The signature text, absent when the delta carries none.
        #[serde(default)]
        signature: Option<String>,
    },

    /// Tool argument bytes.
    #[serde(rename = "arguments_delta")]
    ArgumentsDelta {
        /// The argument bytes, officially optional.
        #[serde(default)]
        arguments: String,
    },

    /// Every other delta kind — media output deltas (`image`, `audio`, `video`, `document`),
    /// `text_annotation_delta`, the server-side tool call/result deltas, and future kinds —
    /// carries no content the IR can represent: parsed and ignored, never aborting a stream of
    /// official events (the family treats ignorable steps the same way).
    #[serde(other)]
    Ignored,
}

/// One content block of a `thought_summary` delta, reduced to the members the IR can carry.
#[derive(Debug, Default, Deserialize)]
struct DeltaBlock {
    /// Block kind tag; non-text blocks carry no IR-representable content.
    #[serde(rename = "type", default)]
    block_type: String,

    /// The text content, empty on non-text blocks.
    #[serde(default)]
    text: String,
}

/// The `step.start` frame payload.
#[derive(Debug, Deserialize)]
struct StepStartFrame {
    /// Upstream step index the frame opens.
    index: u64,

    /// The opening shape of the step.
    #[serde(default)]
    step: PartialStep,
}

/// The `step.delta` frame payload.
#[derive(Debug, Deserialize)]
struct StepDeltaFrame {
    /// Upstream step index the delta belongs to.
    index: u64,

    /// The incremental delta.
    delta: StepDelta,
}

/// The `step.stop` frame payload: officially the index alone. The optional `step_usage` (per-step)
/// and `usage` members are deliberately unmodelled — folding per-step usage into the IR totals
/// would double-count the interaction total that rides `interaction.completed` — and steps closed
/// by a stop are assembled from the state their deltas streamed.
#[derive(Debug, Deserialize)]
struct StepStopFrame {
    /// Upstream step index the frame closes.
    index: u64,
}

/// The `interaction.created` frame payload: the partial resource under the official member.
#[derive(Debug, Deserialize)]
struct CreatedFrame {
    /// The partial interaction resource.
    #[serde(default)]
    interaction: InteractionBody,
}

/// The `interaction.completed` frame payload: the terminal partial resource under the official
/// member.
#[derive(Debug, Deserialize)]
struct CompletedFrame {
    /// The terminal partial interaction resource.
    #[serde(default)]
    interaction: InteractionBody,
}

/// Upstream token accounting in the official Interactions shape; the by-modality breakdowns,
/// `total_tokens`, and `total_tool_use_tokens` are unmodelled and ignored.
#[derive(Debug, Default, Deserialize)]
struct UsageReport {
    /// Tokens consumed by the input.
    #[serde(default)]
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    #[serde(default)]
    total_output_tokens: u64,

    /// Tokens spent on model reasoning.
    #[serde(default)]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache.
    #[serde(default)]
    total_cached_tokens: Option<u64>,
}

impl UsageReport {
    /// Folds the report into IR usage: output includes reasoning (as in the GenerateContent
    /// codec), the reasoning breakdown rides beside it, and there is no cache-write accounting.
    fn into_usage(self) -> IrUsage {
        IrUsage {
            input_tokens: self.total_input_tokens,
            output_tokens: self
                .total_output_tokens
                .saturating_add(self.total_thought_tokens.unwrap_or(0)),
            cached_tokens: self.total_cached_tokens,
            cache_write_tokens: None,
            reasoning_tokens: self.total_thought_tokens,
        }
    }
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Emits [`IrEvent::MessageStart`] exactly once, first, with the given identity.
    fn open_events(&mut self, id: String, model: String) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        vec![IrEvent::MessageStart { id, model }]
    }

    /// Emits [`IrEvent::MessageStart`] with empty identity when no `interaction.created` arrived
    /// before content: every event stream opens with it exactly once.
    fn ensure_open(&mut self) -> Vec<IrEvent> {
        self.open_events(String::new(), String::new())
    }

    /// Removes and returns the step tracked under the upstream index, when one is open.
    fn take_open(&mut self, index: u64) -> Option<OpenStep> {
        self.open_steps
            .iter()
            .position(|open| open.index() == index)
            .map(|position| self.open_steps.swap_remove(position))
    }

    /// Returns the step tracked under the upstream index, when one is open.
    fn find_open(&mut self, index: u64) -> Option<&mut OpenStep> {
        self.open_steps
            .iter_mut()
            .find(|open| open.index() == index)
    }

    /// Ingests one complete Interaction body: message start, one event group per step, then the
    /// terminal status and usage.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for an embedded upstream error object, for steps whose kind
    /// contradicts the contract, and for a failed terminal status.
    fn ingest_body(&mut self, body: InteractionBody) -> Result<Vec<IrEvent>, PayloadError> {
        // An error object inside a 2xx body is a malformed upstream payload: the check sits
        // beside the protocol fields, shared with the other codecs.
        if let Some(error) = &body.error {
            return Err(error.failure());
        }

        let mut events = self.open_events(body.id, body.model);

        for step in body.steps {
            events.extend(self.ingest_complete_step(step)?);
        }

        events.extend(self.ingest_status(body.status)?);

        if let Some(usage) = body.usage {
            events.push(IrEvent::Usage(usage.into_usage()));
        }

        Ok(events)
    }

    /// Ingests one complete step value — an Interaction body member.
    ///
    /// Response steps deliver their content at once: output steps stream as one full delta, thought
    /// steps close as full-text done events with no prior deltas, and function calls open with a
    /// full-arguments delta plus a done — mirroring the Responses JSON path — so streaming
    /// renderers open and complete every item. Steps of other kinds (`user_input`,
    /// `function_result`, and the server-side tool kinds) carry no model output and are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for a step whose kind contradicts the contract.
    fn ingest_complete_step(&mut self, step: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let probe: StepProbe = format::parse_body(step.clone())?;

        match probe.step_type.as_str() {
            STEP_MODEL_OUTPUT => {
                let output: ModelOutputStep = format::parse_body(step)?;
                let text = output
                    .content
                    .into_iter()
                    .map(BodyBlock::into_text)
                    .collect::<String>();

                Ok(vec![IrEvent::TextDelta(text)])
            }

            STEP_THOUGHT => {
                let thought: ThoughtStep = format::parse_body(step.clone())?;
                let text = thought
                    .summary
                    .unwrap_or_default()
                    .into_iter()
                    .map(BodyBlock::into_text)
                    .collect::<String>();

                Ok(vec![IrEvent::ReasoningDone {
                    text,
                    signature: thought.signature,
                    opaque: step,
                }])
            }

            STEP_FUNCTION_CALL => {
                let call: FunctionCallStep = format::parse_body(step)?;
                let index = self.allocate_call_index();

                let arguments = call
                    .arguments
                    .map(format::arguments_object_to_text)
                    .unwrap_or_default();

                Ok(vec![
                    IrEvent::ToolCallDelta {
                        index,
                        call_id: Some(call.id.clone()),
                        name: Some(call.name.clone()),
                        arguments_delta: arguments.clone(),
                    },
                    IrEvent::ToolCallDone {
                        index,
                        call_id: call.id,
                        name: call.name,
                        arguments,
                        thought_signature: None,
                    },
                ])
            }

            // Request-echo (`user_input`, `function_result`), the server-side tool kinds, and any
            // future kind carry no model output.
            _ => Ok(Vec::new()),
        }
    }

    /// Allocates the next stable tool-call index.
    fn allocate_call_index(&mut self) -> usize {
        let index = self.call_counter;

        self.call_counter += 1;

        index
    }

    /// Ingests a terminal status string: the finish mapping, with `failed` rejected as an
    /// upstream failure. A missing status degrades to [`IrFinishReason::Stop`].
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for the failed status.
    fn ingest_status(&mut self, status: Option<String>) -> Result<Vec<IrEvent>, PayloadError> {
        self.is_finished = true;

        match status.as_deref() {
            Some(STATUS_REQUIRES_ACTION) => Ok(vec![IrEvent::Finish(IrFinishReason::ToolCall)]),

            Some(STATUS_INCOMPLETE | STATUS_BUDGET_EXCEEDED) => {
                Ok(vec![IrEvent::Finish(IrFinishReason::Length)])
            }

            // Deliberate: a cancelled interaction ended without failure, and the canonical finish
            // vocabulary has no cancelled member.
            Some(STATUS_CANCELLED) => Ok(vec![IrEvent::Finish(IrFinishReason::Stop)]),

            Some(STATUS_FAILED) => Err(PayloadError::new(
                "upstream interaction failed with status `failed`",
            )),

            // `completed`, the non-terminal `in_progress`/`queued` (which never terminate a
            // conformant stream), and any future status degrade to a clean stop rather than
            // failing a healthy stream.
            _ => Ok(vec![IrEvent::Finish(IrFinishReason::Stop)]),
        }
    }

    /// Ingests a `step.start` frame: records the opened step so later deltas resolve. A repeated
    /// start for an already-open index is ignored; a function-call start records the call but
    /// emits nothing — its deltas and stop carry the content.
    fn ingest_start(&mut self, frame: StepStartFrame) -> Vec<IrEvent> {
        let events = self.ensure_open();

        if self.find_open(frame.index).is_some() {
            return events;
        }

        match frame.step.step_type.as_str() {
            STEP_MODEL_OUTPUT => self
                .open_steps
                .push(OpenStep::Output { index: frame.index }),

            STEP_THOUGHT => self.open_steps.push(OpenStep::Thought {
                index: frame.index,
                summary: String::new(),
                signature: None,
            }),

            STEP_FUNCTION_CALL => {
                let call_index = self.allocate_call_index();

                self.open_steps.push(OpenStep::Call {
                    index: frame.index,
                    call_index,
                    id: frame
                        .step
                        .id
                        .unwrap_or_else(|| format!("call_{}", ids::new_uuid())),
                    name: frame.step.name.unwrap_or_default(),
                    arguments: String::new(),
                });
            }

            // Unknown step kinds open nothing; their deltas fall into the implicit-open rule
            // below only for the known delta kinds, and their stop closes nothing.
            _ => {}
        }

        events
    }

    /// Ingests a `step.delta` frame: the delta kind decides the IR event. A content delta for a
    /// step with no recorded start implicitly opens the matching kind, so a missed start cannot
    /// strand content; an implicit tool call mints its id and allocates its index the same way a
    /// start would.
    fn ingest_delta(&mut self, frame: StepDeltaFrame) -> Vec<IrEvent> {
        let mut events = Vec::new();

        match frame.delta {
            StepDelta::Text { text } => {
                events.extend(self.ensure_open());

                if self.find_open(frame.index).is_none() {
                    self.open_steps
                        .push(OpenStep::Output { index: frame.index });
                }

                events.push(IrEvent::TextDelta(text));
            }

            StepDelta::ThoughtSummary { content } => {
                // A non-text summary block — the default of an absent `content` member included —
                // carries no IR-representable text and is a no-op that opens nothing.
                if content.block_type != "text" {
                    return events;
                }

                events.extend(self.ensure_open());

                if self.find_open(frame.index).is_none() {
                    self.open_steps.push(OpenStep::Thought {
                        index: frame.index,
                        summary: String::new(),
                        signature: None,
                    });
                }

                if let Some(OpenStep::Thought { summary: open, .. }) = self.find_open(frame.index) {
                    open.push_str(&content.text);
                }

                events.push(IrEvent::ReasoningDelta(content.text));
            }

            StepDelta::ThoughtSignature { signature } => {
                // An absent signature member carries nothing: a no-op that must never overwrite
                // or clear a signature already streamed for the step.
                let Some(signature) = signature else {
                    return events;
                };

                events.extend(self.ensure_open());

                if self.find_open(frame.index).is_none() {
                    self.open_steps.push(OpenStep::Thought {
                        index: frame.index,
                        summary: String::new(),
                        signature: None,
                    });
                }

                if let Some(OpenStep::Thought {
                    signature: open, ..
                }) = self.find_open(frame.index)
                {
                    *open = Some(signature);
                }
            }

            StepDelta::ArgumentsDelta { arguments } => {
                events.extend(self.ensure_open());

                if self.find_open(frame.index).is_none() {
                    let call_index = self.allocate_call_index();

                    self.open_steps.push(OpenStep::Call {
                        index: frame.index,
                        call_index,
                        id: format!("call_{}", ids::new_uuid()),
                        name: String::new(),
                        arguments: String::new(),
                    });
                }

                // A delta for a step of another kind contradicts the opened kind, so it is
                // dropped rather than misattributed to foreign content.
                if let Some(OpenStep::Call {
                    call_index,
                    id,
                    name,
                    arguments: open,
                    ..
                }) = self.find_open(frame.index)
                {
                    open.push_str(&arguments);

                    events.push(IrEvent::ToolCallDelta {
                        index: *call_index,
                        call_id: Some(id.clone()),
                        name: Some(name.clone()),
                        arguments_delta: arguments,
                    });
                }
            }

            // Media, annotation, server-side tool, and unknown delta kinds carry no content.
            StepDelta::Ignored => {}
        }

        events
    }

    /// Ingests a `step.stop` frame: closes the tracked step and emits its closing event, assembled
    /// entirely from the state the step's deltas streamed — the official stop carries only the
    /// index. A thought stop closes as a done whose opaque payload is the synthesized official
    /// step, so a same-family replay stays byte-exact; a function-call stop closes with the
    /// assembled call; an output stop closes silently, its text already delivered by the deltas.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for a malformed frame.
    ///
    /// # Panics
    ///
    /// Panics if serializing a thought step's opaque payload fails, which cannot occur for the
    /// first-party wire type.
    fn ingest_stop(&mut self, frame: StepStopFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        match self.take_open(frame.index) {
            Some(OpenStep::Output { .. }) => {}

            Some(OpenStep::Thought {
                summary, signature, ..
            }) => {
                let summary_blocks = if summary.is_empty() {
                    Vec::new()
                } else {
                    vec![super::ContentBlock::Text {
                        text: summary.clone(),
                    }]
                };

                let step = TypedStep::Thought {
                    summary: summary_blocks,
                    signature: signature.clone(),
                };

                #[expect(
                    clippy::expect_used,
                    reason = "the first-party wire type serializes totally"
                )]
                let opaque = serde_json::to_value(step).expect("the thought step serializes");

                events.push(IrEvent::ReasoningDone {
                    text: summary,
                    signature,
                    opaque,
                });
            }

            Some(OpenStep::Call {
                call_index,
                id,
                name,
                arguments,
                ..
            }) => {
                events.push(IrEvent::ToolCallDone {
                    index: call_index,
                    call_id: id,
                    name,
                    arguments,
                    thought_signature: None,
                });
            }

            // Untracked indices — request-echo and unknown step kinds, stray stops — close
            // nothing and emit nothing.
            None => {}
        }

        Ok(events)
    }

    /// Ingests an `interaction.completed` frame: the terminal status, then the usage report when
    /// the frame's partial resource carries one.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for the failed status.
    fn ingest_completed(&mut self, frame: CompletedFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        events.extend(self.ingest_status(frame.interaction.status)?);

        if let Some(usage) = frame.interaction.usage {
            events.push(IrEvent::Usage(usage.into_usage()));
        }

        Ok(events)
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame, discriminated by its `event:` line or, failing that, the
    /// payload's `event_type` member. A frame bearing a non-null `error` member aborts as an
    /// upstream failure whatever its event name; frames with an unknown, absent, null, or empty
    /// event name are ignored for forward compatibility, and a type-invalid `event_type` member
    /// aborts the payload as malformed.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON, upstream error members, and payloads whose
    /// kind contradicts the contract.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        // One lex per frame: the payload parses once, the probe borrows it, and the matched arm
        // consumes it without a re-parse.
        let payload: Value = format::parse_frame(frame)?;

        // The probe is typed: any frame bearing a failure aborts, recognized event or not.
        let probe: FrameProbe = FrameProbe::deserialize(&payload)
            .map_err(|_| PayloadError::new("malformed upstream payload in SSE frame"))?;

        if let Some(error) = probe.error {
            return Err(error.failure());
        }

        // The `event:` line discriminates when the frame carries one, else the payload's official
        // `event_type` member self-describes it. Neither present yields a heartbeat.
        match frame.event.as_deref().or(probe.event_type.as_deref()) {
            Some(EVENT_CREATED) => {
                let created: CreatedFrame = format::parse_body(payload)?;

                Ok(self.open_events(created.interaction.id, created.interaction.model))
            }

            Some(EVENT_STEP_START) => {
                let start: StepStartFrame = format::parse_body(payload)?;

                Ok(self.ingest_start(start))
            }

            Some(EVENT_STEP_DELTA) => {
                let delta: StepDeltaFrame = format::parse_body(payload)?;

                Ok(self.ingest_delta(delta))
            }

            Some(EVENT_STEP_STOP) => {
                let stop: StepStopFrame = format::parse_body(payload)?;

                self.ingest_stop(stop)
            }

            Some(EVENT_COMPLETED) => {
                let completed: CompletedFrame = format::parse_body(payload)?;

                self.ingest_completed(completed)
            }

            // The progress-only status update carries no content: the terminal status and usage
            // arrive only on `interaction.completed`.
            Some(EVENT_STATUS_UPDATE) => Ok(Vec::new()),

            // Heartbeats and future event kinds never surface content.
            _ => Ok(Vec::new()),
        }
    }

    /// Parses a complete upstream non-streaming Interaction body.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream error objects, malformed bodies, and the failed
    /// terminal status.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        // Ingesting the body always ingests the terminal status, which emits the body's own finish
        // and marks the parser finished — the body closes its stream, never the finish hook.
        self.ingest_body(format::parse_body(value)?)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // A clean end-of-stream that carried no finish degrades to `Stop`, so streamed and
        // aggregated renderings of the same stream agree.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one SSE frame of the official upstream vocabulary, discriminated only by the
    /// payload's `event_type` member (no `event:` line).
    fn eventless_frame(event_type: &str, mut data: Value) -> SseFrame {
        #[expect(
            clippy::indexing_slicing,
            reason = "every fixture payload is a JSON object"
        )]
        {
            data["event_type"] = Value::String(event_type.to_string());
        }

        SseFrame {
            data: data.to_string(),
            event: None,
        }
    }

    /// Builds one SSE frame carrying an explicit `event:` line, the alternate official
    /// discrimination path.
    fn frame(event: &str, data: Value) -> SseFrame {
        SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        }
    }

    /// Ingests one eventless frame on the given parser.
    ///
    /// # Panics
    ///
    /// Panics if the frame fails to parse, which the fixture prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture frames are constructed to parse"
    )]
    fn ingest(parser: &mut UpstreamParser, event_type: &str, data: Value) -> Vec<IrEvent> {
        parser
            .ingest_frame(&eventless_frame(event_type, data))
            .expect("the fixture frame parses")
    }

    /// Verifies the official streaming vocabulary end to end over eventless frames: created opens
    /// the stream under the `interaction` member, text and thought deltas stream under opened
    /// steps, a stop closes the thought as a done carrying the streamed state, an arguments delta
    /// assembles one call whose stop closes it, and the completed frame reports the terminal
    /// status and usage under the partial `interaction` resource.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the official contract.
    #[test]
    fn parses_the_official_streaming_vocabulary() {
        let mut parser = UpstreamParser::new();

        let created = ingest(
            &mut parser,
            super::super::EVENT_CREATED,
            serde_json::json!({
                "interaction": {
                    "id": "int_1",
                    "model": "upstream-model",
                    "status": "in_progress",
                }
            }),
        );

        assert_eq!(
            created,
            vec![IrEvent::MessageStart {
                id: "int_1".to_string(),
                model: "upstream-model".to_string(),
            }],
        );

        // A repeated created event is a no-op: the stream opens exactly once.
        assert!(
            ingest(
                &mut parser,
                super::super::EVENT_CREATED,
                serde_json::json!({"interaction": {"id": "int_2"}}),
            )
            .is_empty()
        );

        // An output start records the step and emits nothing on its own.
        assert!(
            ingest(
                &mut parser,
                super::super::EVENT_STEP_START,
                serde_json::json!({"index": 0, "step": {"type": "model_output"}}),
            )
            .is_empty()
        );

        let text_delta = ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({"index": 0, "delta": {"type": "text", "text": "hi"}}),
        );

        assert_eq!(text_delta, vec![IrEvent::TextDelta("hi".to_string())]);

        ingest(
            &mut parser,
            super::super::EVENT_STEP_START,
            serde_json::json!({"index": 1, "step": {"type": "thought"}}),
        );

        ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({
                "index": 1,
                "delta": {
                    "type": "thought_summary",
                    "content": {"type": "text", "text": "reasoning"},
                },
            }),
        );

        ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({
                "index": 1,
                "delta": {"type": "thought_signature", "signature": "sig-1"},
            }),
        );

        // The official stop carries the index alone; the done assembles from the streamed state.
        let thought_stop = ingest(
            &mut parser,
            super::super::EVENT_STEP_STOP,
            serde_json::json!({"index": 1}),
        );

        assert_eq!(
            thought_stop,
            vec![IrEvent::ReasoningDone {
                text: "reasoning".to_string(),
                signature: Some("sig-1".to_string()),
                opaque: serde_json::json!({
                    "type": "thought",
                    "summary": [{"type": "text", "text": "reasoning"}],
                    "signature": "sig-1",
                }),
            }],
        );

        ingest(
            &mut parser,
            super::super::EVENT_STEP_START,
            serde_json::json!({
                "index": 2,
                "step": {"type": "function_call", "id": "call_7", "name": "search"},
            }),
        );

        let call_delta = ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({
                "index": 2,
                "delta": {"type": "arguments_delta", "arguments": r##"{"q":"##},
            }),
        );

        assert_eq!(
            call_delta,
            vec![IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call_7".to_string()),
                name: Some("search".to_string()),
                arguments_delta: r##"{"q":"##.to_string(),
            }],
        );

        let call_stop = ingest(
            &mut parser,
            super::super::EVENT_STEP_STOP,
            serde_json::json!({"index": 2}),
        );

        assert_eq!(
            call_stop,
            vec![IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_7".to_string(),
                name: "search".to_string(),
                arguments: r##"{"q":"##.to_string(),
                thought_signature: None,
            }],
        );

        let completed = ingest(
            &mut parser,
            super::super::EVENT_COMPLETED,
            serde_json::json!({
                "interaction": {
                    "id": "int_1",
                    "status": "requires_action",
                    "usage": {
                        "total_input_tokens": 10,
                        "total_output_tokens": 20,
                        "total_thought_tokens": 8,
                        "total_cached_tokens": 3,
                        "total_tool_use_tokens": 99,
                        "total_tokens": 38,
                    },
                },
                "event_id": "evt_1",
            }),
        );

        assert_eq!(
            completed,
            vec![
                IrEvent::Finish(IrFinishReason::ToolCall),
                IrEvent::Usage(IrUsage {
                    input_tokens: 10,
                    // Output includes reasoning; the tool-use total is dropped.
                    output_tokens: 28,
                    cached_tokens: Some(3),
                    cache_write_tokens: None,
                    reasoning_tokens: Some(8),
                }),
            ],
        );

        assert!(parser.finish().is_empty(), "no spurious Stop may follow");
    }

    /// A text delta with no recorded start implicitly opens its step: a missed start cannot
    /// strand content.
    ///
    /// # Panics
    ///
    /// Panics if the implicit open fails to deliver the delta.
    #[test]
    fn implicitly_opens_a_step_for_a_delta_without_a_start() {
        let mut parser = UpstreamParser::new();

        let events = ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({"index": 4, "delta": {"type": "text", "text": "hi"}}),
        );

        assert_eq!(
            events,
            vec![
                // The implicit open emits the stream-opening MessageStart first.
                IrEvent::MessageStart {
                    id: String::new(),
                    model: String::new(),
                },
                IrEvent::TextDelta("hi".to_string()),
            ],
        );

        // The stop closes the implicitly opened step without further content.
        let stop = ingest(
            &mut parser,
            super::super::EVENT_STEP_STOP,
            serde_json::json!({"index": 4}),
        );

        assert!(stop.is_empty(), "a closed output step emits nothing");
    }

    /// The officially optional `thought_summary` `content` and `thought_signature` `signature`
    /// members parse as no-ops when absent: a conformant member-less delta neither aborts the
    /// stream, nor emits content, nor opens a step.
    ///
    /// # Panics
    ///
    /// Panics if a member-less delta fails to parse or delivers content.
    #[test]
    fn parses_member_less_thought_deltas_as_no_ops() {
        let mut parser = UpstreamParser::new();

        for delta in [
            serde_json::json!({"type": "thought_summary"}),
            serde_json::json!({"type": "thought_signature"}),
        ] {
            assert!(
                ingest(
                    &mut parser,
                    super::super::EVENT_STEP_DELTA,
                    serde_json::json!({"index": 0, "delta": delta}),
                )
                .is_empty(),
                "a member-less delta carries no content: {delta}",
            );
        }

        // Carrying nothing, the deltas opened no step: the stop merely opens the stream.
        assert_eq!(
            ingest(
                &mut parser,
                super::super::EVENT_STEP_STOP,
                serde_json::json!({"index": 0}),
            ),
            vec![IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            }],
        );
    }

    /// A `thought_signature` delta without the officially optional `signature` member never
    /// clears a signature already streamed for the open thought step.
    ///
    /// # Panics
    ///
    /// Panics if the member-less delta fails to parse or the closed block diverges from the
    /// streamed state.
    #[test]
    fn preserves_a_streamed_signature_against_a_member_less_signature_delta() {
        let mut parser = UpstreamParser::new();

        ingest(
            &mut parser,
            super::super::EVENT_STEP_START,
            serde_json::json!({"index": 1, "step": {"type": "thought"}}),
        );

        ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({
                "index": 1,
                "delta": {"type": "thought_signature", "signature": "sig-1"},
            }),
        );

        // A conformant member-less repetition of the same delta kind.
        ingest(
            &mut parser,
            super::super::EVENT_STEP_DELTA,
            serde_json::json!({"index": 1, "delta": {"type": "thought_signature"}}),
        );

        assert_eq!(
            ingest(
                &mut parser,
                super::super::EVENT_STEP_STOP,
                serde_json::json!({"index": 1}),
            ),
            vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: Some("sig-1".to_string()),
                opaque: serde_json::json!({"type": "thought", "signature": "sig-1"}),
            }],
        );
    }

    /// Media, annotation, server-side tool, and unknown delta kinds — and progress-only status
    /// updates, unknown events, and heartbeats — carry no content and never abort a stream of
    /// official events; frames discriminated by an explicit `event:` line parse the same way.
    ///
    /// # Panics
    ///
    /// Panics if an ignorable frame fails or delivers content.
    #[test]
    fn ignores_irrelevant_frames_and_honors_the_event_line() {
        let mut parser = UpstreamParser::new();

        for delta in [
            serde_json::json!({"type": "image", "uri": "https://example.com/i.png"}),
            serde_json::json!({"type": "text_annotation_delta", "annotations": []}),
            serde_json::json!({"type": "hologram_delta", "data": "x"}),
            serde_json::json!({"type": "google_search_call", "arguments": {}}),
        ] {
            assert!(
                ingest(
                    &mut parser,
                    super::super::EVENT_STEP_DELTA,
                    serde_json::json!({"index": 0, "delta": delta}),
                )
                .is_empty(),
                "an ignorable delta carries no content: {delta}",
            );
        }

        // The progress-only status update parses and ignores; the terminal status arrives only
        // on the completed frame.
        assert!(
            ingest(
                &mut parser,
                super::super::EVENT_STATUS_UPDATE,
                serde_json::json!({"interaction_id": "int_1", "status": "requires_action"}),
            )
            .is_empty(),
        );

        // An explicit `event:` line discriminates the same vocabulary.
        #[expect(
            clippy::expect_used,
            reason = "the event-lined frames are constructed to parse"
        )]
        {
            let created = parser
                .ingest_frame(&frame(
                    super::super::EVENT_CREATED,
                    serde_json::json!({"interaction": {"id": "int_9", "model": "m"}}),
                ))
                .expect("the event-lined frame parses");

            assert!(
                matches!(created.first(), Some(IrEvent::MessageStart { .. })),
                "the created event opens the stream: {created:?}",
            );

            let delta = parser
                .ingest_frame(&frame(
                    super::super::EVENT_STEP_DELTA,
                    serde_json::json!({"index": 0, "delta": {"type": "text", "text": "hi"}}),
                ))
                .expect("the event-lined delta parses");

            assert!(delta.contains(&IrEvent::TextDelta("hi".to_string())));
        }

        // Heartbeats and future kinds stay silent.
        #[expect(clippy::expect_used, reason = "the heartbeat is constructed to parse")]
        let heartbeat = parser
            .ingest_frame(&frame("interaction.ping", serde_json::json!({"t": 1})))
            .expect("the heartbeat parses");

        assert!(heartbeat.is_empty());

        #[expect(
            clippy::expect_used,
            reason = "the eventless heartbeat is constructed to parse"
        )]
        let eventless = parser
            .ingest_frame(&SseFrame {
                data: serde_json::json!({"t": 1}).to_string(),
                event: None,
            })
            .expect("an eventless heartbeat parses");

        assert!(
            eventless.is_empty(),
            "an eventless heartbeat carries nothing"
        );
    }

    /// A thought stop without any streamed summary closes as an empty-text done whose opaque is
    /// the minimal synthesized step, and a stray stop for an untracked index closes nothing.
    ///
    /// # Panics
    ///
    /// Panics if the closed block diverges from the streamed state.
    #[test]
    fn closes_a_summary_less_thought_stop_with_the_synthesized_step() {
        let mut parser = UpstreamParser::new();

        ingest(
            &mut parser,
            super::super::EVENT_STEP_START,
            serde_json::json!({"index": 5, "step": {"type": "thought"}}),
        );

        let stop = ingest(
            &mut parser,
            super::super::EVENT_STEP_STOP,
            serde_json::json!({"index": 5}),
        );

        assert_eq!(
            stop,
            vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: serde_json::json!({"type": "thought"}),
            }],
        );

        // A stray stop for an untracked index closes nothing and emits nothing.
        let stray = ingest(
            &mut parser,
            super::super::EVENT_STEP_STOP,
            serde_json::json!({"index": 99}),
        );

        assert!(stray.is_empty());
    }

    /// Verifies the non-streaming parse of the official Interaction resource: `model_output`
    /// steps deliver text from their content blocks, thought steps close as full-text dones with
    /// no prior deltas carrying their complete step JSON as the opaque, function calls pair a
    /// full-arguments delta with a done, request-echo steps are ignored, and the status and usage
    /// close the stream.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the contract.
    #[test]
    fn parses_a_non_streaming_interaction_body() {
        let body = serde_json::json!({
            "id": "int_3",
            "model": "upstream-model",
            "object": "interaction",
            "status": "completed",
            "steps": [
                {"type": "model_output", "content": [
                    {"type": "text", "text": "hello "},
                    {"type": "text", "text": "world"},
                ]},
                {"type": "thought", "summary": [
                    {"type": "text", "text": "considered"},
                ], "signature": "sig-t"},
                {"type": "function_call", "id": "call_1", "name": "search", "arguments": {
                    "q": "hi",
                }},
                {"type": "user_input", "content": [{"type": "text", "text": "ignored"}]},
                {"type": "function_result", "call_id": "call_1", "result": "ignored"},
                {"type": "google_search_call", "arguments": {"queries": ["x"]}, "id": "g_1",
                 "signature": "sig-g"},
            ],
            "usage": {
                "total_input_tokens": 7,
                "total_output_tokens": 9,
            },
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("the interaction body parses");

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "int_3".to_string(),
                    model: "upstream-model".to_string(),
                },
                IrEvent::TextDelta("hello world".to_string()),
                IrEvent::ReasoningDone {
                    text: "considered".to_string(),
                    signature: Some("sig-t".to_string()),
                    opaque: serde_json::json!({
                        "type": "thought",
                        "summary": [{"type": "text", "text": "considered"}],
                        "signature": "sig-t",
                    }),
                },
                IrEvent::ToolCallDelta {
                    index: 0,
                    call_id: Some("call_1".to_string()),
                    name: Some("search".to_string()),
                    arguments_delta: r#"{"q":"hi"}"#.to_string(),
                },
                IrEvent::ToolCallDone {
                    index: 0,
                    call_id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: r#"{"q":"hi"}"#.to_string(),
                    thought_signature: None,
                },
                IrEvent::Finish(IrFinishReason::Stop),
                IrEvent::Usage(IrUsage {
                    input_tokens: 7,
                    output_tokens: 9,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
            ],
        );
    }

    /// Verifies the terminal status mapping: `requires_action` finishes with tool-call,
    /// `incomplete` and the deprecated `budget_exceeded` with length, `cancelled` with a clean
    /// stop, `failed` aborts as an upstream failure on both the streaming and the buffered paths,
    /// and a stream that ends cleanly without any status degrades to stop.
    ///
    /// # Panics
    ///
    /// Panics if a status maps outside its documented finish.
    #[test]
    fn maps_terminal_statuses() {
        for (status, finish) in [
            ("requires_action", IrFinishReason::ToolCall),
            ("incomplete", IrFinishReason::Length),
            ("budget_exceeded", IrFinishReason::Length),
            ("cancelled", IrFinishReason::Stop),
            ("completed", IrFinishReason::Stop),
            // The non-terminal statuses never terminate a conformant stream; arriving anyway,
            // they degrade to a clean stop.
            ("in_progress", IrFinishReason::Stop),
            ("queued", IrFinishReason::Stop),
            ("a_future_status", IrFinishReason::Stop),
        ] {
            let mut parser = UpstreamParser::new();

            let events = ingest(
                &mut parser,
                super::super::EVENT_COMPLETED,
                serde_json::json!({"interaction": {"id": "i", "status": status}}),
            );

            assert_eq!(
                events.last(),
                Some(&IrEvent::Finish(finish)),
                "status `{status}` must finish with its documented reason",
            );
        }

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the failed status is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&eventless_frame(
                super::super::EVENT_COMPLETED,
                serde_json::json!({"interaction": {"status": "failed"}}),
            ))
            .expect_err("a failed status must abort the stream");

        assert!(error.to_string().contains("failed"));

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the failed body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(serde_json::json!({"status": "failed"}))
            .expect_err("a failed body must abort");

        assert!(error.to_string().contains("failed"));

        let mut parser = UpstreamParser::new();

        ingest(
            &mut parser,
            super::super::EVENT_CREATED,
            serde_json::json!({"interaction": {"id": "i"}}),
        );

        assert_eq!(
            parser.finish(),
            vec![IrEvent::Finish(IrFinishReason::Stop)],
            "a clean end without a status degrades to stop",
        );
    }

    /// An upstream failure embedded in a success transport aborts the same way on both paths: a
    /// frame bearing a non-null `error` member — the official `error` event included — and a JSON
    /// body carrying one both fail.
    ///
    /// # Panics
    ///
    /// Panics if an embedded error parses as a success.
    #[test]
    fn rejects_embedded_upstream_errors() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the error frame is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&eventless_frame(
                "error",
                serde_json::json!({"error": {"code": "UNAVAILABLE", "message": "overloaded"}}),
            ))
            .expect_err("an embedded error must abort the stream");

        assert!(error.to_string().contains("overloaded"));

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the error body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(serde_json::json!({"error": "quota exhausted"}))
            .expect_err("an embedded error must abort the body");

        assert_eq!(error.to_string(), "quota exhausted");
    }

    /// The old self-pinned dialect contradicts the official one and is rejected where a JSON kind
    /// is pinned: a function_call step carrying JSON-encoded-string arguments — the old spelling —
    /// fails as a malformed payload, and the old terminal event name carries no content.
    ///
    /// # Panics
    ///
    /// Panics if the old spelling parses as a success.
    #[test]
    fn rejects_old_dialect_argument_spelling() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the old spelling is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(serde_json::json!({
                "status": "completed",
                "steps": [{"type": "function_call", "id": "c", "name": "f",
                           "arguments": r#"{"q":"hi"}"#}],
            }))
            .expect_err("the old JSON-encoded-string arguments must fail");

        assert!(
            error.to_string().contains("malformed"),
            "the failure must report a malformed payload: {error}",
        );

        // The old terminal event name is simply unknown today: a future-compatible no-op.
        let mut parser = UpstreamParser::new();

        assert!(
            ingest(
                &mut parser,
                "interaction.complete",
                serde_json::json!({
                    "status": "completed",
                })
            )
            .is_empty(),
        );
    }
}
