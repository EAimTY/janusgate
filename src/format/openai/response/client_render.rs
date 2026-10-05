//! Client-response rendering for the Responses codec: renders IR events as Responses SSE events or
//! an aggregated non-streaming body.
//!
//! Every rendered payload is a first-party type in the official Responses shape (README §6.3), so
//! the wire contract lives in the type system rather than in assembled JSON.

use crate::{
    format::{self, ClientSseRenderer, openai::error::OpenAiErrorBody},
    ids,
    ir::{IrAggregation, IrEvent, IrFinishReason, IrUsage},
    sse, unix_time,
};
use serde::Serialize;
use std::mem;

/// Sequence number of the first rendered event.
const FIRST_SEQUENCE_NUMBER: u64 = 0;

/// Position of the only content part and summary part the gateway renders.
const PRIMARY_INDEX: u32 = 0;

/// Streaming client response renderer producing Responses SSE events.
pub(crate) struct SseRenderer {
    /// Monotonic sequence number of the next event.
    sequence: u64,

    /// Digest-only projection of actual outward item positions.
    projection: Option<super::affinity::Output>,

    /// Gateway-issued response identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Unix creation timestamp.
    created_at: i64,

    /// Whether `response.created` was emitted.
    is_created: bool,

    /// Open reasoning item awaiting completion.
    reasoning: Option<OpenTextItem>,

    /// Open assistant message item awaiting completion.
    message: Option<OpenTextItem>,

    /// Compaction state items awaiting their added/done frame pairs, keyed by
    /// their output position; drained once the response has opened, ahead of
    /// every IR-derived item.
    prefix_items: Vec<(usize, crate::format::compaction_wire::Checkpoint)>,

    /// Actual final prefix frame in the latest render batch.
    prefix_boundary: Option<usize>,

    /// Digest-only native checkpoint waiting for its actual output-item done handoff.
    completed_artifact: Option<crate::format::CompletedArtifact>,

    /// Whether an authenticated prefix has already seeded the active projection.
    is_prefix_prepared: bool,

    /// Open function call items keyed by IR tool call index.
    calls: Vec<(usize, TrackedCall)>,

    /// Completed output items for the final response object.
    completed_items: Vec<TrackedItem>,

    /// Next output index to assign.
    next_output_index: usize,

    /// Token accounting observed before completion.
    usage: Option<IrUsage>,

    /// Finish reason reported before [`IrEvent::Done`], deciding whether the terminal event is
    /// `response.completed` or `response.incomplete`.
    finish: Option<IrFinishReason>,
}

/// An open text-bearing item — a reasoning summary or an assistant message — accumulating its text
/// across deltas.
#[derive(Debug)]
struct OpenTextItem {
    /// Gateway-issued item identifier.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Text accumulated so far.
    text: String,
}

/// A function call item tracked while its stream is in flight: the call as rendered so far beside
/// its position in the output array.
#[derive(Debug)]
struct TrackedCall {
    /// Position of the item within the output array.
    output_index: usize,

    /// The call item, completed in place by its done event.
    item: FunctionCallItem,
}

/// A completed output item tracked for the terminal response object.
#[derive(Debug)]
struct TrackedItem {
    /// Position of the item within the output array.
    output_index: usize,

    /// The rendered item.
    item: OutputItem,
}

/// One rendered output item: a gateway-rendered item in the official shape, or
/// a verbatim vendor-shaped state item replayed at the output head.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum OutputItem {
    /// A gateway-rendered item.
    Rendered(RenderedOutputItem),

    /// A vendor-shaped state item — a compaction item or native state replayed
    /// ahead of the IR-derived items — carried verbatim; its shape is the
    /// vendor's, not the gateway's.
    State(crate::format::compaction_wire::Checkpoint),
}

/// The gateway-rendered output items in the official Responses shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RenderedOutputItem {
    /// A reasoning item carrying its summary.
    Reasoning {
        /// Gateway-issued item identifier.
        id: String,

        /// Summary parts; the gateway renders exactly one.
        summary: Vec<SummaryPart>,
    },

    /// An assistant message item carrying its output text.
    Message {
        /// Gateway-issued item identifier.
        id: String,

        /// Always `assistant`.
        role: &'static str,

        /// Item lifecycle status.
        status: ItemStatus,

        /// Content parts; the gateway renders exactly one.
        content: Vec<OutputTextPart>,
    },

    /// A function call requested by the model.
    FunctionCall(FunctionCallItem),
}

/// One function call output item. Every identity field is a string in the official shape
/// (openai-python's `ResponseFunctionToolCallItem` types them without defaults), so an item opened
/// before its identity is known carries empty strings until its done event stamps the authoritative
/// values.
#[derive(Debug, Clone, Serialize)]
pub(super) struct FunctionCallItem {
    /// Gateway-issued item identifier.
    id: String,

    /// Item lifecycle status.
    status: ItemStatus,

    /// Identifier of the invocation, round-tripped to the matching tool result.
    pub(super) call_id: String,

    /// Name of the invoked function.
    pub(super) name: String,

    /// JSON-encoded arguments, accumulated across the call's stream.
    pub(super) arguments: String,

    /// Upstream thought signature the client must replay on the next turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thought_signature: Option<String>,
}

/// Lifecycle status of a rendered output item.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ItemStatus {
    /// The item is still streaming.
    InProgress,

    /// The item is complete.
    Completed,
}

