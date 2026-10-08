//! Upstream-event parsing for the Responses codec: parses upstream SSE events or a complete JSON
//! body into IR events.
//!
//! Upstream payloads are third-party, so the models below carry exactly the members the codec
//! reads, ignore every member it does not, and default what upstreams legitimately omit.

use crate::{
    format::{
        self, PayloadError, UpstreamError, UpstreamEventParser,
        openai::{CompletionTokensDetails, PromptTokensDetails},
    },
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Parses upstream Responses payloads — SSE events or a full JSON body — into IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Message-text fallback-recovery state: live delta delivery exhausts every fallback source
    /// for the kind, and per-index records keep each item from delivering twice.
    text_recovery: FallbackRecovery,

    /// Reasoning-text fallback-recovery state; same contract as `text_recovery`.
    reasoning_recovery: FallbackRecovery,

    /// Output indices whose function call live events already delivered the call. Only
    /// content-bearing call events count as live delivery — streamed argument bytes for the
    /// index, or a streamed `output_item.done` item completing it — because a bare
    /// `response.output_item.added` opening carries the item skeleton without argument bytes and
    /// delivers no call content, so it never exhausts the terminal output array's copy of the
    /// call (README §6.3).
    delivered_call_indices: IndexRecord,

    /// Output indices whose function call a [`IrEvent::ToolCallDone`] already completed: a
    /// streamed done item for an already-completed index is suppressed instead of re-emitting
    /// the completion (README §6.2). This record is narrower than the delivered-call record —
    /// streamed argument bytes deliver content without completing the call, and the streamed
    /// done item must still deliver that completion.
    completed_call_indices: IndexRecord,

    /// Whether a finish was already emitted; `finish` degrades a stream that ends without one
    /// through [`format::degrade_finish_once`].
    is_finished: bool,

    /// Identities of the compaction items already delivered as events, so a
    /// conformant stream's terminal-array copies do not double-deliver.
    delivered_compaction_identities: Vec<String>,
}

/// Which walk of the upstream output produced a compaction item: a live
/// streamed completion, or a terminal array/JSON body walk whose copies are
/// gated on live delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionItemOrigin {
    /// A streamed `output_item.done` frame delivered the item.
    Live,

    /// The terminal event's output array, or a complete JSON body, carried it.
    Terminal,
}

/// Computes a compaction item's delivery identity: the producer-minted `id`
/// when present, otherwise the item's serialized bytes, so a conformant
/// stream's terminal copy of a live-delivered item is recognized.
fn compaction_item_identity(item: &Value) -> String {
    /// Typed identity probe that ignores all producer-owned replay metadata.
    #[derive(Deserialize)]
    struct Identity {
        /// Optional provider identity; non-string extension values do not identify the item.
        #[serde(default, deserialize_with = "crate::serde_helpers::string_or_none")]
        id: Option<String>,
    }
    match Identity::deserialize(item)
        .ok()
        .and_then(|identity| identity.id)
    {
        Some(id) => format!("id:{id}"),
        None => format!("raw:{item}"),
    }
}

/// One upstream Responses event: the union of the members the gateway reads across the event
/// vocabulary, selected by `event_type`.
///
/// Upstreams are third-party, so every member is optional — each event type carries its own subset,
/// and upstreams attach identifiers, sequence numbers, and provider extensions the gateway never
/// reads — and no member is required. The `error` member is read on every event type: any frame
/// bearing one is an upstream failure, recognized type or not (README §6.7).
#[derive(Debug, Default, Deserialize)]
struct UpstreamEvent {
    /// Event type discriminator.
    #[serde(rename = "type", default)]
    event_type: String,

    /// Failure the upstream may embed in any event.
    #[serde(default)]
    error: Option<UpstreamError>,

    /// The response object of the response-level events, and of a complete
    /// non-streaming body.
    #[serde(default)]
    response: Option<ResponseObject>,

    /// The output item of the item-level events.
    #[serde(default)]
    item: Option<OutputItem>,

    /// Position of the item within the response's output array.
    #[serde(default)]
    output_index: Option<u64>,

    /// Incremental text of the delta events.
    #[serde(default)]
    delta: Option<String>,

