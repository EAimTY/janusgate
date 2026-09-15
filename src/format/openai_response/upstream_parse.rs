//! Upstream-event parsing for the Responses codec: parses upstream SSE events
//! or a complete JSON body into IR events.
//!
//! Upstream payloads are third-party, so the models below carry exactly the
//! members the codec reads, ignore every member it does not, and default what
//! upstreams legitimately omit.

use crate::{
    format::{self, PayloadError, UpstreamError, UpstreamEventParser},
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde::Deserialize;
use serde_json::Value;

/// Parses upstream Responses payloads — SSE events or a full JSON body — into
/// IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Message-text fallback-recovery state: live delta delivery exhausts
    /// every fallback source for the kind, and per-index records keep each
    /// item from delivering twice.
    text_recovery: FallbackRecovery,

    /// Reasoning-text fallback-recovery state; same contract as
    /// `text_recovery`.
    reasoning_recovery: FallbackRecovery,

    /// Whether a live event already delivered a tool call, marking the
    /// terminal output array as an exhausted source for them.
    is_call_delivered: bool,

    /// Whether a finish reason was already emitted; a clean end-of-stream
    /// without one degrades to [`IrFinishReason::Stop`] in the parser's
    /// finish hook, matching the aggregated rendering default.
    is_finished: bool,
}

/// One upstream Responses event: the union of the members the gateway reads
/// across the event vocabulary, selected by `event_type`.
///
/// Upstreams are third-party, so every member is optional — each event type
/// carries its own subset, and upstreams attach identifiers, sequence numbers,
/// and provider extensions the gateway never reads — and no member is
/// required. The `error` member is read on every event type: any frame bearing
/// one is an upstream failure, recognized type or not (README §6.7).
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

/// The upstream response object, carried by the response-level events as the
/// `response` member and by a complete non-streaming body as the body itself.
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
struct UpstreamUsage {
    /// Tokens consumed by the input.
    #[serde(default)]
    input_tokens: u64,

    /// Tokens produced by the output.
    #[serde(default)]
    output_tokens: u64,

    /// Input token breakdown, absent when the upstream reports totals only.
    #[serde(default)]
    input_tokens_details: Option<InputTokensDetails>,

    /// Output token breakdown, absent when the upstream reports totals only.
    #[serde(default)]
    output_tokens_details: Option<OutputTokensDetails>,
}

/// The upstream input token breakdown.
#[derive(Debug, Default, Deserialize)]
struct InputTokensDetails {
    /// Input tokens served from the upstream prompt cache.
    #[serde(default)]
    cached_tokens: Option<u64>,

    /// Input tokens written to the upstream prompt cache.
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

/// The upstream output token breakdown.
#[derive(Debug, Default, Deserialize)]
struct OutputTokensDetails {
    /// Output tokens spent on model reasoning.
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

/// One upstream output item: the union of the fields the gateway reads across
/// the item kinds it maps to IR events. Items of other kinds carry none of
/// them and are ignored.
#[derive(Debug, Default, Deserialize)]
struct OutputItem {
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

/// One content part of a `message` item. Refusal text is message text: it
/// rides the same text channel as `output_text` parts (README §6.2). Parts of
/// other kinds carry no text and are ignored.
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

/// Exactly-once fallback-recovery state for one output kind: whether live
/// delta events already delivered the kind — exhausting every fallback
/// source for it — and the output indices whose item a fallback source, a
/// streamed `output_item.done` item or a terminal output-array item,
/// already delivered, so the same item never delivers twice across sources
/// while distinct items each deliver exactly once.
#[derive(Debug, Default)]
struct FallbackRecovery {
    /// Whether live delta events already delivered the kind.
    is_delivered: bool,

    /// Output indices whose item a fallback source already delivered.
    recovered_indices: Vec<usize>,
}

impl FallbackRecovery {
    /// Whether a fallback source may recover the item at `index`: live
    /// deltas exhaust every fallback source for the kind, and a fallback
    /// delivery for `index` exhausts further sources for the same item.
    fn can_recover(&self, index: usize) -> bool {
        !self.is_delivered && !self.recovered_indices.contains(&index)
    }

