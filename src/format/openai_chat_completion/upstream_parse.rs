//! Upstream-event parsing for the Chat Completions codec: parses upstream SSE chunks or a complete
//! JSON body into IR events.
//!
//! Upstream payloads are third-party, so the models below carry exactly the fields the codec reads,
//! ignore every field it does not, and default what upstreams legitimately omit.

use crate::{
    format::{self, PayloadError, UpstreamError, UpstreamEventParser},
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde::Deserialize;
use serde_json::Value;

/// Parses upstream Chat Completions responses — streamed chunks or a full JSON body — into IR
/// events.
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

    /// Upstream thought signature carried on the delta entries; the gateway extension must survive
    /// into the completion (README §5.2).
    thought_signature: Option<String>,
}

/// One upstream Chat Completions chunk, or a complete non-streaming body: the two share this shape,
/// a chunk carrying `choices[].delta` where a body carries `choices[].message`.
#[derive(Debug, Default, Deserialize)]
struct UpstreamResponse {
    /// Upstream response identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,

    /// Per-choice payloads; the gateway reads the primary choice.
    #[serde(default)]
    choices: Vec<Choice>,

    /// Token accounting, carried by a complete body and by the terminal chunk of a stream.
    #[serde(default)]
    usage: Option<UpstreamUsage>,

    /// Failure an upstream may embed in a 2xx body (README §6.7).
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// One upstream choice.
#[derive(Debug, Default, Deserialize)]
struct Choice {
    /// Upstream-assigned index; absent on upstreams that omit it.
    #[serde(default)]
    index: Option<u64>,

    /// The incremental payload of a streaming chunk.
    #[serde(default)]
    delta: Option<MessageFields>,

    /// The complete payload of a non-streaming response.
    #[serde(default)]
    message: Option<MessageFields>,

    /// Canonical finish reason string, present on the turn's final payload.
    #[serde(default)]
    finish_reason: Option<String>,
}

/// The text-bearing fields shared by a streaming delta and a complete message, beside the tool
/// calls they carry.
#[derive(Debug, Default, Deserialize)]
struct MessageFields {
    /// Assistant text.
    #[serde(default)]
    content: Option<String>,

    /// Reasoning text under the OpenRouter/vLLM key.
    #[serde(default)]
    reasoning: Option<String>,

    /// Reasoning text under the DeepSeek de-facto key.
    #[serde(default)]
    reasoning_content: Option<String>,

    /// Refusal text.
    #[serde(default)]
    refusal: Option<String>,

    /// Tool call entries: argument fragments in a delta, complete calls in a message.
    #[serde(default)]
    tool_calls: Vec<ToolCallEntry>,
}

/// One tool call entry, as a delta fragment or as a complete call.
#[derive(Debug, Default, Deserialize)]
struct ToolCallEntry {
    /// Upstream-assigned index, carried by delta fragments only.
    #[serde(default)]
    index: Option<u64>,

    /// Invocation identifier, revealed by the opening fragment.
    #[serde(default)]
    id: Option<String>,

    /// The function name and arguments.
    #[serde(default)]
    function: Option<FunctionFields>,

    /// Upstream thought signature the gateway extension carries (README §5.2).
    #[serde(default)]
    thought_signature: Option<String>,
}

/// The function fields of a tool call entry.
#[derive(Debug, Default, Deserialize)]
struct FunctionFields {
    /// Invoked function name.
    #[serde(default)]
    name: Option<String>,

    /// JSON-encoded arguments text, complete or incremental.
    #[serde(default)]
    arguments: Option<String>,
}

/// Upstream token accounting: the totals always, beside the official detail objects when the
/// upstream reports them — `prompt_tokens_details.cached_tokens`,
/// `prompt_tokens_details.cache_write_tokens`, and
/// `completion_tokens_details.reasoning_tokens` ride into the IR breakdowns (README §6.2),
/// while the members the gateway does not read are ignored.
#[derive(Debug, Default, Deserialize)]
struct UpstreamUsage {
    /// Tokens consumed by the input.
    #[serde(default)]
    prompt_tokens: u64,

    /// Tokens produced by the output.
    #[serde(default)]
    completion_tokens: u64,

    /// Input breakdown: the cache-read and cache-write columns.
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,

    /// Output breakdown: the reasoning column.
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokensDetails>,
}

/// The official `prompt_tokens_details` object; every member is optional.
#[derive(Debug, Default, Deserialize)]
struct PromptTokensDetails {
    /// Input tokens served from the upstream prompt cache.
    #[serde(default)]
    cached_tokens: Option<u64>,

