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
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub(crate) mod gemini;
pub(crate) mod openai_chat_completion;
pub(crate) mod openai_response;

/// Wire formats an upstream provider may speak.
///
/// The serde and TypeScript names are the management wire names, shared with
/// [`UpstreamFormat::as_str`] — the storage form and the wire form are one
/// vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum UpstreamFormat {
    /// OpenAI Responses protocol.
    #[serde(rename = "openai_response")]
    OpenAiResponse,

    /// OpenAI Chat Completions protocol.
    #[serde(rename = "openai_chat_completion")]
    OpenAiChatCompletion,

    /// Gemini GenerateContent protocol.
    #[serde(rename = "gemini_generate_content")]
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
    /// surface's classified failure in the OpenAI error shape; each codec
    /// places it in its own terminal frame — the Chat Completions error
    /// envelope or the failed Responses response object.
    fn error_frame(&mut self, error: &OpenAiErrorBody) -> String;
}

/// The OpenAI error object (README §6.7): the shape every outward failure
/// renders as, whether in a JSON error envelope, inside a streamed Chat
/// Completions error frame, or as the `error` of a failed Responses response
/// object.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OpenAiErrorBody {
    /// Human-readable error message.
    pub(crate) message: String,

    /// OpenAI `type` field.
    pub(crate) r#type: &'static str,

    /// OpenAI `param` field, when the failure names a request parameter.
    pub(crate) param: Option<String>,

    /// OpenAI `code` field, when the failure has a stable code.
    pub(crate) code: Option<String>,
}

/// The JSON error envelope `{"error": {...}}` shared by the JSON error
/// response and the streamed Chat Completions error frame, so both render the
/// same envelope from the same type.
#[derive(Debug, Serialize)]
pub(crate) struct OpenAiErrorEnvelope {
    /// The rendered error object.
    pub(crate) error: OpenAiErrorBody,
}

/// An outward client format: parses client requests into IR and renders IR
/// events back in the client's wire shape, live or aggregated.
pub(crate) trait ClientFormat {
    /// The non-streaming success body this format renders.
    type JsonResponse: Serialize;

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
    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse;
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

/// A wire value that is either a plain string or an array of typed items: the
/// two official shapes of message content, tool-result output, and the
/// Responses `input` field.
///
/// Deserialization is hand-written rather than an `untagged` enum because an
/// untagged enum buffers its variants and reports every inner failure as
/// "data did not match any variant", which would lose the member name a
/// rejection has to name (README §6.6).
#[derive(Debug)]
pub(in crate::format) enum StringOrArray<T> {
    /// The plain-string form.
    Text(String),

    /// The array form.
    Array(Vec<T>),
}

impl<'de, T> Deserialize<'de> for StringOrArray<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Visitor accepting either shape and propagating every inner failure.
        struct StringOrArrayVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T> serde::de::Visitor<'de> for StringOrArrayVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = StringOrArray<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a string or an array of items")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(StringOrArray::Text(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(StringOrArray::Text(value))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                // Element-wise ingestion propagates an item's own rejection —
                // including the member name it carries — instead of collapsing
                // it into a shape mismatch.
                let mut items = Vec::new();

                while let Some(item) = sequence.next_element::<T>()? {
                    items.push(item);
                }

                Ok(StringOrArray::Array(items))
            }
        }

        deserializer.deserialize_any(StringOrArrayVisitor(std::marker::PhantomData))
    }
}

/// Parses one client request body into its typed form.
///
/// The protocol's request shapes are concrete types with
/// `deny_unknown_fields`, so one deserialization enforces every strictness
/// rule at once: a member outside the shape, a missing required member, and a
/// member of the wrong JSON kind are all rejected. serde names the offending
/// member in the rejection, which is what README §6.6 requires; the rejection
/// carries no [`InvalidRequest::param`] because serde reports the failure
/// against the body as a whole rather than against one named parameter.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for a body that is not valid JSON and for a body
/// whose shape the protocol does not allow.
pub(in crate::format) fn parse_client_request<T>(body: &[u8]) -> Result<T, InvalidRequest>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|error| match error.classify() {
        // A well-formed body of the wrong shape reports serde's own wording,
        // which names the offending member; a malformed body keeps the
        // JSON-syntax wording.
        serde_json::error::Category::Data => InvalidRequest::request(error.to_string()),

        _ => InvalidRequest::from_json(error),
    })
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

