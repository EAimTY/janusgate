//! Protocol codecs connecting client and upstream wire formats through the IR.
//!
//! Two contracts are declared once here. Every outward format implements
//! [`ClientFormat`], which parses client requests and boxes a
//! [`ClientSseRenderer`] for live streaming (aggregated rendering is a free
//! function behind the same trait). Every upstream format implements
//! [`UpstreamEventParser`]. Upstream payload rendering has no shared
//! contract — the payloads are provider-coupled — so each codec exposes
//! free `render_request` functions that the gateway invokes directly; each
//! returns the upstream URL beside the payload, so every protocol's path
//! shape lives in the codec that owns it while the provider layer threads
//! the endpoint base in. The
//! Gemini codec is upstream-only (README §6.3): it implements
//! [`UpstreamEventParser`] plus its request renderers, but no outward
//! client format.

use crate::{
    ir::{InvalidRequest, IrAggregation, IrEvent, IrFinishReason, IrRequest, IrToolChoice},
    sse::SseFrame,
};
use serde_json::Value;

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
    message: String,
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
    /// that always include usage ignore it. `public_id` is the public
    /// routing ID (README §3) the client addressed.
    fn sse_renderer(
        should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send>
    where
        Self: Sized;

    /// Renders an aggregated completion as a non-streaming response body.
    /// `public_id` is the public routing ID (README §3) the client
    /// addressed.
    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> serde_json::Value;
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
pub(in crate::format) fn finish_reason_from_openai(value: &str) -> IrFinishReason {
    match value {
        "length" => IrFinishReason::Length,
        "tool_calls" | "function_call" => IrFinishReason::ToolCall,
        "content_filter" => IrFinishReason::ContentFilter,
        _ => IrFinishReason::Stop,
    }
}

/// Renders the canonical reason as an OpenAI `finish_reason` string.
pub(in crate::format) fn finish_reason_to_openai(reason: IrFinishReason) -> &'static str {
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
pub(in crate::format) fn allowed_fields<'a>(
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
pub(in crate::format) fn reject_unknown_fields(
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
pub(in crate::format) fn validate_content_part_fields(
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
pub(in crate::format) fn arguments_as_object(
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

/// Placeholder for a missing JSON object field, so read-only upstream parse
/// walks can borrow instead of deep-cloning the subtrees they inspect.
const MISSING: Value = Value::Null;

/// Parses the JSON payload of an upstream SSE frame; the single home of the
/// malformed-frame rejection shared by every codec's `ingest_frame`.
///
/// # Errors
///
/// Returns [`PayloadError`] when the frame data is not valid JSON.
pub(crate) fn parse_frame_json(frame: &SseFrame) -> Result<Value, PayloadError> {
    serde_json::from_str(&frame.data)
        .map_err(|_| PayloadError::new("malformed JSON in upstream SSE frame"))
}

/// Reads a string field of an upstream JSON object, defaulting to the empty
/// string.
pub(crate) fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Rejects an upstream body carrying a top-level error object; shared by the
/// OpenAI-family codecs, whose upstreams embed the error beside the protocol
/// fields.
///
/// An error body inside a 2xx response is a malformed upstream payload
/// (README §6.7), so the embedded `code` is never mirrored as the outward
/// status — a 2xx code would otherwise render a success status carrying an
/// error body. The embedded code and text ride in the message instead.
///
/// A present-but-null `error` is success, not failure: the non-streaming
/// Responses body is the Response object, whose shape carries
/// `"error": null` on every success, and Chat Completions success bodies
/// never carry the key at all.
///
/// # Errors
///
/// Returns [`PayloadError`] carrying the upstream error text and code.
pub(in crate::format) fn reject_upstream_error(value: &Value) -> Result<(), PayloadError> {
    let Some(error) = value.get("error").filter(|error| !error.is_null()) else {
        return Ok(());
    };

    let text = error
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| error.to_string());

    let detail = match error.get("code") {
        Some(code) => format!("{text} (upstream error code: {code})"),
        None => text,
    };

    Err(PayloadError::new(detail))
}

/// Parses an OpenAI-family `tool_choice` mode name; shared by both codecs.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` for unknown modes.
pub(in crate::format) fn parse_tool_choice_mode(
    mode: &str,
) -> Result<IrToolChoice, InvalidRequest> {
    match mode {
        "auto" => Ok(IrToolChoice::Auto),
        "none" => Ok(IrToolChoice::None),
        "required" => Ok(IrToolChoice::Required),
        other => Err(InvalidRequest::field(
            "tool_choice",
            format!("Unknown tool_choice `{other}`"),
        )),
    }
}

/// Rejects an OpenAI-family tool payload whose `type` tag is not `function`.
///
/// `context` names the payload's role and renders into the rejection message
/// (`tool` for advertised tools, `tool_choice` for forced functions).
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `type` for any non-function tag.
pub(in crate::format) fn reject_non_function_type(
    tool_type: &str,
    context: &str,
) -> Result<(), InvalidRequest> {
    if tool_type == "function" {
        return Ok(());
    }

    Err(InvalidRequest::field(
        "type",
        format!("Unsupported {context} type `{tool_type}`"),
    ))
}

/// Rejects a `tool_choice` present on a request that declared no tools.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice`.
pub(in crate::format) fn reject_tool_choice_without_tools(
    is_tools_empty: bool,
) -> Result<(), InvalidRequest> {
    if is_tools_empty {
        return Err(InvalidRequest::field(
            "tool_choice",
            "tool_choice requires at least one tool",
        ));
    }

    Ok(())
}

/// Validates the OpenAI-family sampling parameter ranges; shared by both
/// codecs.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the offending parameter.
pub(in crate::format) fn validate_sampling_ranges(
    temperature: Option<f64>,
    top_p: Option<f64>,
) -> Result<(), InvalidRequest> {
    if temperature.is_some_and(|temperature| !(0.0..=2.0).contains(&temperature)) {
        return Err(InvalidRequest::field(
            "temperature",
            "temperature must be between 0 and 2",
        ));
    }

    if top_p.is_some_and(|top_p| !(0.0..=1.0).contains(&top_p)) {
        return Err(InvalidRequest::field(
            "top_p",
            "top_p must be between 0 and 1",
        ));
    }

    Ok(())
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
pub(in crate::format) fn json_insert(
    base: &mut serde_json::Value,
    key: &str,
    value: serde_json::Value,
) {
    #[expect(
        clippy::expect_used,
        reason = "every base is a `json!` object literal created by the caller"
    )]
    let object = base
        .as_object_mut()
        .expect("JSON value is an object literal");

    object.insert(key.to_string(), value);
}

/// Attaches the gateway `thought_signature` extension (README §6.6) to a
/// rendered tool-call object at its wire position; `None` attaches nothing.
/// The extension key is defined exactly once, here.
pub(in crate::format) fn insert_thought_signature(target: &mut Value, signature: &Option<String>) {
    if let Some(signature) = signature {
        json_insert(
            target,
            "thought_signature",
            Value::String(signature.clone()),
        );
    }
}
