//! Protocol codecs connecting client and upstream wire formats through the IR.
//!
//! The tree is organized by protocol: this root holds the items more than one protocol family
//! consumes plus the two cross-layer vocabulary types ([`UpstreamFormat`], which names all five
//! families for the provider, storage, and gateway layers, and [`RenderOption`], which types every
//! provider definition's render options), each protocol parent ([`anthropic`], [`gemini`],
//! [`openai`]) holds the families speaking that one client/upstream protocol (README §1, §6) plus
//! the items its families share, and each family directory splits by concern (`request_parse`,
//! `client_render`, `upstream_render`, `upstream_parse`).
//!
//! Two contracts are declared once here. Every outward format implements [`ClientFormat`], which
//! parses client requests and boxes a [`ClientSseRenderer`] for live streaming (aggregated
//! rendering is a free function behind the same trait). Every upstream format implements
//! [`UpstreamEventParser`]. Upstream payload rendering has no shared contract — the payloads are
//! provider-coupled — so each codec exposes free body-render functions that the gateway invokes
//! directly; each renders the upstream body alone, and the dispatch's URL, header rows, and
//! streaming decision come from the provider definition's inference descriptor (README §5.2),
//! evaluated by the gateway dispatcher around the rendered body.
//!
//! All five families are bidirectional (README §6.3): each implements [`UpstreamEventParser`] for
//! its same-named upstream protocol plus the free request renderers beside it, and each implements
//! [`ClientFormat`] as the outward client surface served under that protocol's name —
//! `openai_response`, `openai_chat_completion`, `gemini_generate_content`, `gemini_interaction`,
//! and `anthropic_messages`. No family is upstream-parse-only; every family both speaks its
//! upstream protocol and serves its outward surface.

use crate::{
    ir::{
        InvalidRequest, IrAggregation, IrEvent, IrFinishReason, IrMessage, IrPart, IrRequest,
        IrRole,
    },
    sse::SseFrame,
};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, Error as SerdeDeError, SeqAccess, Visitor},
};
use serde_json::{Map, Value, error::Category};
use std::{
    fmt::{Error as FmtError, Formatter},
    marker::PhantomData,
};
use thiserror::Error;

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod openai;

/// Wire formats an upstream provider may speak: the closed vocabulary from which a
/// provider definition pins the one format every inference of the definition speaks
/// (README §1, §5), whatever its [`credential kind`](crate::provider::CredentialKind).
///
/// The serde and TypeScript names are the management wire names.
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

    /// Gemini Interactions protocol.
    #[serde(rename = "gemini_interaction")]
    GeminiInteraction,

    /// Anthropic Messages protocol.
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
}

impl UpstreamFormat {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The wire
    /// words live once each, in the exhaustive [`Self::as_str`] match. A new variant joins
    /// `ALL`; a partial table update is a compile error (the length pin), and the exhaustive
    /// match is what forces the variant's spelling to be stated.
    const ALL: [Self; 5] = [
        Self::OpenAiResponse,
        Self::OpenAiChatCompletion,
        Self::GeminiGenerateContent,
        Self::GeminiInteraction,
        Self::AnthropicMessages,
    ];

    /// Renders the canonical wire-format name.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponse => "openai_response",
            Self::OpenAiChatCompletion => "openai_chat_completion",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::GeminiInteraction => "gemini_interaction",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }

    /// Parses the stored spelling back into the closed set; anything outside it is storage
    /// drift, because the column is written only through [`UpstreamFormat::as_str`] and its
    /// schema `CHECK` pins the vocabulary.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.as_str() == text)
    }
}

/// One render option of a format codec (README §5.2): a closed, per-format switch adjusting the
/// body rendering. Growing this enum is the documented extension path for a future provider that
/// does not fit the plain body rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RenderOption {
    /// Never render `max_output_tokens`: the upstream rejects the parameter as unsupported, and
    /// the model's own output bound applies instead of the client-requested cap (`openai_response`
    /// only).
    OmitMaxOutputTokens,
}

impl RenderOption {
    /// The canonical option name, rendered by rejections and shared with the serde wire name.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OmitMaxOutputTokens => "omit_max_output_tokens",
        }
    }
}

/// An upstream payload the codec could not parse: malformed JSON, a missing required field, or an
/// upstream-reported failure event. The wire failure wording ("Upstream request failed: ...") is
/// composed by the consuming surface; this type carries only the cause detail.
#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct PayloadError {
    /// Human-readable failure detail.
    message: String,
}

