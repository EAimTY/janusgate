//! Protocol codecs connecting client and upstream wire formats through the IR.
//!
//! The tree is organized by protocol: this root holds the items more than one protocol family
//! consumes plus the two cross-layer vocabulary types ([`UpstreamFormat`], which names all five
//! families for the provider, storage, and gateway layers, and [`RenderOption`], which types every
//! provider definition's render options), each protocol parent ([`anthropic`], [`gemini`],
//! [`openai`]) holds the families speaking that one client/upstream protocol
//! (`docs/design/architecture.md` and `docs/reference/inference.md`) plus the items its families
//! share, and each family directory splits by concern (`request_parse`, `client_render`,
//! `upstream_render`, `upstream_parse`, and the pooled-projection pair `affinity` for the streaming
//! render points and `json_affinity` for the aggregated body; families with a compaction channel
//! add their compaction modules beside the pair).
//!
//! Two contracts are declared once here. Every outward format implements [`ClientFormat`], which
//! parses client requests and boxes a [`ClientSseRenderer`] for live streaming (aggregated
//! rendering is a free function behind the same trait). Every upstream format implements
//! [`UpstreamEventParser`]. Upstream payload rendering has no shared contract — the payloads are
//! provider-coupled — so each codec exposes free body-render functions that the gateway invokes
//! directly; each renders the upstream body alone, and the dispatch's URL, header rows, and
//! streaming decision come from the provider definition's inference descriptor
//! (`docs/design/providers.md`), evaluated by the gateway dispatcher around the rendered body.
//!
//! All five families are bidirectional (`docs/reference/inference.md`): each implements
//! [`UpstreamEventParser`] for its same-named upstream protocol plus the free request renderers
//! beside it, and each implements [`ClientFormat`] as the outward client surface served under that
//! protocol's name — `openai_response`, `openai_chat_completion`, `gemini_generate_content`,
//! `gemini_interaction`, and `anthropic_messages`. No family is upstream-parse-only; every family
//! both speaks its upstream protocol and serves its outward surface.

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

pub(crate) mod affinity;
pub(crate) mod anthropic;
pub(crate) mod compaction_wire;
pub(crate) mod gemini;
pub(crate) mod openai;

#[cfg(test)]
mod tests {
    //! Tests for the shared format machinery: the cross-layer vocabulary tables, the role-run
    //! accumulator, the once-only event helpers, the aggregation synthesis, the string-or-array
    //! and upstream-error wire shapes, and the strict parse and reject helpers every codec shares.

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{
        PayloadError, RenderOption, RoleRun, StringOrArray, UpstreamError, UpstreamFormat,
        compaction_control::ThresholdEdit,
    };
    use crate::{
        ir::{
            InvalidRequest, IrAggregatedToolCall, IrAggregation, IrEvent, IrFinishReason,
            IrMessage, IrPart, IrReasoningBlock, IrRole, IrUsage,
            compaction::ThresholdEdit as IrThresholdEdit,
        },
        sse::SseFrame,
    };
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::error::Error as StdError;

    /// Builds one text part carrying the given text.
    fn text_part(text: &str) -> IrPart {
        IrPart::Text {
            text: text.to_string(),
        }
    }

    /// Builds one tool-output part answering the given invocation.
    fn tool_output_part(call_id: &str, content: &str) -> IrPart {
        IrPart::ToolOutput {
            call_id: call_id.to_string(),
            content: content.to_string(),
            is_error: false,
        }
    }

    /// Extracts a validation rejection, failing the test when the fixture unexpectedly parsed.
    fn rejected<T>(result: Result<T, InvalidRequest>) -> Result<InvalidRequest, Box<dyn StdError>> {
        result
            .err()
            .ok_or("the fixture must have been rejected".into())
    }

    /// Every format's canonical wire name parses back onto its variant, and no other name parses.
    #[test]
    fn upstream_format_wire_names_round_trip() -> Result<(), Box<dyn StdError>> {
        for format in [
            UpstreamFormat::OpenAiResponse,
            UpstreamFormat::OpenAiChatCompletion,
            UpstreamFormat::GeminiGenerateContent,
            UpstreamFormat::GeminiInteraction,
            UpstreamFormat::AnthropicMessages,
        ] {
            assert_eq!(UpstreamFormat::parse(format.as_str()), Some(format));

            let wire: UpstreamFormat = serde_json::from_value(json!(format.as_str()))?;
            assert_eq!(wire, format);
        }

        assert_eq!(UpstreamFormat::parse("openai_responses"), None);

        Ok(())
    }

    /// The render option's wire spelling matches its canonical name.
    #[test]
    fn render_option_wire_name() -> Result<(), Box<dyn StdError>> {
        assert_eq!(
            RenderOption::OmitMaxOutputTokens.as_str(),
            "omit_max_output_tokens"
        );

        let option: RenderOption = serde_json::from_value(json!("omit_max_output_tokens"))?;
        assert_eq!(option, RenderOption::OmitMaxOutputTokens);

        Ok(())
    }

