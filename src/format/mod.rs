//! Protocol codecs connecting client and upstream wire formats through the IR.
//!
//! Every format exposes the same structural roles: a client request parser,
//! an upstream payload renderer, an upstream event parser producing IR events,
//! and client response renderers (streaming SSE and aggregated JSON). The
//! contracts for these roles are declared once here and implemented by each
//! codec module, so consumers depend only on `crate::format`.

use crate::{
    error::ApiError,
    ir::{IrAggregation, IrEvent, IrFinishReason, IrRequest},
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

/// A client request parsed into IR with its endpoint options.
#[derive(Debug)]
pub(crate) struct ParsedRequest {
    /// Format-independent request.
    pub(crate) ir: IrRequest,

    /// Whether the client requested a streamed response.
    pub(crate) stream: bool,

    /// Whether the client asked for a trailing usage-only chunk; only the
    /// Chat Completions format exposes the option, every other format
    /// includes usage unconditionally.
    pub(crate) include_usage: bool,
}

/// Format-independent upstream parsing contract implemented by every codec's
/// upstream parser.
pub(crate) trait UpstreamEventParser {
    /// Ingests one upstream SSE frame.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed upstream payloads.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError>;

    /// Parses one complete upstream JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed upstream payloads.
    fn parse_json_response(&mut self, value: serde_json::Value) -> Result<Vec<IrEvent>, ApiError>;

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

    /// Renders a mid-stream error as a terminal frame.
    fn error_frame(&mut self, error: &ApiError) -> String;
}

/// An outward client format: parses client requests into IR and renders IR
/// events back in the client's wire shape, live or aggregated.
pub(crate) trait ClientFormat {
    /// Parses one client request body.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed or unsupported requests.
    fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError>
    where
        Self: Sized;

    /// Boxes the streaming renderer for one parsed request. `include_usage`
    /// is the trailing usage-chunk option; formats that always include usage
    /// ignore it.
    fn sse_renderer(include_usage: bool, public_name: String) -> Box<dyn ClientSseRenderer + Send>
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
/// Returns [`ApiError`] naming the first unknown field.
pub(crate) fn reject_unknown_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), ApiError> {
    for field in object.keys() {
        if !allowed.contains(&field.as_str()) {
            return Err(ApiError::invalid_parameter(
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
/// Returns [`ApiError`] naming the first unknown field.
pub(crate) fn validate_content_part_fields(
    parts: &[serde_json::Value],
    table: &[(&str, &[&str])],
) -> Result<(), ApiError> {
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
) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
    if arguments.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }

    let value: serde_json::Value = serde_json::from_str(arguments).map_err(|error| {
        ApiError::invalid_parameter(
            "arguments",
            format!("Tool call arguments are not valid JSON: {error}"),
        )
    })?;

    match value {
        serde_json::Value::Object(map) => Ok(map),
        _ => Err(ApiError::invalid_parameter(
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