/// A client request parsed into IR with its endpoint options.
#[derive(Debug)]
pub(crate) struct ParsedRequest {
    /// Format-independent request.
    pub(crate) ir: IrRequest,

    /// Whether the client requested a streamed response.
    pub(crate) is_streaming: bool,

    /// Whether the client asked for a trailing usage-only chunk; only the Chat Completions format
    /// exposes the option, every other format includes usage unconditionally.
    pub(crate) should_include_usage: bool,
}

/// Route context the router threads into request parsing (README §6.5): the facts the URL path
/// carries that the body does not.
///
/// OpenAI-family surfaces carry the model in the body, so they parse with an empty context.
/// Gemini surfaces route the model from the URL path — the body carries no model — so they parse
/// with the public model id (and, for GenerateContent, the streaming mode from the URL action,
/// not the body). The pipeline's "model from parsed body" assumption is replaced accordingly:
/// every parser sets `ir.model` from the body or, when the route provides it, from the context.
#[derive(Debug, Clone, Default)]
pub(crate) struct RouteContext {
    /// Public model id from the URL path, when the surface routes it there.
    pub(crate) public_id: Option<String>,

    /// Streaming mode from the URL action, when the surface routes it there (`Some`) rather than
    /// the body (`None`).
    pub(crate) is_streaming: Option<bool>,
}

impl RouteContext {
    /// Empty context: the model and streaming mode both come from the body.
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// Context carrying the URL-routed public model id and, when the URL action decides it, the
    /// streaming mode.
    pub(crate) fn with_model(public_id: String, is_streaming: Option<bool>) -> Self {
        Self {
            public_id: Some(public_id),
            is_streaming,
        }
    }
}

/// Accumulates consecutive same-role contributions into runs: a contribution whose role equals
/// the open run's extends it, and a contribution of any other role flushes the open run as one
/// closed `(role, parts)` run before opening the next. A contribution carrying no parts is
/// transparent — it neither extends nor splits a run, since it produces no boundary to alternate
/// against.
///
/// The role-merging parsers and renderers share this accumulator over their own role and part
/// types. The Anthropic Messages parser chunks a user turn's mixed content into user and tool
/// messages (README §5.8) and the Interactions parser coalesces consecutive same-role steps into
/// one message per run, both finishing into [`IrMessage`]s through [`RoleRun::finish`]; the Gemini
/// renderers coalesce consecutive same-role messages into one content entry or input step per run,
/// as their protocols' role-alternation rules require, taking the closed runs from
/// [`RoleRun::into_runs`].
pub(in crate::format) struct RoleRun<R, P> {
    /// Runs flushed so far, in order.
    runs: Vec<(R, Vec<P>)>,

    /// The open run: its role and the parts accumulated since it opened.
    open: Option<(R, Vec<P>)>,
}

impl<R, P> Default for RoleRun<R, P> {
    /// The empty accumulator: nothing flushed, no run open. Deliberately bound-free: roles and
    /// parts carry no default of their own, and emptiness needs none.
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            open: None,
        }
    }
}

impl<R: PartialEq, P> RoleRun<R, P> {
    /// Extends the open run when it carries `role`, else flushes it and opens a new one. A
    /// `parts` vector carrying no parts is transparent: it neither extends nor splits a run.
    pub(in crate::format) fn push(&mut self, role: R, parts: Vec<P>) {
        if parts.is_empty() {
            return;
        }

        match &mut self.open {
            Some((open_role, open_parts)) if *open_role == role => open_parts.extend(parts),

            _ => {
                self.flush_open();
                self.open = Some((role, parts));
            }
        }
    }

    /// Closes the accumulator, flushing the open run and returning every run — its role and its
    /// parts — in order.
    pub(in crate::format) fn into_runs(mut self) -> Vec<(R, Vec<P>)> {
        self.flush_open();
        self.runs
    }

    /// Moves the open run, when one exists, into the finished runs.
    fn flush_open(&mut self) {
        if let Some((role, parts)) = self.open.take() {
            self.runs.push((role, parts));
        }
    }
}