    /// The role-run accumulator merges same-role contributions, splits on a role change, and
    /// finishes the open run into messages.
    #[test]
    fn role_run_merges_and_splits_runs() {
        let mut runs: RoleRun<IrRole, IrPart> = RoleRun::default();

        runs.push(IrRole::User, vec![text_part("a")]);
        runs.push(IrRole::User, vec![text_part("b")]);
        runs.push(IrRole::Tool, vec![tool_output_part("c1", "r1")]);
        runs.push(IrRole::Assistant, vec![text_part("x")]);

        let messages = runs.finish();

        assert_eq!(
            messages,
            vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![text_part("a"), text_part("b")],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![tool_output_part("c1", "r1")],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![text_part("x")],
                },
            ]
        );
    }

    /// A contribution carrying no parts is transparent: it neither extends nor splits the open
    /// run, so same-role contributions merge across it.
    #[test]
    fn role_run_treats_empty_contributions_as_transparent() {
        let mut runs: RoleRun<IrRole, IrPart> = RoleRun::default();

        runs.push(IrRole::Tool, vec![tool_output_part("c1", "r1")]);
        runs.push(IrRole::User, Vec::new());
        runs.push(IrRole::Assistant, Vec::new());
        runs.push(IrRole::Tool, vec![tool_output_part("c2", "r2")]);

        assert_eq!(
            runs.into_runs(),
            vec![(
                IrRole::Tool,
                vec![tool_output_part("c1", "r1"), tool_output_part("c2", "r2")]
            )]
        );
    }

    /// The degraded finish emits exactly one stop finish on the first call for an unfinished
    /// parser and never again.
    #[test]
    fn degrade_finish_emits_stop_once() {
        let mut is_finished = false;

        assert_eq!(
            super::degrade_finish_once(&mut is_finished),
            vec![IrEvent::finish(IrFinishReason::Stop)]
        );

        assert!(is_finished);
        assert!(super::degrade_finish_once(&mut is_finished).is_empty());

        let mut already_finished = true;

        assert!(super::degrade_finish_once(&mut already_finished).is_empty());
    }

    /// The once-only message start emits exactly one opening event carrying the response identity.
    #[test]
    fn message_start_emitted_exactly_once() {
        let mut is_started = false;

        assert_eq!(
            super::message_start_once(&mut is_started, "up-1".to_string(), "m".to_string()),
            vec![IrEvent::MessageStart {
                id: "up-1".to_string(),
                model: "m".to_string(),
            }]
        );

        assert!(
            super::message_start_once(&mut is_started, "again".to_string(), String::new())
                .is_empty()
        );
    }

    /// The complete-call announcement is the canonical delta-plus-done pair carrying identical
    /// payloads, with the signature only on the done.
    #[test]
    fn complete_call_pair_carries_identical_payloads() {
        let [delta, done] = super::complete_call_events(
            2,
            "call_9".to_string(),
            "find".to_string(),
            "{\"q\":1}".to_string(),
            Some("sig".to_string()),
        );

        assert_eq!(
            delta,
            IrEvent::ToolCallDelta {
                index: 2,
                call_id: Some("call_9".to_string()),
                name: Some("find".to_string()),
                arguments_delta: "{\"q\":1}".to_string(),
            }
        );

        assert_eq!(
            done,
            IrEvent::ToolCallDone {
                index: 2,
                call_id: "call_9".to_string(),
                name: "find".to_string(),
                arguments: "{\"q\":1}".to_string(),
                thought_signature: Some("sig".to_string()),
            }
        );
    }

    /// The aggregation synthesis emits every present stage in canonical order: opening, compaction
    /// states, reasoning blocks, text, tool calls, finish, compaction metadata, usage, terminal.
    #[test]
    fn aggregation_events_render_in_canonical_order() {
        let aggregation = IrAggregation {
            text: "hi".to_string(),
            reasoning_blocks: vec![IrReasoningBlock {
                text: "think".to_string(),
                signature: None,
                opaque: Value::Null,
            }],
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "find".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            }],
            finish: Some(IrFinishReason::Stop),
            usage: Some(IrUsage::default()),
            is_compaction_paused: true,
            compaction_states: vec![json!({"native": true})],
            ..IrAggregation::default()
        };

        let events = super::aggregation_events(&aggregation, "model-x");

        assert!(matches!(
            &events[..],
            [
                IrEvent::MessageStart { id, model },
                IrEvent::CompactionState { payload },
                IrEvent::ReasoningDone { text, .. },
                IrEvent::TextDelta(delta),
                IrEvent::ToolCallDone { index: 0, .. },
                IrEvent::Finish {
                    reason: IrFinishReason::Stop,
                    ..
                },
                IrEvent::CompactionMetadata {
                    is_paused: true,
                    ..
                },
                IrEvent::Usage(_),
                IrEvent::Done,
            ]
            if model == "model-x" && !id.is_empty()
                && payload == &json!({"native": true})
                && text == "think" && delta == "hi"
        ));
        assert_eq!(events.len(), 9);
    }

    /// The aggregation synthesis omits every absent stage and the compaction metadata when no
    /// compaction lifecycle participated, but always opens and terminates the stream.
    #[test]
    fn aggregation_events_skip_absent_stages() {
        let events = super::aggregation_events(&IrAggregation::default(), "m");

        assert!(matches!(
            &events[..],
            [IrEvent::MessageStart { model, .. }, IrEvent::Done] if model == "m"
        ));

        let usage_only = IrAggregation {
            usage: Some(IrUsage::default()),
            ..IrAggregation::default()
        };

        assert!(matches!(
            &super::aggregation_events(&usage_only, "m")[..],
            [
                IrEvent::MessageStart { .. },
                IrEvent::Usage(_),
                IrEvent::Done
            ]
        ));
    }

    /// The string-or-array shape accepts both forms.
    #[test]
    fn string_or_array_accepts_both_shapes() -> Result<(), Box<dyn StdError>> {
        /// Deserialization fixture carrying one string-or-array member.
        #[derive(Debug, Deserialize)]
        struct Fixture {
            /// The member under test.
            content: StringOrArray<String>,
        }

        let text: Fixture = serde_json::from_str("{\"content\": \"plain\"}")?;
        assert!(matches!(text.content, StringOrArray::Text(text) if text == "plain"));

        let array: Fixture = serde_json::from_str("{\"content\": [\"a\", \"b\"]}")?;
        assert!(matches!(array.content, StringOrArray::Array(items) if items.len() == 2));

        Ok(())
    }

    /// A failing array element propagates its own rejection, naming the offending member, instead
    /// of collapsing into a shape mismatch.
    #[test]
    fn string_or_array_propagates_item_rejections() -> Result<(), Box<dyn StdError>> {
        /// Deserialization fixture requiring one member of every element.
        #[derive(Debug, Deserialize)]
        struct ItemFixture {
            /// Required member of every element, never read: the rejection is the subject.
            #[expect(dead_code, reason = "read through serde only")]
            name: String,
        }

        /// Deserialization fixture carrying one array of strict items.
        #[derive(Debug, Deserialize)]
        struct Fixture {
            /// The member under test, never read: the rejection is the subject.
            #[expect(dead_code, reason = "read through serde only")]
            content: StringOrArray<ItemFixture>,
        }

        let error = serde_json::from_str::<Fixture>("{\"content\": [{\"name\": \"a\"}, {}]}")
            .err()
            .ok_or("the empty item must be rejected")?;

        assert!(error.to_string().contains("missing field `name`"));
        assert!(!error.to_string().contains("did not match any variant"));

        Ok(())
    }

    /// Every embedded upstream error shape parses, a present-but-null member is success, and the
    /// failure detail carries the text and code.
    #[test]
    fn upstream_error_forms_and_failure_detail() -> Result<(), Box<dyn StdError>> {
        /// Deserialization fixture carrying an optional embedded upstream error.
        #[derive(Debug, Deserialize)]
        struct Fixture {
            /// The embedded error member.
            error: Option<UpstreamError>,
        }

        let absent: Fixture = serde_json::from_str("{}")?;
        assert!(absent.error.is_none());

        let null: Fixture = serde_json::from_str("{\"error\": null}")?;
        assert!(null.error.is_none());

        let text: Fixture = serde_json::from_str("{\"error\": \"plain\"}")?;
        assert!(matches!(text.error, Some(UpstreamError::Text(_))));
        assert_eq!(
            text.error.map(|error| error.failure().to_string()),
            Some("plain".to_string())
        );

        let object: Fixture =
            serde_json::from_str("{\"error\": {\"message\": \"boom\", \"code\": \"x\"}}")?;
        assert!(matches!(object.error, Some(UpstreamError::Object { .. })));
        assert_eq!(
            object.error.map(|error| error.failure().to_string()),
            Some("boom (upstream error code: \"x\")".to_string())
        );

        let codeless: Fixture = serde_json::from_str("{\"error\": {\"message\": \"boom\"}}")?;
        assert_eq!(
            codeless.error.map(|error| error.failure().to_string()),
            Some("boom".to_string())
        );

        let raw: Fixture = serde_json::from_str("{\"error\": [\"odd\"]}")?;
        assert_eq!(
            raw.error.map(|error| error.failure().to_string()),
            Some("[\"odd\"]".to_string())
        );

        Ok(())
    }

    /// The strict body parser rejects unknown members and malformed JSON, and both rejections
    /// carry no parameter name.
    #[test]
    fn strict_body_parse_rejections() -> Result<(), Box<dyn StdError>> {
        /// Deserialization fixture for the strict body parser.
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fixture {
            /// Required member.
            model: String,
        }

        let unknown = rejected(super::parse_client_request::<Fixture>(
            b"{\"model\": \"m\", \"extra\": 1}",
        ))?;

        assert_eq!(unknown.param, None);
        assert!(unknown.to_string().contains("unknown field `extra`"));

        let malformed = rejected(super::parse_client_request::<Fixture>(b"{\"model\": "))?;

        assert_eq!(malformed.param, None);
        assert!(malformed.to_string().starts_with("Invalid JSON body:"));

        assert_eq!(
            super::parse_client_request::<Fixture>(b"{\"model\": \"m\"}")?.model,
            "m"
        );

        Ok(())
    }

    /// Tool arguments parse as objects, treat blanks as empty objects, and reject anything else
    /// naming `arguments`.
    #[test]
    fn arguments_as_object_reject_policy() -> Result<(), Box<dyn StdError>> {
        assert_eq!(
            super::arguments_as_object("   ").unwrap_or_default().len(),
            0
        );

        let object = super::arguments_as_object("{\"a\": 1}").unwrap_or_default();
        assert_eq!(object.len(), 1);

        for malformed in ["[]", "{\"a\": 1", "null"] {
            let error = rejected(super::arguments_as_object(malformed))?;

            assert_eq!(error.param.clone().unwrap_or_default(), "arguments");
        }

        Ok(())
    }

    /// The render-time arguments degrade policy falls back to the empty object instead of failing
    /// the stream.
    #[test]
    fn arguments_render_degrades_to_empty_object() {
        assert!(super::arguments_to_object_or_empty("[]").is_empty());
        assert!(super::arguments_to_object_or_empty("not json").is_empty());
        assert_eq!(super::arguments_to_object_or_empty("{\"a\": 1}").len(), 1);
    }

    /// Text parts join with the blank-line separator, skipping only a leading run of empty parts.
    #[test]
    fn text_parts_join_with_blank_line_separator() {
        assert_eq!(super::join_text_parts([]), "");
        assert_eq!(super::join_text_parts(["", ""]), "");
        assert_eq!(super::join_text_parts(["", "", "a", "", "b"]), "a\n\n\n\nb");
        assert_eq!(super::join_text_parts(["a", "b"]), "a\n\nb");
    }

    /// Frame and body parsing accept valid upstream payloads and reject malformed ones with the
    /// shared failure wording.
    #[test]
    fn upstream_payload_parse() -> Result<(), Box<dyn StdError>> {
        let value = super::parse_frame::<Value>(&SseFrame {
            data: "{\"x\": 1}".to_string(),
            event: None,
        })?;

        assert_eq!(value, json!({"x": 1}));

        let frame_error = rejected_frame(super::parse_frame::<Value>(&SseFrame {
            data: "not json".to_string(),
            event: None,
        }))?;

        assert_eq!(frame_error, "malformed upstream payload in SSE frame");

        let body = super::parse_body::<Value>(b"{\"y\": 2}")?;

        assert_eq!(body, json!({"y": 2}));

        let body_error = rejected_frame(super::parse_body::<Value>(b"nope"))?;

        assert!(body_error.starts_with("malformed upstream payload: "));

        Ok(())
    }

    /// Extracts a payload-parse failure message, failing the test when the payload parsed.
    fn rejected_frame<T>(result: Result<T, PayloadError>) -> Result<String, Box<dyn StdError>> {
        result
            .err()
            .map(|error| error.to_string())
            .ok_or("the payload must have been rejected".into())
    }

    /// The primary-entry selection prefers the predicate match and falls back to the first entry.
    #[test]
    fn primary_selection_prefers_predicate_then_first() {
        let entries = [10, 20, 30];

        assert_eq!(super::primary(&entries, |entry| *entry == 20), Some(&20));
        assert_eq!(super::primary(&entries, |_| false), Some(&10));

        let empty: [i32; 0] = [];

        assert!(super::primary(&empty, |entry: &i32| entry % 2 == 1).is_none());
    }

    /// The shared OpenAI-family rejections name their parameter and context.
    #[test]
    fn shared_openai_rejections() -> Result<(), Box<dyn StdError>> {
        assert!(super::reject_non_function_type("function", "tool").is_ok());

        let error = rejected(super::reject_non_function_type("builtin", "tool"))?;

        assert!(
            error
                .to_string()
                .contains("Unsupported tool type `builtin`")
        );
        assert_eq!(error.param.clone().unwrap_or_default(), "type");

        let choice_error = rejected(super::reject_non_function_type("builtin", "tool_choice"))?;

        assert!(
            choice_error
                .to_string()
                .contains("Unsupported tool_choice type `builtin`")
        );

        let without_tools = rejected(super::reject_tool_choice_without_tools(true))?;

        assert_eq!(
            without_tools.param.clone().unwrap_or_default(),
            "tool_choice"
        );
        assert!(super::reject_tool_choice_without_tools(false).is_ok());

        Ok(())
    }

    /// The shared sampling ranges accept the official bounds and reject anything outside them,
    /// including a NaN temperature, naming the caller's nucleus-parameter spelling in a
    /// rejection.
    #[test]
    fn sampling_range_validation() -> Result<(), Box<dyn StdError>> {
        assert!(super::validate_sampling_ranges(Some(0.0), Some(1.0), "top_p").is_ok());
        assert!(super::validate_sampling_ranges(Some(2.0), Some(0.0), "top_p").is_ok());
        assert!(super::validate_sampling_ranges(None, None, "top_p").is_ok());

        let hot = rejected(super::validate_sampling_ranges(Some(2.01), None, "top_p"))?;

        assert_eq!(hot.param.clone().unwrap_or_default(), "temperature");

        let wide = rejected(super::validate_sampling_ranges(None, Some(1.01), "top_p"))?;

        assert_eq!(wide.param.clone().unwrap_or_default(), "top_p");

        let gemini_wide = rejected(super::validate_sampling_ranges(None, Some(1.01), "topP"))?;

        assert_eq!(gemini_wide.param.clone().unwrap_or_default(), "topP");

        let nan = rejected(super::validate_sampling_ranges(
            Some(f64::NAN),
            None,
            "top_p",
        ))?;

        assert_eq!(nan.param.clone().unwrap_or_default(), "temperature");

        Ok(())
    }

    /// The parsed threshold edit converts into the IR's vendor-shaped edit: the trigger maps onto
    /// the typed trigger, and the pause flag carries only when requested.
    #[test]
    fn threshold_edit_conversion() {
        let converted = IrThresholdEdit::from(&ThresholdEdit {
            trigger: Some(150_000),
            pause_after_compaction: true,
            instructions: Some("Summarize.".to_string()),
        });

        assert_eq!(
            converted.trigger.map(|trigger| trigger.value),
            Some(150_000)
        );
        assert_eq!(converted.should_pause_after_compaction, Some(true));
        assert_eq!(converted.instructions.unwrap_or_default(), "Summarize.");

        let plain = IrThresholdEdit::from(&ThresholdEdit {
            trigger: None,
            pause_after_compaction: false,
            instructions: None,
        });

        assert!(plain.trigger.is_none());
        assert_eq!(plain.should_pause_after_compaction, None);
        assert_eq!(plain.instructions, None);
    }

    /// The rendered wire payload serializes to its exact JSON text.
    #[test]
    fn rendered_json_serializes_totally() {
        assert_eq!(super::render_json(&super::EmptyObject {}), "{}");
    }
}

