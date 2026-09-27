//! The Gemini protocol's two codec families and their shared items.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module groups the GenerateContent family ([`generate_content`]) and
//! the Interactions family ([`interaction`]) — the two codecs over the Gemini client protocol and
//! the two Gemini upstream wires (README §1, §5.7, §6) — beside their shared error renderer
//! ([`error`], README §6.7), response-format decision, usage fold and split, and tool-output name
//! resolution.

pub(crate) mod error;
pub(crate) mod generate_content;
pub(crate) mod interaction;

use crate::ir::{InvalidRequest, IrPart, IrRequest, IrResponseFormat, IrUsage};
use serde_json::Value;

/// Resolves the function name a tool output answers, scanning the conversation for the matching
/// invocation; shared by both Gemini codecs.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when no matching tool call exists in the conversation.
pub(in crate::format::gemini) fn resolve_tool_output_name(
    call_id: &str,
    ir: &IrRequest,
) -> Result<String, InvalidRequest> {
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

/// The synthesized name of a schema-bearing Gemini-family response format: neither Gemini wire
/// surface has a schema-name slot, so the IR's named-schema form carries this stand-in.
const GEMINI_RESPONSE_SCHEMA_NAME: &str = "response";

/// The reason a Gemini-family response format is rejected: one of the two failure cells of the
/// family decision table, which each surface renders into its own wire vocabulary.
#[derive(Debug)]
pub(in crate::format::gemini) enum GeminiResponseFormatRejection {
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
pub(in crate::format::gemini) fn parse_gemini_response_format(
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
pub(in crate::format::gemini) fn fold_gemini_usage(
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
/// (README §6.3). The components line up position-for-position with the fold's inputs.
pub(in crate::format::gemini) fn split_gemini_usage(
    usage: &IrUsage,
) -> (u64, u64, Option<u64>, Option<u64>) {
    let reasoning_tokens = usage.reasoning_tokens.unwrap_or(0);

    (
        usage.input_tokens,
        usage.output_tokens.saturating_sub(reasoning_tokens),
        usage.reasoning_tokens,
        usage.cached_tokens,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split is the fold's inverse: folding the Gemini members into an IR usage and splitting
    /// it back restores them exactly, with and without reported reasoning, and a usage whose
    /// output total falls below its reported reasoning count clamps the non-reasoning remainder
    /// at zero — the only direction the split can subtract.
    ///
    /// # Panics
    ///
    /// Panics if the split diverges from the fold's inverse.
    #[test]
    fn split_gemini_usage_inverts_the_fold() {
        let folded = fold_gemini_usage(10, 5, Some(1), Some(2));

        assert_eq!(split_gemini_usage(&folded), (10, 5, Some(1), Some(2)));

        let unreasoned = fold_gemini_usage(10, 6, None, None);

        assert_eq!(split_gemini_usage(&unreasoned), (10, 6, None, None));

        // A third-party usage reporting more reasoning than its output total carries: the
        // non-reasoning remainder clamps at zero, never underflows.
        let clamped = IrUsage {
            input_tokens: 4,
            output_tokens: 2,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: Some(5),
        };

        assert_eq!(split_gemini_usage(&clamped), (4, 0, Some(5), None));
    }
}