    /// Failure text of the bare `error` event.
    #[serde(default)]
    message: Option<String>,
}

/// The upstream response object, carried by the response-level events as the `response` member and
/// by a complete non-streaming body as the body itself.
#[derive(Debug, Default, Deserialize)]
struct ResponseObject {
    /// Upstream response identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,

    /// Response status, deciding the finish mapping.
    #[serde(default)]
    status: Option<String>,

    /// The completed output items, in output order.
    #[serde(default)]
    output: Vec<OutputItem>,

    /// Why an `incomplete` response was cut short.
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UpstreamUsage>,

    /// Failure the upstream attached to the response.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Why an upstream response is incomplete.
#[derive(Debug, Default, Deserialize)]
struct IncompleteDetails {
    /// Upstream reason string.
    #[serde(default)]
    reason: Option<String>,
}

/// Upstream token accounting in the Responses shape.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpstreamUsage {
    /// Tokens consumed by the input.
    #[serde(default)]
    pub(crate) input_tokens: u64,

    /// Tokens produced by the output.
    #[serde(default)]
    pub(crate) output_tokens: u64,

    /// Input token breakdown, absent when the upstream reports totals only.
    #[serde(default)]
    pub(crate) input_tokens_details: Option<InputTokensDetails>,

    /// Output token breakdown, absent when the upstream reports totals only.
    #[serde(default)]
    pub(crate) output_tokens_details: Option<OutputTokensDetails>,

    /// Messages the upstream dropped while compacting (xAI extension),
    /// absent when the upstream does not report it.
    #[serde(default)]
    pub(super) dropped_message_count: Option<u64>,
}

/// The object the Responses usage wrapper carries as `input_tokens_details`: the shared official
/// input-side usage detail object [`PromptTokensDetails`] under its product-local name, its
/// `cached_tokens` and `cache_write_tokens` members spelled identically on both wires (README
/// §6.2).
type InputTokensDetails = PromptTokensDetails;

/// The object the Responses usage wrapper carries as `output_tokens_details`: the shared official
/// output-side usage detail object [`CompletionTokensDetails`] under its product-local name, its
/// lone `reasoning_tokens` member spelled identically on both wires (README §6.2).
type OutputTokensDetails = CompletionTokensDetails;

/// One protocol output item, with arbitrary JSON confined to native compaction state.
#[derive(Debug)]
enum OutputItem {
    /// Ordinary output fields consumed by the IR translator.
    Ordinary(OutputFields),
    /// Producer-owned compaction item preserved including all extensions.
    Compaction(Value),
}

impl<'de> Deserialize<'de> for OutputItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// The item kind alone; other members belong to the selected variant.
        #[derive(Deserialize)]
        struct Kind {
            /// Missing tags keep the ordinary ignored-item behavior.
            #[serde(rename = "type", default)]
            kind: String,
        }
        let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
        let kind: Kind = serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
        if kind.kind == "compaction" {
            serde_json::from_str(raw.get())
                .map(Self::Compaction)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_str(raw.get())
                .map(Self::Ordinary)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl OutputItem {
    /// Borrows ordinary fields; native state never enters the ordinary translator.
    fn ordinary(&self) -> Option<&OutputFields> {
        match self {
            Self::Ordinary(fields) => Some(fields),
            Self::Compaction(_) => None,
        }
    }
}

/// One upstream output item: the union of the fields the gateway reads across the item kinds it
/// maps to IR events. Items of other kinds carry none of them and are ignored.
#[derive(Debug, Default, Deserialize)]
struct OutputFields {
    /// Item kind discriminator.
    #[serde(rename = "type", default)]
    item_type: String,

    /// Content parts of a `message` item.
    #[serde(default)]
    content: Vec<ContentPart>,

    /// Summary parts of a `reasoning` item.
    #[serde(default)]
    summary: Vec<SummaryPart>,

    /// Identifier of the invocation of a `function_call` item.
    #[serde(default)]
    call_id: String,

    /// Name of the invoked function of a `function_call` item.
    #[serde(default)]
    name: String,

    /// JSON-encoded arguments of a `function_call` item.
    #[serde(default)]
    arguments: String,

    /// Upstream thought signature the gateway extension carries (README §6.6).
    #[serde(default)]
    thought_signature: Option<String>,
}

/// One content part of a `message` item. Refusal text is message text: it rides the same text
/// channel as `output_text` parts (README §6.2). Parts of other kinds carry no text and are
/// ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentPart {
    /// Assistant output text.
    #[serde(rename = "output_text")]
    OutputText {
        /// The text, absent when the part carries none.
        #[serde(default)]
        text: Option<String>,
    },

    /// A model refusal.
    #[serde(rename = "refusal")]
    Refusal {
        /// The refusal text, absent when the part carries none.
        #[serde(default)]
        refusal: Option<String>,
    },

    /// Any other part kind, which carries no text.
    #[serde(other)]
    Other,
}