    /// Records that a fallback source recovered the item at `index`;
    /// repeats for further parts of the same item are ignored.
    fn record(&mut self, index: usize) {
        if !self.recovered_indices.contains(&index) {
            self.recovered_indices.push(index);
        }
    }
}

/// Origin of an upstream output item, deciding how it maps to IR events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputItemOrigin {
    /// An item of a complete JSON response body; the value is the item's
    /// position in the `output` array.
    Json(usize),

    /// A streaming `output_item.done` event; the value is the upstream
    /// `output_index`.
    StreamDone(usize),

    /// An item of the terminal event's `response.output` array; the value is
    /// the item's position, used as the index of a call the terminal walk
    /// recovers. Consulted only for kinds that no live event delivered.
    Terminal(usize),
}

impl OutputItemOrigin {
    /// The output index a fallback recovery keys on, or [`None`] for the JSON
    /// body origin, whose items always deliver.
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
        // Any frame bearing a top-level error object is an upstream failure,
        // recognized event type or not; without this check such a frame
        // would fall through to the unknown-type arm and parse as an empty
        // success (README §6.7).
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

                if let Some(item) = &event.item
                    && item.item_type == "function_call"
                {
                    self.is_call_delivered = true;

                    events.push(self.open_function_call(item, output_index));
                }

                Ok(events)
            }

            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let mut events = self.message_start_events(event.response.as_ref());

                let deltas = delta_event(&event, IrEvent::ReasoningDelta);

                // Only an event that actually carried a delta exhausts the
                // fallback sources; a delta-typed event without a payload
                // delivered nothing.
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

                self.is_call_delivered = true;