/// Lifecycle status of a rendered response object.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResponseStatus {
    /// The response has been created; its output has not started.
    InProgress,

    /// The response finished cleanly.
    Completed,

    /// The response finished truncated or filtered.
    Incomplete,

    /// The response failed mid-stream.
    Failed,
}

/// One summary part of a reasoning item.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SummaryPart {
    /// The reasoning text the gateway accumulated.
    SummaryText {
        /// Accumulated reasoning text.
        text: String,
    },
}

/// One content part of a rendered message item; the gateway renders text only.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputTextPart {
    /// An output text part carrying the assistant's text.
    OutputText {
        /// Accumulated assistant text.
        text: String,

        /// Official `annotations` field. The gateway never annotates output text, so the array is
        /// always empty; its element shape is the official OpenAI shape, which the gateway never
        /// renders.
        annotations: [(); 0],

        /// Official `logprobs` field. The gateway tracks no token probabilities, so the array is
        /// always empty; its element shape is the official OpenAI shape, which the gateway never
        /// renders.
        logprobs: [(); 0],
    },
}

/// The official Response object, shared by every rendered response — streaming and non-streaming —
/// and by the `response.failed` error frame.
///
/// `parallel_tool_calls`, `tool_choice`, and `tools` are required fields of the official Response
/// shape (openai-python `Response` types them without defaults), so the shell always carries the
/// statics describing the gateway itself: it advertises no parallel tool-call control, tool calls
/// happen whenever the model decides (`auto`), and no tool definitions are echoed back to the
/// client. `usage`, `incomplete_details`, and `error` are carried by the events that report them
/// and omitted elsewhere.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseObject {
    /// Gateway-issued response identifier.
    id: String,

    /// Always `response`.
    object: &'static str,

    /// Unix creation timestamp.
    created_at: i64,

    /// Response lifecycle status.
    status: ResponseStatus,

    /// Public model name the client addressed.
    model: String,

    /// Completed output items in output order.
    output: Vec<OutputItem>,

    /// Always `false`: the gateway exposes no parallel tool-call control.
    parallel_tool_calls: bool,

    /// Always `auto`: the model decides whether to call tools.
    tool_choice: &'static str,

    /// Always empty: the gateway echoes no tool definitions back to the client. The element shape
    /// is the official OpenAI tool shape, which the gateway never renders here.
    tools: [(); 0],

    /// Token accounting, carried by the terminal event and the aggregated body.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<ResponseUsage>,

    /// Why the response is incomplete, carried only by an incomplete response.
    #[serde(skip_serializing_if = "Option::is_none")]
    incomplete_details: Option<IncompleteDetails>,

    /// The failure, carried only by a failed response.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<OpenAiErrorBody>,
}

/// Token accounting in the official Responses shape.
///
/// openai-python's `ResponseUsage` types all five fields as required — the two detail objects
/// included, their inner integers too (official OpenAPI schema: `input_tokens_details` requires
/// `cached_tokens` and `cache_write_tokens`, `output_tokens_details` requires `reasoning_tokens`) —
/// so the details are always carried, zero-filled when the upstream reported no breakdown.
#[derive(Debug, Clone, Serialize)]
struct ResponseUsage {
    /// Tokens consumed by the input.
    input_tokens: u64,

    /// Input token breakdown.
    input_tokens_details: InputTokensDetails,

    /// Tokens produced by the output.
    output_tokens: u64,

    /// Output token breakdown.
    output_tokens_details: OutputTokensDetails,

    /// Sum of input and output tokens.
    total_tokens: u64,
}

impl ResponseUsage {
    /// Renders one observation, zero-filling every absent breakdown.
    fn render(usage: IrUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            input_tokens_details: InputTokensDetails {
                cached_tokens: usage.cached_tokens.unwrap_or(0),
                cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
            },
            output_tokens: usage.output_tokens,
            output_tokens_details: OutputTokensDetails {
                reasoning_tokens: usage.reasoning_tokens.unwrap_or(0),
            },
            total_tokens: usage.input_tokens + usage.output_tokens,
        }
    }
}

/// The official input token breakdown.
#[derive(Debug, Clone, Serialize)]
struct InputTokensDetails {
    /// Input tokens served from the upstream prompt cache.
    cached_tokens: u64,

    /// Input tokens written to the upstream prompt cache.
    cache_write_tokens: u64,
}

/// The official output token breakdown.
#[derive(Debug, Clone, Serialize)]
struct OutputTokensDetails {
    /// Output tokens spent on model reasoning.
    reasoning_tokens: u64,
}

/// Why a response is incomplete.
#[derive(Debug, Clone, Serialize)]
struct IncompleteDetails {
    /// Official reason string.
    reason: &'static str,
}

/// The envelope every rendered SSE event carries: the event name as the payload's `type`
/// discriminator and the event's sequence number, wrapped around the event's own fields.
///
/// The payload carries `type` because strict clients — openai-python's `ResponseStreamEvent` union
/// and openai/codex's serde type-tagged enum — discriminate events on the JSON payload, not the
/// `event:` line.
#[derive(Debug, Serialize)]
struct EventEnvelope<'a, T> {
    /// Event name, rendered both on the `event:` line and as the payload's `type`.
    #[serde(rename = "type")]
    event_type: &'a str,

    /// Monotonic sequence number of this event within the response.
    sequence_number: u64,

    /// The event's own fields.
    #[serde(flatten)]
    payload: T,
}

/// Payload of the five response-level events: the response object as it stands.
#[derive(Debug, Serialize)]
struct ResponseEvent {
    /// The response object.
    response: ResponseObject,
}