/// One summary part of a `reasoning` item.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SummaryPart {
    /// Reasoning summary text.
    #[serde(rename = "summary_text")]
    SummaryText {
        /// The text, absent when the part carries none.
        #[serde(default)]
        text: Option<String>,
    },

    /// Any other part kind, which carries no text.
    #[serde(other)]
    Other,
}

/// Exactly-once record of output indices: an index is recorded at most once, so whatever the
/// record guards — a recovered item, a delivered or completed call — never happens twice for the
/// same index while distinct indices each record exactly once. Queries and writes scan the
/// recorded indices linearly, and a repeat write for an already-recorded index is a no-op.
#[derive(Debug, Default)]
struct IndexRecord {
    /// The recorded indices, in first-recorded order.
    indices: Vec<usize>,
}

impl IndexRecord {
    /// Whether `index` is already recorded.
    fn is_recorded(&self, index: usize) -> bool {
        self.indices.contains(&index)
    }

    /// Records `index`; a repeat for the same index is ignored.
    fn record(&mut self, index: usize) {
        if !self.indices.contains(&index) {
            self.indices.push(index);
        }
    }

    /// Whether nothing is recorded yet.
    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Exactly-once fallback-recovery state for one output kind: whether live delta events already
/// delivered the kind — exhausting every fallback source for it — and the output indices whose item
/// a fallback source, a streamed `output_item.done` item or a terminal output-array item, already
/// delivered, so the same item never delivers twice across sources while distinct items each
/// deliver exactly once.
#[derive(Debug, Default)]
struct FallbackRecovery {
    /// Whether live delta events already delivered the kind.
    is_delivered: bool,

    /// Output indices whose item a fallback source already delivered.
    recovered_indices: IndexRecord,
}

/// Origin of an upstream output item, deciding how it maps to IR events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputItemOrigin {
    /// An item of a complete JSON response body; the value is the item's position in the `output`
    /// array.
    Json(usize),

    /// A streaming `output_item.done` event; the value is the upstream `output_index`.
    StreamDone(usize),

    /// An item of the terminal event's `response.output` array; the value is the item's position,
    /// used as the index of a call the terminal walk recovers. Consulted only for kinds that no
    /// live event delivered.
    Terminal(usize),
}

impl FallbackRecovery {
    /// Whether a fallback source may recover the item at `index`: live deltas exhaust every
    /// fallback source for the kind, and a fallback delivery for `index` exhausts further sources
    /// for the same item.
    fn can_recover(&self, index: usize) -> bool {
        !self.is_delivered && !self.recovered_indices.is_recorded(index)
    }

    /// Records that a fallback source recovered the item at `index`; repeats for further parts of
    /// the same item are ignored.
    fn record(&mut self, index: usize) {
        self.recovered_indices.record(index);
    }
}

