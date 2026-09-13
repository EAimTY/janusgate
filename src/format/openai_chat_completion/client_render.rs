//! Client-response rendering for the Chat Completions codec: renders IR
//! events as Chat Completions SSE frames or an aggregated non-streaming body.

use crate::{
    format::{self, ClientSseRenderer},
    ids,
    ir::{IrAggregation, IrEvent, IrUsage},
    sse, unix_time,
};
use serde_json::Value;

/// Streaming client response renderer producing Chat Completions SSE frames.
#[derive(Debug)]
pub(crate) struct SseRenderer {
    /// Gateway-issued completion identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Unix creation timestamp.
    created: i64,

    /// Whether the leading role chunk was emitted.
    is_role_sent: bool,

    /// Arguments already streamed, per opened tool-call index. An index's
    /// presence marks its call as opened by a delta chunk; a done for an
    /// index outside this map carries the complete call of an upstream
    /// that never streamed deltas, so it renders as one full tool_calls
    /// delta chunk. The accumulated prefixes let a conformant done render
    /// exactly the argument suffix the live deltas did not deliver — a
    /// done carries the complete arguments while an opening delta may
    /// carry none of them — and let the renderer recognize a done whose
    /// arguments the streamed bytes are not a prefix of (README §6.2).
    streamed_arguments: Vec<(usize, String)>,

    /// Whether the trailing usage chunk is wanted.
    should_include_usage: bool,

    /// Latest token accounting observed; rendered once as the terminal
    /// usage chunk, because the official stream shape carries usage only on
    /// the final chunk before `[DONE]`.
    usage: Option<IrUsage>,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(crate) fn new(model: String, should_include_usage: bool) -> Self {
        Self {
            id: format!("chatcmpl-{}", ids::new_uuid()),
            model,
            created: unix_time::unix_now(),
            is_role_sent: false,
            streamed_arguments: Vec::new(),
            should_include_usage,
            usage: None,
        }
    }

    /// Formats one delta chunk with an optional finish reason.
    fn chunk_frame(&self, delta: Value, finish_reason: Option<&str>) -> String {
        let chunk = serde_json::json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason,
            }],
        });

        sse::format_data_frame(&chunk.to_string())
    }

    /// Formats the terminal usage-only chunk carrying an empty choices
    /// array, matching the official stream shape.
    fn usage_frame(&self, usage: IrUsage) -> String {
        let chunk = serde_json::json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [],
            "usage": {
                "prompt_tokens": usage.input_tokens,
                "completion_tokens": usage.output_tokens,
                "total_tokens": usage.input_tokens + usage.output_tokens,
            },
        });

        sse::format_data_frame(&chunk.to_string())
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => {
                if self.is_role_sent {
                    Vec::new()
                } else {
                    self.is_role_sent = true;

                    vec![self.chunk_frame(
                        serde_json::json!({"role": "assistant", "content": ""}),
                        None,
                    )]
                }
            }

            IrEvent::TextDelta(delta) => {
                vec![self.chunk_frame(serde_json::json!({"content": delta}), None)]
            }

            IrEvent::ReasoningDelta(delta) => {
                // Chat Completions has no official reasoning delta field;
                // `reasoning_content` is the de-facto key that reasoner-aware
                // OpenAI-compatible clients read (see DeepSeek's reasoning
                // model guide).
                vec![self.chunk_frame(serde_json::json!({"reasoning_content": delta}), None)]
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                match self
                    .streamed_arguments
                    .iter_mut()
                    .find(|(opened_index, _)| opened_index == index)
                {
                    Some((_, streamed)) => streamed.push_str(arguments_delta),

                    None => self
                        .streamed_arguments
                        .push((*index, arguments_delta.clone())),
                }

                let mut function = serde_json::Map::new();

                if let Some(name) = name {
                    function.insert("name".to_string(), Value::String(name.clone()));
                }

                function.insert(
                    "arguments".to_string(),
                    Value::String(arguments_delta.clone()),
                );

                let mut tool_call = serde_json::Map::new();

                tool_call.insert("index".to_string(), Value::from(*index as u64));

                if let Some(call_id) = call_id {
                    tool_call.insert("id".to_string(), Value::String(call_id.clone()));
                    tool_call.insert("type".to_string(), Value::String("function".to_string()));
                }

                tool_call.insert("function".to_string(), Value::Object(function));

                vec![self.chunk_frame(
                    serde_json::json!({"tool_calls": [Value::Object(tool_call)]}),
                    None,
                )]
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let streamed = self
                    .streamed_arguments
                    .iter()
                    .find(|(opened_index, _)| opened_index == index)
                    .map(|(_, streamed)| streamed);

                match streamed {
                    None => {
                        // The upstream never streamed a delta for this index
                        // (README §6.2 recovery class), so the done payload
                        // carries the complete call and renders as one full
                        // tool_calls delta chunk, exactly the shape a delta
                        // would have produced.
                        let mut tool_call = serde_json::json!({
                            "index": index,
                            "id": call_id,
                            "type": "function",
                            "function": {"name": name, "arguments": arguments},
                        });

                        if let Some(signature) = thought_signature {
                            format::json_insert(
                                &mut tool_call,
                                "thought_signature",
                                Value::String(signature.clone()),
                            );
                        }

                        vec![self.chunk_frame(serde_json::json!({"tool_calls": [tool_call]}), None)]
                    }

                    Some(streamed) => {
                        let mut frames = Vec::new();

                        // The live deltas may have streamed only a prefix
                        // of the done's complete arguments — an opening
                        // delta alone carries none — so the done renders
                        // exactly the un-streamed suffix; a done whose
                        // arguments the deltas already streamed in full
                        // renders no argument chunk. Third-party bytes can
                        // diverge from that shape, and a residual cut from
                        // a non-prefix offset would splice onto the
                        // streamed prefix into arguments matching neither
                        // the deltas nor the done, so `strip_prefix`
                        // silently skips the residual argument frame of a
                        // done whose arguments are not a superstring of
                        // the streamed bytes (README §6.2); the skipped
                        // done's other content still renders.
                        if let Some(unstreamed) = arguments.strip_prefix(streamed.as_str())
                            && !unstreamed.is_empty()
                        {
                            frames.push(self.chunk_frame(
                                serde_json::json!({"tool_calls": [{
                                    "index": index,
                                    "function": {"arguments": unstreamed},
                                }]}),
                                None,
                            ));
                        }

                        if let Some(signature) = thought_signature {
                            frames.push(self.chunk_frame(
                                serde_json::json!({"tool_calls": [{
                                    "index": index,
                                    "thought_signature": signature,
                                }]}),
                                None,
                            ));
                        }

                        frames
                    }
                }
            }

            IrEvent::Finish(reason) => {
                vec![self.chunk_frame(
                    serde_json::json!({}),
                    Some(format::finish_reason_to_openai(*reason)),
                )]
            }

            IrEvent::Usage(usage) => {
                // Gemini upstreams attach cumulative usage to every stream
                // chunk; the latest observation wins and renders once,
                // terminally.
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = Vec::new();

                if self.should_include_usage
                    && let Some(usage) = self.usage
                {
                    frames.push(self.usage_frame(usage));
                }

                frames.push(sse::format_data_frame("[DONE]"));

                frames
            }
        }
    }

    /// Renders a mid-stream error as a terminal SSE frame.
    fn error_frame(&mut self, error: &serde_json::Value) -> String {
        sse::format_data_frame(&error.to_string())
    }
}

