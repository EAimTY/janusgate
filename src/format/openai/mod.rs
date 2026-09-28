//! The OpenAI protocol's two codec families and their shared items.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module groups the Chat Completions family ([`chat_completion`]) and
//! the Responses family ([`response`]) — the two codecs over the OpenAI client protocol and the
//! two OpenAI upstream wires (README §1, §6) — beside the OpenAI error renderer ([`error`],
//! README §6.7) both outward surfaces render over and the `tool_choice` mode vocabulary's inbound
//! parse ([`parse_tool_choice_mode`]) and outbound render ([`render_tool_choice`]) both codecs
//! share.

pub(crate) mod chat_completion;
pub(crate) mod error;
pub(crate) mod response;

use crate::ir::{InvalidRequest, IrToolChoice};
use serde::Serialize;

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

/// The `tool_choice` mode strings in the upstream shape: the outbound spelling of the mode
/// vocabulary [`parse_tool_choice_mode`] parses inbound.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::format::openai) enum ToolChoiceMode {
    /// The model decides freely.
    Auto,

    /// The model may not call tools.
    None,

    /// The model must call a tool.
    Required,
}

/// A `tool_choice` restriction in the upstream shape: a bare mode string or a named-function
/// object.
///
/// The named-function object's wire shape is per-codec — Chat Completions nests the name
/// (`{"type": "function", "function": {"name": ...}}`) while Responses keeps it flat
/// (`{"type": "function", "name": ...}`) — so the object is the rendering codec's own type.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(in crate::format::openai) enum ToolChoice<NamedToolChoice> {
    /// The mode-string form.
    Mode(ToolChoiceMode),

    /// The named-function form.
    Function(NamedToolChoice),
}

/// Renders an IR `tool_choice` into the upstream shape: the outbound half of
/// [`parse_tool_choice_mode`]. The three shared modes map onto [`ToolChoiceMode`] here; the
/// named-function form's wire shape is per-codec, so its object is built by `named` from the
/// required function name.
#[must_use]
pub(in crate::format::openai) fn render_tool_choice<'a, NamedToolChoice>(
    tool_choice: &'a IrToolChoice,
    named: impl FnOnce(&'a str) -> NamedToolChoice,
) -> ToolChoice<NamedToolChoice> {
    match tool_choice {
        IrToolChoice::Auto => ToolChoice::Mode(ToolChoiceMode::Auto),
        IrToolChoice::None => ToolChoice::Mode(ToolChoiceMode::None),
        IrToolChoice::Required => ToolChoice::Mode(ToolChoiceMode::Required),
        IrToolChoice::Function { name } => ToolChoice::Function(named(name)),
    }
}