impl OutputItemOrigin {
    /// The output index a fallback recovery keys on, or [`None`] for the JSON body origin, whose
    /// items always deliver.
    fn recovery_index(self) -> Option<usize> {
        match self {
            Self::Json(_) => None,

            Self::StreamDone(index) | Self::Terminal(index) => Some(index),
        }
    }
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one upstream event object.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream failure events.
    fn ingest_event(&mut self, event: UpstreamEvent) -> Result<Vec<IrEvent>, PayloadError> {
        // Any frame bearing a top-level error object is an upstream failure, recognized event type
        // or not; without this check such a frame would fall through to the unknown-type arm and
        // parse as an empty success (README §6.7).
        if let Some(error) = &event.error {
            return Err(error.failure());
        }

        match event.event_type.as_str() {
            "response.created" | "response.in_progress" => {
                Ok(self.message_start_events(event.response.as_ref()))
            }

            "response.output_item.added" => {
                let mut events = self.message_start_events(event.response.as_ref());

                let output_index = event_output_index(&event);

                if let Some(item) = event.item.as_ref().and_then(OutputItem::ordinary)
                    && item.item_type == "function_call"
                {
                    events.push(self.open_function_call(item, output_index));
                }

                Ok(events)
            }

            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let mut events = self.message_start_events(event.response.as_ref());

                let deltas = delta_event(&event, IrEvent::ReasoningDelta);

                // Only an event that actually carried a delta exhausts the fallback sources; a
                // delta-typed event without a payload delivered nothing.
                if !deltas.is_empty() {
                    self.reasoning_recovery.is_delivered = true;
                }

                events.extend(deltas);

                Ok(events)
            }

            "response.output_text.delta" | "response.refusal.delta" => {
                let mut events = self.message_start_events(event.response.as_ref());

                let deltas = delta_event(&event, IrEvent::TextDelta);

                if !deltas.is_empty() {
                    self.text_recovery.is_delivered = true;
                }

                events.extend(deltas);

                Ok(events)
            }

            "response.function_call_arguments.delta" => {
                let mut events = self.message_start_events(event.response.as_ref());

                let output_index = event_output_index(&event);

                let arguments_delta = event.delta.clone().unwrap_or_default();

                // Only streamed argument bytes count as live delivery of the call; an
                // argument-delta event without a payload delivered nothing and leaves the
                // terminal output array's copy as the call's recovery source (README §6.3).
                if !arguments_delta.is_empty() {
                    self.record_call_delivered(output_index);
                }

                events.push(IrEvent::ToolCallDelta {
                    index: output_index,
                    call_id: None,
                    name: None,
                    arguments_delta,
                });

                Ok(events)
            }

            "response.output_item.done" => {
                let mut events = self.message_start_events(event.response.as_ref());

                if let Some(item) = &event.item {
                    let output_index = event_output_index(&event);

                    self.ingest_output_item(
                        item,
                        OutputItemOrigin::StreamDone(output_index),
                        &mut events,
                    );
                }

                Ok(events)
            }

            "response.completed" | "response.incomplete" => {
                let mut events = self.message_start_events(event.response.as_ref());

                // An absent response object behaves exactly as an empty one: no output to recover,
                // no usage, and the delivered-calls flag alone deciding the finish.
                let response = event.response.unwrap_or_default();

                // A third-party static-key upstream may carry items only in the terminal
                // response object; when no live event delivered a kind, recover it from the output
                // array exactly as from a JSON response body. The walk is gated per item inside
                // `ingest_output_item` — text and reasoning against the live-delivery flags and
                // the per-index recovery records, calls against the delivered record of the item's
                // own array position — so every kind delivers exactly once however the upstream
                // shaped its stream.
                for (position, item) in response.output.iter().enumerate() {
                    self.ingest_output_item(
                        item,
                        OutputItemOrigin::Terminal(position),
                        &mut events,
                    );
                }

                self.is_finished = true;

                // The walk above recorded every call it recovered into the delivered indices, so
                // calls recovered from the output array decide the finish mapping too.
                events.push(IrEvent::finish(finish_reason_for_response(
                    &response,
                    self.has_delivered_call(),
                )));

                push_usage_events(&response, &mut events);

                Ok(events)
            }

            "response.failed" => Err(PayloadError::new(
                event
                    .response
                    .as_ref()
                    .and_then(|response| response.error.as_ref())
                    .and_then(UpstreamError::message)
                    .unwrap_or("upstream response failed"),
            )),

            "error" => Err(PayloadError::new(
                event.message.as_deref().unwrap_or("upstream stream error"),
            )),

            _ => Ok(Vec::new()),
        }
    }

    /// Ingests one output item from a JSON response body, a streaming `output_item.done` event, or
    /// the terminal event's output array.
    ///
    /// Conformant upstreams deliver message text, reasoning text, and tool calls as live events, so
    /// streamed and terminal items are fallback sources consulted only when no live event delivered
    /// that kind of item — third-party static-key upstreams may stream complete items
    /// without deltas, and their items must still reach the client exactly once. Fallback recovery
    /// of text and reasoning is keyed by the item's output index: a recovery delivery for one index
    /// never silences another item of the same kind, and one index is never delivered twice, so the
    /// terminal array's copy of an item already delivered as a streamed done event is recognized by
    /// the shared index. Tool-call recovery is keyed per index on content-bearing live events only:
    /// streamed argument bytes or a streamed done item count as live delivery of that index's call,
    /// while a bare `output_item.added` opening — the item skeleton without argument bytes — does
    /// not (README §6.3).
    fn ingest_output_item(
        &mut self,
        item: &OutputItem,
        origin: OutputItemOrigin,
        events: &mut Vec<IrEvent>,
    ) {
        let Some(item) = item.ordinary() else {
            return;
        };
        match item.item_type.as_str() {
            "message" => {
                let should_extract = match origin.recovery_index() {
                    Some(index) => self.text_recovery.can_recover(index),
                    None => true,
                };

                if !should_extract {
                    return;
                }

                for part in &item.content {
                    // Refusal text is message text: it rides the same text channel as `output_text`
                    // parts (README §6.2).
                    let text = match part {
                        ContentPart::OutputText { text } => text.as_deref(),

                        ContentPart::Refusal { refusal } => refusal.as_deref(),

                        ContentPart::Other => None,
                    };

                    if let Some(text) = text {
                        // The recovery record is written only when a text part was actually pushed,
                        // so a text-less item does not exhaust its output index for a later item
                        // that carries the text.
                        if let Some(index) = origin.recovery_index() {
                            self.text_recovery.record(index);
                        }

                        events.push(IrEvent::TextDelta(text.to_string()));
                    }
                }
            }

            "reasoning" => {
                let should_extract = match origin.recovery_index() {
                    Some(index) => self.reasoning_recovery.can_recover(index),
                    None => true,
                };

                if !should_extract {
                    return;
                }

                for part in &item.summary {
                    let SummaryPart::SummaryText { text } = part else {
                        continue;
                    };

                    if let Some(text) = text {
                        // Same per-extraction rule as the message arm: a summary-less item does not
                        // exhaust its output index.
                        if let Some(index) = origin.recovery_index() {
                            self.reasoning_recovery.record(index);
                        }

                        events.push(IrEvent::ReasoningDelta(text.clone()));
                    }
                }
            }

            "function_call" => match origin {
                // A JSON body delivers the whole call at once: the shared complete-call
                // announcement (`format::complete_call_events`) — a full-arguments delta plus a
                // done — opens and completes the item for the streaming renderers.
                OutputItemOrigin::Json(index) => {
                    self.record_call_delivered(index);
                    self.record_call_completed(index);

                    events.extend(format::complete_call_events(
                        index,
                        item.call_id.clone(),
                        item.name.clone(),
                        item.arguments.clone(),
                        item.thought_signature.clone(),
                    ));
                }

                // A streamed done item is content-bearing: it completes its call and exhausts the
                // terminal output array's copy of the same index (README §6.3) — but a done for an
                // index another done already completed is suppressed, so a duplicating upstream
                // cannot re-deliver the call (README §6.2). Argument bytes streamed for the index
                // do not complete it: the done item is still the event that must deliver the
                // completion.
                OutputItemOrigin::StreamDone(index) => {
                    if !self.is_call_completed(index) {
                        self.record_call_delivered(index);
                        self.record_call_completed(index);

                        events.push(self.complete_function_call(item, index));
                    }
                }

                // The terminal fallback completes a call exactly-once: the output array delivers
                // the call at this index only when no content-bearing live event — streamed
                // argument bytes or a streamed done item — delivered it; re-emitting a delivered
                // call would duplicate it, while a call only opened by `output_item.added` still
                // recovers here because the bare opening carries no argument bytes (README §6.3).
                // The record is written during the walk, which is safe because every index is
                // visited once and each delivery decision keys on its own index.
                OutputItemOrigin::Terminal(index) => {
                    if !self.is_call_delivered(index) {
                        self.record_call_delivered(index);
                        self.record_call_completed(index);

                        events.push(self.complete_function_call(item, index));
                    }
                }
            },

            _ => {}
        }
    }

    /// Emits the opening delta for a function call item revealed by `output_item.added`.
    ///
    /// The official opening event carries the item skeleton only — call id and name, no argument
    /// bytes, which arrive through `response.function_call_arguments.delta` — so the opening
    /// delta reveals the call to streaming renderers without delivering call content and never
    /// counts as live delivery (README §6.3).
    fn open_function_call(&self, item: &OutputFields, output_index: usize) -> IrEvent {
        IrEvent::ToolCallDelta {
            index: output_index,
            call_id: Some(item.call_id.clone()),
            name: Some(item.name.clone()),
            arguments_delta: String::new(),
        }
    }

    /// Builds the [`IrEvent::ToolCallDone`] completing the function call item at `index`.
    fn complete_function_call(&self, item: &OutputFields, index: usize) -> IrEvent {
        IrEvent::ToolCallDone {
            index,
            call_id: item.call_id.clone(),
            name: item.name.clone(),
            arguments: item.arguments.clone(),
            thought_signature: item.thought_signature.clone(),
        }
    }

    /// Whether the function call at `index` was already delivered — by a content-bearing live
    /// event, or by the terminal walk's own recovery of the index.
    fn is_call_delivered(&self, index: usize) -> bool {
        self.delivered_call_indices.is_recorded(index)
    }

    /// Records the function call at `index` as delivered; repeats for the same index are ignored.
    fn record_call_delivered(&mut self, index: usize) {
        self.delivered_call_indices.record(index);
    }

    /// Whether a [`IrEvent::ToolCallDone`] already completed the function call at `index`.
    fn is_call_completed(&self, index: usize) -> bool {
        self.completed_call_indices.is_recorded(index)
    }

    /// Records the function call at `index` as completed; repeats for the same index are ignored.
    fn record_call_completed(&mut self, index: usize) {
        self.completed_call_indices.record(index);
    }

    /// Whether the turn delivered any function call, deciding the canonical tool-call finish.
    fn has_delivered_call(&self) -> bool {
        !self.delivered_call_indices.is_empty()
    }

    /// Emits [`IrEvent::MessageStart`] exactly once, first (README §6.2): every ingest arm opens
    /// with this call, so recovery paths that never see the live
    /// `response.created`/`response.in_progress` events — no-live-delta upstreams per README §6.3 —
    /// still open their event stream with it, and the once-only guard turns repeats into no-ops.
    ///
    /// The identity comes from the response object the event carries; an event carrying none opens
    /// the stream with empty identity.
    fn message_start_events(&mut self, response: Option<&ResponseObject>) -> Vec<IrEvent> {
        let (id, model) = match response {
            Some(response) => (response.id.clone(), response.model.clone()),

            None => (String::new(), String::new()),
        };

        format::message_start_once(&mut self.is_started, id, model)
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a `response.*` event.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON and upstream failure events.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let event: UpstreamEvent = format::parse_frame(frame)?;

        // The opening events precede the compaction pre-pass: a stream whose first frame
        // carries a compaction item — a streamed `output_item.done` item, or a terminal-only
        // static-key stream whose sole frame is a terminal response event — must still open
        // with MessageStart (README §6.2). Only the frame kinds the compaction pre-pass reads
        // are opened here; every one of them opens the stream in `ingest_event` too, so no
        // other frame — an unknown type above all — gains a spurious opening, and the
        // once-only guard turns `ingest_event`'s own opening into a no-op.
        let mut events = match event.event_type.as_str() {
            "response.output_item.done" | "response.completed" | "response.incomplete" => {
                self.message_start_events(event.response.as_ref())
            }
            _ => Vec::new(),
        };

        self.record_frame_compaction_items(&event, &mut events);

        events.extend(self.ingest_event(event)?);

        Ok(events)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream failure statuses and malformed bodies.
    fn parse_json_response(&mut self, body: &[u8]) -> Result<Vec<IrEvent>, PayloadError> {
        let response = format::parse_body::<ResponseObject>(body)?;

        // A 2xx body carrying a top-level error object is an upstream failure (README §6.7), not an
        // empty success; the failed-status fallback below still covers a failure without an error
        // object.
        if let Some(error) = &response.error {
            return Err(error.failure());
        }

        let mut events = self.message_start_events(Some(&response));

        // The compaction items ride the event stream at the output head (a
        // native compaction precedes the answer output it precedes upstream);
        // the compaction renderers own their outward lifecycle through these
        // events alone.
        for item in &response.output {
            if let OutputItem::Compaction(item) = item {
                events.extend(self.record_compaction_item(item, CompactionItemOrigin::Terminal));
            }
        }

        for (position, item) in response.output.iter().enumerate() {
            self.ingest_output_item(item, OutputItemOrigin::Json(position), &mut events);
        }

        if response.status.as_deref() == Some("failed") {
            let message = response
                .error
                .as_ref()
                .and_then(UpstreamError::message)
                .unwrap_or("upstream response failed");

            return Err(PayloadError::new(message));
        }

        self.is_finished = true;

        // The output walk above recorded every `function_call` item the body carried into the
        // delivered indices.
        events.push(IrEvent::finish(finish_reason_for_response(
            &response,
            self.has_delivered_call(),
        )));

        push_usage_events(&response, &mut events);

        Ok(events)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        format::degrade_finish_once(&mut self.is_finished)
    }
}