/// Payload of `response.output_item.added` and `response.output_item.done`.
#[derive(Debug, Serialize)]
struct OutputItemEvent {
    /// Position of the item within the output array.
    output_index: usize,

    /// The item as it stands.
    item: OutputItem,
}

/// Payload of `response.reasoning_summary_part.added` and `response.reasoning_summary_part.done`.
#[derive(Debug, Serialize)]
struct ReasoningSummaryPartEvent {
    /// Identifier of the reasoning item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Position of the summary part within the item.
    summary_index: u32,

    /// The summary part.
    part: SummaryPart,
}

/// Payload of `response.reasoning_summary_text.delta`.
#[derive(Debug, Serialize)]
struct ReasoningSummaryTextDelta {
    /// Identifier of the reasoning item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Position of the summary part within the item.
    summary_index: u32,

    /// Incremental reasoning text.
    delta: String,
}

/// Payload of `response.reasoning_summary_text.done`.
#[derive(Debug, Serialize)]
struct ReasoningSummaryTextDone {
    /// Identifier of the reasoning item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Position of the summary part within the item.
    summary_index: u32,

    /// Complete reasoning text.
    text: String,
}

/// Payload of `response.content_part.added` and `response.content_part.done`.
#[derive(Debug, Serialize)]
struct ContentPartEvent {
    /// Identifier of the message item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Position of the content part within the item.
    content_index: u32,

    /// The content part.
    part: OutputTextPart,
}

/// Payload of `response.output_text.delta`.
#[derive(Debug, Serialize)]
struct OutputTextDelta {
    /// Identifier of the message item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Position of the content part within the item.
    content_index: u32,

    /// Incremental assistant text.
    delta: String,

    /// Official `logprobs` field, always empty (see [`OutputTextPart`]).
    logprobs: [(); 0],
}

/// Payload of `response.output_text.done`.
#[derive(Debug, Serialize)]
struct OutputTextDone {
    /// Identifier of the message item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Position of the content part within the item.
    content_index: u32,

    /// Complete assistant text.
    text: String,

    /// Official `logprobs` field, always empty (see [`OutputTextPart`]).
    logprobs: [(); 0],
}

/// Payload of `response.function_call_arguments.delta`.
#[derive(Debug, Serialize)]
struct FunctionCallArgumentsDelta {
    /// Identifier of the function call item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Incremental JSON-encoded arguments text.
    delta: String,
}

/// Payload of `response.function_call_arguments.done`.
#[derive(Debug, Serialize)]
struct FunctionCallArgumentsDone {
    /// Identifier of the function call item.
    item_id: String,

    /// Position of the item within the output array.
    output_index: usize,

    /// Complete JSON-encoded arguments text.
    arguments: String,
}

impl SseRenderer {
    /// Creates a renderer for one streamed response.
    pub(crate) fn new(model: String) -> Self {
        Self {
            sequence: FIRST_SEQUENCE_NUMBER,
            projection: None,
            id: format!("resp_{}", ids::new_uuid()),
            model,
            created_at: unix_time::unix_now(),
            is_created: false,
            reasoning: None,
            message: None,
            prefix_items: Vec::new(),
            prefix_boundary: None,
            completed_artifact: None,
            is_prefix_prepared: false,
            calls: Vec::new(),
            completed_items: Vec::new(),
            next_output_index: 0,
            usage: None,
            finish: None,
        }
    }

    /// Schedules the given vendor-shaped compaction items to render ahead of
    /// every IR-derived item: each becomes an `output_item.added` and
    /// `output_item.done` pair at its own output position once the response
    /// has opened, and the terminal response object's output array opens with
    /// them in the same order — the checkpoint-before-answer ordering the
    /// automatic and trigger compaction flows require (README §6.8).
    pub(crate) fn with_prefix_items(
        mut self,
        items: Vec<crate::format::compaction_wire::Checkpoint>,
    ) -> Self {
        self.next_output_index = items.len();

        self.prefix_items = items.into_iter().enumerate().collect();

        self
    }

    /// Drains the scheduled prefix items, emitting each one's added/done frame
    /// pair and recording it as a completed item for the terminal response
    /// object.
    fn take_prefix_frames(&mut self, preceding_frames: usize) -> Vec<String> {
        let items = std::mem::take(&mut self.prefix_items);

        let mut frames = Vec::with_capacity(items.len() * 2);

        for (output_index, item) in items {
            if !self.is_prefix_prepared
                && let Some(projection) = &mut self.projection
            {
                projection.observe_state();
            }
            let rendered = OutputItem::State(item);

            frames.push(self.event_frame(
                "response.output_item.added",
                OutputItemEvent {
                    output_index,
                    item: rendered.clone(),
                },
            ));

            frames.push(self.event_frame(
                "response.output_item.done",
                OutputItemEvent {
                    output_index,
                    item: rendered.clone(),
                },
            ));

            self.completed_items.push(TrackedItem {
                output_index,
                item: rendered,
            });
        }

        if !frames.is_empty() {
            self.prefix_boundary = Some(preceding_frames + frames.len() - 1);
        }
        frames
    }

