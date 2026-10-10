//! The Gemini protocol's two codec families and their shared items.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module groups the GenerateContent family ([`generate_content`]) and
//! the Interactions family ([`interaction`]) — the two codecs over the Gemini client protocol and
//! the two Gemini upstream wires (`docs/design/architecture.md` and `docs/design/providers.md` and
//! `docs/reference/inference.md`) — beside their shared mid-stream error frame helper
//! ([`error_data_frame`], rendering through [`crate::api::error::gemini`], `docs/reference/
//! inference.md`), response-format decision, usage fold and split, tool-output name resolution, and
//! the thought-step family predicate deciding replay membership for both codecs
//! (`docs/reference/inference.md`).

pub(crate) mod generate_content;
pub(crate) mod interaction;

use crate::ir::{InvalidRequest, IrPart, IrRequest, IrResponseFormat, IrUsage};
use serde::Deserialize;
use serde_json::Value;

/// Renders a mid-stream stream failure as the Gemini terminal SSE data frame — the one recipe
/// both Gemini codecs' SSE renderers emit when a stream aborts, shared here so the two cannot
/// drift: the Gemini error render ([`crate::api::error::gemini`]) classifies the API error into
/// the official Status envelope (`docs/reference/inference.md`), which is serialized and framed as
/// the single `data:` frame that terminates the stream.
fn error_data_frame(error: &crate::api::Error) -> String {
    let (_, envelope) = crate::api::error::gemini::render(error);

    crate::sse::format_data_frame(&crate::format::render_json(&envelope))
}

/// Resolves the function name a tool output answers, scanning the conversation for the matching
/// invocation; shared by both Gemini codecs.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when no matching tool call exists in the conversation.
fn resolve_tool_output_name(call_id: &str, ir: &IrRequest) -> Result<String, InvalidRequest> {
    for message in &ir.messages {
        for part in &message.parts {
            if let IrPart::ToolCall {
                call_id: candidate,
                name,
                ..
            } = part
                && candidate == call_id
            {
                return Ok(name.clone());
            }
        }
    }

    Err(InvalidRequest::field(
        "messages",
        format!("Tool output references unknown call_id `{call_id}`"),
    ))
}

/// Step type tag of a model reasoning step in the official Interactions vocabulary
/// (ai.google.dev/api/interactions-api): the protocol-pinned discriminant both Gemini codecs must
/// agree on, because an opaque replay payload carrying it is the credential for Gemini-family
/// membership on either wire (`docs/reference/inference.md`).
const STEP_THOUGHT: &str = "thought";

/// Answers the Gemini-family membership of a reasoning block's opaque replay payload: an object
/// whose `type` is [`STEP_THOUGHT`] is Gemini-family state, anything else — cross-family blocks,
/// text-only blocks — is not.
///
/// The predicate is the one family membership check of both Gemini codecs, shared so the two can
/// never drift: the Interactions codec replays an accepted payload verbatim onto its own wire,
/// while the GenerateContent codec bridges the same payload onto its thinking part
/// (`thought: true` text, the signature riding `thoughtSignature`) — the
/// `docs/reference/inference.md` bridge only round-trips if both codecs answer membership
/// identically, and one shared predicate makes a one-directional drift unrepresentable.
///
/// The probe is a typed struct rather than a member lookup, so the payload is modelled, never
/// walked ad hoc.
fn is_gemini_thought_step(opaque: &Value) -> bool {
    /// Minimal shape probe for the family-membership check.
    #[derive(Debug, Deserialize)]
    struct ThoughtProbe {
        /// Step type tag; anything but the thought tag is a foreign block.
        #[serde(rename = "type")]
        step_type: String,
    }

    // Deserializing from the borrowed value copies only the tag String, never the stored step.
    ThoughtProbe::deserialize(opaque).is_ok_and(|probe| probe.step_type == STEP_THOUGHT)
}

/// The synthesized name of a schema-bearing Gemini-family response format: neither Gemini wire
/// surface has a schema-name slot, so the IR's named-schema form carries this stand-in.
const GEMINI_RESPONSE_SCHEMA_NAME: &str = "response";

