//! Client-response rendering for the Responses codec: renders IR events as
//! Responses SSE events or an aggregated non-streaming body.

use crate::{
    format::{self, ClientSseRenderer},
    ids,
    ir::{IrAggregation, IrEvent, IrFinishReason, IrUsage},
    sse, unix_time,
};
use serde_json::Value;

/// Streaming client response renderer producing Responses SSE events.
#[derive(Debug)]
pub(super) struct SseRenderer {
    /// Monotonic sequence number of the next event.
    sequence: u64,

    /// Gateway-issued response identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Unix creation timestamp.
    created_at: i64,

    /// Whether `response.created` was emitted.
    is_created: bool,

    /// Open reasoning item awaiting completion: `(item_id, output_index,
    /// accumulated text)`.
    reasoning: Option<(String, usize, String)>,

    /// Open assistant message item awaiting completion.
    message: Option<(String, usize, String)>,

    /// Open function call items keyed by IR tool call index.
    calls: Vec<(usize, OutputItem)>,

    /// Completed output items for the final response object.
    completed_items: Vec<OutputItem>,

    /// Next output index to assign.
    next_output_index: usize,

    /// Token accounting observed before completion.
    usage: Option<IrUsage>,

    /// Finish reason reported before [`IrEvent::Done`], deciding whether the
    /// terminal event is `response.completed` or `response.incomplete`.
    finish: Option<IrFinishReason>,
}

/// One output item tracked by the client-side SSE renderer.
#[derive(Debug)]
struct OutputItem {
    /// Position of the item within the output array.
    output_index: usize,

    /// The rendered item.
    item: Value,
}

impl SseRenderer {
    /// Creates a renderer for one streamed response.
    pub(super) fn new(model: String) -> Self {
        Self {
            sequence: 0,
            id: format!("resp_{}", ids::new_uuid()),
            model,
            created_at: unix_time::unix_now(),
            is_created: false,
            reasoning: None,
            message: None,
            calls: Vec::new(),
            completed_items: Vec::new(),
            next_output_index: 0,
            usage: None,
            finish: None,
        }
    }

