//! The OpenAI protocol's two codec families and their shared items.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module groups the Chat Completions family ([`chat_completion`]) and
//! the Responses family ([`response`]) — the two codecs over the OpenAI client protocol and the
//! two OpenAI upstream wires (`docs/design/architecture.md` and `docs/reference/inference.md`; the
//! outward error render both surfaces share lives in [`crate::api::error::openai`],
//! `docs/reference/inference.md`) — with the `tool_choice` mode vocabulary's inbound parse
//! ([`parse_tool_choice_mode`]) and outbound render ([`render_tool_choice`]), and the official
//! usage detail objects ([`PromptTokensDetails`], [`CompletionTokensDetails`]) both codecs share.

pub(crate) mod chat_completion;
pub(crate) mod response;

use crate::ir::{InvalidRequest, IrToolChoice};
use serde::{Deserialize, Serialize};

/// Parses an OpenAI-family `tool_choice` mode name; shared by both codecs.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` for unknown modes.
fn parse_tool_choice_mode(mode: &str) -> Result<IrToolChoice, InvalidRequest> {
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
enum ToolChoiceMode {
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
enum ToolChoice<NamedToolChoice> {
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
fn render_tool_choice<'a, NamedToolChoice>(
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

/// The official input-side usage detail object both OpenAI products speak
/// (`docs/reference/inference.md`): Chat Completions carries it as `prompt_tokens_details`,
/// Responses as `input_tokens_details`. Its official member set is `cached_tokens`, the input
/// tokens served from the upstream prompt cache, plus `cache_write_tokens`, an official member some
/// upstreams omit; every member is optional and the parsing codec reads each member the upstream
/// carries.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PromptTokensDetails {
    /// Input tokens served from the upstream prompt cache.
    #[serde(default)]
    pub(crate) cached_tokens: Option<u64>,

    /// Input tokens written to the upstream prompt cache; an official member some upstreams omit.
    #[serde(default)]
    pub(crate) cache_write_tokens: Option<u64>,
}

/// The official output-side usage detail object both OpenAI products speak
/// (`docs/reference/inference.md`): Chat Completions carries it as `completion_tokens_details`,
/// Responses as `output_tokens_details`. Its official member set is `reasoning_tokens`, the output
/// tokens spent on model reasoning, and every member is optional.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CompletionTokensDetails {
    /// Output tokens spent on model reasoning.
    #[serde(default)]
    pub(crate) reasoning_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    //! Tests for the OpenAI protocol's shared items: the `tool_choice` mode vocabulary's inbound
    //! parse and outbound render, and the official usage detail objects.

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{
        CompletionTokensDetails, PromptTokensDetails, parse_tool_choice_mode, render_tool_choice,
    };
    use crate::ir::IrToolChoice;
    use serde_json::json;
    use std::error::Error as StdError;

    /// The three shared modes parse onto their IR variants and any other string is rejected
    /// naming `tool_choice`.
    #[test]
    fn tool_choice_mode_parse() -> Result<(), Box<dyn StdError>> {
        assert_eq!(parse_tool_choice_mode("auto")?, IrToolChoice::Auto);
        assert_eq!(parse_tool_choice_mode("none")?, IrToolChoice::None);
        assert_eq!(parse_tool_choice_mode("required")?, IrToolChoice::Required);

        let unknown = parse_tool_choice_mode("mandatory")
            .err()
            .ok_or("the unknown mode must be rejected")?;

        assert_eq!(unknown.param.clone().unwrap_or_default(), "tool_choice");
        assert!(
            unknown
                .to_string()
                .contains("Unknown tool_choice `mandatory`")
        );

        Ok(())
    }

    /// The mode render is the exact inverse of the parse, and the named-function form's wire shape
    /// is built by the rendering codec: flat for Responses, nested for Chat Completions.
    #[test]
    fn tool_choice_render_shapes() -> Result<(), Box<dyn StdError>> {
        let required =
            serde_json::to_value(render_tool_choice(&IrToolChoice::Required, |_name| {
                json!(null)
            }))?;

        assert_eq!(required, json!("required"));

        let nested = serde_json::to_value(render_tool_choice(
            &IrToolChoice::Function {
                name: "find".to_string(),
            },
            |name| json!({"type": "function", "function": {"name": name}}),
        ))?;

        assert_eq!(
            nested,
            json!({"type": "function", "function": {"name": "find"}})
        );

        let flat = serde_json::to_value(render_tool_choice(
            &IrToolChoice::Function {
                name: "find".to_string(),
            },
            |name| json!({"type": "function", "name": name}),
        ))?;

        assert_eq!(flat, json!({"type": "function", "name": "find"}));

        Ok(())
    }

    /// The shared usage detail objects parse the members the upstream carries and default every
    /// absent member, so an upstream reporting totals only stays acceptable.
    #[test]
    fn usage_detail_objects_default_members() -> Result<(), Box<dyn StdError>> {
        let bare: PromptTokensDetails = serde_json::from_value(json!({}))?;

        assert_eq!(bare.cached_tokens, None);
        assert_eq!(bare.cache_write_tokens, None);

        let full: PromptTokensDetails = serde_json::from_value(json!({
            "cached_tokens": 3,
            "cache_write_tokens": 2
        }))?;

        assert_eq!(full.cached_tokens, Some(3));
        assert_eq!(full.cache_write_tokens, Some(2));

        let reasoning: CompletionTokensDetails =
            serde_json::from_value(json!({"reasoning_tokens": 4}))?;

        assert_eq!(reasoning.reasoning_tokens, Some(4));

        let bare_output: CompletionTokensDetails = serde_json::from_value(json!({}))?;

        assert_eq!(bare_output.reasoning_tokens, None);

        Ok(())
    }
}