/// A JSON object whose declared protocol shape has no members. Render-only: the gateway
/// constructs and serializes it outward, never parsing it back.
#[derive(Debug, Clone, Serialize)]
struct EmptyObject {}

/// Wire formats an upstream provider may speak: the closed vocabulary from which a
/// provider definition pins the one format every inference of the definition speaks
/// (`docs/design/architecture.md` and `docs/design/providers.md`), whatever its [`credential
/// kind`](crate::provider::CredentialKind).
///
/// The serde and TypeScript names are the management wire names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum UpstreamFormat {
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

/// One render option of a format codec (`docs/design/providers.md`): a closed, per-format switch
/// adjusting the body rendering. Growing this enum is the documented extension path for a future
/// provider that does not fit the plain body rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum RenderOption {
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
#[derive(Debug, Clone)]
pub(crate) struct ParsedRequest {
    /// Format-independent request.
    pub(crate) ir: IrRequest,

    /// Actual client tool identity/result source facts, never sent to a model or stored in cache.
    pub(crate) affinity: affinity::Sources,

    /// Whether the client requested a streamed response.
    pub(crate) is_streaming: bool,

    /// Whether the client asked for a trailing usage-only chunk; only the Chat Completions format
    /// exposes the option, every other format includes usage unconditionally.
    pub(crate) should_include_usage: bool,

