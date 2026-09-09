//! Intermediate representation shared by every protocol translation.
//!
//! Client requests parse into an [`IrRequest`]; every upstream response —
//! streamed or buffered — parses into a sequence of [`IrEvent`] values; every
//! client response renders from IR.

use crate::error::ApiError;

/// Canonical thinking levels, ordered by increasing reasoning strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReasoningEffort {
    /// No reasoning (OpenAI `minimal`, Gemini budget `0`).
    Off,

    /// Light reasoning.
    Low,

    /// Moderate reasoning.
    Medium,

    /// Strong reasoning.
    High,
}

impl ReasoningEffort {
    /// Parses a client-supplied OpenAI effort string; `minimal` maps to
    /// [`ReasoningEffort::Off`].
    pub(crate) fn parse_openai(value: &str) -> Option<Self> {
        match value {
            "minimal" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// Renders the OpenAI reasoning effort string.
    pub(crate) fn to_openai(self) -> &'static str {
        match self {
            Self::Off => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Renders the Gemini thinking budget.
    pub(crate) fn to_thinking_budget(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Low => 1024,
            Self::Medium => 8192,
            Self::High => 24576,
        }
    }

    /// Parses the canonical level name used in `thinking_levels_override`.
    pub(crate) fn parse_canonical(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// Clamps this level into the allowed set: lowered to the nearest allowed
    /// level at or below it, or raised to the lowest allowed level when none
    /// is at or below it.
    ///
    /// # Panics
    ///
    /// Panics when `allowed` is empty, violating the non-empty contract.
    pub(crate) fn clamp_into(self, allowed: &[Self]) -> Self {
        let mut below: Option<Self> = None;
        let mut lowest: Option<Self> = None;

        for candidate in allowed {
            if *candidate <= self && below.is_none_or(|current| *candidate > current) {
                below = Some(*candidate);
            }

            if lowest.is_none_or(|current| *candidate < current) {
                lowest = Some(*candidate);
            }
        }

        // `allowed` is validated non-empty, so a lowest level always exists.
        #[expect(
            clippy::expect_used,
            reason = "allowed thinking levels are validated non-empty"
        )]
        below.unwrap_or_else(|| lowest.expect("allowed thinking levels are non-empty"))
    }
}

/// Reason a completion finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishReason {
    /// The model completed its output.
    Stop,

    /// The output hit a token limit.
    Length,

    /// The model requested tool execution.
    ToolCall,

    /// The output was cut off by a content filter.
    ContentFilter,
}

/// Role of a conversation participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IrRole {
    /// The human side of the conversation.
    User,

    /// The model side of the conversation.
    Assistant,

    /// A tool result delivered back to the model.
    Tool,
}

/// Image content accepted from clients and rendered to upstreams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IrImage {
    /// Base64-encoded inline image data with its MIME type.
    Inline {
        /// MIME type of the image data.
        mime_type: String,

        /// Base64-encoded image bytes.
        data_base64: String,
    },

    /// Remote image referenced by URL.
    Url(String),
}

/// One typed segment of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IrPart {
    /// Plain text content.
    Text {
        /// The text content.
        text: String,
    },

    /// Image content.
    Image {
        /// The image payload.
        image: IrImage,
    },

    /// An assistant tool invocation.
    ToolCall {
        /// Identifier of the invocation, round-tripped to the matching tool
        /// output.
        call_id: String,

        /// Name of the invoked function.
        name: String,

        /// JSON-encoded arguments object.
        arguments: String,

        /// Upstream thought signature that must be reattached across turns.
        thought_signature: Option<String>,
    },

    /// A tool result for a prior [`IrPart::ToolCall`].
    ToolOutput {
        /// Identifier of the invocation being answered.
        call_id: String,

        /// JSON or plain-text result content.
        content: String,
    },
}

/// One conversation message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IrMessage {
    /// Participating role.
    pub(crate) role: IrRole,

    /// Ordered typed segments.
    pub(crate) parts: Vec<IrPart>,
}

/// A function tool advertised to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IrTool {
    /// Function name the model calls.
    pub(crate) name: String,

    /// Human-readable description of the function.
    pub(crate) description: Option<String>,

    /// JSON Schema of the accepted arguments.
    pub(crate) parameters: Option<serde_json::Value>,
}

/// Restriction on which tool the model may call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolChoice {
    /// The model decides freely.
    Auto,

    /// The model may not call tools.
    None,

    /// The model must call a tool.
    Required,

    /// The model must call the named function.
    Function {
        /// Required function name.
        name: String,
    },
}

