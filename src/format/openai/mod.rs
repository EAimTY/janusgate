//! The OpenAI protocol's two codec families and their shared items.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module groups the Chat Completions family ([`chat_completion`]) and
//! the Responses family ([`response`]) — the two codecs over the OpenAI client protocol and the
//! two OpenAI upstream wires (README §1, §6) — beside the OpenAI error renderer ([`error`],
//! README §6.7) both outward surfaces render over and the `tool_choice` mode parsing both
//! request parsers share.

pub(crate) mod chat_completion;
pub(crate) mod error;
pub(crate) mod response;

use crate::ir::{InvalidRequest, IrToolChoice};

/// Parses an OpenAI-family `tool_choice` mode name; shared by both codecs.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` for unknown modes.
pub(in crate::format::openai) fn parse_tool_choice_mode(
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