    /// Closes any open reasoning or message items, emitting their done event sequences and
    /// recording the completed items.
    fn close_open_items(&mut self) -> Vec<String> {
        let mut frames = Vec::new();

        if let Some(open) = self.reasoning.take() {
            let summary_part = SummaryPart::SummaryText {
                text: open.text.clone(),
            };

            let completed_item = OutputItem::Rendered(RenderedOutputItem::Reasoning {
                id: open.item_id.clone(),
                summary: vec![summary_part.clone()],
            });

            frames.push(self.event_frame(
                "response.reasoning_summary_text.done",
                ReasoningSummaryTextDone {
                    item_id: open.item_id.clone(),
                    output_index: open.output_index,
                    summary_index: PRIMARY_INDEX,
                    text: open.text.clone(),
                },
            ));

            frames.push(self.event_frame(
                "response.reasoning_summary_part.done",
                ReasoningSummaryPartEvent {
                    item_id: open.item_id.clone(),
                    output_index: open.output_index,
                    summary_index: PRIMARY_INDEX,
                    part: summary_part,
                },
            ));

            frames.push(self.event_frame(
                "response.output_item.done",
                OutputItemEvent {
                    output_index: open.output_index,
                    item: completed_item.clone(),
                },
            ));

            self.completed_items.push(TrackedItem {
                output_index: open.output_index,
                item: completed_item,
            });
        }

        if let Some(open) = self.message.take() {
            let content_part = OutputTextPart::OutputText {
                text: open.text.clone(),
                annotations: [],
                logprobs: [],
            };

            let completed_item = OutputItem::Rendered(RenderedOutputItem::Message {
                id: open.item_id.clone(),
                role: "assistant",
                status: ItemStatus::Completed,
                content: vec![content_part.clone()],
            });

            frames.push(self.event_frame(
                "response.output_text.done",
                OutputTextDone {
                    item_id: open.item_id.clone(),
                    output_index: open.output_index,
                    content_index: PRIMARY_INDEX,
                    text: open.text.clone(),
                    logprobs: [],
                },
            ));

            frames.push(self.event_frame(
                "response.content_part.done",
                ContentPartEvent {
                    item_id: open.item_id.clone(),
                    output_index: open.output_index,
                    content_index: PRIMARY_INDEX,
                    part: content_part,
                },
            ));

            frames.push(self.event_frame(
                "response.output_item.done",
                OutputItemEvent {
                    output_index: open.output_index,
                    item: completed_item.clone(),
                },
            ));

            self.completed_items.push(TrackedItem {
                output_index: open.output_index,
                item: completed_item,
            });
        }

        frames
    }

    /// Builds the response object shell shared by all events.
    fn response_shell(&self, status: ResponseStatus, output: Vec<OutputItem>) -> ResponseObject {
        ResponseObject {
            id: self.id.clone(),
            object: "response",
            created_at: self.created_at,
            status,
            model: self.model.clone(),
            output,
            parallel_tool_calls: false,
            tool_choice: "auto",
            tools: [],
            usage: None,
            incomplete_details: None,
            error: None,
        }
    }

    /// Formats one SSE frame with the given event name and typed payload, assigning the next
    /// sequence number.
    fn event_frame<T>(&mut self, event_name: &'static str, payload: T) -> String
    where
        T: Serialize,
    {
        let envelope = EventEnvelope {
            event_type: event_name,
            sequence_number: self.sequence,
            payload,
        };

        self.sequence += 1;

        sse::format_frame(Some(event_name), &format::render_json(&envelope))
    }
}

impl ClientSseRenderer for SseRenderer {
    fn take_completed_artifact(&mut self) -> Option<crate::format::CompletedArtifact> {
        self.completed_artifact.take()
    }

    fn take_prefix_boundary(&mut self) -> Option<usize> {
        self.prefix_boundary.take()
    }

    fn begin_checkpoint_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.begin_projection(projector);
        self.is_prefix_prepared = true;
    }

    fn begin_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.projection = Some(super::affinity::Output::new(projector));
    }

    fn take_projection(&mut self) -> Option<crate::affinity::fingerprint::Projector> {
        self.projection
            .take()
            .and_then(super::affinity::Output::finish)
    }

    /// Renders one IR event into zero or more SSE frames. Prefix items
    /// scheduled through [`SseRenderer::with_prefix_items`] render once the
    /// response has opened, ahead of the event's own frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        self.prefix_boundary = None;
        self.completed_artifact = None;
        // Prefix frames precede every IR-derived item, but never the
        // response's own opening: they drain only once `response.created` has
        // rendered (or, on a recovery path that never opens the response,
        // ride the first rendered event).
        let mut frames = if self.is_created {
            self.take_prefix_frames(0)
        } else {
            Vec::new()
        };

        let preceding_frames = frames.len();
        let mut event_frames = self.render_event(event);
        if let Some(artifact) = &mut self.completed_artifact {
            artifact.terminal_index += preceding_frames;
        }

        frames.append(&mut event_frames);

        frames
    }

    /// Renders a mid-stream error as a terminal `response.failed` event, classified once through
    /// the shared API error (README §6.7).
    ///
    /// Strict clients validate the embedded response object like any other, so it carries the full
    /// required shell rather than a bare id/status/error subset.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let mut shell = self.response_shell(ResponseStatus::Failed, Vec::new());

        shell.error = Some(crate::format::openai::error::render_body(error));

        self.event_frame("response.failed", ResponseEvent { response: shell })
    }
}

