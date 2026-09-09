//! Protocol codecs connecting client and upstream wire formats through the IR.
//!
//! Every format exposes the same structural roles: a client request parser,
//! an upstream payload renderer, an upstream event parser producing IR events,
//! and client response renderers (streaming SSE and aggregated JSON).

use crate::{error::ApiError, ir::FinishReason};

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

/// Maps an OpenAI `finish_reason` string to the canonical reason; unknown
/// values degrade to [`FinishReason::Stop`] rather than failing a healthy
/// stream.
pub(crate) fn finish_reason_from_openai(value: &str) -> FinishReason {
    match value {
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCall,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

/// Renders the canonical reason as an OpenAI `finish_reason` string.
pub(crate) fn finish_reason_to_openai(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCall => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
    }
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