impl RoleRun<IrRole, IrPart> {
    /// Closes the accumulator, flushing the open run and returning every coalesced message in
    /// order — the IR form the request parsers consume, pairing each run [`RoleRun::into_runs`]
    /// returns into an [`IrMessage`].
    pub(in crate::format) fn finish(self) -> Vec<IrMessage> {
        self.into_runs()
            .into_iter()
            .map(|(role, parts)| IrMessage { role, parts })
            .collect()
    }
}

/// Format-independent upstream parsing contract implemented by every codec's upstream parser.
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
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError>;

    /// Ends a cleanly terminated upstream stream, flushing state buffered across frames into final
    /// IR events.
    ///
    /// Called exactly once when the upstream body reached a clean end of stream, after the last
    /// frame was ingested and before the terminal [`IrEvent::Done`]. It is never called once an
    /// error aborted the stream, so an aborted stream publishes no further parser state. Parsers
    /// that buffer no completable cross-frame state keep the default.
    fn finish(&mut self) -> Vec<IrEvent> {
        Vec::new()
    }
}

/// Emits the degraded finish of a clean upstream end-of-stream that carried no finish of its own —
/// no finish reason (Chat Completions) or no terminal event (Responses/Gemini): the missing finish
/// degrades to [`IrFinishReason::Stop`], so streamed and aggregated renderings of the same stream
/// agree (README §6.3).
///
/// The finished flag lives in each parser, which sets it on every path that emits a real finish —
/// a streamed terminal event, a stop reason, or the complete-body parse — and whose
/// [`UpstreamEventParser::finish`] hook delegates here: the first call on an unfinished parser
/// marks the flag and returns the single [`IrEvent::Finish`] carrying [`IrFinishReason::Stop`], and
/// a parser that already finished emits nothing.
pub(in crate::format) fn degrade_finish_once(is_finished: &mut bool) -> Vec<IrEvent> {
    if *is_finished {
        return Vec::new();
    }

    *is_finished = true;

    vec![IrEvent::finish(IrFinishReason::Stop)]
}

/// Emits the once-only opening [`IrEvent::MessageStart`] of an upstream event stream: exactly
/// once, first (README §6.2), carrying the response's identity.
///
/// The started flag lives in each parser, which passes it on every path that may open the
/// stream — the stream-head announcement and the recovery paths that never saw it. The first
/// call on an unstarted parser marks the flag and returns the single [`IrEvent::MessageStart`]
/// carrying `id` and `model`; a parser that already started emits nothing.
pub(in crate::format) fn message_start_once(
    is_started: &mut bool,
    id: String,
    model: String,
) -> Vec<IrEvent> {
    if *is_started {
        return Vec::new();
    }

    *is_started = true;

    vec![IrEvent::MessageStart { id, model }]
}

/// Emits the canonical two-event announcement of one complete upstream tool call: a
/// [`IrEvent::ToolCallDelta`] revealing the call with its full arguments as the delta, followed
/// by a [`IrEvent::ToolCallDone`] carrying identical payloads. The JSON-body parse paths of the
/// codecs deliver a whole call at once and share this pair so the streaming renderers open and
/// complete the call exactly as a streamed one; each parser keeps its own delivery and
/// completion bookkeeping beside it.
pub(in crate::format) fn complete_call_events(
    index: usize,
    call_id: String,
    name: String,
    arguments: String,
    thought_signature: Option<String>,
) -> [IrEvent; 2] {
    [
        IrEvent::ToolCallDelta {
            index,
            call_id: Some(call_id.clone()),
            name: Some(name.clone()),
            arguments_delta: arguments.clone(),
        },
        IrEvent::ToolCallDone {
            index,
            call_id,
            name,
            arguments,
            thought_signature,
        },
    ]
}

/// Client-side SSE rendering contract implemented by every outward codec.
pub(crate) trait ClientSseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String>;

    /// Renders a mid-stream error as a terminal frame from the shared [`crate::api::Error`]
    /// classification: the HTTP error path and the stream-frame path share one classification
    /// (README §6.7); each codec places it in its own terminal frame — the Chat Completions
    /// error envelope or the failed Responses response object — through its format's render
    /// module.
    fn error_frame(&mut self, error: &crate::api::Error) -> String;
}

/// An outward client format: parses client requests into IR and renders IR events back in the
/// client's wire shape, live or aggregated.
pub(crate) trait ClientFormat {
    /// The non-streaming success body this format renders.
    type JsonResponse;