impl SseRenderer {
    /// Renders one IR event into zero or more SSE frames: the body of
    /// [`ClientSseRenderer::render`], which wraps it with the prefix-item
    /// frames.
    fn render_event(&mut self, event: &IrEvent) -> Vec<String> {
        match event {
            IrEvent::CompactionState { payload } => {
                // Native state closes the preceding ordinary item. A following text delta
                // therefore receives a new position after the checkpoint instead of extending
                // an earlier covered item.
                let mut frames = self.close_open_items();
                let anchors = self.projection.as_mut().and_then(|projection| {
                    projection.complete_state(self.next_output_index, payload)
                });
                // An upstream-observed native compaction item renders at its
                // arrival position: the added/done pair the prefix items use,
                // at the renderer's next output index, recorded for the
                // terminal response object.
                let output_index = self.next_output_index;

                self.next_output_index += 1;

                let rendered = OutputItem::State(payload.clone().into());

                frames.extend(vec![
                    self.event_frame(
                        "response.output_item.added",
                        OutputItemEvent {
                            output_index,
                            item: rendered.clone(),
                        },
                    ),
                    self.event_frame(
                        "response.output_item.done",
                        OutputItemEvent {
                            output_index,
                            item: rendered.clone(),
                        },
                    ),
                ]);

                self.completed_items.push(TrackedItem {
                    output_index,
                    item: rendered,
                });

                if let Some(anchors) = anchors {
                    // Every complete state renders its added/done pair in this batch.
                    self.completed_artifact = Some(crate::format::CompletedArtifact {
                        terminal_index: frames.len() - 1,
                        anchors,
                    });
                }
                frames
            }

            IrEvent::MessageStart { .. } => {
                if self.is_created {
                    return Vec::new();
                }

                self.is_created = true;

                let shell = self.response_shell(ResponseStatus::InProgress, Vec::new());

                let mut frames = vec![
                    self.event_frame(
                        "response.created",
                        ResponseEvent {
                            response: shell.clone(),
                        },
                    ),
                    self.event_frame("response.in_progress", ResponseEvent { response: shell }),
                ];

                frames.extend(self.take_prefix_frames(frames.len()));

                frames
            }

            IrEvent::ReasoningDelta(delta) => {
                let (mut frames, item_id, output_index) = match &self.reasoning {
                    Some(open) => (Vec::new(), open.item_id.clone(), open.output_index),

                    None => {
                        let item_id = format!("rs_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;
                        self.reasoning = Some(OpenTextItem {
                            item_id: item_id.clone(),
                            output_index,
                            text: String::new(),
                        });

                        let frames = vec![
                            self.event_frame(
                                "response.output_item.added",
                                OutputItemEvent {
                                    output_index,
                                    item: OutputItem::Rendered(RenderedOutputItem::Reasoning {
                                        id: item_id.clone(),
                                        summary: Vec::new(),
                                    }),
                                },
                            ),
                            self.event_frame(
                                "response.reasoning_summary_part.added",
                                ReasoningSummaryPartEvent {
                                    item_id: item_id.clone(),
                                    output_index,
                                    summary_index: PRIMARY_INDEX,
                                    part: SummaryPart::SummaryText {
                                        text: String::new(),
                                    },
                                },
                            ),
                        ];

                        (frames, item_id, output_index)
                    }
                };

                if let Some(open) = &mut self.reasoning {
                    open.text.push_str(delta);
                }

                frames.push(self.event_frame(
                    "response.reasoning_summary_text.delta",
                    ReasoningSummaryTextDelta {
                        item_id,
                        output_index,
                        summary_index: PRIMARY_INDEX,
                        delta: delta.clone(),
                    },
                ));

                frames
            }

            // A done carrying text opens its block when no deltas preceded it; an empty-text done
            // only marks the block the deltas already delivered. The opaque payload is dropped on
            // OpenAI surfaces (README §6.2); block closure happens on Finish via
            // `close_open_items`.
            IrEvent::ReasoningDone { text, .. } => {
                if text.is_empty() {
                    Vec::new()
                } else {
                    self.render(&IrEvent::ReasoningDelta(text.clone()))
                }
            }

            IrEvent::TextDelta(delta) => {
                let (mut frames, item_id, output_index) = match &self.message {
                    Some(open) => (Vec::new(), open.item_id.clone(), open.output_index),

                    None => {
                        let item_id = format!("msg_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;
                        self.message = Some(OpenTextItem {
                            item_id: item_id.clone(),
                            output_index,
                            text: String::new(),
                        });

                        let frames = vec![
                            self.event_frame(
                                "response.output_item.added",
                                OutputItemEvent {
                                    output_index,
                                    item: OutputItem::Rendered(RenderedOutputItem::Message {
                                        id: item_id.clone(),
                                        role: "assistant",
                                        status: ItemStatus::InProgress,
                                        content: Vec::new(),
                                    }),
                                },
                            ),
                            self.event_frame(
                                "response.content_part.added",
                                ContentPartEvent {
                                    item_id: item_id.clone(),
                                    output_index,
                                    content_index: PRIMARY_INDEX,
                                    part: OutputTextPart::OutputText {
                                        text: String::new(),
                                        annotations: [],
                                        logprobs: [],
                                    },
                                },
                            ),
                        ];

                        (frames, item_id, output_index)
                    }
                };

                if let Some(projection) = &mut self.projection {
                    projection.text(output_index, delta.as_bytes());
                }

                if let Some(open) = &mut self.message {
                    open.text.push_str(delta);
                }

                frames.push(self.event_frame(
                    "response.output_text.delta",
                    OutputTextDelta {
                        item_id,
                        output_index,
                        content_index: PRIMARY_INDEX,
                        delta: delta.clone(),
                        // `logprobs` is a required field of the official event shape (openai-python
                        // `ResponseTextDeltaEvent`), so the gateway always carries it, empty when
                        // untracked.
                        logprobs: [],
                    },
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
                    Some((_, open)) => (open.item.id.clone(), open.output_index),

                    None => {
                        let item_id = format!("fc_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;

                        // A delta may open the item without revealing its identity: a bare
                        // `response.function_call_arguments.delta` with no preceding
                        // `output_item.added`, or a Chat Completions delta whose first tool-call
                        // entry carries only arguments, arrives exactly so. Every identity field of
                        // the official function_call item is a required string, so unknown identity
                        // renders as empty strings — the same defaults the done path's recovery
                        // branch opens with — until the done event stamps the real `call_id`,
                        // `name`, and `arguments`.
                        let item = FunctionCallItem {
                            id: item_id.clone(),
                            status: ItemStatus::InProgress,
                            call_id: call_id.clone().unwrap_or_default(),
                            name: name.clone().unwrap_or_default(),
                            arguments: String::new(),
                            thought_signature: None,
                        };

                        frames.push(self.event_frame(
                            "response.output_item.added",
                            OutputItemEvent {
                                output_index,
                                item: OutputItem::Rendered(RenderedOutputItem::FunctionCall(
                                    item.clone(),
                                )),
                            },
                        ));

                        self.calls
                            .push((*index, TrackedCall { output_index, item }));

                        (item_id, output_index)
                    }
                };

                if let Some(projection) = &mut self.projection {
                    projection.reserve_call(output_index);
                }

                if !arguments_delta.is_empty() {
                    frames.push(self.event_frame(
                        "response.function_call_arguments.delta",
                        FunctionCallArgumentsDelta {
                            item_id,
                            output_index,
                            delta: arguments_delta.clone(),
                        },
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
                        // A third-party upstream may deliver `output_item.done` for a call it never
                        // revealed with `output_item.added` or argument deltas — the recovery class
                        // of README §6.3. The done payload carries the complete call, so the item
                        // is opened here exactly as a live delta would have opened it; dropping it
                        // would lose the call only on the streaming path, while aggregation keeps
                        // it.
                        let item_id = format!("fc_{}", ids::new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;

                        let frames = vec![self.event_frame(
                            "response.output_item.added",
                            OutputItemEvent {
                                output_index,
                                item: OutputItem::Rendered(RenderedOutputItem::FunctionCall(
                                    FunctionCallItem {
                                        id: item_id.clone(),
                                        status: ItemStatus::InProgress,
                                        call_id: call_id.clone(),
                                        name: name.clone(),
                                        arguments: String::new(),
                                        thought_signature: None,
                                    },
                                )),
                            },
                        )];

                        (
                            TrackedCall {
                                output_index,
                                item: FunctionCallItem {
                                    id: item_id,
                                    status: ItemStatus::InProgress,
                                    call_id: String::new(),
                                    name: String::new(),
                                    arguments: String::new(),
                                    thought_signature: None,
                                },
                            },
                            frames,
                        )
                    }
                };

                open.item.status = ItemStatus::Completed;
                open.item.call_id = call_id.clone();
                open.item.name = name.clone();
                open.item.arguments = arguments.clone();
                open.item.thought_signature = thought_signature.clone();

                let item_id = open.item.id.clone();
                let output_index = open.output_index;

                frames.push(self.event_frame(
                    "response.function_call_arguments.done",
                    FunctionCallArgumentsDone {
                        item_id,
                        output_index,
                        arguments: arguments.clone(),
                    },
                ));

                if let Some(projection) = &mut self.projection {
                    projection.complete_call(output_index, &open.item);
                }

                let completed_item =
                    OutputItem::Rendered(RenderedOutputItem::FunctionCall(open.item));

                frames.push(self.event_frame(
                    "response.output_item.done",
                    OutputItemEvent {
                        output_index,
                        item: completed_item.clone(),
                    },
                ));

                self.completed_items.push(TrackedItem {
                    output_index,
                    item: completed_item,
                });

                frames
            }

            IrEvent::Finish { reason, .. } => {
                self.finish = Some(*reason);

                self.close_open_items()
            }

            IrEvent::CompactionMetadata { .. } => Vec::new(),
            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = self.close_open_items();

                let mut completed_items = mem::take(&mut self.completed_items);

                completed_items.sort_by_key(|tracked| tracked.output_index);

                let output = completed_items
                    .into_iter()
                    .map(|tracked| tracked.item)
                    .collect::<Vec<_>>();

                let usage = self.usage.unwrap_or_default();

                // A Length/ContentFilter finish must surface as `response.incomplete` with
                // `incomplete_details.reason`, so strict clients can tell a truncated response from
                // a full one.
                let incomplete_reason = self.finish.and_then(incomplete_details_reason);

                let terminal_event = if incomplete_reason.is_some() {
                    "response.incomplete"
                } else {
                    "response.completed"
                };

                let mut shell = self.response_shell(
                    if incomplete_reason.is_some() {
                        ResponseStatus::Incomplete
                    } else {
                        ResponseStatus::Completed
                    },
                    output,
                );

                shell.usage = Some(ResponseUsage::render(usage));

                shell.incomplete_details =
                    incomplete_reason.map(|reason| IncompleteDetails { reason });

                frames.push(self.event_frame(terminal_event, ResponseEvent { response: shell }));

                frames
            }
        }
    }
}

/// Renders an aggregated completion as a non-streaming Responses API response.
/// Upstream-observed native compaction items ride the output head, in their
/// arrival order, ahead of the derived items — a native compaction precedes
/// the answer output it precedes upstream.
pub(super) fn render_json_response(model: &str, aggregation: &IrAggregation) -> ResponseObject {
    let mut output: Vec<OutputItem> = aggregation
        .compaction_states
        .iter()
        .map(|item| OutputItem::State(item.clone().into()))
        .collect();

    if !aggregation.reasoning.is_empty() {
        output.push(OutputItem::Rendered(RenderedOutputItem::Reasoning {
            id: format!("rs_{}", ids::new_uuid()),
            summary: vec![SummaryPart::SummaryText {
                text: aggregation.reasoning.clone(),
            }],
        }));
    }

    if !aggregation.text.is_empty() {
        output.push(OutputItem::Rendered(RenderedOutputItem::Message {
            id: format!("msg_{}", ids::new_uuid()),
            role: "assistant",
            status: ItemStatus::Completed,
            content: vec![OutputTextPart::OutputText {
                text: aggregation.text.clone(),
                annotations: [],
                logprobs: [],
            }],
        }));
    }

    for tool_call in &aggregation.tool_calls {
        output.push(OutputItem::Rendered(RenderedOutputItem::FunctionCall(
            FunctionCallItem {
                id: format!("fc_{}", ids::new_uuid()),
                status: ItemStatus::Completed,
                call_id: tool_call.call_id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
                thought_signature: tool_call.thought_signature.clone(),
            },
        )));
    }

    let incomplete_reason = aggregation.finish.and_then(incomplete_details_reason);

    ResponseObject {
        id: format!("resp_{}", ids::new_uuid()),
        object: "response",
        created_at: unix_time::unix_now(),
        status: if incomplete_reason.is_some() {
            ResponseStatus::Incomplete
        } else {
            ResponseStatus::Completed
        },
        model: model.to_string(),
        output,
        parallel_tool_calls: false,
        tool_choice: "auto",
        tools: [],
        usage: Some(ResponseUsage::render(aggregation.usage.unwrap_or_default())),
        incomplete_details: incomplete_reason.map(|reason| IncompleteDetails { reason }),
        error: None,
    }
}

/// Renders an aggregated completion whose output opens with the given
/// vendor-shaped compaction items: the checkpoint-before-answer ordering of
/// the automatic and trigger compaction flows (README §6.8), rendered as
/// the same item values the streaming path's prefix frames carry.
pub(super) fn render_json_response_with_prefix_items(
    model: &str,
    aggregation: &IrAggregation,
    prefix_items: Vec<crate::format::compaction_wire::Checkpoint>,
) -> ResponseObject {
    let mut response = render_json_response(model, aggregation);

    let mut output: Vec<OutputItem> = prefix_items.into_iter().map(OutputItem::State).collect();

    output.append(&mut response.output);

    response.output = output;

    response
}

/// Renders the `incomplete_details.reason` string for a finish reason that marks an incomplete
/// response; [`None`] for complete responses.
fn incomplete_details_reason(reason: IrFinishReason) -> Option<&'static str> {
    match reason {
        IrFinishReason::Length => Some("max_output_tokens"),

        IrFinishReason::ContentFilter => Some("content_filter"),

        IrFinishReason::Stop | IrFinishReason::ToolCall => None,
    }
}

/// Projects actual ordered output items, excluding unencrypted reasoning dropped on replay.
pub(super) fn project_json_response(
    response: &ResponseObject,
    projector: &mut crate::affinity::fingerprint::Projector,
) -> bool {
    project_json_after_prefix(response, projector, 0)
}

/// Projects actual answer fields after the already normalized prepared checkpoint prefix.
pub(super) fn project_json_after_prefix(
    response: &ResponseObject,
    projector: &mut crate::affinity::fingerprint::Projector,
    prefix_count: usize,
) -> bool {
    for item in response.output.iter().skip(prefix_count) {
        match item {
            OutputItem::Rendered(RenderedOutputItem::Message { content, .. }) => {
                for OutputTextPart::OutputText { text, .. } in content {
                    projector.text(
                        crate::affinity::fingerprint::Role::Assistant,
                        text.as_bytes(),
                    );
                }
            }
            OutputItem::Rendered(RenderedOutputItem::FunctionCall(call)) => {
                if !crate::format::affinity::project_call(
                    projector,
                    &call.call_id,
                    &call.name,
                    &call.arguments,
                    call.thought_signature.as_deref(),
                ) {
                    return false;
                }
            }
            OutputItem::Rendered(RenderedOutputItem::Reasoning { .. }) => {}
            OutputItem::State(crate::format::compaction_wire::Checkpoint::Native(value)) => {
                let Some(state) = super::window_affinity::prepare_native_state(value, projector)
                else {
                    return false;
                };
                if super::compaction::native_artifact_anchor(value, projector).is_some() {
                    *projector = crate::affinity::fingerprint::Projector::new();
                }
                projector.state_prepared(state);
            }
            OutputItem::State(_) => return false,
        }
    }
    true
}

/// Identifies actual complete raw native checkpoints without treating reflected markers as ours.
pub(super) fn completed_json_artifacts(response: &ResponseObject) -> Vec<crate::affinity::Anchor> {
    let projector = crate::affinity::fingerprint::Projector::new();
    response
        .output
        .iter()
        .flat_map(|item| match item {
            OutputItem::State(crate::format::compaction_wire::Checkpoint::Native(value)) => {
                super::window_affinity::prepare_native_checkpoint(value, &projector)
                    .map_or_else(Vec::new, |(_, anchors)| anchors)
            }
            OutputItem::Rendered(_) | OutputItem::State(_) => Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SseRenderer, render_json_response, render_json_response_with_prefix_items};
    use crate::{
        format::{ClientSseRenderer as _, aggregated_body_json},
        ir::{IrAggregation, IrEvent, IrUsage},
    };
    use serde_json::Value;

    /// Parses the JSON payload of the last rendered SSE frame.
    ///
    /// # Panics
    ///
    /// Panics if no frame is rendered or its payload is not JSON.
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

    /// Parses the JSON payload of the first rendered frame carrying `event_name`.
    ///
    /// # Panics
    ///
    /// Panics if no frame carries `event_name` or its payload is not JSON.
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

    /// A tool-call delta opening a never-announced item — the recovery shape a bare
    /// `response.function_call_arguments.delta` upstream event or a Chat Completions delta carrying
    /// only arguments renders — emits an `output_item.added` function_call whose `call_id` and
    /// `name` are strings, empty when unknown: openai-python's `ResponseFunctionToolCallItem`
    /// requires both as strings with no defaults, so a `null` or missing field would fail a strict
    /// typed client on the transient item before the done event repairs it.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the documented shape.
    #[test]
    fn opening_tool_call_delta_renders_required_identity_strings() {
        let mut renderer = SseRenderer::new("m".to_string());

        let frames = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: None,
            name: None,
            arguments_delta: "{\"".to_string(),
        });

        #[expect(
            clippy::expect_used,
            reason = "the added frame is under test; a missing item is the failure"
        )]
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

    /// The streaming terminal event renders the full official usage shape: openai-python's
    /// `ResponseUsage` requires all five fields including both detail objects, so upstream-reported
    /// breakdowns ride through intact.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the documented usage shape.
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

        #[expect(
            clippy::expect_used,
            reason = "the terminal response is under test; missing usage is the failure"
        )]
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

