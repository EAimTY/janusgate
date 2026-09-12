//! Protocol codecs connecting client and upstream wire formats through the IR.
//!
//! Two contracts are declared once here. Every outward format implements
//! [`ClientFormat`], which parses client requests and boxes a
//! [`ClientSseRenderer`] for live streaming (aggregated rendering is a free
//! function behind the same trait). Every upstream format implements
//! [`UpstreamEventParser`]. Upstream payload rendering has no shared
//! contract — the payloads are provider-coupled — so each codec exposes
//! free `render_request` functions that the gateway invokes directly. The
//! Gemini codec is upstream-only (README §6.3): it implements
//! [`UpstreamEventParser`] plus its request renderers, but no outward
//! client format.

use crate::{
    ir::{InvalidRequest, IrAggregation, IrEvent, IrFinishReason, IrRequest},
    sse::SseFrame,
};

pub(crate) mod gemini;
pub(crate) mod openai_chat_completion;
pub(crate) mod openai_response;

/// Wire formats an upstream provider may speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpstreamFormat {
    /// OpenAI Responses protocol.
    OpenAiResponse,

    /// OpenAI Chat Completions protocol.
    OpenAiChatCompletion,

    /// Gemini GenerateContent protocol.
    GeminiGenerateContent,
}

/// An upstream payload the codec could not parse: malformed JSON, a missing
/// required field, or an upstream-reported failure event. The wire failure
/// wording ("Upstream request failed: ...") is composed by the consuming
/// surface; this type carries only the cause detail.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct PayloadError {
    /// Human-readable failure detail.
    pub(crate) message: String,
}

impl PayloadError {
    /// Builds a payload parse failure from its detail.
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A client request parsed into IR with its endpoint options.
#[derive(Debug)]
pub(crate) struct ParsedRequest {
    /// Format-independent request.
    pub(crate) ir: IrRequest,

    /// Whether the client requested a streamed response.
    pub(crate) is_streaming: bool,

    /// Whether the client asked for a trailing usage-only chunk; only the
    /// Chat Completions format exposes the option, every other format
    /// includes usage unconditionally.
    pub(crate) should_include_usage: bool,
}

/// Format-independent upstream parsing contract implemented by every codec's
/// upstream parser.
pub(crate) trait UpstreamEventParser {
    /// Ingests one upstream SSE frame.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed upstream payloads.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError>;

    /// Parses one complete upstream JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed upstream payloads.
    fn parse_json_response(
        &mut self,
        value: serde_json::Value,
    ) -> Result<Vec<IrEvent>, PayloadError>;

    /// Ends a cleanly terminated upstream stream, flushing state buffered
    /// across frames into final IR events.
    ///
    /// Called exactly once when the upstream body reached a clean end of
    /// stream, after the last frame was ingested and before the terminal
    /// [`IrEvent::Done`]. It is never called once an error aborted the
    /// stream, so an aborted stream publishes no further parser state.
    /// Parsers that buffer no completable cross-frame state keep the default.
    fn finish(&mut self) -> Vec<IrEvent> {
        Vec::new()
    }
}

/// Client-side SSE rendering contract implemented by every outward codec.
pub(crate) trait ClientSseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String>;

    /// Renders a mid-stream error as a terminal frame. `error` is the
    /// surface's rendered OpenAI error envelope (`{"error": {...}}`).
    fn error_frame(&mut self, error: &serde_json::Value) -> String;
}

/// An outward client format: parses client requests into IR and renders IR
/// events back in the client's wire shape, live or aggregated.
pub(crate) trait ClientFormat {
    /// Parses one client request body.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] for malformed or unsupported requests.
    fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest>
    where
        Self: Sized;

    /// Boxes the streaming renderer for one parsed request.
    /// `should_include_usage` is the trailing usage-chunk option; formats
    /// that always include usage ignore it.
    fn sse_renderer(
        should_include_usage: bool,
        public_name: String,
    ) -> Box<dyn ClientSseRenderer + Send>
    where
        Self: Sized;

    /// Renders an aggregated completion as a non-streaming response body.
    fn render_json_response(public_name: &str, aggregation: &IrAggregation) -> serde_json::Value;
}

