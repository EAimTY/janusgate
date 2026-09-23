//! Upstream-event parsing for the Gemini Interactions codec: parses upstream SSE streams or a
//! complete Interaction body into IR events, discriminating streams on the `event:` name, never
//! on JSON members.

use super::{
    EVENT_COMPLETE, EVENT_CREATED, EVENT_STEP_DELTA, EVENT_STEP_START, EVENT_STEP_STOP,
    STATUS_FAILED, STATUS_INCOMPLETE, STATUS_REQUIRES_ACTION, STEP_FUNCTION_CALL, STEP_OUTPUT_TEXT,
    STEP_THOUGHT,
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
/// events, discriminating streams on the `event:` name, never on JSON members.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether a finish was already emitted; a clean end-of-stream without one degrades to
    /// [`IrFinishReason::Stop`] in the finish hook.
    is_finished: bool,

    /// Count of tool calls opened in this response; the stable per-call index source.
    call_counter: usize,

    /// Steps opened by `step.start` and not yet closed by `step.stop`, keyed by the upstream step
    /// index.
    open_steps: Vec<OpenStep>,
}

/// One upstream step tracked across stream frames.
#[derive(Debug)]
enum OpenStep {
    /// An open model text step.
    Text {
        /// Upstream step index.
        index: u64,
    },

    /// An open model reasoning block, accumulating its summary text. The step identifier needs
    /// no tracking: every stop carries its complete step, which becomes the block's opaque
    /// payload verbatim.
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
            Self::Text { index } | Self::Thought { index, .. } | Self::Call { index, .. } => *index,
        }
    }
}

/// One upstream Interaction body or envelope member: the non-streaming response shape.
///
/// Every member is optional and unmodeled members are ignored: upstreams are third-party. Steps
/// are raw values — fragments whose per-kind shape the parser models separately — so a thought
/// step's complete JSON survives as the block's opaque replay payload.
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

    /// The interaction's steps in order.
    #[serde(default)]
    steps: Vec<Value>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UsageReport>,

    /// Failure the upstream may embed in a 2xx body.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Minimal probe reading the failure member an upstream may embed in any frame or body.
#[derive(Debug, Deserialize)]
struct ErrorProbe {
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

/// An `output_text` step of an Interaction body or a `step.stop` frame.
#[derive(Debug, Deserialize)]
struct OutputTextStep {
    /// The text content, empty when the upstream omits it.
    #[serde(default)]
    text: String,
}

/// A `thought` step of an Interaction body or a `step.stop` frame: the summary text and the
/// terminating signature. The step identifier is deliberately unmodelled — it rides inside the
/// opaque fragment untouched, so the codec never reads it.
///
/// No unknown-field rejection: upstream payloads are third-party, and every member beyond the
/// modelled ones rides the opaque fragment untouched.
#[derive(Debug, Deserialize)]
struct ThoughtStep {
    /// Reasoning text, empty when the upstream omits it.
    #[serde(default)]
    summary: Option<String>,

    /// Upstream signature terminating the block, absent when the upstream sent none.
    #[serde(default)]
    signature: Option<String>,
}

/// A `function_call` step of an Interaction body or a `step.stop` frame.
#[derive(Debug, Deserialize)]
struct FunctionCallStep {
    /// Identifier of the invocation, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Name of the invoked function, empty when the upstream omits it.
    #[serde(default)]
    name: String,

    /// Arguments the model supplied, as JSON-encoded text or as the object itself.
    #[serde(default)]
    arguments: Option<StepArguments>,

    /// Upstream thought signature that must be replayed on the next turn.
    #[serde(default)]
    signature: Option<String>,
}

/// The two wire spellings of function-call arguments: the JSON-encoded text form and the object
/// form. Both normalize to the encoded-text form the IR carries.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StepArguments {
    /// The JSON-encoded text form.
    Text(String),

    /// The object form.
    Object(Map<String, Value>),
}