/// Response format restriction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IrResponseFormat {
    /// Free-form text (the implicit default).
    Text,

    /// Any valid JSON value.
    Json,

    /// JSON conforming to a named schema.
    JsonSchema {
        /// Schema name.
        name: String,

        /// JSON Schema of the response.
        schema: serde_json::Value,
    },
}

/// A fully format-independent completion request.
#[derive(Debug, Clone, Default)]
pub(crate) struct IrRequest {
    /// Upstream model name rendered verbatim into upstream payloads.
    pub(crate) model: String,

    /// Concatenated system instructions.
    pub(crate) instructions: Option<String>,

    /// Conversation messages in order.
    pub(crate) messages: Vec<IrMessage>,

    /// Available function tools.
    pub(crate) tools: Vec<IrTool>,

    /// Tool selection restriction.
    pub(crate) tool_choice: Option<ToolChoice>,

    /// Sampling temperature.
    pub(crate) temperature: Option<f64>,

    /// Nucleus sampling threshold.
    pub(crate) top_p: Option<f64>,

    /// Output token budget.
    pub(crate) max_output_tokens: Option<u64>,

    /// Response format restriction.
    pub(crate) response_format: Option<IrResponseFormat>,

    /// Canonical reasoning level requested by the client, before any
    /// `thinking_levels_override` filtering.
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
}

/// One event of a completion, in upstream-agnostic form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IrEvent {
    /// Identifies the completion; emitted exactly once, first.
    MessageStart {
        /// Upstream response identifier.
        id: String,

        /// Upstream model name.
        model: String,
    },

    /// Incremental assistant text.
    TextDelta(String),

    /// Incremental reasoning or summary text.
    ReasoningDelta(String),

    /// Incremental tool call assembly; `index` is stable per call.
    ToolCallDelta {
        /// Stable index of the call within the response.
        index: usize,

        /// Identifier of the invocation, when first revealed.
        call_id: Option<String>,

        /// Name of the invoked function, when first revealed.
        name: Option<String>,

        /// Incremental JSON-encoded arguments text.
        arguments_delta: String,
    },

    /// A complete tool call.
    ToolCallDone {
        /// Stable index of the call within the response.
        index: usize,

        /// Identifier of the invocation.
        call_id: String,

        /// Name of the invoked function.
        name: String,

        /// Complete JSON-encoded arguments text.
        arguments: String,

        /// Upstream thought signature that must be reattached across turns.
        thought_signature: Option<String>,
    },

    /// The completion finished.
    Finish(FinishReason),

    /// Token accounting.
    Usage {
        /// Tokens consumed by the input.
        input_tokens: u64,

        /// Tokens produced by the output.
        output_tokens: u64,
    },

    /// Terminal marker; always appended by the gateway after the upstream
    /// response is fully consumed, never by parsers.
    Done,
}

/// One completed tool call produced by aggregation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AggregatedToolCall {
    /// Stable index of the call within the response.
    pub(crate) index: usize,

    /// Identifier of the invocation.
    pub(crate) call_id: String,

    /// Name of the invoked function.
    pub(crate) name: String,

    /// Complete JSON-encoded arguments text.
    pub(crate) arguments: String,

    /// Upstream thought signature to reattach across turns.
    pub(crate) thought_signature: Option<String>,
}

/// The complete result of folding an IR event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Aggregation {
    /// Upstream response identifier from [`IrEvent::MessageStart`].
    pub(crate) id: Option<String>,

    /// Upstream model name from [`IrEvent::MessageStart`].
    pub(crate) model: Option<String>,

    /// All accumulated assistant text.
    pub(crate) text: String,

    /// All accumulated reasoning text.
    pub(crate) reasoning: String,

    /// Completed tool calls in emission order.
    pub(crate) tool_calls: Vec<AggregatedToolCall>,

    /// Reason the completion finished, when reported.
    pub(crate) finish: Option<FinishReason>,

    /// Token accounting, when reported.
    pub(crate) usage: Option<(u64, u64)>,
}

/// Folds IR events into an [`Aggregation`].
#[derive(Debug, Default)]
pub(crate) struct Aggregator {
    /// State under construction.
    aggregation: Aggregation,

    /// Arguments accumulated per in-flight tool call index.
    pending_arguments: Vec<(usize, String)>,
}