    /// Remote-compaction facts recognized in the request: the parsed controls
    /// and the ordered compaction state found in the history. `None` for the
    /// surfaces that cannot express compaction (Chat Completions and the two
    /// Gemini surfaces) and for requests carrying none; the wire codecs own
    /// its population.
    pub(crate) compaction: Option<ParsedCompaction>,
}

/// Remote-compaction facts of one parsed request, shared by the Responses and
/// Messages surfaces: the caller's compaction control, the ordered compaction
/// state markers found in the history, and the opaque state items the upstream
/// render must replay (which also ride [`IrRequest::state_items`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedCompaction {
    /// The compaction operation the caller explicitly requested; `None` for a
    /// pure replay request whose history merely carries compaction state.
    pub(crate) control: Option<CompactionControl>,

    /// Compaction state markers found in the history, in history order. A
    /// marker is either a recognized gateway carrier (sealed envelope
    /// candidate) or a native compaction item/block; each carries its IR
    /// message offset — the number of IR messages parsed before it.
    pub(crate) boundaries: Vec<CompactionBoundary>,

    /// The request's individual instruction items, in encounter order, before
    /// concatenation into [`IrRequest::instructions`]. The restoration step
    /// computes their overlap identities against a checkpoint's covered
    /// instruction identities, so unmatched current instructions survive
    /// boundary restoration while covered ones are superseded. Kept here
    /// because the IR concatenates instructions into one string.
    pub(crate) instruction_items: Vec<ParsedInstructionItem>,
}