/// Renders an aggregated completion as a non-streaming Chat Completions
/// response.
pub(crate) fn render_json_response(model: &str, aggregation: &IrAggregation) -> Value {
    let tool_calls: Vec<Value> = aggregation
        .tool_calls
        .iter()
        .map(|tool_call| {
            let mut tool_call_json = serde_json::json!({
                "id": tool_call.call_id,
                "type": "function",
                "function": {
                    "name": tool_call.name,
                    "arguments": tool_call.arguments,
                },
            });

            if let Some(signature) = &tool_call.thought_signature {
                format::json_insert(
                    &mut tool_call_json,
                    "thought_signature",
                    Value::String(signature.clone()),
                );
            }

            tool_call_json
        })
        .collect();

    let mut message = serde_json::json!({"role": "assistant"});

    let content = if aggregation.text.is_empty() {
        Value::Null
    } else {
        Value::String(aggregation.text.clone())
    };

    format::json_insert(&mut message, "content", content);

    if !aggregation.reasoning.is_empty() {
        format::json_insert(
            &mut message,
            "reasoning_content",
            Value::String(aggregation.reasoning.clone()),
        );
    }

    if !tool_calls.is_empty() {
        format::json_insert(&mut message, "tool_calls", Value::Array(tool_calls));
    }

    let finish_reason = aggregation
        .finish
        .map(format::finish_reason_to_openai)
        .unwrap_or("stop");

    let usage = aggregation.usage.unwrap_or_default();

    serde_json::json!({
        "id": format!("chatcmpl-{}", ids::new_uuid()),
        "object": "chat.completion",
        "created": unix_time::unix_now(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": usage.input_tokens,
            "completion_tokens": usage.output_tokens,
            "total_tokens": usage.input_tokens + usage.output_tokens,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the JSON payload of every rendered SSE frame.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn frame_payloads(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .map(|frame| {
                serde_json::from_str(
                    frame
                        .strip_prefix("data: ")
                        .expect("every frame carries a data line")
                        .trim_end(),
                )
                .expect("frame data is rendered JSON")
            })
            .collect()
    }

    /// Joins the tool-call argument fragments a rendered frame set delivers,
    /// exactly as a client assembling the stream would.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn assembled_arguments(frames: &[String]) -> String {
        frame_payloads(frames)
            .iter()
            .map(|payload| {
                payload
                    .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .expect("every rendered chunk carries one tool call")
                    .as_str()
                    .expect("tool-call arguments render as a JSON string")
                    .to_string()
            })
            .collect()
    }

    /// Parses the JSON payload of the only rendered SSE frame.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn lone_frame_payload(frames: &[String]) -> Value {
        let mut payloads = frame_payloads(frames);

        let payload = payloads.pop().expect("the event renders a frame");

        assert!(payloads.is_empty(), "the event renders exactly one frame");

        payload
    }

    /// A call opened by an empty delta — the Responses upstream opens every
    /// function call with a zero-byte `output_item.added` delta — receives
    /// its full arguments from the done event exactly once, because a
    /// streamed done always carries the authoritative complete arguments
    /// (README §6.2).
    #[test]
    fn done_renders_arguments_the_opening_delta_never_streamed() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: String::new(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city":"Paris","unit":"c"}"#.to_string(),
            thought_signature: None,
        });

        assert_eq!(assembled_arguments(&opened), "");

        assert_eq!(assembled_arguments(&done), r#"{"city":"Paris","unit":"c"}"#);
    }

    /// Live deltas that already streamed the full arguments — the Gemini
    /// upstream delivers every call this way — are not repeated by the done
    /// event, so the client assembles the arguments exactly once.
    #[test]
    fn done_repeats_no_arguments_the_live_deltas_already_streamed() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: r#"{"city":"Paris"}"#.to_string(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city":"Paris"}"#.to_string(),
            thought_signature: None,
        });

        assert_eq!(assembled_arguments(&opened), r#"{"city":"Paris"}"#);

        assert_eq!(assembled_arguments(&done), "");

        assert!(done.is_empty());
    }

    /// A divergent upstream whose done arguments are not a superstring of
    /// the streamed bytes — shorter than the streamed bytes, or diverging
    /// from them at a byte the deltas did not stream — loses its residual
    /// arguments silently: the done skips its argument frame (README
    /// §6.2), while its other content still renders.
    #[test]
    fn done_with_arguments_divergent_from_the_streamed_bytes_skips_the_argument_frame() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: r#"{"city":"Paris"}"#.to_string(),
        });

        // The done is shorter than the streamed bytes, so the streamed
        // bytes are not a prefix of its arguments.
        let shorter = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        });

        let reopened = renderer.render(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_2".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: "é".to_string(),
        });

        // The streamed bytes "é" are not a byte-prefix of the done's
        // "xé" — it diverges at the first byte — so the residual argument
        // frame is skipped; the done's thought signature still renders.
        let divergent = renderer.render(&IrEvent::ToolCallDone {
            index: 1,
            call_id: "call_2".to_string(),
            name: "get_weather".to_string(),
            arguments: "xé".to_string(),
            thought_signature: Some("sig".to_string()),
        });

        assert_eq!(assembled_arguments(&opened), r#"{"city":"Paris"}"#);

        assert!(shorter.is_empty());

        assert_eq!(assembled_arguments(&reopened), "é");

        let payload = lone_frame_payload(&divergent);

        assert_eq!(
            payload.pointer("/choices/0/delta/tool_calls/0/thought_signature"),
            Some(&serde_json::json!("sig"))
        );

        assert!(
            payload
                .pointer("/choices/0/delta/tool_calls/0/function")
                .is_none()
        );
    }

    /// A done that is longer than, but divergent from, the streamed bytes
    /// renders no residual: a suffix cut past a non-prefix offset would
    /// splice onto the streamed prefix into arguments matching neither the
    /// deltas nor the done, so the divergent done silently skips its
    /// argument frame (README §6.2).
    #[test]
    fn done_longer_but_divergent_from_the_streamed_bytes_skips_the_argument_frame() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: "abc".to_string(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: "xbcd".to_string(),
            thought_signature: None,
        });

        assert_eq!(assembled_arguments(&opened), "abc");

        assert!(done.is_empty());
    }

    /// A done for an index no delta ever opened — the recovery class of
    /// README §6.2 — renders the complete call as one full tool_calls delta
    /// chunk, exactly the shape a delta would have produced.
    #[test]
    fn done_without_a_prior_delta_renders_the_complete_call() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 2,
            call_id: "call_3".to_string(),
            name: "get_weather".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("sig".to_string()),
        });

        assert_eq!(
            lone_frame_payload(&done).get("choices"),
            Some(&serde_json::json!([{
                "index": 0,
                "delta": {"tool_calls": [{
                    "index": 2,
                    "id": "call_3",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{}"},
                    "thought_signature": "sig",
                }]},
                "finish_reason": null,
            }]))
        );
    }
}