    /// Breakdowns absent from the upstream — every cross-format upstream, and any Responses
    /// upstream that reports totals only — render zero-filled, because strict SDK consumers require
    /// the detail fields to be present.
    ///
    /// # Panics
    ///
    /// Panics if the rendered usage diverges from the documented shape.
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

        let body = aggregated_body_json(&render_json_response("m", &aggregation));

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

    /// Prefix items render once the response has opened and ahead of every
    /// IR-derived item — the checkpoint-before-answer ordering (README
    /// §4.3) — each as an added/done pair at its own output position, and the
    /// terminal response object's output array opens with them in order.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the ordering contract.
    #[test]
    fn prefix_items_render_ahead_of_the_ir_derived_items() {
        let checkpoint = serde_json::json!({
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "state",
        });

        let mut renderer = SseRenderer::new("m".to_string()).with_prefix_items(vec![
            checkpoint.clone().into(),
            serde_json::json!({"type": "reasoning", "encrypted_content": "cipher"}).into(),
        ]);

        let mut frames = renderer.render(&IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        });

        frames.extend(renderer.render(&IrEvent::TextDelta("answer".to_string())));

        frames.extend(renderer.render(&IrEvent::Done));

        // The first two frames open the response; the four following are the
        // two prefix items' added/done pairs; only then does the answer item
        // open.
        let event_names: Vec<&str> = frames
            .iter()
            .filter_map(|frame| frame.split_once("\ndata: ").map(|(head, _)| head))
            .filter_map(|head| head.strip_prefix("event: "))
            .collect();