impl StepArguments {
    /// Normalizes either spelling to the JSON-encoded text the IR carries.
    fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,

            Self::Object(map) => crate::format::arguments_object_to_text(map),
        }
    }
}

/// A partial step carried by `step.start`: the opening shape of a step, with only the members an
/// opening needs.
#[derive(Debug, Default, Deserialize)]
struct PartialStep {
    /// Step type tag, empty when the frame carries none.
    #[serde(rename = "type", default)]
    step_type: String,

    /// Step identifier of thought and function-call steps.
    #[serde(default)]
    id: Option<String>,

    /// Function name of a function-call step.
    #[serde(default)]
    name: Option<String>,
}

/// One incremental delta carried by `step.delta`, tagged by its kind.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StepDelta {
    /// Model text bytes.
    #[serde(rename = "text")]
    Text {
        /// The text bytes.
        text: String,
    },

    /// Reasoning summary bytes.
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        /// The summary bytes.
        summary: String,
    },

    /// The signature terminating a reasoning block.
    #[serde(rename = "thought_signature")]
    ThoughtSignature {
        /// The signature text.
        signature: String,
    },

    /// Tool argument bytes.
    #[serde(rename = "tool_arguments")]
    ToolArguments {
        /// The argument bytes.
        arguments: String,
    },
}

/// The `step.start` frame payload.
#[derive(Debug, Deserialize)]
struct StepStartFrame {
    /// Upstream step index the frame opens.
    step_index: u64,

    /// The opening shape of the step.
    #[serde(default)]
    step: PartialStep,
}

/// The `step.delta` frame payload.
#[derive(Debug, Deserialize)]
struct StepDeltaFrame {
    /// Upstream step index the delta belongs to.
    step_index: u64,

    /// The incremental delta.
    delta: StepDelta,
}

/// The `step.stop` frame payload: the step index beside the complete step as a raw fragment, so
/// a thought step's JSON survives verbatim as the block's opaque payload.
#[derive(Debug, Deserialize)]
struct StepStopFrame {
    /// Upstream step index the frame closes.
    step_index: u64,

    /// The complete step.
    #[serde(default)]
    step: Value,
}

/// The `interaction.created` frame payload.
#[derive(Debug, Deserialize)]
struct CreatedFrame {
    /// Upstream interaction identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,
}

/// The `interaction.complete` frame payload.
#[derive(Debug, Deserialize)]
struct CompleteFrame {
    /// Terminal status string, deciding the finish mapping.
    #[serde(default)]
    status: Option<String>,

    /// Human-readable failure detail of a failed interaction.
    #[serde(default)]
    message: Option<String>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UsageReport>,
}

