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

    /// Tool call indexes opened by a delta chunk; a done for an index outside
    /// this set carries the complete call of an upstream that never streamed
    /// deltas, so it renders as one full tool_calls delta chunk.
    opened_tool_call_indexes: Vec<usize>,

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
            opened_tool_call_indexes: Vec::new(),
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
                if !self.opened_tool_call_indexes.contains(index) {
                    self.opened_tool_call_indexes.push(*index);
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
                let is_opened = self.opened_tool_call_indexes.contains(index);

                if !is_opened {
                    // The upstream never streamed a delta for this index
                    // (README §6.3 recovery class), so the done payload
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
                } else if let Some(signature) = thought_signature {
                    vec![self.chunk_frame(
                        serde_json::json!({"tool_calls": [{
                            "index": index,
                            "thought_signature": signature,
                        }]}),
                        None,
                    )]
                } else {
                    // The call was fully assembled by its deltas.
                    Vec::new()
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