    /// Closes any open reasoning or message items, emitting their done event
    /// sequences and recording the completed items.
    fn close_open_items(&mut self) -> Vec<String> {
        let mut frames = Vec::new();

        if let Some((item_id, output_index, text)) = self.reasoning.take() {
            let completed_item = serde_json::json!({
                "id": item_id,
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": text}],
            });

            for (event_name, payload) in [
                (
                    "response.reasoning_summary_text.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "text": text,
                    }),
                ),
                (
                    "response.reasoning_summary_part.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "part": {"type": "summary_text", "text": text},
                    }),
                ),
                (
                    "response.output_item.done",
                    serde_json::json!({
                        "output_index": output_index,
                        "item": completed_item.clone(),
                    }),
                ),
            ] {
                frames.push(self.event_frame(event_name, payload));
            }

            self.completed_items.push(OutputItem {
                output_index,
                item: completed_item,
            });
        }

        if let Some((item_id, output_index, text)) = self.message.take() {
            let content_part = serde_json::json!({
                "type": "output_text",
                "text": text,
                "annotations": [],
                "logprobs": [],
            });

            let completed_item = serde_json::json!({
                "id": item_id,
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [content_part.clone()],
            });

            for (event_name, payload) in [
                (
                    "response.output_text.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "text": content_part.get("text").cloned().unwrap_or(Value::Null),
                        "logprobs": [],
                    }),
                ),
                (
                    "response.content_part.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": content_part,
                    }),
                ),
                (
                    "response.output_item.done",
                    serde_json::json!({
                        "output_index": output_index,
                        "item": completed_item.clone(),
                    }),
                ),
            ] {
                frames.push(self.event_frame(event_name, payload));
            }

            self.completed_items.push(OutputItem {
                output_index,
                item: completed_item,
            });
        }

        frames
    }

    /// Builds the response object shell shared by all events.
    fn response_shell(&self, status: &str, output: Value) -> Value {
        response_shell(
            self.id.clone(),
            self.created_at,
            &self.model,
            status,
            output,
        )
    }

    /// Formats one SSE frame with the given event name and payload, assigning
    /// the next sequence number.
    ///
    /// The payload carries `type` because strict clients — openai-python's
    /// `ResponseStreamEvent` union and openai/codex's serde type-tagged enum —
    /// discriminate events on the JSON payload, not the `event:` line.
    fn event_frame(&mut self, event_name: &str, mut payload: Value) -> String {
        format::json_insert(&mut payload, "type", Value::String(event_name.to_string()));

        format::json_insert(&mut payload, "sequence_number", Value::from(self.sequence));

        self.sequence += 1;

        sse::format_frame(Some(event_name), &payload.to_string())
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => {
                if self.is_created {
                    return Vec::new();
                }

                self.is_created = true;

                let shell = self.response_shell("in_progress", Value::Array(Vec::new()));

                vec![
                    self.event_frame(
                        "response.created",
                        serde_json::json!({"response": shell.clone()}),
                    ),
                    self.event_frame(
                        "response.in_progress",
                        serde_json::json!({"response": shell}),
                    ),
                ]
            }

            IrEvent::ReasoningDelta(delta) => {
                let (mut frames, item_id, output_index) = match &self.reasoning {
                    Some((item_id, output_index, _)) => {
                        (Vec::new(), item_id.clone(), *output_index)
                    }

                    None => {
                        let item_id = format!("rs_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;
                        self.reasoning = Some((item_id.clone(), output_index, String::new()));

                        let mut frames = vec![self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({
                                "output_index": output_index,
                                "item": {"id": item_id, "type": "reasoning", "summary": []},
                            }),
                        )];

                        frames.push(self.event_frame(
                            "response.reasoning_summary_part.added",
                            serde_json::json!({
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "part": {"type": "summary_text", "text": ""},
                            }),
                        ));

                        (frames, item_id, output_index)
                    }
                };

                if let Some((_, _, accumulated)) = &mut self.reasoning {
                    accumulated.push_str(delta);
                }

                frames.push(self.event_frame(
                    "response.reasoning_summary_text.delta",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "delta": delta,
                    }),
                ));

                frames
            }

            IrEvent::TextDelta(delta) => {
                let (mut frames, item_id, output_index) = match &self.message {
                    Some((item_id, output_index, _)) => {
                        (Vec::new(), item_id.clone(), *output_index)
                    }

                    None => {
                        let item_id = format!("msg_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;
                        self.message = Some((item_id.clone(), output_index, String::new()));

                        let mut frames = vec![self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "message",
                                    "role": "assistant",
                                    "status": "in_progress",
                                    "content": [],
                                },
                            }),
                        )];

                        frames.push(self.event_frame(
                            "response.content_part.added",
                            serde_json::json!({
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "part": {
                                    "type": "output_text",
                                    "text": "",
                                    "annotations": [],
                                    "logprobs": [],
                                },
                            }),
                        ));

                        (frames, item_id, output_index)
                    }
                };

                if let Some((_, _, accumulated)) = &mut self.message {
                    accumulated.push_str(delta);
                }

                frames.push(self.event_frame(
                    "response.output_text.delta",
                    // `logprobs` is a required field of the official event
                    // shape (openai-python `ResponseTextDeltaEvent`), so the
                    // gateway always carries it, empty when untracked.
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": delta,
                        "logprobs": [],
                    }),
                ));

                frames
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                let mut frames = Vec::new();

                let (item_id, output_index) = match self
                    .calls
                    .iter()
                    .find(|(open_index, _)| open_index == index)
                {
                    Some((_, open)) => {
                        let item_id = open
                            .item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();

                        (item_id, open.output_index)
                    }

                    None => {
                        let item_id = format!("fc_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;

                        // A delta may open the item without revealing its
                        // identity: a bare
                        // `response.function_call_arguments.delta`
                        // with no preceding `output_item.added`, or a Chat
                        // Completions delta whose first tool-call entry
                        // carries only arguments, arrives exactly so. Every
                        // field of the official function_call item is a
                        // required string (openai-python's
                        // `ResponseFunctionToolCallItem` has no field
                        // defaults), so unknown identity renders as empty
                        // strings — the same defaults the done path's
                        // recovery branch opens with — until the done event
                        // stamps the real `call_id`, `name`, and `arguments`.
                        let item = serde_json::json!({
                            "id": item_id,
                            "type": "function_call",
                            "status": "in_progress",
                            "call_id": call_id.as_deref().unwrap_or_default(),
                            "name": name.as_deref().unwrap_or_default(),
                            "arguments": "",
                        });

                        frames.push(self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({"output_index": output_index, "item": item.clone()}),
                        ));

                        self.calls.push((*index, OutputItem { output_index, item }));

                        (item_id, output_index)
                    }
                };

                if !arguments_delta.is_empty() {
                    frames.push(self.event_frame(
                        "response.function_call_arguments.delta",
                        serde_json::json!({
                            "item_id": item_id,
                            "output_index": output_index,
                            "delta": arguments_delta,
                        }),
                    ));
                }

                frames
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let position = self
                    .calls
                    .iter()
                    .position(|(open_index, _)| open_index == index);

                let (mut open, mut frames) = match position {
                    Some(position) => (self.calls.swap_remove(position).1, Vec::new()),

                    None => {
                        // A third-party upstream may deliver
                        // `output_item.done` for a call it never revealed
                        // with `output_item.added` or argument deltas — the
                        // recovery class of README §6.3. The done payload
                        // carries the complete call, so the item is opened
                        // here exactly as a live delta would have opened it;
                        // dropping it would lose the call only on the
                        // streaming path, while aggregation keeps it.
                        let item_id = format!("fc_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;

                        let frames = vec![self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "function_call",
                                    "status": "in_progress",
                                    "call_id": call_id,
                                    "name": name,
                                    "arguments": "",
                                },
                            }),
                        )];

                        (
                            OutputItem {
                                output_index,
                                item: serde_json::json!({
                                    "id": item_id,
                                    "type": "function_call",
                                }),
                            },
                            frames,
                        )
                    }
                };

                format::json_insert(
                    &mut open.item,
                    "status",
                    Value::String("completed".to_string()),
                );
                format::json_insert(&mut open.item, "call_id", Value::String(call_id.clone()));
                format::json_insert(&mut open.item, "name", Value::String(name.clone()));
                format::json_insert(
                    &mut open.item,
                    "arguments",
                    Value::String(arguments.clone()),
                );

                if let Some(signature) = thought_signature {
                    format::json_insert(
                        &mut open.item,
                        "thought_signature",
                        Value::String(signature.clone()),
                    );
                }

                let output_index = open.output_index;
                let item_id = open
                    .item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();

                frames.push(self.event_frame(
                    "response.function_call_arguments.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "arguments": arguments,
                    }),
                ));

                frames.push(self.event_frame(
                    "response.output_item.done",
                    serde_json::json!({"output_index": output_index, "item": open.item.clone()}),
                ));

                self.completed_items.push(OutputItem {
                    output_index,
                    item: open.item,
                });

                frames
            }

            IrEvent::Finish(reason) => {
                self.finish = Some(*reason);

                self.close_open_items()
            }

            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = self.close_open_items();

                let mut output: Vec<(usize, Value)> = self
                    .completed_items
                    .iter()
                    .map(|open| (open.output_index, open.item.clone()))
                    .collect();

                output.sort_by_key(|(output_index, _)| *output_index);

                let output: Vec<Value> = output.into_iter().map(|(_, item)| item).collect();

                let usage = self.usage.unwrap_or_default();

                // A Length/ContentFilter finish must surface as
                // `response.incomplete` with `incomplete_details.reason`, so
                // strict clients can tell a truncated response from a full
                // one.
                let incomplete_reason = self.finish.and_then(incomplete_details_reason);
                let is_incomplete = incomplete_reason.is_some();

                let mut shell = self.response_shell(
                    if is_incomplete {
                        "incomplete"
                    } else {
                        "completed"
                    },
                    Value::Array(output),
                );

                format::json_insert(&mut shell, "usage", usage_json(usage));

                if let Some(reason) = incomplete_reason {
                    format::json_insert(
                        &mut shell,
                        "incomplete_details",
                        serde_json::json!({"reason": reason}),
                    );
                }

                let terminal_event = if is_incomplete {
                    "response.incomplete"
                } else {
                    "response.completed"
                };

                frames
                    .push(self.event_frame(terminal_event, serde_json::json!({"response": shell})));

                frames
            }
        }
    }

    /// Renders a mid-stream error as a terminal `response.failed` event.
    fn error_frame(&mut self, error: &serde_json::Value) -> String {
        let error_payload = error
            .get("error")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // Strict clients validate the embedded response object like any
        // other, so it carries the full required shell rather than a bare
        // id/status/error subset.
        let mut shell = self.response_shell("failed", Value::Array(Vec::new()));

        format::json_insert(&mut shell, "error", error_payload);

        self.event_frame(
            "response.failed",
            serde_json::json!({
                "response": shell,
            }),
        )
    }
}