impl UpstreamFormat {
    /// Parses the canonical wire-format name stored in the provider
    /// `protocol` column.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_response" => Some(Self::OpenAiResponse),
            "openai_chat_completion" => Some(Self::OpenAiChatCompletion),
            "gemini_generate_content" => Some(Self::GeminiGenerateContent),
            _ => None,
        }
    }

    /// Renders the canonical wire-format name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponse => "openai_response",
            Self::OpenAiChatCompletion => "openai_chat_completion",
            Self::GeminiGenerateContent => "gemini_generate_content",
        }
    }
}

/// Maps an OpenAI `finish_reason` string to the canonical reason; unknown
/// values degrade to [`IrFinishReason::Stop`] rather than failing a healthy
/// stream.
pub(crate) fn finish_reason_from_openai(value: &str) -> IrFinishReason {
    match value {
        "length" => IrFinishReason::Length,
        "tool_calls" | "function_call" => IrFinishReason::ToolCall,
        "content_filter" => IrFinishReason::ContentFilter,
        _ => IrFinishReason::Stop,
    }
}

/// Renders the canonical reason as an OpenAI `finish_reason` string.
pub(crate) fn finish_reason_to_openai(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop => "stop",
        IrFinishReason::Length => "length",
        IrFinishReason::ToolCall => "tool_calls",
        IrFinishReason::ContentFilter => "content_filter",
    }
}

/// Looks up the allowed-field table entry whose tag name equals `tag`.
///
/// Internally tagged serde enums cannot reject unknown fields themselves, so
/// request parsers keep an allowlist per variant and match it against the raw
/// JSON; variants absent from the table are left to serde to reject.
pub(crate) fn allowed_fields<'a>(
    table: &'a [(&'a str, &'a [&'a str])],
    tag: &str,
) -> Option<&'a [&'a str]> {
    table
        .iter()
        .find(|(name, _)| *name == tag)
        .map(|(_, fields)| *fields)
}

/// Rejects every field of `object` absent from `allowed`, naming the first
/// offender against `context`.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first unknown field.
pub(crate) fn reject_unknown_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), InvalidRequest> {
    for field in object.keys() {
        if !allowed.contains(&field.as_str()) {
            return Err(InvalidRequest::field(
                field,
                format!("Unsupported field `{field}` in {context}"),
            ));
        }
    }

    Ok(())
}

/// Rejects unknown fields on tagged content-part objects, matching each
/// part's `type` against `table`.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first unknown field.
pub(crate) fn validate_content_part_fields(
    parts: &[serde_json::Value],
    table: &[(&str, &[&str])],
) -> Result<(), InvalidRequest> {
    for part in parts {
        let Some(object) = part.as_object() else {
            continue;
        };

        let part_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        let Some(allowed) = allowed_fields(table, part_type) else {
            continue;
        };

        reject_unknown_fields(object, allowed, &format!("`{part_type}` content part"))?;
    }

    Ok(())
}

/// Parses tool function arguments into a JSON object, treating an empty
/// argument string as an empty object.
///
/// # Errors
///
/// Returns an error when the arguments are not a JSON object.
pub(crate) fn arguments_as_object(
    arguments: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, InvalidRequest> {
    if arguments.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }

    let value: serde_json::Value = serde_json::from_str(arguments).map_err(|error| {
        InvalidRequest::field(
            "arguments",
            format!("Tool call arguments are not valid JSON: {error}"),
        )
    })?;

    match value {
        serde_json::Value::Object(map) => Ok(map),
        _ => Err(InvalidRequest::field(
            "arguments",
            "Tool call arguments must encode a JSON object",
        )),
    }
}

/// Inserts `value` under `key` into a JSON object literal.
///
/// Every caller passes a `json!` object literal, so a non-object base is an
/// internal invariant violation.
///
/// # Panics
///
/// Panics when the base value is not a JSON object, violating the stated
/// invariant.
pub(crate) fn json_insert(base: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    #[expect(
        clippy::expect_used,
        reason = "every base is a `json!` object literal created by the caller"
    )]
    let object = base
        .as_object_mut()
        .expect("JSON value is an object literal");

    object.insert(key.to_string(), value);
}
