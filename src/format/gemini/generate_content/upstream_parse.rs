//! Upstream-event parsing for the Gemini GenerateContent codec: parses upstream SSE frames or a
//! complete JSON body into IR events.
//!
//! Upstream payloads are third-party, so the models below carry exactly the members the codec
//! reads, ignore every member it does not, and default what upstreams legitimately omit.

use crate::{
    format::{self, PayloadError, UpstreamError, UpstreamEventParser, gemini::fold_gemini_usage},
    ids,
    ir::{IrEvent, IrFinishReason},
    sse::SseFrame,
};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Parses upstream Gemini responses — SSE frames or a full JSON body — into IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Count of function calls synthesized in this response.
    call_counter: usize,

    /// Whether the response delivered any `functionCall` part; Gemini ends tool-call turns with
    /// the plain `STOP` reason, so the delivered calls decide the canonical tool-call finish.
    is_call_delivered: bool,

    /// Whether a finish was already emitted; `finish` degrades a stream that ends without one
    /// through [`format::degrade_finish_once`].
    is_finished: bool,
}

/// One upstream GenerateContentResponse, or the Antigravity envelope wrapping one.
///
/// The Antigravity API wraps every GenerateContentResponse — `:generateContent` JSON bodies
/// and `alt=sse` frames alike — in a `response` member beside `traceId` and `metadata` (README
/// §5.2), while Gemini-compatible upstreams carry the bare response, which has no top-level
/// `response` member of its own. Every member is optional because the body is either the envelope
/// or the bare response, and upstreams are third-party: members the gateway does not read are
/// ignored.
#[derive(Debug, Default, Deserialize)]
struct UpstreamBody {
    /// The response wrapped by the Antigravity envelope.
    #[serde(default)]
    response: Option<Box<UpstreamBody>>,

    /// Failure the upstream may embed in a 2xx body (README §6.7).
    #[serde(default)]
    error: Option<UpstreamError>,

    /// Upstream response identifier.
    #[serde(rename = "responseId", default)]
    response_id: Option<String>,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(rename = "modelVersion", default)]
    model_version: Option<String>,

    /// Candidate answers; the gateway reads the primary one.
    #[serde(default)]
    candidates: Vec<Candidate>,

    /// Prompt-level feedback, sent only when no candidate was generated.
    #[serde(rename = "promptFeedback", default)]
    prompt_feedback: Option<PromptFeedback>,

    /// Token accounting.
    #[serde(rename = "usageMetadata", default)]
    usage_metadata: Option<UsageMetadata>,
}

impl UpstreamBody {
    /// The GenerateContentResponse to ingest: the envelope's wrapped response when the body is an
    /// envelope, else the body itself.
    fn payload(&self) -> &Self {
        match &self.response {
            Some(payload) => payload,

            None => self,
        }
    }
}

/// One upstream candidate answer.
#[derive(Debug, Default, Deserialize)]
struct Candidate {
    /// Upstream-assigned index, absent on upstreams that omit it.
    #[serde(default)]
    index: Option<u64>,

    /// The candidate's content.
    #[serde(default)]
    content: Option<CandidateContent>,

    /// Upstream finish reason string.
    #[serde(rename = "finishReason", default)]
    finish_reason: Option<String>,
}

/// The content of one upstream candidate.
#[derive(Debug, Default, Deserialize)]
struct CandidateContent {
    /// The content parts in order.
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

/// One upstream content part, carrying its text, its tool call, or neither.
#[derive(Debug, Default, Deserialize)]
struct GeminiPart {
    /// Text content, absent on a non-text part.
    #[serde(default)]
    text: Option<String>,

    /// Whether the text is reasoning rather than answer text.
    #[serde(default)]
    thought: Option<bool>,

    /// The tool call, present on a `functionCall` part.
    #[serde(rename = "functionCall", default)]
    function_call: Option<FunctionCallPayload>,

    /// Upstream thought signature that must be replayed on the next turn.
    #[serde(rename = "thoughtSignature", default)]
    thought_signature: Option<String>,
}

/// The tool call of one upstream part.
#[derive(Debug, Default, Deserialize)]
struct FunctionCallPayload {
    /// Name of the invoked function, empty when the upstream omits it.
    #[serde(default)]
    name: Option<String>,

    /// Arguments the model supplied; the object's members are the tool's own schema, which the
    /// gateway forwards verbatim.
    #[serde(default)]
    args: Option<Map<String, Value>>,