    /// Input tokens written to the upstream prompt cache.
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

/// The official `completion_tokens_details` object; every member is optional.
#[derive(Debug, Default, Deserialize)]
struct CompletionTokensDetails {
    /// Output tokens spent on model reasoning.
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one streaming chunk object.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] when the chunk embeds an upstream error.
    fn ingest_chunk(&mut self, response: UpstreamResponse) -> Result<Vec<IrEvent>, PayloadError> {
        Self::reject_embedded_error(&response)?;

        let mut events = self.message_start_events(&response);

        if let Some(choice) = primary_choice(&response.choices) {
            if let Some(delta) = &choice.delta {
                push_text_events(delta, &mut events);

                for tool_call in &delta.tool_calls {
                    self.ingest_tool_call_delta(tool_call, &mut events);
                }
            }

            if let Some(finish_reason) = &choice.finish_reason {
                events.append(&mut self.flush_pending_calls());

                self.is_finished = true;

                events.push(IrEvent::Finish(format::finish_reason_from_openai(
                    finish_reason,
                )));
            }
        }

        if let Some(usage) = &response.usage {
            events.push(usage_event(usage));
        }

        Ok(events)
    }

    /// Rejects a body carrying a non-null embedded `error` (README §6.7).
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] carrying the upstream error text and code.
    fn reject_embedded_error(response: &UpstreamResponse) -> Result<(), PayloadError> {
        match &response.error {
            Some(error) => Err(error.failure()),
            None => Ok(()),
        }
    }