/// Builds the official Response object shell shared by every rendered
/// response, streaming and non-streaming.
///
/// `parallel_tool_calls`, `tool_choice`, and `tools` are required fields of
/// the official Response shape (openai-python `Response` types them without
/// defaults), so the shell always carries the statics describing the gateway
/// itself: it advertises no parallel tool-call control, tool calls happen
/// whenever the model decides (`auto`), and no tool definitions are echoed
/// back to the client. Callers attach `usage` and, where applicable,
/// `incomplete_details` or `error` after construction.
fn response_shell(id: String, created_at: i64, model: &str, status: &str, output: Value) -> Value {
    serde_json::json!({
        "id": id,
        "object": "response",
        "created_at": created_at,
        "status": status,
        "model": model,
        "output": output,
        "parallel_tool_calls": false,
        "tool_choice": "auto",
        "tools": [],
    })
}

/// Renders an aggregated completion as a non-streaming Responses API response.
pub(super) fn render_json_response(model: &str, aggregation: &IrAggregation) -> Value {
    let mut output: Vec<Value> = Vec::new();

    if !aggregation.reasoning.is_empty() {
        output.push(serde_json::json!({
            "id": format!("rs_{}", ids::new_uuid()),
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": aggregation.reasoning}],
        }));
    }

    if !aggregation.text.is_empty() {
        output.push(serde_json::json!({
            "id": format!("msg_{}", ids::new_uuid()),
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": aggregation.text,
                "annotations": [],
                "logprobs": [],
            }],
        }));
    }

    for tool_call in &aggregation.tool_calls {
        let mut item = serde_json::json!({
            "id": format!("fc_{}", ids::new_uuid()),
            "type": "function_call",
            "status": "completed",
            "call_id": tool_call.call_id,
            "name": tool_call.name,
            "arguments": tool_call.arguments,
        });

        if let Some(signature) = &tool_call.thought_signature {
            format::json_insert(
                &mut item,
                "thought_signature",
                Value::String(signature.clone()),
            );
        }

        output.push(item);
    }

    let usage = aggregation.usage.unwrap_or_default();

    let incomplete_reason = aggregation.finish.and_then(incomplete_details_reason);

    let mut body = response_shell(
        format!("resp_{}", ids::new_uuid()),
        unix_time::unix_now(),
        model,
        if incomplete_reason.is_some() {
            "incomplete"
        } else {
            "completed"
        },
        Value::Array(output),
    );

    format::json_insert(&mut body, "usage", usage_json(usage));

    if let Some(reason) = incomplete_reason {
        format::json_insert(
            &mut body,
            "incomplete_details",
            serde_json::json!({"reason": reason}),
        );
    }

    body
}