/// One individual instruction item of a parsed request, before concatenation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedInstructionItem {
    /// The instruction's role.
    pub(crate) role: crate::compaction_model::identity::InstructionRole,

    /// The instruction text.
    pub(crate) text: String,
}

/// The compaction operation one request explicitly controls.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompactionControl {
    /// The Responses standalone compact endpoint — recorded when the request
    /// arrived through `/responses/compact`; the route owns this control, the
    /// body never states it.
    ResponsesStandalone,

    /// A Responses explicit trigger: a terminal `compaction_trigger` control
    /// item requesting exactly one completed compaction item.
    ResponsesTrigger,

    /// Responses automatic threshold compaction: parsed `context_management`
    /// entries with their thresholds.
    ResponsesAutomatic {
        /// The requested entries, in order; each carries its threshold when
        /// stated.
        entries: Vec<crate::ir::compaction::AutomaticEntry>,
    },

    /// Messages on-demand compaction (`compaction: {"type": "summarize"}`).
    MessagesOnDemand {
        /// Custom summarization instructions replacing the gateway's default
        /// summarization prompt.
        instructions: Option<String>,
    },

    /// Messages threshold compaction through `context_management.edits`.
    MessagesThreshold {
        /// The requested `compact_20260112` edits, in order.
        edits: Vec<compaction_control::ThresholdEdit>,
    },
}

/// Parsed control payloads of the compaction controls. The Responses
/// automatic entry is field-identical to its wire form, so it lives directly
/// as the IR's [`crate::ir::compaction::AutomaticEntry`]; the Messages edit's
/// parsed semantic shape differs from its vendor wire object, which
/// [`crate::ir::compaction::ThresholdEdit`] carries.
pub(crate) mod compaction_control {
    /// One Messages `compact_20260112` threshold edit.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) struct ThresholdEdit {
        /// The input-token trigger value in tokens, when stated; `None` keeps
        /// the protocol's 150,000-token default.
        pub(crate) trigger: Option<u64>,

        /// Whether the caller requested pausing after the compaction instead
        /// of continuing generation.
        pub(crate) pause_after_compaction: bool,

        /// Custom summarization instructions replacing the gateway's default
        /// summarization prompt.
        pub(crate) instructions: Option<String>,
    }
}

impl From<&compaction_control::ThresholdEdit> for crate::ir::compaction::ThresholdEdit {
    fn from(edit: &compaction_control::ThresholdEdit) -> Self {
        Self {
            trigger: edit
                .trigger
                .map(|value| crate::ir::compaction::InputTokenTrigger { value }),
            should_pause_after_compaction: edit.pause_after_compaction.then_some(true),
            instructions: edit.instructions.clone(),
        }
    }
}