impl Aggregator {
    /// Ingests one event, updating the aggregation in place.
    ///
    /// # Panics
    ///
    /// Panics when a newly started tool call cannot be buffered, violating
    /// the push-then-read invariant.
    pub(crate) fn ingest(&mut self, event: &IrEvent) {
        match event {
            IrEvent::MessageStart { id, model } => {
                self.aggregation.id = Some(id.clone());
                self.aggregation.model = Some(model.clone());
            }

            IrEvent::TextDelta(delta) => self.aggregation.text.push_str(delta),

            IrEvent::ReasoningDelta(delta) => self.aggregation.reasoning.push_str(delta),

            IrEvent::ToolCallDelta {
                index,
                arguments_delta,
                ..
            } => {
                let pending = match self
                    .pending_arguments
                    .iter_mut()
                    .find(|(pending_index, _)| pending_index == index)
                {
                    Some(pending) => pending,
                    None => {
                        self.pending_arguments.push((*index, String::new()));

                        #[expect(clippy::expect_used, reason = "the entry was just pushed above")]
                        self.pending_arguments
                            .last_mut()
                            .expect("entry was just pushed")
                    }
                };

                pending.1.push_str(arguments_delta);
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                self.pending_arguments
                    .retain(|(pending_index, _)| pending_index != index);

                self.aggregation.tool_calls.push(AggregatedToolCall {
                    index: *index,
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    thought_signature: thought_signature.clone(),
                });
            }

            IrEvent::Finish(reason) => self.aggregation.finish = Some(*reason),

            IrEvent::Usage {
                input_tokens,
                output_tokens,
            } => self.aggregation.usage = Some((*input_tokens, *output_tokens)),

            IrEvent::Done => {}
        }
    }

    /// Consumes the accumulator, dropping any unfinished in-flight tool call
    /// state.
    pub(crate) fn finish(self) -> Aggregation {
        self.aggregation
    }
}

/// Filters a requested reasoning level against a model's configured
/// `thinking_levels_override`.
///
/// A `None` override is pure passthrough: the requested level (or absence)
/// flows through untouched. When an override is configured, a requested level
/// is clamped into the allowed set, and a missing request defaults to the
/// highest allowed level.
pub(crate) fn apply_thinking_levels(
    requested: Option<ReasoningEffort>,
    allowed_override: Option<&[ReasoningEffort]>,
) -> Option<ReasoningEffort> {
    match allowed_override {
        None => requested,

        Some(allowed) => {
            let highest = allowed.iter().max().copied();

            match requested {
                Some(requested) => Some(requested.clamp_into(allowed)),
                None => highest,
            }
        }
    }
}