    /// Ingests one `delta.tool_calls` entry, buffering arguments until the
    /// calls complete.
    ///
    /// # Panics
    ///
    /// Panics when a newly started tool call cannot be buffered, violating
    /// the push-then-read invariant.
    fn ingest_tool_call_delta(&mut self, tool_call: &ToolCallEntry, events: &mut Vec<IrEvent>) {
        let index = tool_call.index.unwrap_or_default() as usize;

        let call_id = tool_call.id.as_deref();
        let name = tool_call
            .function
            .as_ref()
            .and_then(|function| function.name.as_deref());
        let arguments_delta = tool_call
            .function
            .as_ref()
            .and_then(|function| function.arguments.as_deref())
            .unwrap_or_default();

        let pending = match self
            .pending_tool_calls
            .iter_mut()
            .find(|pending| pending.index == index)
        {
            Some(pending) => pending,

            None => {
                self.pending_tool_calls.push(PendingToolCall {
                    index,
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
        if tool_call.thought_signature.is_some() {
            pending.thought_signature = tool_call.thought_signature.clone();
        }

        pending.arguments.push_str(arguments_delta);

        events.push(IrEvent::ToolCallDelta {
            index,
            call_id: call_id.map(str::to_string),
            name: name.map(str::to_string),
            arguments_delta: arguments_delta.to_string(),
        });
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
    fn message_start_events(&mut self, response: &UpstreamResponse) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        vec![IrEvent::MessageStart {
            id: response.id.clone(),
            model: response.model.clone(),
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

        let response = format::parse_frame::<UpstreamResponse>(frame)?;

        self.ingest_chunk(response)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream error payloads and malformed
    /// bodies.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let response = format::parse_body::<UpstreamResponse>(value)?;

        Self::reject_embedded_error(&response)?;

        let mut events = self.message_start_events(&response);

        if let Some(message) =
            primary_choice(&response.choices).and_then(|choice| choice.message.as_ref())
        {
            events.append(&mut message_events_from_full(message));
        }

        let finish_reason = primary_choice(&response.choices)
            .and_then(|choice| choice.finish_reason.as_deref())
            .map(format::finish_reason_from_openai)
            .unwrap_or(IrFinishReason::Stop);

        self.is_finished = true;

        events.push(IrEvent::Finish(finish_reason));

        if let Some(usage) = &response.usage {
            events.push(usage_event(usage));
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

/// Selects the primary entry of an upstream `choices` array: the entry whose `index` is 0,
/// tolerating index-less upstreams by taking the first entry instead. Parallel entries beyond the
/// primary are not supported and the remainder is ignored.
fn primary_choice(choices: &[Choice]) -> Option<&Choice> {
    choices
        .iter()
        .find(|choice| choice.index == Some(0))
        .or_else(|| choices.first())
}

/// Renders one upstream usage observation as an IR event.
fn usage_event(usage: &UpstreamUsage) -> IrEvent {
    IrEvent::Usage(IrUsage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        cached_tokens: usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        cache_write_tokens: usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens),
        reasoning_tokens: usage
            .completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
    })
}

/// Emits the text-bearing events of one upstream delta or complete message object into `events`:
/// reasoning first, then the object's content text, then its refusal text. Refusal text rides the
/// ordinary text channel after any content text of the same object (README §6.2); this helper is
/// the single home of that ordering contract.
fn push_text_events(fields: &MessageFields, events: &mut Vec<IrEvent>) {
    // Both `reasoning` (OpenRouter, vLLM) and `reasoning_content` (the DeepSeek de-facto standard
    // for OpenAI-compatible reasoners) are accepted; when both keys are present, `reasoning` wins.
    if let Some(reasoning) = fields
        .reasoning
        .as_deref()
        .or(fields.reasoning_content.as_deref())
        && !reasoning.is_empty()
    {
        events.push(IrEvent::ReasoningDelta(reasoning.to_string()));
    }

    if let Some(content) = fields.content.as_deref()
        && !content.is_empty()
    {
        events.push(IrEvent::TextDelta(content.to_string()));
    }

    if let Some(refusal) = fields.refusal.as_deref()
        && !refusal.is_empty()
    {
        events.push(IrEvent::TextDelta(refusal.to_string()));
    }
}

/// Extracts message text (including refusal text), reasoning, and complete tool calls from a full
/// upstream message object into IR events. A complete call carries no index of its own, so its
/// position in the array is its index.
fn message_events_from_full(message: &MessageFields) -> Vec<IrEvent> {
    let mut events = Vec::new();

    push_text_events(message, &mut events);

    for (index, tool_call) in message.tool_calls.iter().enumerate() {
        let call_id = tool_call.id.clone().unwrap_or_default();

        let name = tool_call
            .function
            .as_ref()
            .and_then(|function| function.name.clone())
            .unwrap_or_default();

        let arguments = tool_call
            .function
            .as_ref()
            .and_then(|function| function.arguments.clone())
            .unwrap_or_default();

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
            thought_signature: tool_call.thought_signature.clone(),
        });
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::SseFrame;

    /// Builds one upstream frame carrying `data`.
    ///
    /// # Panics
    ///
    /// Panics if the payload is not valid JSON, which cannot occur for the fixtures.
    fn frame(data: &str) -> SseFrame {
        #[expect(
            clippy::expect_used,
            reason = "fixture payloads are constructed to be valid JSON"
        )]
        let parsed: Value = serde_json::from_str(data).expect("fixture payloads are valid JSON");

        SseFrame {
            data: parsed.to_string(),
            event: None,
        }
    }

    /// Parses the official terminal usage chunk (README §6.2): the three documented breakdown
    /// members ride through to the IR usage, beside the totals the gateway already carried.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the upstream report.
    #[test]
    fn parses_usage_breakdowns_from_the_terminal_chunk() {
        let mut parser = UpstreamParser::new();

        let start = parser.ingest_frame(&frame(
            r#"{"id":"c1","model":"m","choices":[{"index":0,"delta":{},
               "finish_reason":"stop"}]}"#,
        ));

        #[expect(
            clippy::expect_used,
            reason = "the finish chunk is constructed to parse"
        )]
        let start = start.expect("the finish chunk parses");

        assert_eq!(
            start,
            vec![
                IrEvent::MessageStart {
                    id: "c1".to_string(),
                    model: "m".to_string(),
                },
                IrEvent::Finish(IrFinishReason::Stop),
            ],
        );

        let usage = parser.ingest_frame(&frame(
            r#"{"id":"c1","model":"m","choices":[],"usage":{
               "prompt_tokens": 100,
               "completion_tokens": 50,
               "total_tokens": 150,
               "prompt_tokens_details": {"cached_tokens": 80, "cache_write_tokens": 7},
               "completion_tokens_details": {"reasoning_tokens": 20}}}"#,
        ));

        #[expect(
            clippy::expect_used,
            reason = "the usage chunk is constructed to parse"
        )]
        let usage = usage.expect("the usage chunk parses");

        assert_eq!(
            usage,
            vec![IrEvent::Usage(IrUsage {
                input_tokens: 100,
                output_tokens: 50,
                cached_tokens: Some(80),
                cache_write_tokens: Some(7),
                reasoning_tokens: Some(20),
            })],
        );
    }
}