/// The reason a Gemini-family response format is rejected: one of the two failure cells of the
/// family decision table, which each surface renders into its own wire vocabulary.
#[derive(Debug)]
enum GeminiResponseFormatRejection {
    /// A schema was given for `text/plain`, whose output cannot carry one.
    SchemaWithoutJsonMime,

    /// The MIME type is not one the gateway renders.
    UnsupportedMime(String),
}

/// Decides the response format shared by both Gemini codecs over a present MIME type:
/// `text/plain` is free-form text, `application/json` is JSON — named-schema under
/// [`GEMINI_RESPONSE_SCHEMA_NAME`] when a schema rides beside it — and a schema without
/// `application/json` or any other MIME type is rejected. The absent-MIME-type cells are each
/// surface's own: GenerateContent treats an absent `responseMimeType` as no response format at
/// all, and Interactions defaults the `text` member's `mime_type` to `text/plain` before
/// delegating here.
///
/// # Errors
///
/// Returns [`GeminiResponseFormatRejection`] for a schema beside a MIME type that cannot carry
/// one and for a MIME type the gateway does not render.
fn parse_gemini_response_format(
    mime_type: &str,
    schema: Option<&Value>,
) -> Result<Option<IrResponseFormat>, GeminiResponseFormatRejection> {
    match (mime_type, schema) {
        ("text/plain", None) => Ok(Some(IrResponseFormat::Text)),

        ("application/json", None) => Ok(Some(IrResponseFormat::Json)),

        ("application/json", Some(schema)) => Ok(Some(IrResponseFormat::JsonSchema {
            name: GEMINI_RESPONSE_SCHEMA_NAME.to_string(),
            schema: schema.clone(),
        })),

        ("text/plain", Some(_)) => Err(GeminiResponseFormatRejection::SchemaWithoutJsonMime),

        (other, _) => Err(GeminiResponseFormatRejection::UnsupportedMime(
            other.to_string(),
        )),
    }
}

/// Folds a Gemini-family upstream usage report into [`IrUsage`], shared by both Gemini codecs.
///
/// The cross-family invariant: IR output includes reasoning — the reported non-reasoning output
/// plus the thought tokens — while the reasoning breakdown rides beside it, the cached count rides
/// beside the input, and Gemini reports no cache-write accounting, so
/// [`cache_write_tokens`](IrUsage::cache_write_tokens) is always [`None`]. The fold adds with
/// `saturating_add`: for every token count whose sum fits `u64` — every count a real upstream
/// reports — the rendered total is the exact sum, and a third-party total can never panic (debug)
/// or wrap (release) the parser the way a plain add could at overflow.
fn fold_gemini_usage(
    input_tokens: u64,
    output_tokens_excluding_reasoning: u64,
    reasoning_tokens: Option<u64>,
    cached_tokens: Option<u64>,
) -> IrUsage {
    IrUsage {
        input_tokens,
        output_tokens: output_tokens_excluding_reasoning
            .saturating_add(reasoning_tokens.unwrap_or(0)),
        cached_tokens,
        cache_write_tokens: None,
        reasoning_tokens,
    }
}

/// Splits an IR usage report back into the Gemini usage members — the inverse of
/// [`fold_gemini_usage`], shared by both Gemini client renderers. The IR output total includes
/// reasoning (the fold's own invariant), so the split recovers the non-reasoning output by
/// subtracting the thought tokens — `saturating_sub`, the mirror of the fold's `saturating_add`
/// — restoring exactly the members the fold consumed; each renderer rebuilds its wire members
/// from the components and renders its official total as the sum of the members it emits
/// (`docs/reference/inference.md`). The components line up position-for-position with the fold's
/// inputs.
fn split_gemini_usage(usage: &IrUsage) -> (u64, u64, Option<u64>, Option<u64>) {
    let reasoning_tokens = usage.reasoning_tokens.unwrap_or(0);

    (
        usage.input_tokens,
        usage.output_tokens.saturating_sub(reasoning_tokens),
        usage.reasoning_tokens,
        usage.cached_tokens,
    )
}
