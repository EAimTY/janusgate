//! Upstream-event parsing for the Gemini Interactions codec: parses upstream SSE streams or a
//! complete Interaction body into IR events. Streams are discriminated by the `event:` line when
//! the frame carries one, else by the payload's official `event_type` member.

use super::{
    EVENT_COMPLETED, EVENT_CREATED, EVENT_STATUS_UPDATE, EVENT_STEP_DELTA, EVENT_STEP_START,
    EVENT_STEP_STOP, STATUS_BUDGET_EXCEEDED, STATUS_CANCELLED, STATUS_FAILED, STATUS_INCOMPLETE,
    STATUS_REQUIRES_ACTION, STEP_FUNCTION_CALL, STEP_MODEL_OUTPUT, TypedStep,
};
use crate::{
    format::{self, PayloadError, UpstreamError, UpstreamEventParser, gemini::STEP_THOUGHT},
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

    /// Whether a finish was already emitted; `finish` degrades a stream that ends without one
    /// through [`format::degrade_finish_once`].
    is_finished: bool,

    /// Count of tool calls opened in this response; the stable per-call index source.
    call_counter: usize,

    /// Steps opened by `step.start` (or implicitly by a first delta) and not yet closed by
    /// `step.stop`, keyed by the upstream step index.
    open_steps: Vec<OpenStep>,

    /// Whether a live step event delivered content: content-bearing delivery silences the
    /// terminal frame's text and thought copies, so those kinds are never delivered twice. An
    /// opening — a bare `step.start`, an implicit open, a delta without a payload — carries no
    /// content (README §6.3's content-bearing-event rule, as the Responses parser gates it) and
    /// leaves their recovery live. The terminal frame's `function_call` copies answer to
    /// [`Self::has_delivered_call`] instead, so a delivered text never silences an undelivered
    /// call.
    is_step_delivered: bool,

    /// Whether a function call was already delivered live: a closing done always carries the
    /// complete call — the streamed argument bytes, or the empty object a zero-argument call's
    /// state assembles to — so the terminal frame's `function_call` copies are skipped and the
    /// call is never delivered a second time under a fresh index. An opening and a payload-less
    /// delta deliver no call and leave the terminal call recovery live. The record is the call
    /// kind's own, independent of [`Self::is_step_delivered`]: delivered text never silences
    /// an undelivered call, and a delivered call never silences undelivered text.
    has_delivered_call: bool,
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

    /// An open model reasoning block, accumulating its summary blocks and signature. The official
    /// stream delivers one summary block per text-carrying `thought_summary` delta, and the
    /// closing done's opaque replay payload is the synthesized official step — one summary block
    /// per delta beside the signature — so a same-family replay stays byte-exact with the JSON
    /// body path, which preserves the raw step verbatim.
    Thought {
        /// Upstream step index.
        index: u64,

        /// Summary blocks streamed so far, in delta order.
        summary: Vec<String>,

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

    /// Mints the implicit-open defaults for a thought step: no summary blocks and no signature,
    /// exactly as a `thought` start would record them.
    fn thought(index: u64) -> Self {
        Self::Thought {
            index,
            summary: Vec::new(),
            signature: None,
        }
    }

    /// Mints an open function-call step under `index` with the implicit-open defaults, exactly as
    /// a `function_call` start would record them: the step's stable tool-call index is allocated
    /// from the parser's counter, the identifier is the synthesized `call_{uuid}` when the
    /// upstream supplies none, an absent function name defaults to empty, and the argument bytes
    /// start empty for the deltas to stream.
    fn call(
        parser: &mut UpstreamParser,
        index: u64,
        id: Option<String>,
        name: Option<String>,
    ) -> Self {
        Self::Call {
            index,
            call_index: parser.allocate_call_index(),
            id: id.unwrap_or_else(|| format!("call_{}", ids::new_uuid())),
            name: name.unwrap_or_default(),
            arguments: String::new(),
        }
    }
}

/// One upstream Interaction body or the partial resource an
/// `interaction.created`/`interaction.completed` frame carries under its `interaction` member.
///
/// Every member is optional and unmodeled members are ignored: upstreams are third-party. Ordinary
/// steps use concrete per-kind fields; only a thought step's complete JSON survives as the block's
/// opaque replay payload. The streaming lifecycle payloads
/// may omit any field of the full resource ("Streaming lifecycle payloads may omit fields that are
/// only available on full non-streaming Interaction responses"); the terminal
/// `interaction.completed` resource carries its steps only when the upstream includes them, so the
/// stream ingests steps from the live step events and falls back to the terminal resource's steps
/// when no live step event opened one.
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

    /// The interaction's steps in order; a body member always, and a terminal-frame member only
    /// when the upstream includes it — recovered exactly once when no live step event delivered
    /// content.
    #[serde(default)]
    steps: Vec<CompleteStep>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UsageReport>,

    /// Failure the upstream may embed in a 2xx body.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Complete model-visible step; only thoughts require opaque producer metadata for replay.
/// The manual deserialization below discriminates the step kinds; nothing here serializes.
#[derive(Debug)]
enum CompleteStep {
    /// Model text output.
    ModelOutput(ModelOutputStep),
    /// A thought's complete producer-owned fragment.
    Thought(Value),
    /// A model-requested function invocation.
    FunctionCall(FunctionCallStep),
    /// Request echoes and future non-output steps carry no representable content.
    Ignored,
}

impl<'de> serde::Deserialize<'de> for CompleteStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
        let kind: StepProbe = serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
        match kind.step_type.as_str() {
            STEP_MODEL_OUTPUT => serde_json::from_str(raw.get())
                .map(Self::ModelOutput)
                .map_err(serde::de::Error::custom),
            STEP_THOUGHT => serde_json::from_str(raw.get())
                .map(Self::Thought)
                .map_err(serde::de::Error::custom),
            STEP_FUNCTION_CALL => serde_json::from_str(raw.get())
                .map(Self::FunctionCall)
                .map_err(serde::de::Error::custom),
            _ => Ok(Self::Ignored),
        }
    }
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
#[serde(tag = "type", rename = "model_output")]
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
#[serde(tag = "type", rename = "function_call")]
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
    /// Maps the report's members onto the shared family fold
    /// [`format::gemini::fold_gemini_usage`].
    fn into_usage(self) -> IrUsage {
        format::gemini::fold_gemini_usage(
            self.total_input_tokens,
            self.total_output_tokens,
            self.total_thought_tokens,
            self.total_cached_tokens,
        )
    }
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Emits [`IrEvent::MessageStart`] exactly once, first, with the given identity.
    fn open_events(&mut self, id: String, model: String) -> Vec<IrEvent> {
        format::message_start_once(&mut self.is_started, id, model)
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

    /// Records a step opened by a live `step.start`/`step.delta` event. An opening carries no
    /// content: the terminal frame's steps fallback stays live until a content-bearing delta or
    /// closing done delivers. An opening for an unknown step kind opens nothing and stays
    /// recoverable.
    fn push_open(&mut self, step: OpenStep) {
        self.open_steps.push(step);
    }

    /// Returns the step tracked under the upstream index, when one is open.
    fn find_open(&mut self, index: u64) -> Option<&mut OpenStep> {
        self.open_steps
            .iter_mut()
            .find(|open| open.index() == index)
    }

    /// States the implicit-open rule once: a content delta (or a start) for a step with no
    /// recorded opening implicitly opens the matching kind exactly as a start would, after the
    /// stream itself is opened — a missed start cannot strand content. Opens the stream, records
    /// the step `mint` builds under `index` when none is open there, and returns the events the
    /// caller extends with: the stream-opening [`IrEvent::MessageStart`] when the stream had not
    /// opened.
    ///
    /// The mint runs only when an opening is actually recorded, so a delta or a repeated start
    /// for an already-open step never mints a discarded opening — and never consumes a tool-call
    /// index.
    fn implicit_open(
        &mut self,
        index: u64,
        mint: impl FnOnce(&mut Self) -> OpenStep,
    ) -> Vec<IrEvent> {
        let events = self.ensure_open();

        if self.find_open(index).is_none() {
            let step = mint(self);

            self.push_open(step);
        }

        events
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
    fn ingest_complete_step(&mut self, step: CompleteStep) -> Result<Vec<IrEvent>, PayloadError> {
        match step {
            CompleteStep::ModelOutput(output) => {
                let text = output
                    .content
                    .into_iter()
                    .map(BodyBlock::into_text)
                    .collect::<String>();

                Ok(vec![IrEvent::TextDelta(text)])
            }

            CompleteStep::Thought(step) => {
                let thought = ThoughtStep::deserialize(&step).map_err(|error| {
                    PayloadError::new(format!("malformed upstream payload: {error}"))
                })?;
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

            CompleteStep::FunctionCall(call) => {
                let index = self.allocate_call_index();

                let arguments = call
                    .arguments
                    .map(|object| Value::Object(object).to_string())
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
            CompleteStep::Ignored => Ok(Vec::new()),
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
            Some(STATUS_REQUIRES_ACTION) => Ok(vec![IrEvent::finish(IrFinishReason::ToolCall)]),

            Some(STATUS_INCOMPLETE | STATUS_BUDGET_EXCEEDED) => {
                Ok(vec![IrEvent::finish(IrFinishReason::Length)])
            }

            // Deliberate: a cancelled interaction ended without failure, and the canonical finish
            // vocabulary has no cancelled member.
            Some(STATUS_CANCELLED) => Ok(vec![IrEvent::finish(IrFinishReason::Stop)]),

            Some(STATUS_FAILED) => Err(PayloadError::new(
                "upstream interaction failed with status `failed`",
            )),

            // `completed`, the non-terminal `in_progress`/`queued` (which never terminate a
            // conformant stream), and any future status degrade to a clean stop rather than
            // failing a healthy stream.
            _ => Ok(vec![IrEvent::finish(IrFinishReason::Stop)]),
        }
    }

    /// Ingests a `step.start` frame: records the opened step so later deltas resolve. A repeated
    /// start for an already-open index is ignored; a function-call start records the call but
    /// emits nothing — its deltas and stop carry the content.
    fn ingest_start(&mut self, frame: StepStartFrame) -> Vec<IrEvent> {
        match frame.step.step_type.as_str() {
            STEP_MODEL_OUTPUT => {
                self.implicit_open(frame.index, |_| OpenStep::Output { index: frame.index })
            }

            STEP_THOUGHT => self.implicit_open(frame.index, |_| OpenStep::thought(frame.index)),

            STEP_FUNCTION_CALL => self.implicit_open(frame.index, |parser| {
                OpenStep::call(parser, frame.index, frame.step.id, frame.step.name)
            }),

            // Unknown step kinds open nothing; their deltas fall into the implicit-open rule
            // only for the known delta kinds, and their stop closes nothing.
            _ => self.ensure_open(),
        }
    }

    /// Ingests a `step.delta` frame: the delta kind decides the IR event. A content delta for a
    /// step with no recorded start implicitly opens the matching kind, so a missed start cannot
    /// strand content; an implicit tool call mints its id and allocates its index the same way a
    /// start would. A delta whose kind contradicts the step already open at its index is dropped
    /// rather than misattributed to foreign content.
    fn ingest_delta(&mut self, frame: StepDeltaFrame) -> Vec<IrEvent> {
        let mut events = Vec::new();

        match frame.delta {
            StepDelta::Text { text } => {
                events.extend(
                    self.implicit_open(frame.index, |_| OpenStep::Output { index: frame.index }),
                );

                // A delta for a step of another kind contradicts the opened kind, so it is
                // dropped rather than misattributed to foreign content.
                if matches!(self.find_open(frame.index), Some(OpenStep::Output { .. })) {
                    // Only delivered bytes count as live delivery; a payload-less delta delivered
                    // nothing and leaves the terminal fallback live (README §6.3).
                    if !text.is_empty() {
                        self.is_step_delivered = true;
                    }

                    events.push(IrEvent::TextDelta(text));
                }
            }

            StepDelta::ThoughtSummary { content } => {
                // A non-text summary block — the default of an absent `content` member included —
                // carries no IR-representable text and is a no-op that opens nothing.
                if content.block_type != "text" {
                    return events;
                }

                events.extend(self.implicit_open(frame.index, |_| OpenStep::thought(frame.index)));

                // A delta for a step of another kind contradicts the opened kind, so it is
                // dropped rather than misattributed to foreign content. Each text-carrying
                // delta is one summary block: the boundaries the official upstream declared.
                if let Some(OpenStep::Thought { summary: open, .. }) = self.find_open(frame.index) {
                    open.push(content.text.clone());

                    // Only delivered bytes count as live delivery; a payload-less delta
                    // delivered nothing and leaves the terminal fallback live (README §6.3).
                    if !content.text.is_empty() {
                        self.is_step_delivered = true;
                    }

                    events.push(IrEvent::ReasoningDelta(content.text));
                }
            }

            StepDelta::ThoughtSignature { signature } => {
                // An absent signature member carries nothing: a no-op that must never overwrite
                // or clear a signature already streamed for the step.
                let Some(signature) = signature else {
                    return events;
                };

                events.extend(self.implicit_open(frame.index, |_| OpenStep::thought(frame.index)));

                if let Some(OpenStep::Thought {
                    signature: open, ..
                }) = self.find_open(frame.index)
                {
                    *open = Some(signature);
                }
            }

            StepDelta::ArgumentsDelta { arguments } => {
                events.extend(self.implicit_open(frame.index, |parser| {
                    OpenStep::call(parser, frame.index, None, None)
                }));

                // A delta for a step of another kind contradicts the opened kind, so it is
                // dropped rather than misattributed to foreign content.
                let mut is_payload_delivered = false;

                if let Some(OpenStep::Call {
                    call_index,
                    id,
                    name,
                    arguments: open,
                    ..
                }) = self.find_open(frame.index)
                {
                    open.push_str(&arguments);

                    // Only delivered bytes count as live delivery; a payload-less delta
                    // delivered nothing and leaves the terminal fallback live (README §6.3).
                    // The flag applies after the step borrow ends.
                    is_payload_delivered = !arguments.is_empty();

                    events.push(IrEvent::ToolCallDelta {
                        index: *call_index,
                        call_id: Some(id.clone()),
                        name: Some(name.clone()),
                        arguments_delta: arguments,
                    });
                }

                if is_payload_delivered {
                    self.is_step_delivered = true;
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
                // One summary block per streamed delta: the synthesized step preserves the block
                // boundaries the upstream declared, exactly as the JSON body path preserves the
                // raw step's blocks verbatim.
                let summary_blocks = summary
                    .iter()
                    .map(|text| super::ContentBlock::Text { text: text.clone() })
                    .collect::<Vec<_>>();

                let step = TypedStep::Thought {
                    summary: summary_blocks,
                    signature: signature.clone(),
                };

                #[expect(
                    clippy::expect_used,
                    reason = "the first-party wire type serializes totally"
                )]
                let opaque = serde_json::to_value(step).expect("the thought step serializes");

                // The done's text stays empty: the deltas are the authoritative bytes and the
                // empty-text done merely closes the block they delivered — the §6.2
                // deltas-then-empty-done pairing, exactly as the GenerateContent parser pairs
                // them — so no surface ever renders a summary byte twice.
                events.push(IrEvent::ReasoningDone {
                    text: String::new(),
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
                // The done always delivers the complete call — the streamed argument bytes,
                // or the empty object a zero-argument call's state assembles to — so it marks
                // the call delivered and silences the terminal resource's `function_call`
                // copies: recovering the same call there would deliver it twice.
                self.has_delivered_call = true;

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
    /// the frame's partial resource carries one. The official terminal resource carries its steps
    /// only when the upstream includes them ("if included in this event"), so the resource's final
    /// `steps` are the exactly-once fallback content source for the kinds no live
    /// `step.start`/`step.delta` event delivered, ingested through the same complete-step path as
    /// a body's steps: text and thought steps answer to the live content-delivery record, and
    /// `function_call` steps to the live call-delivery record alone — a stream whose text was
    /// delivered live still recovers a call it only opened. Content a live event did deliver is
    /// never delivered twice.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for the failed status and for a recovered step whose kind
    /// contradicts the contract.
    fn ingest_completed(&mut self, frame: CompletedFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        // The gates read the delivery records, which the complete-step walk below never
        // mutates, so every recovered step sees the same pre-walk delivery state.
        for step in frame.interaction.steps {
            // Text and thought steps are gated as a kind: any live content-bearing delivery
            // silences their terminal copies, so no text or thought is ever delivered twice.
            if matches!(
                step,
                CompleteStep::ModelOutput(_) | CompleteStep::Thought(_)
            ) && self.is_step_delivered
            {
                continue;
            }

            // A call delivered live silences the terminal copies of calls: the terminal
            // resource echoes the steps a conformant stream already delivered, and re-ingesting
            // an echoed call would deliver it a second time under a fresh index the
            // aggregator's per-index dedup cannot catch. The gate is the call kind's own
            // delivery record, independent of [`Self::is_step_delivered`]: a call only opened,
            // or one the stream never surfaced at all, stays recoverable even beside delivered
            // text.
            if matches!(step, CompleteStep::FunctionCall(_)) && self.has_delivered_call {
                continue;
            }

            events.extend(self.ingest_complete_step(step)?);
        }

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
        // Borrow the complete JSON document while the typed probe selects its event vocabulary;
        // only producer-owned thought fragments need an arbitrary JSON representation.
        let payload: &serde_json::value::RawValue = format::parse_frame(frame)?;

        // The probe is typed: any frame bearing a failure aborts, recognized event or not.
        let probe: FrameProbe = serde_json::from_str::<FrameProbe>(payload.get())
            .map_err(|_| PayloadError::new("malformed upstream payload in SSE frame"))?;

        if let Some(error) = probe.error {
            return Err(error.failure());
        }

        // The `event:` line discriminates when the frame carries one, else the payload's official
        // `event_type` member self-describes it. Neither present yields a heartbeat.
        match frame.event.as_deref().or(probe.event_type.as_deref()) {
            Some(EVENT_CREATED) => {
                let created: CreatedFrame = format::parse_body(payload.get().as_bytes())?;

                Ok(self.open_events(created.interaction.id, created.interaction.model))
            }

            Some(EVENT_STEP_START) => {
                let start: StepStartFrame = format::parse_body(payload.get().as_bytes())?;

                Ok(self.ingest_start(start))
            }

            Some(EVENT_STEP_DELTA) => {
                let delta: StepDeltaFrame = format::parse_body(payload.get().as_bytes())?;

                Ok(self.ingest_delta(delta))
            }

            Some(EVENT_STEP_STOP) => {
                let stop: StepStopFrame = format::parse_body(payload.get().as_bytes())?;

                self.ingest_stop(stop)
            }

            Some(EVENT_COMPLETED) => {
                let completed: CompletedFrame = format::parse_body(payload.get().as_bytes())?;

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
    fn parse_json_response(&mut self, body: &[u8]) -> Result<Vec<IrEvent>, PayloadError> {
        // Ingesting the body always ingests the terminal status, which emits the body's own finish
        // and marks the parser finished — the body closes its stream, never the finish hook.
        self.ingest_body(format::parse_body(body)?)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        format::degrade_finish_once(&mut self.is_finished)
    }
}