                events.push(IrEvent::ToolCallDelta {
                    index: output_index,
                    call_id: None,
                    name: None,
                    arguments_delta: event.delta.clone().unwrap_or_default(),
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

                // An absent response object behaves exactly as an empty one:
                // no output to recover, no usage, and the delivered-calls
                // flag alone deciding the finish.
                let response = event.response.unwrap_or_default();

                // A third-party `openai_compatible` upstream may carry items
                // only in the terminal response object; when no live event
                // delivered a kind, recover it from the output array exactly
                // as from a JSON response body. Text and reasoning recovery
                // is decided per item inside `ingest_output_item` against
                // the live-delivery flags and the per-index recovery
                // records, so several message or reasoning items of the
                // array each deliver exactly once. The call recovery
                // decision is snapshotted before the walk: the walk must
                // consult the pre-walk state for every item, so a terminal
                // output array carrying several function_call items
                // delivers each of them instead of only the first.
                let is_call_recoverable = !self.is_call_delivered;

                // Whether the walk actually recovered calls: the delivery
                // flag keeps its pre-walk value for every item of the walk,
                // so the flip afterwards records the walk's outcome — the
                // finish mapping consults it, and a text-only output array
                // must not mark the turn as a tool-call turn.
                let mut is_call_recovered = false;

                if !self.text_recovery.is_delivered
                    || !self.reasoning_recovery.is_delivered
                    || is_call_recoverable
                {
                    for (position, item) in response.output.iter().enumerate() {
                        self.ingest_output_item(
                            item,
                            OutputItemOrigin::Terminal(position),
                            &mut events,
                        );

                        if is_call_recoverable && item.item_type == "function_call" {
                            is_call_recovered = true;
                        }
                    }
                }

                if is_call_recovered {
                    self.is_call_delivered = true;
                }

                self.is_finished = true;

                // The delivery flag was flipped after the terminal walk above,
                // so calls recovered from the output array count too.
                events.push(IrEvent::Finish(finish_reason_for_response(
                    &response,
                    self.is_call_delivered,
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

    /// Ingests one output item from a JSON response body, a streaming
    /// `output_item.done` event, or the terminal event's output array.
    ///
    /// Conformant upstreams deliver message text, reasoning text, and tool
    /// calls as live events, so streamed and terminal items are fallback
    /// sources consulted only when no live event delivered that kind of item
    /// — third-party `openai_compatible` upstreams may stream complete items
    /// without deltas, and their items must still reach the client exactly
    /// once. Fallback recovery of text and reasoning is keyed by the item's
    /// output index: a recovery delivery for one index never silences
    /// another item of the same kind, and one index is never delivered
    /// twice, so the terminal array's copy of an item already delivered as a
    /// streamed done event is recognized by the shared index.
    fn ingest_output_item(
        &mut self,
        item: &OutputItem,
        origin: OutputItemOrigin,
        events: &mut Vec<IrEvent>,
    ) {
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
                    // Refusal text is message text: it rides the same text
                    // channel as `output_text` parts (README §6.2).
                    let text = match part {
                        ContentPart::OutputText { text } => text.as_deref(),

                        ContentPart::Refusal { refusal } => refusal.as_deref(),

                        ContentPart::Other => None,
                    };

                    if let Some(text) = text {
                        // The recovery record is written only when a text
                        // part was actually pushed, so a text-less item
                        // does not exhaust its output index for a later
                        // item that carries the text.
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
                        // Same per-extraction rule as the message arm: a
                        // summary-less item does not exhaust its output
                        // index.
                        if let Some(index) = origin.recovery_index() {
                            self.reasoning_recovery.record(index);
                        }

                        events.push(IrEvent::ReasoningDelta(text.clone()));
                    }
                }
            }

            "function_call" => match origin {
                // A JSON body delivers the whole call at once; mirror the
                // chat and gemini JSON paths with a full-arguments delta
                // plus a done per call, so streaming renderers open and
                // complete the item.
                OutputItemOrigin::Json(index) => {
                    self.is_call_delivered = true;

                    events.push(IrEvent::ToolCallDelta {
                        index,
                        call_id: Some(item.call_id.clone()),
                        name: Some(item.name.clone()),
                        arguments_delta: item.arguments.clone(),
                    });

                    events.push(self.complete_function_call(item, index));
                }

                OutputItemOrigin::StreamDone(index) => {
                    self.is_call_delivered = true;

                    events.push(self.complete_function_call(item, index));
                }

                // The terminal fallback completes a call exactly-once:
                // only when no live call event was delivered before the
                // terminal event may the output array deliver calls,
                // mirroring the per-kind text recovery; re-emitting
                // delivered calls here would duplicate them. The delivery
                // flag is owned by the terminal-event arm, which flips it
                // only after the whole output array is walked, so several
                // recovered calls each deliver exactly once; the flag
                // keeps its pre-walk value for every item of this walk.
                OutputItemOrigin::Terminal(index) => {
                    if !self.is_call_delivered {
                        events.push(self.complete_function_call(item, index));
                    }
                }
            },

            _ => {}
        }
    }

    /// Emits the opening delta for a function call item revealed by
    /// `output_item.added`.
    fn open_function_call(&self, item: &OutputItem, output_index: usize) -> IrEvent {
        IrEvent::ToolCallDelta {
            index: output_index,
            call_id: Some(item.call_id.clone()),
            name: Some(item.name.clone()),
            arguments_delta: String::new(),
        }
    }

    /// Emits the completion of a function call item delivered by a streaming
    /// `output_item.done`.
    fn complete_function_call(&self, item: &OutputItem, index: usize) -> IrEvent {
        IrEvent::ToolCallDone {
            index,
            call_id: item.call_id.clone(),
            name: item.name.clone(),
            arguments: item.arguments.clone(),
            thought_signature: item.thought_signature.clone(),
        }
    }

    /// Emits [`IrEvent::MessageStart`] exactly once, first (README §6.2):
    /// every ingest arm opens with this call, so recovery paths that never
    /// see the live `response.created`/`response.in_progress` events —
    /// no-live-delta upstreams per README §6.3 — still open their event
    /// stream with it, and the once-only guard turns repeats into no-ops.
    ///
    /// The identity comes from the response object the event carries; an
    /// event carrying none opens the stream with empty identity.
    fn message_start_events(&mut self, response: Option<&ResponseObject>) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        let (id, model) = match response {
            Some(response) => (response.id.clone(), response.model.clone()),

            None => (String::new(), String::new()),
        };

        vec![IrEvent::MessageStart { id, model }]
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a `response.*` event.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON and upstream failure events.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let value = format::parse_frame::<UpstreamEvent>(frame)?;

        self.ingest_event(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream failure statuses and malformed
    /// bodies.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let response = format::parse_body::<ResponseObject>(value)?;

        // A 2xx body carrying a top-level error object is an upstream
        // failure (README §6.7), not an empty success; the failed-status
        // fallback below still covers a failure without an error object.
        if let Some(error) = &response.error {
            return Err(error.failure());
        }

        let mut events = self.message_start_events(Some(&response));

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

        // The output walk above flipped the delivery flag for every
        // `function_call` item the body carried.
        events.push(IrEvent::Finish(finish_reason_for_response(
            &response,
            self.is_call_delivered,
        )));

        push_usage_events(&response, &mut events);

        Ok(events)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming deltas
        // without ever sending the terminal `response.completed` or
        // `response.incomplete` event; the missing finish reason degrades to
        // `Stop` so the streamed client sees the same terminal finish as the
        // aggregated rendering of the same stream.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

/// Reads the `output_index` field of an upstream stream event, defaulting to
/// the first position.
fn event_output_index(event: &UpstreamEvent) -> usize {
    event.output_index.unwrap_or_default() as usize
}

/// Builds a delta event from an upstream delta payload; an event carrying no
/// delta delivered nothing.
fn delta_event(event: &UpstreamEvent, make_event_fn: impl Fn(String) -> IrEvent) -> Vec<IrEvent> {
    match &event.delta {
        Some(delta) => vec![make_event_fn(delta.clone())],

        None => Vec::new(),
    }
}

/// Finish reason for a response object already known not to be failed.
///
/// An `incomplete` status maps through `incomplete_details.reason` (unknown
/// reasons degrade to [`IrFinishReason::Length`] — the output was cut short).
/// Any other status finished normally, and a normally finished turn that
/// delivered function calls carries the canonical [`IrFinishReason::ToolCall`]
/// finish: the Responses status vocabulary has no tool-call value (completed /
/// failed / in_progress / cancelled / queued / incomplete), so the delivered
/// calls decide, matching official Chat Completions semantics for
/// text+tool-call mixed turns too.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2xx JSON body carrying a top-level error object is an upstream
    /// failure (README §6.7), not an empty success: the upstream's error
    /// text and embedded code must reach the caller.
    #[test]
    fn json_body_with_error_object_is_rejected() {
        let body = serde_json::json!({
            "error": {"message": "quota exceeded", "code": "insufficient_quota"},
        });

        let mut parser = UpstreamParser::new();

        let error = parser
            .parse_json_response(body)
            .expect_err("a 2xx error body must not parse as an empty success");

        assert_eq!(
            error.to_string(),
            "quota exceeded (upstream error code: \"insufficient_quota\")",
        );
    }

    /// The non-streaming Responses body is the Response object, whose shape
    /// carries `"error": null` on every success; such a body must parse as a
    /// complete success, not reject with the null literal as the failure
    /// text (README §6.7).
    #[test]
    fn json_body_with_null_error_field_parses_as_success() {
        let mut parser = UpstreamParser::new();

        let events = parser
            .parse_json_response(serde_json::json!({
                "error": null,
                "status": "completed",
                "output": [],
            }))
            .expect("a null top-level error is the Response object's success form");

        assert!(matches!(events.first(), Some(IrEvent::MessageStart { .. })));

        assert_eq!(events.last(), Some(&IrEvent::Finish(IrFinishReason::Stop)),);
    }

    /// A stream frame bearing a top-level error object is a failure even
    /// when its `type` is unrecognized, and a bare error object with no
    /// `type` at all likewise aborts instead of parsing as an empty success
    /// (README §6.3: the stream aborts with a terminal error frame).
    #[test]
    fn error_bearing_frames_are_rejected() {
        let mut parser = UpstreamParser::new();

        let frame = SseFrame {
            data: serde_json::json!({
                "type": "response.custom_failure",
                "error": {"message": "backend exploded"},
            })
            .to_string(),
        };

        let error = parser
            .ingest_frame(&frame)
            .expect_err("an unrecognized frame bearing an error object must fail");

        assert_eq!(error.to_string(), "backend exploded");

        let mut parser = UpstreamParser::new();

        let frame = SseFrame {
            data: serde_json::json!({"error": "hard failure"}).to_string(),
        };

        let error = parser
            .ingest_frame(&frame)
            .expect_err("a bare error frame must fail");

        assert_eq!(error.to_string(), "hard failure");
    }

    /// The rejection keys on the error object only: an unrecognized frame
    /// without one is still ignored, exactly as before.
    #[test]
    fn unrecognized_frames_without_error_objects_are_still_ignored() {
        let mut parser = UpstreamParser::new();

        let frame = SseFrame {
            data: serde_json::json!({
                "type": "response.heartbeat",
                "sequence_number": 7,
            })
            .to_string(),
        };

        let events = parser
            .ingest_frame(&frame)
            .expect("an unrecognized frame without an error object is ignored");

        assert!(events.is_empty());
    }

    /// The usage detail objects an upstream reports — `cached_tokens`,
    /// `cache_write_tokens`, `reasoning_tokens` — parse into the IR usage so
    /// the outward Responses rendering can carry them through; totals-only
    /// usage leaves them absent for the renderer's zero-fill.
    #[test]
    fn usage_breakdowns_parse_into_ir() {
        let mut parser = UpstreamParser::new();

        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp_1",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": {
                        "cached_tokens": 6,
                        "cache_write_tokens": 2,
                    },
                    "output_tokens": 4,
                    "output_tokens_details": {"reasoning_tokens": 3},
                    "total_tokens": 14,
                },
            }))
            .expect("a completed response parses");