/// Upstream token accounting in the Interactions shape.
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

        events.extend(self.ingest_status(body.status, None)?);

        if let Some(usage) = body.usage {
            events.push(IrEvent::Usage(usage.into_usage()));
        }

        Ok(events)
    }

    /// Ingests one complete step value — an Interaction body member or a `step.stop` payload.
    ///
    /// Response steps deliver their content at once: text steps stream as one full delta, thought
    /// steps close as full-text done events with no prior deltas, and function calls open with a
    /// full-arguments delta plus a done — mirroring the Responses JSON path — so streaming
    /// renderers open and complete every item. Steps of other kinds (`user_input`,
    /// `function_result`, and future kinds) carry no model output and are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for a step whose kind contradicts the contract.
    fn ingest_complete_step(&mut self, step: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let probe: StepProbe = format::parse_body(step.clone())?;

        match probe.step_type.as_str() {
            STEP_OUTPUT_TEXT => {
                let text: OutputTextStep = format::parse_body(step)?;

                Ok(vec![IrEvent::TextDelta(text.text)])
            }

            STEP_THOUGHT => {
                let thought: ThoughtStep = format::parse_body(step.clone())?;

                Ok(vec![IrEvent::ReasoningDone {
                    text: thought.summary.unwrap_or_default(),
                    signature: thought.signature,
                    opaque: step,
                }])
            }

            STEP_FUNCTION_CALL => {
                let call: FunctionCallStep = format::parse_body(step)?;
                let index = self.allocate_call_index();

                let arguments = call
                    .arguments
                    .map(StepArguments::into_text)
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
                        thought_signature: call.signature,
                    },
                ])
            }

            // Request-echo (`user_input`, `function_result`) and future step kinds carry no
            // model output.
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
    fn ingest_status(
        &mut self,
        status: Option<String>,
        message: Option<String>,
    ) -> Result<Vec<IrEvent>, PayloadError> {
        self.is_finished = true;

        match status.as_deref() {
            Some(STATUS_REQUIRES_ACTION) => Ok(vec![IrEvent::Finish(IrFinishReason::ToolCall)]),

            Some(STATUS_INCOMPLETE) => Ok(vec![IrEvent::Finish(IrFinishReason::Length)]),

            Some(STATUS_FAILED) => Err(PayloadError::new(format!(
                "upstream interaction failed with status `failed`{}",
                message
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default(),
            ))),

            // `completed`, a missing status, and any future status degrade to a clean stop rather
            // than failing a healthy stream.
            _ => Ok(vec![IrEvent::Finish(IrFinishReason::Stop)]),
        }
    }

    /// Ingests a `step.start` frame: records the opened step so later deltas resolve. A repeated
    /// start for an already-open index is ignored; a function-call start records the call but
    /// emits nothing — its deltas and stop carry the content.
    fn ingest_start(&mut self, frame: StepStartFrame) -> Vec<IrEvent> {
        let events = self.ensure_open();

        if self.find_open(frame.step_index).is_some() {
            return events;
        }

        match frame.step.step_type.as_str() {
            STEP_OUTPUT_TEXT => self.open_steps.push(OpenStep::Text {
                index: frame.step_index,
            }),

            STEP_THOUGHT => self.open_steps.push(OpenStep::Thought {
                index: frame.step_index,
                summary: String::new(),
                signature: None,
            }),

            STEP_FUNCTION_CALL => {
                let call_index = self.allocate_call_index();

                self.open_steps.push(OpenStep::Call {
                    index: frame.step_index,
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

    /// Ingests a `step.delta` frame: the delta kind decides the IR event. A delta for a step with
    /// no recorded start implicitly opens the matching kind, so a missed start cannot strand
    /// content; an implicit tool call mints its id and allocates its index the same way a start
    /// would.
    fn ingest_delta(&mut self, frame: StepDeltaFrame) -> Vec<IrEvent> {
        let mut events = self.ensure_open();

        match frame.delta {
            StepDelta::Text { text } => {
                if self.find_open(frame.step_index).is_none() {
                    self.open_steps.push(OpenStep::Text {
                        index: frame.step_index,
                    });
                }

                events.push(IrEvent::TextDelta(text));
            }

            StepDelta::ThoughtSummary { summary } => {
                if self.find_open(frame.step_index).is_none() {
                    self.open_steps.push(OpenStep::Thought {
                        index: frame.step_index,
                        summary: String::new(),
                        signature: None,
                    });
                }

                if let Some(OpenStep::Thought { summary: open, .. }) =
                    self.find_open(frame.step_index)
                {
                    open.push_str(&summary);
                }

                events.push(IrEvent::ReasoningDelta(summary));
            }

            StepDelta::ThoughtSignature { signature } => {
                if self.find_open(frame.step_index).is_none() {
                    self.open_steps.push(OpenStep::Thought {
                        index: frame.step_index,
                        summary: String::new(),
                        signature: None,
                    });
                }

                if let Some(OpenStep::Thought {
                    signature: open, ..
                }) = self.find_open(frame.step_index)
                {
                    *open = Some(signature);
                }
            }

            StepDelta::ToolArguments { arguments } => {
                if self.find_open(frame.step_index).is_none() {
                    let call_index = self.allocate_call_index();

                    self.open_steps.push(OpenStep::Call {
                        index: frame.step_index,
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
                }) = self.find_open(frame.step_index)
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
        }

        events
    }

    /// Ingests a `step.stop` frame carrying the complete step: closes the tracked step and emits
    /// the closing event. Thought stops close as empty-text done events — the streamed deltas
    /// delivered the text — unless no deltas preceded them, in which case the stop's own summary
    /// is the authoritative text; the step value itself becomes the block's opaque payload.
    /// Function-call stops resolve every member against the tracked state, preferring the stop's
    /// own values and falling back to the streamed ones.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for a stop step whose kind contradicts the contract.
    fn ingest_stop(&mut self, frame: StepStopFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        let probe: StepProbe = format::parse_body(frame.step.clone())?;

        match probe.step_type.as_str() {
            STEP_OUTPUT_TEXT => {
                self.take_open(frame.step_index);
            }

            STEP_THOUGHT => {
                let thought: ThoughtStep = format::parse_body(frame.step.clone())?;
                let open = self.take_open(frame.step_index);

                let (summary, signature) = match open {
                    Some(OpenStep::Thought {
                        summary, signature, ..
                    }) => (summary, signature),

                    _ => (String::new(), Option::<String>::None),
                };

                // Deltas plus an empty-text done, or a full-text done with no deltas — never both
                // with overlapping text: streamed summary bytes make the done empty, while a stop
                // that arrives without any deltas carries the authoritative text itself.
                let text = if summary.is_empty() {
                    thought.summary.unwrap_or_default()
                } else {
                    String::new()
                };

                let signature = thought.signature.or(signature);

                events.push(IrEvent::ReasoningDone {
                    text,
                    signature,
                    opaque: frame.step,
                });
            }

            STEP_FUNCTION_CALL => {
                let call: FunctionCallStep = format::parse_body(frame.step)?;
                let open = self.take_open(frame.step_index);

                let (call_index, open_id, open_name, open_arguments, open_signature) = match open {
                    Some(OpenStep::Call {
                        call_index,
                        id,
                        name,
                        arguments,
                        ..
                    }) => (call_index, Some(id), Some(name), Some(arguments), None),

                    _ => (
                        self.allocate_call_index(),
                        Option::<String>::None,
                        Option::<String>::None,
                        Option::<String>::None,
                        Option::<String>::None,
                    ),
                };

                let arguments = call
                    .arguments
                    .map(StepArguments::into_text)
                    .or(open_arguments)
                    .unwrap_or_default();

                events.push(IrEvent::ToolCallDone {
                    index: call_index,
                    call_id: nonempty_or(call.id, open_id),
                    name: nonempty_or(call.name, open_name),
                    arguments,
                    thought_signature: call.signature.or(open_signature),
                });
            }

            // Unknown and request-echo (`user_input`, `function_result`) stops close any
            // tracked step of the index and emit nothing.
            _ => {
                self.take_open(frame.step_index);
            }
        }

        Ok(events)
    }

    /// Ingests an `interaction.complete` frame: the terminal status, then the usage report when
    /// the frame carries one.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for the failed status.
    fn ingest_complete(&mut self, frame: CompleteFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        events.extend(self.ingest_status(frame.status, frame.message)?);

        if let Some(usage) = frame.usage {
            events.push(IrEvent::Usage(usage.into_usage()));
        }

        Ok(events)
    }
}

/// Prefers the upstream value when it is non-empty, else the tracked fallback: a stop carrying an
/// empty id or name (the upstream's unset spelling) must not clobber the streamed values.
fn nonempty_or(primary: String, fallback: Option<String>) -> String {
    if primary.is_empty() {
        fallback.unwrap_or_default()
    } else {
        primary
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame, discriminated on its `event:` name. A frame bearing a
    /// non-null `error` member aborts as an upstream failure whatever its event name; frames with
    /// an unknown or missing event name are ignored for forward compatibility.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON, upstream error members, and payloads whose
    /// kind contradicts the contract.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        // The error probe is typed: any frame bearing a failure aborts, recognized event or
        // not.
        let probe: ErrorProbe = format::parse_frame(frame)?;

        if let Some(error) = probe.error {
            return Err(error.failure());
        }

        match frame.event.as_deref() {
            Some(EVENT_CREATED) => {
                let created: CreatedFrame = format::parse_frame(frame)?;

                Ok(self.open_events(created.id, created.model))
            }

            Some(EVENT_STEP_START) => {
                let start: StepStartFrame = format::parse_frame(frame)?;

                Ok(self.ingest_start(start))
            }

            Some(EVENT_STEP_DELTA) => {
                let delta: StepDeltaFrame = format::parse_frame(frame)?;

                Ok(self.ingest_delta(delta))
            }

            Some(EVENT_STEP_STOP) => {
                let stop: StepStopFrame = format::parse_frame(frame)?;

                self.ingest_stop(stop)
            }

            Some(EVENT_COMPLETE) => {
                let complete: CompleteFrame = format::parse_frame(frame)?;

                self.ingest_complete(complete)
            }

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
        let body: InteractionBody = format::parse_body(value)?;

        // A missing status degrades to a clean stop here rather than in the finish hook: the JSON
        // pump never calls it, so the body must close its own stream.
        if body.status.is_none() {
            let mut events = self.ingest_body(body)?;

            if !self.is_finished {
                self.is_finished = true;

                events.push(IrEvent::Finish(IrFinishReason::Stop));
            }

            return Ok(events);
        }

        self.ingest_body(body)
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

    /// Builds one SSE frame of the upstream stream vocabulary.
    fn frame(event: &str, data: Value) -> SseFrame {
        SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        }
    }

    /// Ingests one frame of the upstream stream vocabulary on the given parser.
    ///
    /// # Panics
    ///
    /// Panics if the frame fails to parse, which the fixture prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture frames are constructed to parse"
    )]
    fn ingest(parser: &mut UpstreamParser, event: &str, data: Value) -> Vec<IrEvent> {
        parser
            .ingest_frame(&frame(event, data))
            .expect("the fixture frame parses")
    }

    /// Opens a parser with a created frame carrying a fixed identity.
    fn ingest_created(parser: &mut UpstreamParser) {
        ingest(
            parser,
            EVENT_CREATED,
            serde_json::json!({"id": "i", "model": "m"}),
        );
    }

    /// Verifies the event-name-discriminated streaming parse end to end: created opens the stream,
    /// text deltas stream, thought summary deltas plus a signature close as an empty-text done
    /// carrying the signature and the verbatim step, tool argument deltas assemble one call, and
    /// the terminal status reports finish and usage.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the contract.
    #[test]
    fn parses_the_streaming_vocabulary_by_event_name() {
        let mut parser = UpstreamParser::new();

        let created = ingest(
            &mut parser,
            EVENT_CREATED,
            serde_json::json!({"id": "int_1", "model": "upstream-model"}),
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
                EVENT_CREATED,
                serde_json::json!({"id": "int_2", "model": "other"}),
            )
            .is_empty()
        );

        // A text start records the step and emits nothing on its own.
        assert!(
            ingest(
                &mut parser,
                EVENT_STEP_START,
                serde_json::json!({"step_index": 0, "step": {"type": "output_text"}}),
            )
            .is_empty()
        );

        let text_delta = ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({"step_index": 0, "delta": {"type": "text", "text": "hi"}}),
        );

        assert!(
            text_delta.contains(&IrEvent::TextDelta("hi".to_string())),
            "unexpected text events: {text_delta:?}",
        );

        ingest(
            &mut parser,
            EVENT_STEP_START,
            serde_json::json!({"step_index": 1, "step": {"type": "thought", "id": "th_9"}}),
        );

        ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({
                "step_index": 1,
                "delta": {"type": "thought_summary", "summary": "reasoning"},
            }),
        );

        ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({
                "step_index": 1,
                "delta": {"type": "thought_signature", "signature": "sig-1"},
            }),
        );

        let thought_stop = ingest(
            &mut parser,
            EVENT_STEP_STOP,
            serde_json::json!({
                "step_index": 1,
                "step": {
                    "type": "thought",
                    "id": "th_9",
                    "summary": "reasoning",
                    "signature": "sig-1",
                },
            }),
        );

        assert_eq!(thought_stop.len(), 1);
        #[expect(
            clippy::expect_used,
            reason = "the preceding length assertion guarantees one event"
        )]
        let stop = thought_stop
            .first()
            .expect("the thought stop closes with one event");
        assert!(
            matches!(stop, IrEvent::ReasoningDone { .. }),
            "a thought stop must close with a reasoning done: {thought_stop:?}",
        );

        if let IrEvent::ReasoningDone {
            text,
            signature,
            opaque,
        } = stop
        {
            // Deltas delivered the text, so the done is empty-text — never overlapping.
            assert_eq!(text, "");
            assert_eq!(signature, &Some("sig-1".to_string()));
            assert_eq!(
                opaque,
                &serde_json::json!({
                    "type": "thought",
                    "id": "th_9",
                    "summary": "reasoning",
                    "signature": "sig-1",
                }),
            );
        }

        ingest(
            &mut parser,
            EVENT_STEP_START,
            serde_json::json!({
                "step_index": 2,
                "step": {"type": "function_call", "id": "call_7", "name": "search"},
            }),
        );

        let call_delta = ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({
                "step_index": 2,
                "delta": {"type": "tool_arguments", "arguments": r##"{"q":"##},
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
            EVENT_STEP_STOP,
            serde_json::json!({
                "step_index": 2,
                "step": {
                    "type": "function_call",
                    "id": "call_7",
                    "name": "search",
                    "arguments": r#"{"q":"hi"}"#,
                },
            }),
        );

        assert_eq!(
            call_stop,
            vec![IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_7".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"hi"}"#.to_string(),
                thought_signature: None,
            }],
        );

        let complete = ingest(
            &mut parser,
            EVENT_COMPLETE,
            serde_json::json!({
                "status": "requires_action",
                "usage": {
                    "total_input_tokens": 10,
                    "total_output_tokens": 20,
                    "total_thought_tokens": 8,
                    "total_cached_tokens": 3,
                    "total_tool_use_tokens": 99,
                },
            }),
        );

        assert_eq!(
            complete,
            vec![
                IrEvent::Finish(IrFinishReason::ToolCall),
                IrEvent::Usage(IrUsage {
                    input_tokens: 10,
                    // Output includes reasoning; the tool-use tokens are dropped.
                    output_tokens: 28,
                    cached_tokens: Some(3),
                    cache_write_tokens: None,
                    reasoning_tokens: Some(8),
                }),
            ],
        );

        assert!(parser.finish().is_empty(), "no spurious Stop may follow");
    }

    /// A thought stop that arrives without any prior summary deltas closes as a full-text done:
    /// the stop's own summary is authoritative, so its text is never lost.
    ///
    /// # Panics
    ///
    /// Panics if the stop-only block loses its text.
    #[test]
    fn closes_a_delta_less_thought_stop_with_its_own_text() {
        let mut parser = UpstreamParser::new();

        ingest_created(&mut parser);

        let stop = ingest(
            &mut parser,
            EVENT_STEP_STOP,
            serde_json::json!({
                "step_index": 5,
                "step": {"type": "thought", "id": "th_5", "summary": "whole"},
            }),
        );

        assert_eq!(stop.len(), 1);
        #[expect(
            clippy::expect_used,
            reason = "the preceding length assertion guarantees one event"
        )]
        let only = stop.first().expect("the stop closes with one event");
        assert!(
            matches!(only, IrEvent::ReasoningDone { .. }),
            "a thought stop must close with a reasoning done: {stop:?}",
        );

        if let IrEvent::ReasoningDone { text, opaque, .. } = only {
            assert_eq!(text, "whole");
            assert_eq!(
                opaque,
                &serde_json::json!({"type": "thought", "id": "th_5", "summary": "whole"}),
            );
        }
    }

    /// Verifies the non-streaming parse: the Interaction body's steps deliver text deltas,
    /// full-text thought dones with no prior deltas, and full-arguments call pairs, then the
    /// status and usage close the stream.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the contract.
    #[test]
    fn parses_a_non_streaming_interaction_body() {
        let body = serde_json::json!({
            "id": "int_3",
            "model": "upstream-model",
            "status": "completed",
            "steps": [
                {"type": "output_text", "text": "hello"},
                {"type": "thought", "id": "th_1", "summary": "considered", "signature": "sig-t"},
                {"type": "function_call", "id": "call_1", "name": "search", "arguments": {
                    "q": "hi",
                }},
                {"type": "user_input", "content": []},
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
                IrEvent::TextDelta("hello".to_string()),
                IrEvent::ReasoningDone {
                    text: "considered".to_string(),
                    signature: Some("sig-t".to_string()),
                    opaque: serde_json::json!({
                        "type": "thought",
                        "id": "th_1",
                        "summary": "considered",
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

    /// Verifies the terminal status mapping: `incomplete` finishes with length, `failed` aborts
    /// as an upstream failure on both the streaming and the buffered paths, and a stream that
    /// ends cleanly without any status degrades to stop.
    ///
    /// # Panics
    ///
    /// Panics if a status maps outside its documented finish.
    #[test]
    fn maps_terminal_statuses() {
        let mut parser = UpstreamParser::new();

        let events = ingest(
            &mut parser,
            EVENT_COMPLETE,
            serde_json::json!({"status": "incomplete"}),
        );

        assert_eq!(
            events.last(),
            Some(&IrEvent::Finish(IrFinishReason::Length))
        );

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the failed status is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&frame(
                EVENT_COMPLETE,
                serde_json::json!({"status": "failed", "message": "crashed"}),
            ))
            .expect_err("a failed status must abort the stream");

        assert!(error.to_string().contains("failed"));
        assert!(error.to_string().contains("crashed"));

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

        ingest_created(&mut parser);

        assert_eq!(
            parser.finish(),
            vec![IrEvent::Finish(IrFinishReason::Stop)],
            "a clean end without a status degrades to stop",
        );
    }

    /// An upstream failure embedded in a success transport aborts the same way on both paths: a
    /// frame bearing a non-null `error` member and a JSON body carrying one both fail, while
    /// frames with unknown or missing event names are ignored for forward compatibility.
    ///
    /// # Panics
    ///
    /// Panics if an embedded error parses as a success or a benign frame fails.
    #[test]
    fn rejects_embedded_upstream_errors_and_ignores_unknown_events() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the error frame is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&frame(
                EVENT_STEP_DELTA,
                serde_json::json!({"error": {"message": "overloaded"}}),
            ))
            .expect_err("an embedded error must abort the stream");

        assert_eq!(error.to_string(), "overloaded");

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the error body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(serde_json::json!({"error": "quota exhausted"}))
            .expect_err("an embedded error must abort the body");

        assert_eq!(error.to_string(), "quota exhausted");

        let mut parser = UpstreamParser::new();

        assert!(
            ingest(&mut parser, "interaction.ping", serde_json::json!({"t": 1})).is_empty(),
            "an unknown event carries no content",
        );

        #[expect(
            clippy::expect_used,
            reason = "the eventless frame is constructed to parse"
        )]
        let eventless = parser
            .ingest_frame(&SseFrame {
                data: serde_json::json!({"t": 1}).to_string(),
                event: None,
            })
            .expect("an eventless frame parses");

        assert!(
            eventless.is_empty(),
            "an eventless frame carries no content"
        );
    }
}