    /// Parses one client request body with the router's route context: the URL-path facts the
    /// body does not carry (README §6.5). Surfaces routing the model from the body ignore the
    /// context; Gemini surfaces take the public model id (and streaming mode, when the URL
    /// action decides it) from it.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] for malformed or unsupported requests.
    fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest>
    where
        Self: Sized;

    /// Boxes the streaming renderer for one parsed request. `should_include_usage` is the trailing
    /// usage-chunk option; formats that always include usage ignore it. `public_id` is the public
    /// routing ID (README §3) the client addressed.
    fn sse_renderer(
        should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send>
    where
        Self: Sized;

    /// Renders an aggregated completion as a non-streaming response body. `public_id` is the public
    /// routing ID (README §3) the client addressed.
    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse;
}

/// A wire value that is either a plain string or an array of typed items: the two official shapes
/// of message content, tool-result output, and the Responses `input` field.
///
/// Deserialization is hand-written rather than an `untagged` enum because an untagged enum buffers
/// its variants and reports every inner failure as "data did not match any variant", which would
/// lose the member name a rejection has to name (README §6.6).
#[derive(Debug)]
pub(in crate::format) enum StringOrArray<T> {
    /// The plain-string form.
    Text(String),

    /// The array form.
    Array(Vec<T>),
}

/// The `error` member an upstream may embed in a 2xx response body (README §6.7).
///
/// An error body inside a 2xx response is a malformed upstream payload, so the embedded `code` is
/// never mirrored as the outward status — a 2xx code would otherwise render a success status
/// carrying an error body — and the embedded code and text ride in the message instead.
///
/// A present-but-null `error` is success, not failure: the non-streaming Responses body is the
/// Response object, whose shape carries `"error": null` on every success, and Chat Completions
/// success bodies never carry the key at all. Modelling the member as an [`Option`] is what makes
/// that distinction — JSON `null` deserializes to [`None`].
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(in crate::format) enum UpstreamError {
    /// The plain-string form.
    Text(String),

    /// The object form carrying the human-readable text, with whatever code the upstream attached
    /// beside it.
    Object {
        /// Human-readable error text.
        message: String,

        /// Upstream-specific error code, of any JSON shape.
        #[serde(default)]
        code: Option<Value>,
    },

    /// Any other JSON shape, rendered verbatim: an upstream error member is third-party data whose
    /// shape the gateway does not define.
    Raw(Value),
}

impl UpstreamError {
    /// The human-readable text of the error, when it carries one: the string form's text or the
    /// object form's `message`.
    pub(in crate::format) fn message(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),

            Self::Object { message, .. } => Some(message),

            Self::Raw(_) => None,
        }
    }

    /// Renders the failure carrying the embedded text, with the upstream code beside it when the
    /// error carries one.
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

impl PayloadError {
    /// Builds a payload parse failure from its detail.
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl<'de, T> Deserialize<'de> for StringOrArray<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Visitor accepting either shape and propagating every inner failure.
        struct StringOrArrayVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for StringOrArrayVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = StringOrArray<T>;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
                formatter.write_str("a string or an array of items")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: SerdeDeError,
            {
                Ok(StringOrArray::Text(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: SerdeDeError,
            {
                Ok(StringOrArray::Text(value))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // Element-wise ingestion propagates an item's own rejection — including the member
                // name it carries — instead of collapsing it into a shape mismatch.
                let mut items = Vec::new();

                while let Some(item) = sequence.next_element::<T>()? {
                    items.push(item);
                }

                Ok(StringOrArray::Array(items))
            }
        }

        deserializer.deserialize_any(StringOrArrayVisitor(PhantomData))
    }
}

/// Parses one client request body into its typed form.
///
/// The protocol's request shapes are concrete types with `deny_unknown_fields`, so one
/// deserialization enforces every strictness rule at once: a member outside the shape, a missing
/// required member, and a member of the wrong JSON kind are all rejected. serde names the offending
/// member in the rejection, which is what README §6.6 requires; the rejection carries no
/// [`InvalidRequest::param`] because serde reports the failure against the body as a whole rather
/// than against one named parameter.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for a body that is not valid JSON and for a body whose shape the
/// protocol does not allow.
pub(in crate::format) fn parse_client_request<T>(body: &[u8]) -> Result<T, InvalidRequest>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|error| match error.classify() {
        // A well-formed body of the wrong shape reports serde's own wording, which names the
        // offending member; a malformed body keeps the JSON-syntax wording.
        Category::Data => InvalidRequest::request(error.to_string()),

        _ => InvalidRequest::from_json(error),
    })
}

