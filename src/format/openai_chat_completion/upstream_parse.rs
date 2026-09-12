//! Upstream-event parsing for the Chat Completions codec: parses upstream SSE
//! chunks or a complete JSON body into IR events.

use crate::{
    format::{self, PayloadError, UpstreamEventParser},
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde_json::Value;

/// Parses upstream Chat Completions responses — streamed chunks or a full
/// JSON body — into IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether a finish reason was already emitted; a clean end-of-stream
    /// without one degrades to [`IrFinishReason::Stop`] in the parser's
    /// finish hook, matching the aggregated rendering default.
    is_finished: bool,

    /// Tool calls being assembled from deltas.
    pending_tool_calls: Vec<PendingToolCall>,

    /// Whether completed calls were already flushed.
    has_flushed_calls: bool,
}

/// Tool call being assembled from upstream deltas.
#[derive(Debug)]
struct PendingToolCall {
    /// Stable index of the call within the response.
    index: usize,

    /// Invocation identifier.
    call_id: String,

    /// Function name.
    name: String,

    /// Arguments accumulated so far.
    arguments: String,

    /// Upstream thought signature carried on the delta entries; the gateway
    /// extension must survive into the completion (README §5.2).
    thought_signature: Option<String>,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one streaming chunk object.
    fn ingest_chunk(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        format::reject_upstream_error(&value)?;

        let mut events = self.message_start_events(&value);

        if let Some(choice) = first_choice(&value) {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = reasoning_field(delta)
                    && !reasoning.is_empty()
                {
                    events.push(IrEvent::ReasoningDelta(reasoning.to_string()));
                }

                if let Some(content) = delta.get("content").and_then(Value::as_str)
                    && !content.is_empty()
                {
                    events.push(IrEvent::TextDelta(content.to_string()));
                }

                // Refusal text rides the ordinary text channel, after any
                // content text of the same delta (README §6.2).
                if let Some(refusal) = delta.get("refusal").and_then(Value::as_str)
                    && !refusal.is_empty()
                {
                    events.push(IrEvent::TextDelta(refusal.to_string()));
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        self.ingest_tool_call_delta(tool_call, &mut events)?;
                    }
                }
            }

            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                events.append(&mut self.flush_pending_calls());

                self.is_finished = true;

                events.push(IrEvent::Finish(format::finish_reason_from_openai(
                    finish_reason,
                )));
            }
        }

        if let Some(usage) = parse_usage(value.get("usage")) {
            events.push(usage);
        }

        Ok(events)
    }

    /// Ingests one `delta.tool_calls` entry, buffering arguments until the
    /// calls complete.
    ///
    /// # Panics
    ///
    /// Panics when a newly started tool call cannot be buffered, violating
    /// the push-then-read invariant.
    fn ingest_tool_call_delta(
        &mut self,
        tool_call: &Value,
        events: &mut Vec<IrEvent>,
    ) -> Result<(), PayloadError> {
        let index = tool_call
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default();

        let function = tool_call.get("function").unwrap_or(&format::MISSING);

        let call_id = tool_call.get("id").and_then(Value::as_str);
        let name = function.get("name").and_then(Value::as_str);
        let arguments_delta = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let thought_signature = tool_call
            .get("thought_signature")
            .and_then(Value::as_str)
            .map(str::to_string);

        let pending = match self
            .pending_tool_calls
            .iter_mut()
            .find(|pending| pending.index as u64 == index)
        {
            Some(pending) => pending,
            None => {
                self.pending_tool_calls.push(PendingToolCall {
                    index: index as usize,
                    call_id: call_id.unwrap_or_default().to_string(),
                    name: name.unwrap_or_default().to_string(),
                    arguments: String::new(),
                    thought_signature: None,
                });
                #[expect(clippy::expect_used, reason = "the entry was just pushed above")]
                self.pending_tool_calls
                    .last_mut()
                    .expect("entry was just pushed")
            }
        };

        if let Some(call_id) = call_id {
            pending.call_id = call_id.to_string();
        }

        if let Some(name) = name {
            pending.name = name.to_string();
        }

        // The last delta entry carrying a signature wins; upstreams repeat it
        // on the entry that completes the call.
        if thought_signature.is_some() {
            pending.thought_signature = thought_signature;
        }

        pending.arguments.push_str(arguments_delta);

        events.push(IrEvent::ToolCallDelta {
            index: index as usize,
            call_id: call_id.map(str::to_string),
            name: name.map(str::to_string),
            arguments_delta: arguments_delta.to_string(),
        });

        Ok(())
    }

    /// Emits [`IrEvent::ToolCallDone`] for every pending call, marking them
    /// complete.
    fn flush_pending_calls(&mut self) -> Vec<IrEvent> {
        if self.has_flushed_calls {
            return Vec::new();
        }

        self.has_flushed_calls = true;

        self.pending_tool_calls
            .drain(..)
            .map(|pending| IrEvent::ToolCallDone {
                index: pending.index,
                call_id: pending.call_id,
                name: pending.name,
                arguments: pending.arguments,
                thought_signature: pending.thought_signature,
            })
            .collect()
    }

    /// Emits [`IrEvent::MessageStart`] once per response.
    fn message_start_events(&mut self, value: &Value) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        vec![IrEvent::MessageStart {
            id: format::string_field(value, "id"),
            model: format::string_field(value, "model"),
        }]
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON and upstream error payloads.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        if frame.data.trim() == "[DONE]" {
            return Ok(self.flush_pending_calls());
        }

        let value = format::parse_frame_json(frame)?;

        self.ingest_chunk(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream error payloads and malformed
    /// bodies.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        format::reject_upstream_error(&value)?;

        let mut events = self.message_start_events(&value);

        if let Some(message) = first_choice(&value).and_then(|choice| choice.get("message")) {
            events.append(&mut message_events_from_full(message));
        }

        let finish_reason = first_choice(&value)
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .map(format::finish_reason_from_openai)
            .unwrap_or(IrFinishReason::Stop);

        self.is_finished = true;

        events.push(IrEvent::Finish(finish_reason));

        if let Some(usage) = parse_usage(value.get("usage")) {
            events.push(usage);
        }

        Ok(events)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming tool-call
        // deltas without ever sending a finish reason or the `[DONE]`
        // sentinel; the buffered calls are complete and must be flushed, and
        // the missing finish reason degrades to `Stop` so the streamed
        // client sees the same terminal finish reason as the aggregated
        // rendering of the same stream.
        let mut events = self.flush_pending_calls();

        if !self.is_finished {
            self.is_finished = true;

            events.push(IrEvent::Finish(IrFinishReason::Stop));
        }

        events
    }
}