/// The compact endpoint's upstream response body, read out of the complete JSON
/// document a standalone compact dispatch answers with: the identity and
/// creation time when the upstream carries them, the complete ordered output
/// window, and the usage.
#[derive(Debug, Deserialize)]
struct CompactedResponseBodyIn {
    /// Upstream response identifier.
    #[serde(default)]
    id: Option<String>,

    /// Unix creation timestamp in seconds.
    #[serde(default)]
    created_at: Option<i64>,

    /// The complete ordered output window: retained items plus the single
    /// compaction item, verbatim.
    #[serde(default)]
    output: Vec<Value>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UpstreamUsage>,

    /// Failure the upstream attached to the response; a present, non-null
    /// member is a failed operation (README §6.7).
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// The parsed native result of a standalone compact dispatch: everything the
/// outward `response.compaction` body carries through.
#[derive(Debug)]
pub(crate) struct CompactedResponseNative {
    /// Upstream response identifier, when the upstream carried one.
    pub(super) id: Option<String>,

    /// The complete ordered output window, verbatim.
    pub(crate) output: Vec<Value>,

    /// Token accounting, when the upstream reported it.
    pub(super) usage: Option<UpstreamUsage>,

    /// Unix creation timestamp in seconds, when the upstream carried one.
    pub(super) created_at: Option<i64>,
}

/// Parses a standalone compact upstream response body.
///
/// A top-level non-null `error` member is a failed operation (README §6.7).
/// The body is usable only when its output is non-empty and carries exactly one
/// `compaction` item — a native compacted window always contains retained
/// items or the compaction item, and any other shape cannot stand in for a
/// compaction result (README §6.8).
///
/// # Errors
///
/// Returns [`PayloadError`] for a malformed body, an embedded error member, or
/// an unusable output window.
pub(crate) fn parse_compact_response(body: &[u8]) -> Result<CompactedResponseNative, PayloadError> {
    let body = format::parse_body::<CompactedResponseBodyIn>(body)?;

    if let Some(error) = &body.error {
        return Err(error.failure());
    }

    let compaction_count = body
        .output
        .iter()
        .filter(|item| is_compaction_item(item))
        .count();

    if body.output.is_empty() {
        return Err(PayloadError::new(
            "the upstream compacted response carries no output items",
        ));
    }

    if compaction_count != 1 {
        return Err(PayloadError::new(format!(
            "the upstream compacted response must carry exactly one compaction item; \
             it carried {compaction_count}"
        )));
    }

    Ok(CompactedResponseNative {
        id: body.id,
        output: body.output,
        usage: body.usage,
        created_at: body.created_at,
    })
}

/// Whether one raw output item is a `compaction` item. The probe reads the
/// item's own type tag only: the rest of the item is vendor-shaped state whose
/// members the gateway does not define.
fn is_compaction_item(item: &Value) -> bool {
    #[derive(Deserialize)]
    struct TypeProbe {
        /// Item kind discriminator.
        #[serde(rename = "type")]
        item_type: String,
    }

    TypeProbe::deserialize(item).is_ok_and(|probe| probe.item_type == "compaction")
}

impl UpstreamParser {
    /// Records one raw output item verbatim when it is a compaction item,
    /// yielding its [`IrEvent::CompactionState`] at the arrival position. A
    /// terminal array's copy of an already live-delivered item is skipped by
    /// the item's `id` (or its serialized bytes when the producer minted no
    /// id), so a conformant stream delivers every distinct native compaction
    /// exactly once.
    fn record_compaction_item(
        &mut self,
        item: &Value,
        origin: CompactionItemOrigin,
    ) -> Vec<IrEvent> {
        if !is_compaction_item(item) {
            return Vec::new();
        }

        let identity = compaction_item_identity(item);

        if origin == CompactionItemOrigin::Terminal
            && self.delivered_compaction_identities.contains(&identity)
        {
            return Vec::new();
        }

        self.delivered_compaction_identities.push(identity);

        vec![IrEvent::CompactionState {
            payload: item.clone(),
        }]
    }