    /// Invocation identifier, absent on an upstream that does not mint one.
    #[serde(default)]
    id: Option<String>,
}

/// Prompt-level feedback of a blocked prompt.
#[derive(Debug, Default, Deserialize)]
struct PromptFeedback {
    /// Why the prompt was blocked.
    #[serde(rename = "blockReason", default)]
    block_reason: Option<String>,
}

/// Upstream token accounting in the Gemini shape.
#[derive(Debug, Default, Deserialize)]
struct UsageMetadata {
    /// Tokens consumed by the prompt.
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u64,

    /// Tokens produced by the candidates, excluding reasoning.
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u64,

    /// Tokens spent on model reasoning.
    #[serde(rename = "thoughtsTokenCount", default)]
    thoughts_token_count: Option<u64>,

    /// Prompt tokens served from the upstream cache.
    #[serde(rename = "cachedContentTokenCount", default)]
    cached_content_token_count: Option<u64>,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one GenerateContentResponse object.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for an embedded upstream error object and for an
    /// invalid-generation finish (`MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`,
    /// `TOO_MANY_TOOL_CALLS`).
    fn ingest_response(&mut self, body: UpstreamBody) -> Result<Vec<IrEvent>, PayloadError> {
        // An error object inside a 2xx body is a malformed upstream payload (README §6.7); the
        // check sits beside the protocol fields, before the `response` envelope below is unwrapped,
        // shared with the OpenAI-family codecs.
        if let Some(error) = &body.error {
            return Err(error.failure());
        }

        let value = body.payload();

        // The veto covers wherever the document carries the member: the Antigravity envelope
        // wraps the response — and any failure riding it — one level in, so the wrapped
        // document's own `error` member aborts too instead of parsing as an empty success
        // (README §6.7). Where the body carries no envelope, this re-reads the outer body's
        // already-vetoed `None`.
        if let Some(error) = &value.error {
            return Err(error.failure());
        }

        let mut events = Vec::new();

        if !self.is_started {
            let id = value
                .response_id
                .clone()
                .unwrap_or_else(|| format!("gen-{}", ids::new_uuid()));
            let model = value.model_version.clone().unwrap_or_default();

            events.extend(format::message_start_once(&mut self.is_started, id, model));
        }

        if let Some(candidate) =
            format::primary(&value.candidates, |candidate| candidate.index == Some(0))
        {
            if let Some(content) = &candidate.content {
                for part in &content.parts {
                    self.ingest_part(part, &mut events);
                }
            }

            if let Some(finish_reason) = &candidate.finish_reason {
                self.is_finished = true;

                events.push(IrEvent::finish(finish_reason_from_gemini(
                    finish_reason,
                    self.is_call_delivered,
                )?));
            }
        }

        // A prompt-level block carries `promptFeedback.blockReason` and no candidates, so no
        // `finishReason` ever arrives and the clean end of stream would degrade the block into a
        // successful `Stop`. Upstream sends `promptFeedback` only when no candidate was generated,
        // so a candidate finish above always wins. The verbatim block reason rides the finish so
        // the native render surface can mirror `promptFeedback` (README §6.3).
        if !self.is_finished
            && let Some(block_reason) = value
                .prompt_feedback
                .as_ref()
                .and_then(|feedback| feedback.block_reason.as_deref())
        {
            self.is_finished = true;

            events.push(IrEvent::Finish {
                reason: IrFinishReason::ContentFilter,
                prompt_block_reason: Some(block_reason.to_string()),
            });
        }

        if let Some(usage) = &value.usage_metadata {
            events.push(IrEvent::Usage(fold_gemini_usage(
                usage.prompt_token_count,
                usage.candidates_token_count,
                usage.thoughts_token_count,
                usage.cached_content_token_count,
            )));
        }

        Ok(events)
    }

    /// Ingests one content part of the primary candidate.
    ///
    /// # Panics
    ///
    /// Panics if serializing the borrowed `args` map fails, which cannot occur: serializing a JSON
    /// object map is infallible.
    fn ingest_part(&mut self, part: &GeminiPart, events: &mut Vec<IrEvent>) {
        let is_thought = part.thought.unwrap_or(false);

        if let Some(text) = &part.text {
            events.push(if is_thought {
                IrEvent::ReasoningDelta(text.clone())
            } else {
                IrEvent::TextDelta(text.clone())
            });
        }

        // A thinking part's `thoughtSignature` is the replayable reasoning state the
        // thinking docs require clients to resend exactly as received
        // (`ai.google.dev/gemini-api/docs/thinking`), so it must survive the IR. The
        // empty-text done closes the delta-delivered block carrying the signature — the
        // conformant deltas-then-empty-done pairing of README §6.2 — and the Interactions
        // surface replays it as the signature-bearing thought step. A functionCall part's
        // signature keeps its own channel below.
        if is_thought && let Some(signature) = &part.thought_signature {
            events.push(IrEvent::ReasoningDone {
                text: String::new(),
                signature: Some(signature.clone()),
                opaque: Value::Null,
            });
        }

        let Some(function_call) = &part.function_call else {
            return;
        };

        self.is_call_delivered = true;

        let name = function_call.name.clone().unwrap_or_default();

        // The arguments serialize straight from the borrowed map — no deep copy — with a missing
        // `args` rendering an empty JSON object.
        let arguments = match &function_call.args {
            Some(args) =>
            {
                #[expect(
                    clippy::expect_used,
                    reason = "serializing a JSON object map cannot fail"
                )]
                serde_json::to_string(args).expect("JSON map serialization is infallible")
            }