/// One compaction state marker found in a request's history, ordered by
/// position.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompactionBoundary {
    /// A gateway carrier: a sealed envelope candidate whose raw carrier string
    /// rides the history in a protocol-specific slot (a Responses item's
    /// `encrypted_content`, a Messages block's `signature`).
    Gateway {
        /// Number of IR messages parsed before the marker.
        offset: usize,

        /// The raw carrier string, to unseal during preparation.
        carrier: String,

        /// For a Messages carrier: the visible content the signature binds,
        /// when the block presented one; the preparation step verifies the
        /// digest against it.
        visible_content: Option<String>,
    },

    /// Native compaction state: a producer-native item or block replayed in
    /// the history, opaque to the gateway and forwarded to a compatible
    /// producer unchanged (and never across protocols).
    Native {
        /// Number of IR messages parsed before the marker.
        offset: usize,

        /// The native state's own facts.
        artifact: NativeArtifact,
    },
}

/// Native compaction state recognized in a request's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeArtifact {
    /// A Responses `compaction` item: opaque native state whose item bytes
    /// ride [`crate::ir::IrRequest::state_items`].
    ResponsesCompactionItem,

    /// An Anthropic compaction block whose bytes ride
    /// [`crate::ir::IrRequest::state_items`].
    MessagesCompactionBlock {
        /// Whether the block is the native null-content no-op: it retains its
        /// documented no-op meaning and cannot authorize discarding history.
        is_null_no_op: bool,

        /// The block's replay placement in the source protocol.
        placement: BoundaryPlacement,
    },
}

/// Replay placement of a native compaction block within its source protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryPlacement {
    /// The block restores with the leading placement (an on-demand block):
    /// everything before it is summarized history and must not remain.
    Leading,

    /// The block sits at its covered historical boundary (a threshold block):
    /// entries before it are the superseded prefix, entries after it the
    /// fresh suffix.
    Historical,
}