    /// Records the compaction items one stream frame carries: a completed item
    /// event's own item, or a terminal response event's output array. The item
    /// slots are read off the raw value because the compaction items are
    /// vendor-shaped fragments preserved verbatim.
    fn record_frame_compaction_items(&mut self, event: &UpstreamEvent, events: &mut Vec<IrEvent>) {
        match event.event_type.as_str() {
            "response.output_item.done" => {
                if let Some(OutputItem::Compaction(item)) = &event.item {
                    events.extend(self.record_compaction_item(item, CompactionItemOrigin::Live));
                }
            }
            "response.completed" | "response.incomplete" => {
                if let Some(response) = &event.response {
                    for item in &response.output {
                        if let OutputItem::Compaction(item) = item {
                            events.extend(
                                self.record_compaction_item(item, CompactionItemOrigin::Terminal),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Reads the `output_index` field of an upstream stream event, defaulting to the first position.
fn event_output_index(event: &UpstreamEvent) -> usize {
    event.output_index.unwrap_or_default() as usize
}

/// Builds a delta event from an upstream delta payload; an event carrying no delta delivered
/// nothing.
fn delta_event(event: &UpstreamEvent, make_event_fn: impl Fn(String) -> IrEvent) -> Vec<IrEvent> {
    match &event.delta {
        Some(delta) => vec![make_event_fn(delta.clone())],

        None => Vec::new(),
    }
}

/// Finish reason for a response object already known not to be failed.
///
/// An `incomplete` status maps through `incomplete_details.reason` (unknown reasons degrade to
/// [`IrFinishReason::Length`] — the output was cut short). Any other status finished normally, and
/// a normally finished turn that delivered function calls carries the canonical
/// [`IrFinishReason::ToolCall`] finish: the Responses status vocabulary has no tool-call value
/// (completed / failed / in_progress / cancelled / queued / incomplete), so the delivered calls
/// decide, matching official Chat Completions semantics for text+tool-call mixed turns too.
fn finish_reason_for_response(
    response: &ResponseObject,
    is_call_delivered: bool,
) -> IrFinishReason {
    if response.status.as_deref() != Some("incomplete") {
        if is_call_delivered {
            return IrFinishReason::ToolCall;
        }

        return IrFinishReason::Stop;
    }

    match response
        .incomplete_details
        .as_ref()
        .and_then(|details| details.reason.as_deref())
    {
        Some("content_filter") => IrFinishReason::ContentFilter,

        _ => IrFinishReason::Length,
    }
}

/// Appends an [`IrEvent::Usage`] for the response's usage object, when present.
fn push_usage_events(response: &ResponseObject, events: &mut Vec<IrEvent>) {
    let Some(usage) = &response.usage else {
        return;
    };

    events.push(IrEvent::Usage(IrUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        cache_write_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens),
        reasoning_tokens: usage
            .output_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
    }));
}