/// Renders the usage object shared by the streaming terminal events and the
/// aggregated response body.
///
/// openai-python's `ResponseUsage` types all five fields as required — the two
/// detail objects included, their inner integers too (official OpenAPI schema:
/// `input_tokens_details` requires `cached_tokens` and `cache_write_tokens`,
/// `output_tokens_details` requires `reasoning_tokens`) — so the details are
/// always carried, zero-filled when the upstream reported no breakdown.
fn usage_json(usage: IrUsage) -> Value {
    serde_json::json!({
        "input_tokens": usage.input_tokens,
        "input_tokens_details": {
            "cached_tokens": usage.cached_tokens.unwrap_or(0),
            "cache_write_tokens": usage.cache_write_tokens.unwrap_or(0),
        },
        "output_tokens": usage.output_tokens,
        "output_tokens_details": {
            "reasoning_tokens": usage.reasoning_tokens.unwrap_or(0),
        },
        "total_tokens": usage.input_tokens + usage.output_tokens,
    })
}

/// Renders the `incomplete_details.reason` string for a finish reason that
/// marks an incomplete response; [`None`] for complete responses.
fn incomplete_details_reason(reason: IrFinishReason) -> Option<&'static str> {
    match reason {
        IrFinishReason::Length => Some("max_output_tokens"),

        IrFinishReason::ContentFilter => Some("content_filter"),

        IrFinishReason::Stop | IrFinishReason::ToolCall => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the JSON payload of the last rendered SSE frame.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn terminal_frame_payload(frames: &[String]) -> Value {
        let data = frames
            .last()
            .expect("the Done event renders a terminal frame")
            .split_once("\ndata: ")
            .expect("every frame carries a data line")
            .1;

        serde_json::from_str(data.trim_end()).expect("frame data is rendered JSON")
    }

    /// Parses the JSON payload of the first rendered frame carrying
    /// `event_name`.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing frame is the failure"
    )]
    fn frame_payload(frames: &[String], event_name: &str) -> Value {
        let data = frames
            .iter()
            .find(|frame| frame.starts_with(&format!("event: {event_name}\n")))
            .expect("the searched-for frame is rendered")
            .split_once("\ndata: ")
            .expect("every frame carries a data line")
            .1;

        serde_json::from_str(data.trim_end()).expect("frame data is rendered JSON")
    }

    /// A tool-call delta opening a never-announced item — the recovery shape
    /// a bare `response.function_call_arguments.delta` upstream event or a
    /// Chat Completions delta carrying only arguments renders — emits an
    /// `output_item.added` function_call whose `call_id` and `name` are
    /// strings, empty when unknown: openai-python's
    /// `ResponseFunctionToolCallItem` requires both as strings with no
    /// defaults, so a `null` or missing field would fail a strict typed
    /// client on the transient item before the done event repairs it.
    #[test]
    fn opening_tool_call_delta_renders_required_identity_strings() {
        let mut renderer = SseRenderer::new("m".to_string());

        let frames = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: None,
            name: None,
            arguments_delta: "{\"".to_string(),
        });

        let item = frame_payload(&frames, "response.output_item.added")
            .get("item")
            .expect("the added frame carries the item")
            .clone();

        assert_eq!(item.get("call_id"), Some(&Value::String(String::new())));

        assert_eq!(item.get("name"), Some(&Value::String(String::new())));

        assert_eq!(
            item.get("type"),
            Some(&Value::String("function_call".to_string()))
        );

        assert_eq!(
            item.get("status"),
            Some(&Value::String("in_progress".to_string()))
        );

        assert_eq!(item.get("arguments"), Some(&Value::String(String::new())));

        let arguments = frame_payload(&frames, "response.function_call_arguments.delta");

        assert_eq!(
            arguments.get("delta"),
            Some(&Value::String("{\"".to_string()))
        );
    }

    /// The streaming terminal event renders the full official usage shape:
    /// openai-python's `ResponseUsage` requires all five fields including both
    /// detail objects, so upstream-reported breakdowns ride through intact.
    #[test]
    fn terminal_event_usage_carries_upstream_breakdowns() {
        let mut renderer = SseRenderer::new("m".to_string());

        renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 4,
            cached_tokens: Some(6),
            cache_write_tokens: Some(2),
            reasoning_tokens: Some(3),
        }));

        let frames = renderer.render(&IrEvent::Done);

        let payload = terminal_frame_payload(&frames);

        let usage = payload
            .pointer("/response/usage")
            .expect("the terminal response carries usage");

        assert_eq!(
            usage,
            &serde_json::json!({
                "input_tokens": 10,
                "input_tokens_details": {
                    "cached_tokens": 6,
                    "cache_write_tokens": 2,
                },
                "output_tokens": 4,
                "output_tokens_details": {"reasoning_tokens": 3},
                "total_tokens": 14,
            })
        );
    }

    /// Breakdowns absent from the upstream — every cross-format upstream, and
    /// any Responses upstream that reports totals only — render zero-filled,
    /// because strict SDK consumers require the detail fields to be present.
    #[test]
    fn aggregated_response_usage_zero_fills_absent_breakdowns() {
        let aggregation = IrAggregation {
            usage: Some(IrUsage {
                input_tokens: 7,
                output_tokens: 2,
                ..IrUsage::default()
            }),
            ..IrAggregation::default()
        };

        let body = render_json_response("m", &aggregation);

        assert_eq!(
            body.get("usage"),
            Some(&serde_json::json!({
                "input_tokens": 7,
                "input_tokens_details": {
                    "cached_tokens": 0,
                    "cache_write_tokens": 0,
                },
                "output_tokens": 2,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 9,
            }))
        );
    }
}
