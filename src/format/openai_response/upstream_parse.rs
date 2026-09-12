//! Upstream-event parsing for the Responses codec: parses upstream SSE events
//! or a complete JSON body into IR events.

use crate::{
    format::{PayloadError, UpstreamEventParser, reject_upstream_error},
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde_json::Value;

/// Placeholder for a missing JSON object field, so the read-only walks below
/// borrow instead of deep-cloning the subtrees they inspect.
const MISSING: Value = Value::Null;

/// Parses upstream Responses payloads — SSE events or a full JSON body — into
/// IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether live deltas already delivered the message text, marking done
    /// and terminal items as exhausted sources for it.
    is_text_delivered: bool,

    /// Whether live deltas already delivered the reasoning text, marking
    /// done and terminal items as exhausted sources for it.
    is_reasoning_delivered: bool,

    /// Whether a live event already delivered a tool call, marking the
    /// terminal output array as an exhausted source for them.
    is_call_delivered: bool,

    /// Whether a finish reason was already emitted; a clean end-of-stream
    /// without one degrades to [`IrFinishReason::Stop`] in the parser's
    /// finish hook, matching the aggregated rendering default.
    is_finished: bool,
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

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one upstream event object.
    fn ingest_event(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        // Any frame bearing a top-level error object is an upstream failure,
        // recognized event type or not; without this check such a frame
        // would fall through to the unknown-type arm and parse as an empty
        // success (README §6.7).
        reject_upstream_error(&value)?;

        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match event_type {
            "response.created" | "response.in_progress" => Ok(self.message_start_events(&value)),

            "response.output_item.added" => {
                let mut events = self.message_start_events(&value);

                let output_index = event_output_index(&value);

                if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                {
                    self.is_call_delivered = true;

                    events.push(self.open_function_call(item, output_index));
                }

                Ok(events)
            }

            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let events = self.delta_event(&value, IrEvent::ReasoningDelta);

                // Only an event that actually carried a delta exhausts the
                // fallback sources; a delta-typed event without a payload
                // delivered nothing.
                if !events.is_empty() {
                    self.is_reasoning_delivered = true;
                }

                Ok(events)
            }

            "response.output_text.delta" | "response.refusal.delta" => {
                let events = self.delta_event(&value, IrEvent::TextDelta);

                if !events.is_empty() {
                    self.is_text_delivered = true;
                }

                Ok(events)
            }

            "response.function_call_arguments.delta" => {
                let output_index = event_output_index(&value);

                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                self.is_call_delivered = true;

                Ok(vec![IrEvent::ToolCallDelta {
                    index: output_index,
                    call_id: None,
                    name: None,
                    arguments_delta: delta.to_string(),
                }])
            }

            "response.output_item.done" => {
                let mut events = Vec::new();

                if let Some(item) = value.get("item") {
                    let output_index = event_output_index(&value);

                    self.ingest_output_item(
                        item,
                        OutputItemOrigin::StreamDone(output_index),
                        &mut events,
                    )?;
                }

                Ok(events)
            }

            "response.completed" | "response.incomplete" => {
                let response = value.get("response").unwrap_or(&MISSING);

                let mut events = Vec::new();

                // A third-party `openai_compatible` upstream may carry items
                // only in the terminal response object; when no live event
                // delivered a kind, recover it from the output array exactly
                // as from a JSON response body. The call recovery decision is
                // snapshotted before the walk: the walk must consult the
                // pre-walk state for every item, so a terminal output array
                // carrying several function_call items delivers each of them
                // instead of only the first.
                let is_call_recoverable = !self.is_call_delivered;

                // Whether the walk actually recovered calls: the delivery
                // flag keeps its pre-walk value for every item of the walk,
                // so the flip afterwards records the walk's outcome — the
                // finish mapping consults it, and a text-only output array
                // must not mark the turn as a tool-call turn.
                let mut is_call_recovered = false;

                if (!self.is_text_delivered || !self.is_reasoning_delivered || is_call_recoverable)
                    && let Some(output) = response.get("output").and_then(Value::as_array)
                {
                    for (position, item) in output.iter().enumerate() {
                        self.ingest_output_item(
                            item,
                            OutputItemOrigin::Terminal(position),
                            &mut events,
                        )?;

                        if is_call_recoverable
                            && item.get("type").and_then(Value::as_str) == Some("function_call")
                        {
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
                    response,
                    self.is_call_delivered,
                )));
                push_usage_events(response, &mut events);

                Ok(events)
            }

            "response.failed" => {
                let message = value
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream response failed");

                Err(PayloadError::new(message))
            }

            "error" => {
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream stream error");

                Err(PayloadError::new(message))
            }

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
    /// once.
    fn ingest_output_item(
        &mut self,
        item: &Value,
        origin: OutputItemOrigin,
        events: &mut Vec<IrEvent>,
    ) -> Result<(), PayloadError> {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                let should_extract = match origin {
                    OutputItemOrigin::Json(_) => true,

                    OutputItemOrigin::StreamDone(_) | OutputItemOrigin::Terminal(_) => {
                        !self.is_text_delivered
                    }
                };

                if should_extract
                    && let Some(content) = item.get("content").and_then(Value::as_array)
                {
                    for part in content {
                        // Refusal text is message text: it rides the same
                        // text channel as `output_text` parts (README §6.2).
                        let text = match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => part.get("text").and_then(Value::as_str),

                            Some("refusal") => part.get("refusal").and_then(Value::as_str),

                            _ => None,
                        };

                        if let Some(text) = text {
                            // The delivery flag is set only when a delta was
                            // actually pushed, so a text-less item does not
                            // exhaust the source for a later item that
                            // carries the text.
                            self.is_text_delivered = true;

                            events.push(IrEvent::TextDelta(text.to_string()));
                        }
                    }
                }

                Ok(())
            }

            Some("reasoning") => {
                let should_extract = match origin {
                    OutputItemOrigin::Json(_) => true,

                    OutputItemOrigin::StreamDone(_) | OutputItemOrigin::Terminal(_) => {
                        !self.is_reasoning_delivered
                    }
                };

                if should_extract
                    && let Some(summary) = item.get("summary").and_then(Value::as_array)
                {
                    for part in summary {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            // Same per-extraction rule as the message arm: a
                            // summary-less item does not exhaust the source.
                            self.is_reasoning_delivered = true;

                            events.push(IrEvent::ReasoningDelta(text.to_string()));
                        }
                    }
                }

                Ok(())
            }

            Some("function_call") => {
                match origin {
                    // A JSON body delivers the whole call at once; mirror the
                    // chat and gemini JSON paths with a full-arguments delta
                    // plus a done per call, so streaming renderers open and
                    // complete the item.
                    OutputItemOrigin::Json(index) => {
                        self.is_call_delivered = true;

                        let call_id = string_field(item, "call_id");
                        let name = string_field(item, "name");
                        let arguments = string_field(item, "arguments");

                        events.push(IrEvent::ToolCallDelta {
                            index,
                            call_id: Some(call_id.clone()),
                            name: Some(name.clone()),
                            arguments_delta: arguments.clone(),
                        });

                        events.push(IrEvent::ToolCallDone {
                            index,
                            call_id,
                            name,
                            arguments,
                            thought_signature: item
                                .get("thought_signature")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        });
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
                }

                Ok(())
            }

            _ => Ok(()),
        }
    }

    /// Emits the opening delta for a function call item revealed by
    /// `output_item.added`.
    fn open_function_call(&self, item: &Value, output_index: usize) -> IrEvent {
        IrEvent::ToolCallDelta {
            index: output_index,
            call_id: Some(string_field(item, "call_id")),
            name: Some(string_field(item, "name")),
            arguments_delta: String::new(),
        }
    }

    /// Emits the completion of a function call item delivered by a streaming
    /// `output_item.done`.
    fn complete_function_call(&self, item: &Value, index: usize) -> IrEvent {
        IrEvent::ToolCallDone {
            index,
            call_id: string_field(item, "call_id"),
            name: string_field(item, "name"),
            arguments: string_field(item, "arguments"),
            thought_signature: item
                .get("thought_signature")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    /// Builds a delta event from an upstream delta payload.
    fn delta_event(
        &self,
        value: &Value,
        make_event_fn: impl Fn(String) -> IrEvent,
    ) -> Vec<IrEvent> {
        match value.get("delta") {
            Some(Value::String(delta)) => vec![make_event_fn(delta.clone())],
            _ => Vec::new(),
        }
    }

    /// Emits [`IrEvent::MessageStart`] once per response.
    fn message_start_events(&mut self, value: &Value) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        let response = value.get("response").unwrap_or(value);

        vec![IrEvent::MessageStart {
            id: response
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: response
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }]
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a `response.*` event.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON and upstream failure events.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let value: Value = serde_json::from_str(&frame.data)
            .map_err(|_| PayloadError::new("malformed JSON in upstream SSE frame"))?;

        self.ingest_event(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream failure statuses and malformed
    /// bodies.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        // A 2xx body carrying a top-level error object is an upstream
        // failure (README §6.7), not an empty success; the failed-status
        // fallback below still covers a failure without an error object.
        reject_upstream_error(&value)?;

        let mut events = self.message_start_events(&value);

        if let Some(output) = value.get("output").and_then(Value::as_array) {
            for (position, item) in output.iter().enumerate() {
                self.ingest_output_item(item, OutputItemOrigin::Json(position), &mut events)?;
            }
        }

        let status = value.get("status");

        if status.and_then(Value::as_str) == Some("failed") {
            let message = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("upstream response failed");

            return Err(PayloadError::new(message));
        }

        self.is_finished = true;

        // The output walk above flipped the delivery flag for every
        // `function_call` item the body carried.
        events.push(IrEvent::Finish(finish_reason_for_response(
            &value,
            self.is_call_delivered,
        )));

        push_usage_events(&value, &mut events);

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

/// Reads the `output_index` field of an upstream stream event.
fn event_output_index(value: &Value) -> usize {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize
}

/// Reads a string field of an upstream JSON object, defaulting to the empty
/// string.
fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
fn finish_reason_for_response(response: &Value, is_call_delivered: bool) -> IrFinishReason {
    if response.get("status").and_then(Value::as_str) != Some("incomplete") {
        if is_call_delivered {
            return IrFinishReason::ToolCall;
        }

        return IrFinishReason::Stop;
    }

    match response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
    {
        Some("content_filter") => IrFinishReason::ContentFilter,

        _ => IrFinishReason::Length,
    }
}

/// Appends an [`IrEvent::Usage`] for the response's usage object, when present.
fn push_usage_events(response: &Value, events: &mut Vec<IrEvent>) {
    if let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) {
        let input_tokens_details = usage.get("input_tokens_details");
        let output_tokens_details = usage.get("output_tokens_details");

        events.push(IrEvent::Usage(IrUsage {
            input_tokens: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cached_tokens: input_tokens_details
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64),
            cache_write_tokens: input_tokens_details
                .and_then(|details| details.get("cache_write_tokens"))
                .and_then(Value::as_u64),
            reasoning_tokens: output_tokens_details
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64),
        }));
    }
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
            event: None,
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
            event: None,
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
            event: None,
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
}