/// Parses a `thinking_levels_override` JSON array into canonical levels,
/// rejecting empty arrays, unknown names, and duplicates.
pub(crate) fn parse_thinking_levels_override(json: &str) -> Result<Vec<ReasoningEffort>, ApiError> {
    let values: Vec<String> = serde_json::from_str(json)
        .map_err(|error| ApiError::invalid_request(format!("Invalid thinking levels: {error}")))?;

    let mut levels = Vec::with_capacity(values.len());

    for value in &values {
        let level = ReasoningEffort::parse_canonical(value).ok_or_else(|| {
            ApiError::invalid_request(format!(
                "Unknown thinking level `{value}`; expected one of off, low, medium, high"
            ))
        })?;

        if levels.contains(&level) {
            return Err(ApiError::invalid_request(format!(
                "Duplicate thinking level `{value}`"
            )));
        }

        levels.push(level);
    }

    if levels.is_empty() {
        return Err(ApiError::invalid_request(
            "Thinking levels must be a non-empty array",
        ));
    }

    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::{
        AggregatedToolCall, Aggregator, FinishReason, IrEvent, ReasoningEffort,
        apply_thinking_levels, parse_thinking_levels_override,
    };

    /// Folds a slice of events into a finished aggregation.
    fn aggregate(events: &[IrEvent]) -> super::Aggregation {
        let mut aggregator = Aggregator::default();

        for event in events {
            aggregator.ingest(event);
        }

        aggregator.finish()
    }

    /// Verifies that openai effort parsing round trips holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn openai_effort_parsing_round_trips() {
        assert_eq!(
            ReasoningEffort::parse_openai("minimal"),
            Some(ReasoningEffort::Off)
        );

        assert_eq!(
            ReasoningEffort::parse_openai("high"),
            Some(ReasoningEffort::High)
        );

        assert_eq!(ReasoningEffort::parse_openai("off"), None);

        assert_eq!(ReasoningEffort::Off.to_openai(), "minimal");
        assert_eq!(ReasoningEffort::High.to_openai(), "high");
    }

    /// Verifies that thinking budgets match documented levels holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn thinking_budgets_match_documented_levels() {
        assert_eq!(ReasoningEffort::Off.to_thinking_budget(), 0);
        assert_eq!(ReasoningEffort::Low.to_thinking_budget(), 1024);
        assert_eq!(ReasoningEffort::Medium.to_thinking_budget(), 8192);
        assert_eq!(ReasoningEffort::High.to_thinking_budget(), 24576);
    }

    /// Verifies that clamping follows nearest allowed level holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn clamping_follows_nearest_allowed_level() {
        let allowed = [ReasoningEffort::Off, ReasoningEffort::Medium];

        assert_eq!(
            ReasoningEffort::High.clamp_into(&allowed),
            ReasoningEffort::Medium
        );

        assert_eq!(
            ReasoningEffort::Low.clamp_into(&allowed),
            ReasoningEffort::Off
        );

        assert_eq!(
            ReasoningEffort::Off.clamp_into(&[ReasoningEffort::Medium, ReasoningEffort::High]),
            ReasoningEffort::Medium
        );
    }

    /// Verifies that thinking level filtering matches contract holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn thinking_level_filtering_matches_contract() {
        let allowed = [ReasoningEffort::Low, ReasoningEffort::High];

        assert_eq!(
            apply_thinking_levels(Some(ReasoningEffort::Medium), Some(&allowed)),
            Some(ReasoningEffort::Low)
        );

        assert_eq!(
            apply_thinking_levels(Some(ReasoningEffort::High), Some(&allowed)),
            Some(ReasoningEffort::High)
        );

        assert_eq!(
            apply_thinking_levels(None, Some(&allowed)),
            Some(ReasoningEffort::High)
        );

        assert_eq!(apply_thinking_levels(None, None), None);

        assert_eq!(
            apply_thinking_levels(Some(ReasoningEffort::Medium), None),
            Some(ReasoningEffort::Medium)
        );
    }

    /// Verifies that thinking level override parsing validates input holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn thinking_level_override_parsing_validates_input() {
        #[expect(clippy::expect_used, reason = "a parse failure fails the test")]
        let parsed = parse_thinking_levels_override(r#"["high","off"]"#).expect("must parse");

        assert_eq!(parsed, vec![ReasoningEffort::High, ReasoningEffort::Off]);

        assert!(parse_thinking_levels_override("[]").is_err());
        assert!(parse_thinking_levels_override(r#"["turbo"]"#).is_err());
        assert!(parse_thinking_levels_override(r#"["low","low"]"#).is_err());
        assert!(parse_thinking_levels_override("not json").is_err());
    }

    /// Verifies that aggregation folds all event kinds holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn aggregation_folds_all_event_kinds() {
        let aggregation = aggregate(&[
            IrEvent::MessageStart {
                id: "resp_1".to_string(),
                model: "gpt-5".to_string(),
            },
            IrEvent::ReasoningDelta("thinking".to_string()),
            IrEvent::TextDelta("hel".to_string()),
            IrEvent::TextDelta("lo".to_string()),
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call_a".to_string()),
                name: Some("weather".to_string()),
                arguments_delta: "{\"city\":".to_string(),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: None,
                name: None,
                arguments_delta: "\"oslo\"}".to_string(),
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_a".to_string(),
                name: "weather".to_string(),
                arguments: "{\"city\":\"oslo\"}".to_string(),
                thought_signature: Some("sig".to_string()),
            },
            IrEvent::Finish(FinishReason::ToolCall),
            IrEvent::Usage {
                input_tokens: 10,
                output_tokens: 5,
            },
            IrEvent::Done,
        ]);

        assert_eq!(aggregation.id.as_deref(), Some("resp_1"));
        assert_eq!(aggregation.model.as_deref(), Some("gpt-5"));
        assert_eq!(aggregation.reasoning, "thinking");
        assert_eq!(aggregation.text, "hello");
        assert_eq!(
            aggregation.tool_calls,
            vec![AggregatedToolCall {
                index: 0,
                call_id: "call_a".to_string(),
                name: "weather".to_string(),
                arguments: "{\"city\":\"oslo\"}".to_string(),
                thought_signature: Some("sig".to_string()),
            }]
        );
        assert_eq!(aggregation.finish, Some(FinishReason::ToolCall));
        assert_eq!(aggregation.usage, Some((10, 5)));
    }
}