/// Parses the JSON payload of an upstream SSE frame into a codec's typed
/// upstream shape; the single home of the malformed-frame rejection shared by
/// every codec's `ingest_frame`.
///
/// # Errors
///
/// Returns [`PayloadError`] when the frame data is not valid JSON or does not
/// match the codec's upstream shape.
pub(in crate::format) fn parse_frame<T>(frame: &SseFrame) -> Result<T, PayloadError>
where
    T: DeserializeOwned,
{
    serde_json::from_str(&frame.data)
        .map_err(|_| PayloadError::new("malformed upstream payload in SSE frame"))
}

/// Parses one complete upstream JSON body into a codec's typed upstream shape.
///
/// # Errors
///
/// Returns [`PayloadError`] when the body does not match the codec's upstream
/// shape.
pub(in crate::format) fn parse_body<T>(value: Value) -> Result<T, PayloadError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value)
        .map_err(|error| PayloadError::new(format!("malformed upstream payload: {error}")))
}

/// The `error` member an upstream may embed in a 2xx response body
/// (README §6.7).
///
/// An error body inside a 2xx response is a malformed upstream payload, so the
/// embedded `code` is never mirrored as the outward status — a 2xx code would
/// otherwise render a success status carrying an error body — and the embedded
/// code and text ride in the message instead.
///
/// A present-but-null `error` is success, not failure: the non-streaming
/// Responses body is the Response object, whose shape carries `"error": null`
/// on every success, and Chat Completions success bodies never carry the key at
/// all. Modelling the member as an [`Option`] is what makes that distinction —
/// JSON `null` deserializes to [`None`].
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(in crate::format) enum UpstreamError {
    /// The plain-string form.
    Text(String),

    /// The object form carrying the human-readable text, with whatever code
    /// the upstream attached beside it.
    Object {
        /// Human-readable error text.
        message: String,

        /// Upstream-specific error code, of any JSON shape.
        #[serde(default)]
        code: Option<Value>,
    },

    /// Any other JSON shape, rendered verbatim: an upstream error member is
    /// third-party data whose shape the gateway does not define.
    Raw(Value),
}

impl UpstreamError {
    /// The human-readable text of the error, when it carries one: the string
    /// form's text or the object form's `message`.
    pub(in crate::format) fn message(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),

            Self::Object { message, .. } => Some(message),

            Self::Raw(_) => None,
        }
    }

    /// Renders the failure carrying the embedded text, with the upstream code
    /// beside it when the error carries one.
    pub(in crate::format) fn failure(&self) -> PayloadError {
        let detail = match self {
            Self::Text(text) => text.clone(),

            Self::Object { message, code } => match code {
                Some(code) => format!("{message} (upstream error code: {code})"),
                None => message.clone(),
            },

            Self::Raw(value) => value.to_string(),
        };

        PayloadError::new(detail)
    }
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

/// Serializes a rendered outward payload into its wire form.
///
/// Every outward payload is a first-party type whose serialization is total,
/// so a serialization failure is an invariant violation rather than a
/// recoverable condition.
///
/// # Panics
///
/// Panics if `value` fails to serialize, which cannot occur for the rendered
/// wire types.
pub(in crate::format) fn render_json<T>(value: &T) -> String
where
    T: Serialize + ?Sized,
{
    #[expect(
        clippy::expect_used,
        reason = "rendered wire types are first-party and serialize totally"
    )]
    serde_json::to_string(value).expect("rendered wire payloads serialize")
}
