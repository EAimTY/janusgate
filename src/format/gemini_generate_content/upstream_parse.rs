//! Upstream-event parsing for the Gemini GenerateContent codec: parses upstream SSE frames or a
//! complete JSON body into IR events.
//!
//! Upstream payloads are third-party, so the models below carry exactly the members the codec
//! reads, ignore every member it does not, and default what upstreams legitimately omit.

use crate::{
    format::{self, PayloadError, UpstreamError, UpstreamEventParser},
    ids,
    ir::{IrEvent, IrFinishReason, IrUsage},
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

    /// Whether a finish reason was already emitted; a clean end-of-stream without one degrades to
    /// [`IrFinishReason::Stop`] in the parser's finish hook, matching the aggregated rendering
    /// default.
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
    /// Returns [`PayloadError`] for an embedded upstream error object and for a
    /// `MALFORMED_FUNCTION_CALL` finish.
    fn ingest_response(&mut self, body: UpstreamBody) -> Result<Vec<IrEvent>, PayloadError> {
        // An error object inside a 2xx body is a malformed upstream payload (README §6.7); the
        // check sits beside the protocol fields, before the `response` envelope below is unwrapped,
        // shared with the OpenAI-family codecs.
        if let Some(error) = &body.error {
            return Err(error.failure());
        }

        let value = body.payload();

        let mut events = Vec::new();

        if !self.is_started {
            self.is_started = true;

            events.push(IrEvent::MessageStart {
                id: value
                    .response_id
                    .clone()
                    .unwrap_or_else(|| format!("gen-{}", ids::new_uuid())),
                model: value.model_version.clone().unwrap_or_default(),
            });
        }

        if let Some(candidate) = primary_candidate(&value.candidates) {
            if let Some(content) = &candidate.content {
                for part in &content.parts {
                    self.ingest_part(part, &mut events);
                }
            }

            if let Some(finish_reason) = &candidate.finish_reason {
                self.is_finished = true;

                events.push(IrEvent::Finish(finish_reason_from_gemini(
                    finish_reason,
                    self.is_call_delivered,
                )?));
            }
        }

        // A prompt-level block carries `promptFeedback.blockReason` and no candidates, so no
        // `finishReason` ever arrives and the clean end of stream would degrade the block into a
        // successful `Stop`. Upstream sends `promptFeedback` only when no candidate was generated,
        // so a candidate finish above always wins.
        if !self.is_finished
            && value
                .prompt_feedback
                .as_ref()
                .and_then(|feedback| feedback.block_reason.as_deref())
                .is_some()
        {
            self.is_finished = true;

            events.push(IrEvent::Finish(IrFinishReason::ContentFilter));
        }

        if let Some(usage) = &value.usage_metadata {
            let reasoning_tokens = usage.thoughts_token_count;

            events.push(IrEvent::Usage(IrUsage {
                input_tokens: usage.prompt_token_count,
                output_tokens: usage.candidates_token_count + reasoning_tokens.unwrap_or(0),
                cached_tokens: usage.cached_content_token_count,
                // Gemini reports no cache-write accounting.
                cache_write_tokens: None,
                reasoning_tokens,
            }));
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
        if let Some(text) = &part.text {
            let is_thought = part.thought.unwrap_or(false);

            events.push(if is_thought {
                IrEvent::ReasoningDelta(text.clone())
            } else {
                IrEvent::TextDelta(text.clone())
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

        events.push(IrEvent::ToolCallDelta {
            index: self.call_counter,
            call_id: Some(call_id.clone()),
            name: Some(name.clone()),
            arguments_delta: arguments.clone(),
        });

        events.push(IrEvent::ToolCallDone {
            index: self.call_counter,
            call_id,
            name,
            arguments,
            thought_signature: part.thought_signature.clone(),
        });

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
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let body = format::parse_body::<UpstreamBody>(value)?;

        self.ingest_response(body)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming content without ever sending a
        // `finishReason`; the missing finish reason degrades to `Stop` so the streamed client sees
        // the same terminal finish reason as the aggregated rendering of the same stream.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

/// Selects the primary candidate of an upstream `candidates` array: the entry whose `index` is 0,
/// tolerating index-less upstreams by taking the first entry instead. Parallel candidates beyond
/// the primary are not supported and the remainder is ignored.
fn primary_candidate(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates
        .iter()
        .find(|candidate| candidate.index == Some(0))
        .or_else(|| candidates.first())
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
/// Returns [`PayloadError`] for `MALFORMED_FUNCTION_CALL`: the model generated an invalid function
/// call, so the turn delivered no usable result and must surface as an upstream failure (README
/// §6.7) instead of a successful `Stop` finish that would mask the failure.
fn finish_reason_from_gemini(
    value: &str,
    is_call_delivered: bool,
) -> Result<IrFinishReason, PayloadError> {
    match value {
        "MAX_TOKENS" => Ok(IrFinishReason::Length),

        // The official `FinishReason` vocabulary flags filtered content with these six reasons —
        // `LANGUAGE` ("the candidate content was flagged for using an unsupported language")
        // grouped with its safety siblings — so each is a filtered turn, never a clean stop.
        "LANGUAGE" | "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            Ok(IrFinishReason::ContentFilter)
        }

        "MALFORMED_FUNCTION_CALL" => Err(PayloadError::new(
            "upstream finish reason `MALFORMED_FUNCTION_CALL`: \
             the function call generated by the model is invalid",
        )),

        _ if is_call_delivered => Ok(IrFinishReason::ToolCall),

        _ => Ok(IrFinishReason::Stop),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A turn whose upstream reported `MALFORMED_FUNCTION_CALL` is a failed
    /// generation, not a successful completion: both the JSON body and the
    /// streaming path reject it as an upstream failure (README §6.7) instead
    /// of rendering a clean finish that would mask the failure.
    ///
    /// # Panics
    ///
    /// Panics if the flagged turn parses as a success or its wording drifts.
    #[test]
    fn malformed_function_call_finish_reason_is_an_upstream_failure() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "MALFORMED_FUNCTION_CALL",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the flagged body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(body)
            .expect_err("a malformed function call turn must fail");

        assert_eq!(
            error.to_string(),
            "upstream finish reason `MALFORMED_FUNCTION_CALL`: \
             the function call generated by the model is invalid",
        );

        let frame = SseFrame {
            data: serde_json::json!({"candidates": [{"finishReason": "MALFORMED_FUNCTION_CALL"}]})
                .to_string(),
            event: None,
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the flagged frame is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&frame)
            .expect_err("a malformed function call stream frame must fail");

        assert!(error.to_string().contains("MALFORMED_FUNCTION_CALL"));
    }

    /// Ordinary finish reasons keep mapping through the now fallible mapper:
    /// a turn cut off by the token limit still parses and reports the
    /// canonical length finish.
    ///
    /// # Panics
    ///
    /// Panics if the flagged turn fails to parse.
    #[test]
    fn ordinary_finish_reasons_still_map() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "MAX_TOKENS",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the MAX_TOKENS turn is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a MAX_TOKENS turn parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Length)));
    }

    /// A prompt-level block carries `promptFeedback.blockReason` and no
    /// candidates, so no `finishReason` ever arrives and the clean end of
    /// stream must not degrade the block into a successful `Stop` finish:
    /// both the aggregated body and the streaming path report the canonical
    /// content-filter finish.
    ///
    /// # Panics
    ///
    /// Panics if the blocked turn fails to parse or its finish drifts.
    #[test]
    fn prompt_block_finishes_as_content_filter() {
        let body = serde_json::json!({"promptFeedback": {"blockReason": "SAFETY"}});

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the prompt-blocked body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body.clone())
            .expect("a prompt-blocked body parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
        assert!(parser.finish().is_empty(), "no spurious Stop may follow");

        let frame = SseFrame {
            data: body.to_string(),
            event: None,
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the prompt-blocked frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frame)
            .expect("a prompt-blocked frame parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// A turn the upstream flagged for an unsupported language (`LANGUAGE`)
    /// belongs to the same flagged-content class as the safety siblings: both
    /// the aggregated body and the streaming path report the canonical
    /// content-filter finish instead of a clean `Stop` that would hide the
    /// filtering from clients branching on it.
    ///
    /// # Panics
    ///
    /// Panics if the flagged turn fails to parse or its finish drifts.
    #[test]
    fn language_finish_reason_maps_to_content_filter() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "LANGUAGE",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the LANGUAGE-flagged body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body.clone())
            .expect("a LANGUAGE-flagged body parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
        assert!(parser.finish().is_empty(), "no spurious Stop may follow");

        let frame = SseFrame {
            data: body.to_string(),
            event: None,
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the LANGUAGE-flagged frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frame)
            .expect("a LANGUAGE-flagged frame parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// Upstream sends `promptFeedback` only when no candidate was generated,
    /// so a response that also carried a finishing candidate keeps that
    /// candidate's finish reason instead of being overridden by the feedback.
    ///
    /// # Panics
    ///
    /// Panics if the finishing candidate fails to parse.
    #[test]
    fn candidate_finish_wins_over_prompt_feedback() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "ok"}]},
                "finishReason": "STOP",
            }],
            "promptFeedback": {"blockReason": "SAFETY"},
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the finishing candidate is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a finishing candidate parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Stop)));
        assert!(!events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// The `usageMetadata` breakdown fields flow into the IR usage detail:
    /// `cachedContentTokenCount` becomes the cached input count and
    /// `thoughtsTokenCount` the reasoning output count, while the output
    /// total keeps including the reasoning tokens and the absent
    /// cache-write count stays [`None`].
    ///
    /// # Panics
    ///
    /// Panics if the usage breakdown diverges from the upstream report.
    #[test]
    fn usage_breakdown_reports_cache_and_reasoning_tokens() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "hi"}]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 20,
                "thoughtsTokenCount": 8,
                "cachedContentTokenCount": 60,
            },
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the usage-bearing body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a usage-bearing body parses");

        let usage = events.iter().find_map(|event| match event {
            IrEvent::Usage(usage) => Some(*usage),
            _ => None,
        });

        assert_eq!(
            usage,
            Some(IrUsage {
                input_tokens: 100,
                output_tokens: 28,
                cached_tokens: Some(60),
                cache_write_tokens: None,
                reasoning_tokens: Some(8),
            })
        );
    }

    /// An error object inside a 2xx body is rejected through the shared
    /// OpenAI-family rejection, wording included; the shared classification
    /// also rejects the non-object error values the retired inline copy
    /// silently ignored (README §6.7 malformed-payload handling).
    ///
    /// # Panics
    ///
    /// Panics if an error body parses as a success or its wording drifts.
    #[test]
    fn embedded_error_object_uses_shared_rejection() {
        let body = serde_json::json!({
            "error": {"code": 400, "message": "API key not valid"},
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the embedded error object is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(body)
            .expect_err("an embedded error object must fail");

        assert_eq!(
            error.to_string(),
            "API key not valid (upstream error code: 400)"
        );

        let body = serde_json::json!({"error": "quota exhausted"});

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the non-object error value is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(body)
            .expect_err("a non-object error value must fail");

        assert_eq!(error.to_string(), "quota exhausted");
    }
}