/// Reads the reasoning text of an upstream delta or message object.
///
/// Both `reasoning` (OpenRouter, vLLM) and `reasoning_content` (the DeepSeek
/// de-facto standard for OpenAI-compatible reasoners) are accepted; when both
/// keys are present, `reasoning` wins.
fn reasoning_field(value: &Value) -> Option<&str> {
    value
        .get("reasoning")
        .and_then(Value::as_str)
        .or_else(|| value.get("reasoning_content").and_then(Value::as_str))
}

/// Extracts the primary choice, preferring `index == 0`; parallel choices are
/// not supported and the remainder is ignored.
fn first_choice(value: &Value) -> Option<&Value> {
    let choices = value.get("choices")?.as_array()?;

    choices
        .iter()
        .find(|choice| choice.get("index").and_then(Value::as_u64) == Some(0))
        .or_else(|| choices.first())
}

/// Converts an upstream usage object into an [`IrEvent::Usage`], if present.
fn parse_usage(usage: Option<&Value>) -> Option<IrEvent> {
    let usage = usage?;

    if !usage.is_object() {
        return None;
    }

    Some(IrEvent::Usage(IrUsage {
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        // The outward Chat Completions surface renders only the totals, and
        // the Responses surface zero-fills an absent breakdown; the native
        // chat detail objects are not carried across formats.
        ..IrUsage::default()
    }))
}

/// Extracts message text (including refusal text), reasoning, and complete
/// tool calls from a full upstream message object into IR events.
fn message_events_from_full(message: &Value) -> Vec<IrEvent> {
    let mut events = Vec::new();

    if let Some(reasoning) = reasoning_field(message)
        && !reasoning.is_empty()
    {
        events.push(IrEvent::ReasoningDelta(reasoning.to_string()));
    }

    if let Some(content) = message.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        events.push(IrEvent::TextDelta(content.to_string()));
    }

    // Refusal text rides the ordinary text channel, after the message's
    // content text (README §6.2).
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str)
        && !refusal.is_empty()
    {
        events.push(IrEvent::TextDelta(refusal.to_string()));
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, tool_call) in tool_calls.iter().enumerate() {
            let function = tool_call.get("function").unwrap_or(&format::MISSING);

            let call_id = format::string_field(tool_call, "id");
            let name = format::string_field(function, "name");
            let arguments = format::string_field(function, "arguments");

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
                thought_signature: tool_call
                    .get("thought_signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    }

    events
}