            None => "{}".to_string(),
        };

        // A populated upstream `functionCall.id` is the cross-turn matching channel: the replayed
        // `functionResponse.id` must answer it, and without it two same-name calls in one turn are
        // unresolvable. An id-less call mints one so the pairing channel always exists; the minted
        // id also rides outward as the client-facing tool-call id.
        let call_id = match function_call.id.clone() {
            Some(id) if !id.is_empty() => id,
            _ => format!("call_{}", ids::new_uuid()),
        };

        events.extend(format::complete_call_events(
            self.call_counter,
            call_id,
            name,
            arguments,
            part.thought_signature.clone(),
        ));

        self.call_counter += 1;
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a GenerateContentResponse.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON and upstream error objects.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let body = format::parse_frame::<UpstreamBody>(frame)?;

        self.ingest_response(body)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream error objects and malformed bodies.
    fn parse_json_response(&mut self, body: &[u8]) -> Result<Vec<IrEvent>, PayloadError> {
        let body = format::parse_body::<UpstreamBody>(body)?;

        self.ingest_response(body)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        format::degrade_finish_once(&mut self.is_finished)
    }
}

/// Maps a Gemini `finishReason` to the canonical reason.
///
/// Gemini ends function-call turns with the plain `STOP` reason — the `finishReason` vocabulary has
/// no tool-call value — so a turn that delivered `functionCall` parts carries the canonical
/// [`IrFinishReason::ToolCall`] finish, matching official Chat Completions semantics for
/// text+tool-call mixed turns too.
///
/// # Errors
///
/// Returns [`PayloadError`] for the invalid-generation reasons `MALFORMED_FUNCTION_CALL`,
/// `UNEXPECTED_TOOL_CALL`, and `TOO_MANY_TOOL_CALLS`: the turn delivered no usable result and must
/// surface as an upstream failure (README §6.7) instead of a successful `Stop` finish that would
/// mask the failure.
fn finish_reason_from_gemini(
    value: &str,
    is_call_delivered: bool,
) -> Result<IrFinishReason, PayloadError> {
    match value {
        "MAX_TOKENS" => Ok(IrFinishReason::Length),

        // The official `FinishReason` vocabulary flags filtered content with these nine reasons —
        // `LANGUAGE` ("the candidate content was flagged for using an unsupported language")
        // grouped with its safety siblings, `PROHIBITED_CONTENT`/`RECITATION` with their
        // image-generation siblings `IMAGE_PROHIBITED_CONTENT` and `IMAGE_RECITATION`, and
        // `IMAGE_SAFETY`, raised when the generated images have safety violations, among
        // them — so each is a filtered turn, never a clean stop.
        "LANGUAGE"
        | "SAFETY"
        | "RECITATION"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION" => Ok(IrFinishReason::ContentFilter),

        "MALFORMED_FUNCTION_CALL" => Err(PayloadError::new(
            "upstream finish reason `MALFORMED_FUNCTION_CALL`: \
             the function call generated by the model is invalid",
        )),

        // The same invalid-generation class as `MALFORMED_FUNCTION_CALL`: the model emitted a tool
        // call although the request enabled none, so the turn delivered an unusable result and
        // must surface as an upstream failure instead of a successful finish that would mask it.
        "UNEXPECTED_TOOL_CALL" => Err(PayloadError::new(
            "upstream finish reason `UNEXPECTED_TOOL_CALL`: \
             the model generated a tool call but no tools were enabled in the request",
        )),

        // The same invalid-generation class: the system cut the generation after too many
        // consecutive tool calls, so the turn never completed and must surface as an upstream
        // failure instead of a successful finish that would mask the forced loop exit — and lure
        // the client into resuming the very loop the upstream terminated.
        "TOO_MANY_TOOL_CALLS" => Err(PayloadError::new(
            "upstream finish reason `TOO_MANY_TOOL_CALLS`: \
             the model called too many tools consecutively and the system exited execution",
        )),

        _ if is_call_delivered => Ok(IrFinishReason::ToolCall),

        _ => Ok(IrFinishReason::Stop),
    }
}