/// Parses tool function arguments into a JSON object, treating an empty argument string as an empty
/// object.
///
/// This is the reject policy: tool-call arguments are validated at parse and upstream-render
/// time, where a non-object is a contract violation that fails the request.
///
/// # Errors
///
/// Returns an error when the arguments are not a JSON object.
pub(in crate::format) fn arguments_as_object(
    arguments: &str,
) -> Result<Map<String, Value>, InvalidRequest> {
    if arguments.trim().is_empty() {
        return Ok(Map::new());
    }

    let value: Value = serde_json::from_str(arguments).map_err(|error| {
        InvalidRequest::field(
            "arguments",
            format!("Tool call arguments are not valid JSON: {error}"),
        )
    })?;

    match value {
        Value::Object(map) => Ok(map),
        _ => Err(InvalidRequest::field(
            "arguments",
            "Tool call arguments must encode a JSON object",
        )),
    }
}

/// Renders tool arguments into the object the wire carries: the parsed object when the arguments
/// encode one, the empty object otherwise — the degraded remainder carries no inventable shape.
///
/// This is the degrade policy, the render-time counterpart of [`arguments_as_object`]: client
/// renderers never fail a healthy stream over malformed arguments text, so every non-object
/// falls back to the empty object instead of erroring.
pub(in crate::format) fn arguments_to_object_or_empty(arguments: &str) -> Map<String, Value> {
    arguments_as_object(arguments).unwrap_or_default()
}

/// Parses the JSON payload of an upstream SSE frame into a codec's typed upstream shape; the single
/// home of the malformed-frame rejection shared by every codec's `ingest_frame`.
///
/// # Errors
///
/// Returns [`PayloadError`] when the frame data is not valid JSON or does not match the codec's
/// upstream shape.
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
/// Returns [`PayloadError`] when the body does not match the codec's upstream shape.
pub(in crate::format) fn parse_body<T>(value: Value) -> Result<T, PayloadError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value)
        .map_err(|error| PayloadError::new(format!("malformed upstream payload: {error}")))
}

/// Rejects an OpenAI-family tool payload whose `type` tag is not `function`.
///
/// `context` names the payload's role and renders into the rejection message (`tool` for advertised
/// tools, `tool_choice` for forced functions).
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

/// Validates the sampling parameter ranges the OpenAI and Gemini request shapes share: both
/// protocols' client surfaces accept `temperature` and `top_p` with the same official bounds.
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
/// Every outward payload is a first-party type whose serialization is total, so a serialization
/// failure is an invariant violation rather than a recoverable condition.
///
/// # Panics
///
/// Panics if `value` fails to serialize, which cannot occur for the rendered wire types.
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

#[cfg(test)]
mod tests {
    use super::{RoleRun, UpstreamFormat};

    /// A zero-part contribution is transparent: it neither extends nor splits the open run, so a
    /// foreign-role contribution carrying no parts leaves a same-role run unbroken, and a
    /// same-role one leaves it continuous.
    ///
    /// # Panics
    ///
    /// Panics if the closed runs diverge from the transparency rule.
    #[test]
    fn zero_part_contributions_are_transparent() {
        let mut runs = RoleRun::default();

        runs.push('a', vec![1]);
        runs.push('b', vec![]);
        runs.push('a', vec![2, 3]);
        runs.push('a', vec![]);
        runs.push('b', vec![4]);

        assert_eq!(runs.into_runs(), vec![('a', vec![1, 2, 3]), ('b', vec![4])]);
    }

    /// The derived parse stays one vocabulary with the literal `as_str` home for every word of
    /// the closed set, and a spelling outside it parses to nothing. Only the two formats the
    /// storage fixtures round-trip are pinned elsewhere; this pins the whole table at once.
    ///
    /// # Panics
    ///
    /// Panics if a word fails to round-trip or an unknown spelling parses.
    #[test]
    fn upstream_format_parses_its_whole_vocabulary() {
        for format in UpstreamFormat::ALL {
            assert_eq!(UpstreamFormat::parse(format.as_str()), Some(format));
        }

        assert_eq!(UpstreamFormat::parse("openai_responses"), None);
    }
}