/// Route context the router threads into request parsing (`docs/design/routing.md`): the facts the
/// URL path carries that the body does not.
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

    /// Inbound beta requirements declared by the caller: the `anthropic-beta` header entries the
    /// Anthropic Messages surface presents, split on commas, trimmed, empty entries dropped, in
    /// header order and case as sent. Empty on every other surface. The request parsers validate
    /// their own inbound beta requirements against this list, independently from any outbound
    /// beta requirement.
    pub(crate) anthropic_betas: Vec<String>,
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
            anthropic_betas: Vec::new(),
        }
    }

    /// Context carrying the caller's `anthropic-beta` entries for the surfaces that validate
    /// inbound beta requirements.
    pub(crate) fn with_anthropic_betas(mut self, betas: Vec<String>) -> Self {
        self.anthropic_betas = betas;

        self
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
/// messages (`docs/design/providers.md`) and the Interactions parser coalesces consecutive
/// same-role steps into one message per run, both finishing into [`IrMessage`]s through
/// [`RoleRun::finish`]; the Gemini renderers coalesce consecutive same-role messages into one
/// content entry or input step per run, as their protocols' role-alternation rules require, taking
/// the closed runs from [`RoleRun::into_runs`].
struct RoleRun<R, P> {
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
    fn push(&mut self, role: R, parts: Vec<P>) {
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
    fn into_runs(mut self) -> Vec<(R, Vec<P>)> {
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
    fn finish(self) -> Vec<IrMessage> {
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
    fn parse_json_response(&mut self, body: &[u8]) -> Result<Vec<IrEvent>, PayloadError>;

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
/// agree (`docs/reference/inference.md`).
///
/// The finished flag lives in each parser, which sets it on every path that emits a real finish —
/// a streamed terminal event, a stop reason, or the complete-body parse — and whose
/// [`UpstreamEventParser::finish`] hook delegates here: the first call on an unfinished parser
/// marks the flag and returns the single [`IrEvent::Finish`] carrying [`IrFinishReason::Stop`], and
/// a parser that already finished emits nothing.
fn degrade_finish_once(is_finished: &mut bool) -> Vec<IrEvent> {
    if *is_finished {
        return Vec::new();
    }

    *is_finished = true;

    vec![IrEvent::finish(IrFinishReason::Stop)]
}

/// Emits the once-only opening [`IrEvent::MessageStart`] of an upstream event stream: exactly
/// once, first (`docs/reference/inference.md`), carrying the response's identity.
///
/// The started flag lives in each parser, which passes it on every path that may open the
/// stream — the stream-head announcement and the recovery paths that never saw it. The first
/// call on an unstarted parser marks the flag and returns the single [`IrEvent::MessageStart`]
/// carrying `id` and `model`; a parser that already started emits nothing.
fn message_start_once(is_started: &mut bool, id: String, model: String) -> Vec<IrEvent> {
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
fn complete_call_events(
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

/// Digest-only metadata of an actual complete artifact awaiting its last emitted frame.
pub(crate) struct CompletedArtifact {
    /// Position of the artifact's final frame in the current render batch.
    pub(crate) terminal_index: usize,
    /// Fully replayable checkpoint state and artifact hints, without generated content.
    pub(crate) anchors: Vec<crate::affinity::Anchor>,
}

/// Client-side SSE rendering contract implemented by every outward codec.
pub(crate) trait ClientSseRenderer {
    /// Takes a newly completed native artifact from the actual current render batch.
    fn take_completed_artifact(&mut self) -> Option<CompletedArtifact> {
        None
    }

    /// Takes the last completed prepared-prefix frame index from the latest render batch.
    fn take_prefix_boundary(&mut self) -> Option<usize> {
        None
    }

    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String>;

    /// Enables codec-owned incremental projection only for a pooled accepted result.
    /// Codecs without an integrated projection discard this metadata until their adapter exists.
    fn begin_projection(&mut self, _projector: crate::affinity::fingerprint::Projector) {}

    /// Starts ordinary answer projection after one already projected, sealer-proven checkpoint.
    fn begin_checkpoint_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.begin_projection(projector);
    }

    /// Takes a fully replayable emitted continuation at terminal rendering. Unfingerprintable
    /// output, or a codec without an integrated adapter, returns no completed history.
    fn take_projection(&mut self) -> Option<crate::affinity::fingerprint::Projector> {
        None
    }

    /// Renders a mid-stream error as a terminal frame from the shared [`crate::api::Error`]
    /// classification: the HTTP error path and the stream-frame path share one classification
    /// (`docs/reference/inference.md`); each codec places it in its own terminal frame — the Chat
    /// Completions error envelope or the failed Responses response object — through its
    /// format's render module.
    fn error_frame(&mut self, error: &crate::api::Error) -> String;
}

/// An outward client format: parses client requests into IR and renders IR events back in the
/// client's wire shape, live or aggregated.
pub(crate) trait ClientFormat {
    /// The non-streaming success body this format renders.
    type JsonResponse;

    /// Parses one client request body with the router's route context: the URL-path facts the
    /// body does not carry (`docs/design/routing.md`). Surfaces routing the model from the body
    /// ignore the context; Gemini surfaces take the public model id (and streaming mode, when
    /// the URL action decides it) from it.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] for malformed or unsupported requests.
    fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest>
    where
        Self: Sized;

    /// Boxes the streaming renderer for one parsed request. `should_include_usage` is the trailing
    /// usage-chunk option; formats that always include usage ignore it. `public_id` is the public
    /// routing ID (`docs/design/storage.md`) the client addressed.
    fn sse_renderer(
        should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send>
    where
        Self: Sized;

    /// Renders an aggregated completion as a non-streaming response body. `public_id` is the public
    /// routing ID (`docs/design/storage.md`) the client addressed.
    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse;

    /// Projects the actual rendered conversation fields as this codec parses their replay.
    /// Returns false for output that cannot be replayed, without changing the inference result.
    fn project_json_response(
        response: &Self::JsonResponse,
        projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool;

    /// Identifies actual complete native output artifacts at the successful buffered handoff.
    /// Newly sealed gateway checkpoints carry their authenticated identities separately.
    fn completed_json_artifacts(_response: &Self::JsonResponse) -> Vec<crate::affinity::Anchor> {
        Vec::new()
    }

    /// Projects the actual answer following one checkpoint already present in the projector.
    /// Only codecs with a checkpoint channel implement this delivery path.
    fn project_json_checkpoint(
        _response: &Self::JsonResponse,
        _projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool {
        false
    }

    /// Renders an aggregated completion whose output opens with compaction
    /// prefix state — the checkpoint-before-answer ordering of the compaction
    /// flows — beside the vendor-shaped iteration entries a compaction
    /// lifecycle renders under its usage. Formats without a compaction channel
    /// ignore both (their surfaces can never produce them), so the default
    /// delegates to [`ClientFormat::render_json_response`].
    fn render_json_response_with_prefix(
        public_id: &str,
        aggregation: &IrAggregation,
        _prefix: &[crate::format::compaction_wire::Checkpoint],
        _iterations: &[crate::ir::compaction::UsageIteration],
    ) -> Self::JsonResponse {
        Self::render_json_response(public_id, aggregation)
    }
}

/// Re-renders an aggregated completion as the IR event sequence its live
/// rendering would have consumed: the synthesis the compaction pipeline uses
/// to stream a lifecycle the executor validated internally as one complete
/// artifact (`docs/design/compaction.md` — the live and terminal representations of one
/// response).
#[must_use]
pub(crate) fn aggregation_events(aggregation: &IrAggregation, model: &str) -> Vec<IrEvent> {
    let mut events = vec![IrEvent::MessageStart {
        id: crate::ids::new_uuid(),
        model: model.to_string(),
    }];

    for payload in &aggregation.compaction_states {
        events.push(IrEvent::CompactionState {
            payload: payload.clone(),
        });
    }

    for block in &aggregation.reasoning_blocks {
        events.push(IrEvent::ReasoningDone {
            text: block.text.clone(),
            signature: block.signature.clone(),
            opaque: block.opaque.clone(),
        });
    }

    if !aggregation.text.is_empty() {
        events.push(IrEvent::TextDelta(aggregation.text.clone()));
    }

    for (index, call) in aggregation.tool_calls.iter().enumerate() {
        events.push(IrEvent::ToolCallDone {
            index,
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            thought_signature: call.thought_signature.clone(),
        });
    }

    if let Some(finish) = aggregation.finish {
        events.push(IrEvent::Finish {
            reason: finish,
            prompt_block_reason: aggregation.prompt_block_reason.clone(),
        });
    }

    if aggregation.is_compaction_paused || !aggregation.compaction_iterations.is_empty() {
        events.push(IrEvent::CompactionMetadata {
            is_paused: aggregation.is_compaction_paused,
            iterations: aggregation.compaction_iterations.clone(),
        });
    }
    if let Some(usage) = aggregation.usage {
        events.push(IrEvent::Usage(usage));
    }

    events.push(IrEvent::Done);

    events
}

/// A wire value that is either a plain string or an array of typed items: the two official shapes
/// of message content, tool-result output, and the Responses `input` field.
///
/// Deserialization is hand-written rather than an `untagged` enum because an untagged enum buffers
/// its variants and reports every inner failure as "data did not match any variant", which would
/// lose the member name a rejection has to name (`docs/reference/inference.md`).
#[derive(Debug)]
enum StringOrArray<T> {
    /// The plain-string form.
    Text(String),

    /// The array form.
    Array(Vec<T>),
}

/// The `error` member an upstream may embed in a 2xx response body (`docs/reference/inference.md`).
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
enum UpstreamError {
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
    fn message(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),

            Self::Object { message, .. } => Some(message),

            Self::Raw(_) => None,
        }
    }

    /// Renders the failure carrying the embedded text, with the upstream code beside it when the
    /// error carries one.
    fn failure(&self) -> PayloadError {
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
    fn new(message: impl Into<String>) -> Self {
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
/// member in the rejection, which is what `docs/reference/inference.md` requires; the rejection
/// carries no [`InvalidRequest::param`] because serde reports the failure against the body as a
/// whole rather than against one named parameter.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for a body that is not valid JSON and for a body whose shape the
/// protocol does not allow.
fn parse_client_request<T>(body: &[u8]) -> Result<T, InvalidRequest>
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
fn arguments_as_object(arguments: &str) -> Result<Map<String, Value>, InvalidRequest> {
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
fn arguments_to_object_or_empty(arguments: &str) -> Map<String, Value> {
    arguments_as_object(arguments).unwrap_or_default()
}

/// The blank-line separator between flattened text parts (`docs/reference/inference.md`): one
/// cross-codec contract, shared by every surface that flattens a string-or-text-parts shape into
/// one string and by the affinity projection that hashes the same flattened bytes.
const TEXT_PART_SEPARATOR: &str = "\n\n";

/// Flattens text parts into one string under the shared wire contract
/// (`docs/reference/inference.md`): the parts join with [`TEXT_PART_SEPARATOR`], and a leading run
/// of empty parts is skipped — the separator only ever separates text the accumulator already
/// holds, so leading empties carry none and an all-empty input flattens to the empty string.
fn join_text_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .skip_while(|text| text.is_empty())
        .collect::<Vec<_>>()
        .join(TEXT_PART_SEPARATOR)
}

/// Parses the JSON payload of an upstream SSE frame into a codec's typed upstream shape; the single
/// home of the malformed-frame rejection shared by every codec's `ingest_frame`.
///
/// # Errors
///
/// Returns [`PayloadError`] when the frame data is not valid JSON or does not match the codec's
/// upstream shape.
fn parse_frame<'de, T>(frame: &'de SseFrame) -> Result<T, PayloadError>
where
    T: Deserialize<'de>,
{
    serde_json::from_str(&frame.data)
        .map_err(|_| PayloadError::new("malformed upstream payload in SSE frame"))
}

/// Deserializes a complete upstream document directly into a concrete codec payload.
///
/// # Errors
///
/// Returns a payload failure for malformed JSON or a mismatched protocol shape.
fn parse_body<T>(body: &[u8]) -> Result<T, PayloadError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body)
        .map_err(|error| PayloadError::new(format!("malformed upstream payload: {error}")))
}

/// Selects the primary entry of an upstream parallel array: the entry the `is_primary` predicate
/// accepts (each codec passes "the entry whose index is 0"), tolerating index-less upstreams by
/// taking the first entry instead. Parallel entries beyond the primary are not supported and the
/// remainder is ignored.
fn primary<T>(entries: &[T], is_primary: impl Fn(&T) -> bool) -> Option<&T> {
    entries
        .iter()
        .find(|entry| is_primary(entry))
        .or_else(|| entries.first())
}

/// Rejects an OpenAI-family tool payload whose `type` tag is not `function`.
///
/// `context` names the payload's role and renders into the rejection message (`tool` for advertised
/// tools, `tool_choice` for forced functions).
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `type` for any non-function tag.
fn reject_non_function_type(tool_type: &str, context: &str) -> Result<(), InvalidRequest> {
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
fn reject_tool_choice_without_tools(is_tools_empty: bool) -> Result<(), InvalidRequest> {
    if is_tools_empty {
        return Err(InvalidRequest::field(
            "tool_choice",
            "tool_choice requires at least one tool",
        ));
    }

    Ok(())
}

/// Validates the sampling parameter ranges the OpenAI and Gemini request shapes share: both
/// protocols' client surfaces accept `temperature` and a nucleus-sampling threshold with the same
/// official bounds, and a rejection names the calling surface's own wire spelling.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the offending parameter.
fn validate_sampling_ranges(
    temperature: Option<f64>,
    top_p: Option<f64>,
    top_p_param: &str,
) -> Result<(), InvalidRequest> {
    if temperature.is_some_and(|temperature| !(0.0..=2.0).contains(&temperature)) {
        return Err(InvalidRequest::field(
            "temperature",
            "temperature must be between 0 and 2",
        ));
    }

    if top_p.is_some_and(|top_p| !(0.0..=1.0).contains(&top_p)) {
        return Err(InvalidRequest::field(
            top_p_param,
            format!("{top_p_param} must be between 0 and 1"),
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
fn render_json<T>(value: &T) -> String
where
    T: Serialize + ?Sized,
{
    #[expect(
        clippy::expect_used,
        reason = "rendered wire types are first-party and serialize totally"
    )]
    serde_json::to_string(value).expect("rendered wire payloads serialize")
}
