//! The OpenAI protocol's two codec families and their shared items.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module groups the Chat Completions family ([`chat_completion`]) and
//! the Responses family ([`response`]) — the two codecs over the OpenAI client protocol and the
//! two OpenAI upstream wires (README §1, §6) — beside the OpenAI error renderer ([`error`],
//! README §6.7) both outward surfaces render over, the `tool_choice` mode vocabulary's inbound
//! parse ([`parse_tool_choice_mode`]) and outbound render ([`render_tool_choice`]), and the
//! official usage detail objects ([`PromptTokensDetails`], [`CompletionTokensDetails`]) both
//! codecs share.

pub(crate) mod chat_completion;
pub(crate) mod error;
pub(crate) mod response;

use crate::ir::{InvalidRequest, IrToolChoice};
use serde::{Deserialize, Serialize};

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

/// The official input-side usage detail object both OpenAI products speak (README §6.2): Chat
/// Completions carries it as `prompt_tokens_details`, Responses as `input_tokens_details`. Its
/// official member set is `cached_tokens`, the input tokens served from the upstream prompt cache,
/// plus `cache_write_tokens`, an official member some upstreams omit; every member is optional and
/// the parsing codec reads each member the upstream carries.
#[derive(Debug, Default, Deserialize)]
pub(in crate::format::openai) struct PromptTokensDetails {
    /// Input tokens served from the upstream prompt cache.
    #[serde(default)]
    cached_tokens: Option<u64>,

    /// Input tokens written to the upstream prompt cache; an official member some upstreams omit.
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

/// The official output-side usage detail object both OpenAI products speak (README §6.2): Chat
/// Completions carries it as `completion_tokens_details`, Responses as `output_tokens_details`.
/// Its official member set is `reasoning_tokens`, the output tokens spent on model reasoning, and
/// every member is optional.
#[derive(Debug, Default, Deserialize)]
pub(in crate::format::openai) struct CompletionTokensDetails {
    /// Output tokens spent on model reasoning.
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}