        assert!(events.contains(&IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 4,
            cached_tokens: Some(6),
            cache_write_tokens: Some(2),
            reasoning_tokens: Some(3),
        })));

        let mut parser = UpstreamParser::new();

        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp_2",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "total_tokens": 14,
                },
            }))
            .expect("a completed response parses");

        assert!(events.contains(&IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 4,
            ..IrUsage::default()
        })));
    }

    /// Builds an `output_item.done` frame for a complete message item.
    fn message_done_frame(output_index: usize, text: &str) -> SseFrame {
        SseFrame {
            data: serde_json::json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text}],
                },
            })
            .to_string(),
        }
    }

    /// Builds a terminal `response.completed` frame carrying `output`.
    fn completed_frame(output: Value) -> SseFrame {
        SseFrame {
            data: serde_json::json!({
                "type": "response.completed",
                "response": {"status": "completed", "output": output},
            })
            .to_string(),
        }
    }

    /// The MessageStart a recovery path emits: the recovery frames carry no
    /// response identity, so the event's fields are empty.
    fn bare_message_start() -> IrEvent {
        IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        }
    }

    /// The exactly-once recovery contract (README §6.3): a terminal output
    /// array carrying several `message` items — the shape of an upstream
    /// that streams no live events — delivers every item's text exactly
    /// once, and the recovery stream still opens with MessageStart, first
    /// (README §6.2).
    #[test]
    fn terminal_output_array_delivers_every_message_item_once() {
        let mut parser = UpstreamParser::new();

        let output = serde_json::json!([
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "first"}],
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "second"}],
            },
        ]);

        let events = parser
            .ingest_frame(&completed_frame(output))
            .expect("a completed frame parses");

        assert_eq!(
            events,
            vec![
                bare_message_start(),
                IrEvent::TextDelta("first".to_string()),
                IrEvent::TextDelta("second".to_string()),
                IrEvent::Finish(IrFinishReason::Stop),
            ],
        );
    }

    /// Reasoning recovery is independent per index: a terminal array of
    /// reasoning + message + reasoning delivers both reasonings and the
    /// message text exactly once.
    #[test]
    fn terminal_output_array_delivers_interleaved_reasoning_items() {
        let mut parser = UpstreamParser::new();

        let output = serde_json::json!([
            {"type": "reasoning", "summary": [{"type": "summary_text", "text": "think one"}]},
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "answer"}],
            },
            {"type": "reasoning", "summary": [{"type": "summary_text", "text": "think two"}]},
        ]);

        let events = parser
            .ingest_frame(&completed_frame(output))
            .expect("a completed frame parses");

        assert_eq!(
            events,
            vec![
                bare_message_start(),
                IrEvent::ReasoningDelta("think one".to_string()),
                IrEvent::TextDelta("answer".to_string()),
                IrEvent::ReasoningDelta("think two".to_string()),
                IrEvent::Finish(IrFinishReason::Stop),
            ],
        );
    }

    /// Successive streamed `output_item.done` message items — the no-delta
    /// streaming shape — each deliver their text exactly once, and the
    /// terminal array's copy of the same items (shared output indices) is
    /// not re-delivered. The done-only stream still opens with MessageStart,
    /// ahead of the first item's text (README §6.2).
    #[test]
    fn streamed_done_items_deliver_each_message_item_once() {
        let mut parser = UpstreamParser::new();

        let events = parser
            .ingest_frame(&message_done_frame(0, "first"))
            .expect("a done frame parses");

        assert_eq!(
            events,
            vec![
                bare_message_start(),
                IrEvent::TextDelta("first".to_string()),
            ],
        );

        let events = parser
            .ingest_frame(&message_done_frame(1, "second"))
            .expect("a done frame parses");

        assert_eq!(events, vec![IrEvent::TextDelta("second".to_string())]);

        let output = serde_json::json!([
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "first"}],
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "second"}],
            },
        ]);

        let events = parser
            .ingest_frame(&completed_frame(output))
            .expect("a completed frame parses");

        assert_eq!(events, vec![IrEvent::Finish(IrFinishReason::Stop)]);
    }

    /// Live deltas keep precedence: text the deltas already delivered is
    /// not re-delivered by the streamed done item or by any terminal-array
    /// item of the same kind (README §6.3: fallbacks deliver only what live
    /// deltas did not).
    #[test]
    fn live_deltas_silence_fallback_recovery() {
        let mut parser = UpstreamParser::new();

        let delta_frame = SseFrame {
            data: serde_json::json!({
                "type": "response.output_text.delta",
                "delta": "live",
            })
            .to_string(),
        };

        let events = parser.ingest_frame(&delta_frame).expect("a delta parses");

        assert_eq!(
            events,
            vec![bare_message_start(), IrEvent::TextDelta("live".to_string()),],
        );

        let events = parser
            .ingest_frame(&message_done_frame(0, "live"))
            .expect("a done frame parses");

        assert!(events.is_empty());

        let output = serde_json::json!([
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "live"}],
            },
        ]);

        let events = parser
            .ingest_frame(&completed_frame(output))
            .expect("a completed frame parses");

        assert_eq!(events, vec![IrEvent::Finish(IrFinishReason::Stop)]);
    }

    /// A bare function-call arguments delta as a stream's first event — an
    /// upstream that skipped `response.created`, `response.in_progress`,
    /// and `response.output_item.added` — still opens the IR stream with
    /// MessageStart, ahead of the delta it precedes (README §6.2:
    /// MessageStart is emitted exactly once, first).
    #[test]
    fn first_event_being_a_bare_call_delta_still_opens_with_message_start() {
        let mut parser = UpstreamParser::new();

        let frame = SseFrame {
            data: serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 2,
                "delta": "{\"a\"",
            })
            .to_string(),
        };

        let events = parser.ingest_frame(&frame).expect("a delta frame parses");

        assert_eq!(
            events,
            vec![
                bare_message_start(),
                IrEvent::ToolCallDelta {
                    index: 2,
                    call_id: None,
                    name: None,
                    arguments_delta: "{\"a\"".to_string(),
                },
            ],
        );
    }
}