        assert_eq!(
            event_names,
            vec![
                "response.created",
                "response.in_progress",
                "response.output_item.added",
                "response.output_item.done",
                "response.output_item.added",
                "response.output_item.done",
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
                "response.completed",
            ],
        );

        // The prefix items open at output indices 0 and 1, and the derived
        // message follows at index 2.
        let checkpoint_index = frame_payload(&frames, "response.output_item.added")
            .get("output_index")
            .and_then(Value::as_u64);

        assert_eq!(checkpoint_index, Some(0));

        let terminal = terminal_frame_payload(&frames);

        #[expect(
            clippy::expect_used,
            reason = "the terminal response is under test; a missing output is the failure"
        )]
        let output = terminal
            .pointer("/response/output")
            .and_then(Value::as_array)
            .expect("the terminal response carries its output");

        assert_eq!(output.first(), Some(&checkpoint));

        assert_eq!(output.len(), 3);
    }

    /// The aggregated rendering with prefix items opens the output array with
    /// exactly the same item values the streaming path renders, so both
    /// representations of one lifecycle agree (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the aggregated output diverges from the streaming output.
    #[test]
    fn the_aggregated_rendering_opens_with_the_prefix_items() {
        let prefix = vec![serde_json::json!({
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "state",
        })];

        let aggregation = IrAggregation {
            text: "answer".to_string(),
            usage: Some(IrUsage {
                input_tokens: 5,
                output_tokens: 2,
                ..IrUsage::default()
            }),
            ..IrAggregation::default()
        };

        let body = aggregated_body_json(&render_json_response_with_prefix_items(
            "m",
            &aggregation,
            prefix.clone().into_iter().map(Into::into).collect(),
        ));

        #[expect(
            clippy::expect_used,
            reason = "the aggregated body is under test; a missing output is the failure"
        )]
        let output = body
            .get("output")
            .and_then(Value::as_array)
            .expect("the body carries its output");

        #[expect(
            clippy::indexing_slicing,
            reason = "the one-element fixture prefix always carries index 0"
        )]
        {
            assert_eq!(output.first(), Some(&prefix[0]));
        }

        assert_eq!(output.len(), 2);
    }
}
