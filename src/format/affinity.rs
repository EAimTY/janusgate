//! Client-history source metadata kept outside the model-visible inference request.

use super::UpstreamFormat;
use crate::{
    affinity::fingerprint::{Identity, Projector, ReplayFamily, ResultContent, Role, StateKind},
    ir::{IrImage, IrMessage, IrPart, IrRole, StateItem},
};
use std::fmt;

/// Source semantics not recoverable from the tool output's normalized inference text.
#[derive(Clone, Copy)]
pub(super) enum ResultShape {
    /// Accepted text, including the codec's normalized text-block result form.
    Text,
    /// A foreign JSON object, array, number, or boolean serialized into inference text.
    Json,
    /// A foreign null flattened to empty inference text by GenerateContent.
    JsonNull,
}

/// One tool part's actual wire-source identity and result semantics.
#[derive(Clone, Copy)]
enum ToolSource {
    /// Invocation in accepted canonical conversation order.
    Call {
        /// Whether the client actually supplied the invocation's ID.
        has_wire_id: bool,
    },
    /// Result already paired by the codec's normal parser.
    Result {
        /// Whether the result carried its own real reference on the client wire.
        has_wire_id: bool,
        /// Accepted content type before flattening to model-visible text.
        shape: ResultShape,
    },
}

/// Accepted part position, separate from GenerateContent's inference-side message regrouping.
#[derive(Clone, Copy)]
enum PartLocation {
    /// Reference into the parser's completed inference messages.
    Resolved {
        /// Inference message index containing this accepted part.
        message: usize,
        /// Part index within that message.
        part: usize,
    },
    /// A user text/media part whose destination message follows the encountered tool results.
    Deferred {
        /// Part index in the user's pending text/media message.
        part: usize,
    },
}

/// Small request-local source facts, aligned with conversation tool parts in encounter order.
/// It contains no duplicate prompt/result text and never enters inference IR or cache storage.
#[derive(Clone)]
pub(crate) struct Sources {
    /// Client codec, determining which reasoning replay semantics are actually preserved.
    client_format: UpstreamFormat,
    /// Source identity/content types lost when the codec normalizes inference messages.
    tools: Vec<ToolSource>,
    /// Original part order when inference regrouping would otherwise lose client history order.
    order: Option<Vec<PartLocation>>,
}

impl fmt::Debug for Sources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Sources")
            .field("client_format", &self.client_format)
            .field("tool_parts", &self.tools.len())
            .finish()
    }
}

impl Sources {
    /// Source codec family for authenticated carrier projection, without copying source content.
    pub(crate) const fn client_format(&self) -> UpstreamFormat {
        self.client_format
    }

    /// Starts source metadata for one accepted client codec.
    pub(super) fn new(client_format: UpstreamFormat) -> Self {
        Self {
            client_format,
            tools: Vec::new(),
            order: (client_format == UpstreamFormat::GeminiGenerateContent).then(Vec::new),
        }
    }

    /// Records codecs whose every accepted call/result reference is a required real wire ID,
    /// and whose results normalize to text. ID-less/foreign-result codecs record at conversion.
    pub(super) fn from_wire_messages(
        client_format: UpstreamFormat,
        messages: &[IrMessage],
    ) -> Self {
        let mut sources = Self::new(client_format);
        sources.order = None;
        for part in messages.iter().flat_map(|message| &message.parts) {
            match part {
                IrPart::ToolCall { .. } => sources.call(true),
                IrPart::ToolOutput { .. } => sources.result(true, ResultShape::Text),
                _ => {}
            }
        }
        sources
    }

    /// Captures identity provenance at the same point that the parser emits an IR invocation.
    pub(super) fn call(&mut self, has_wire_id: bool) {
        self.tools.push(ToolSource::Call { has_wire_id });
    }

    /// Captures reference provenance and content type before the normal parser loses either.
    pub(super) fn result(&mut self, has_wire_id: bool, shape: ResultShape) {
        self.tools.push(ToolSource::Result { has_wire_id, shape });
    }

    /// Records an accepted part whose inference destination is already known.
    pub(super) fn part(&mut self, message: usize, part: usize) {
        if let Some(order) = &mut self.order {
            order.push(PartLocation::Resolved { message, part });
        }
    }

    /// Preserves a user part's wire position before its regrouped destination message exists.
    ///
    /// # Panics
    ///
    /// Panics if called by a codec without a part-order channel, a first-party parser invariant.
    pub(super) fn defer_part(&mut self, part: usize) -> usize {
        #[expect(
            clippy::expect_used,
            reason = "only GenerateContent defers reordered user parts"
        )]
        let order = self
            .order
            .as_mut()
            .expect("this codec records original part order");
        let slot = order.len();
        order.push(PartLocation::Deferred { part });
        slot
    }

    /// Resolves pending user parts when the codec emits their single regrouped message.
    ///
    /// # Panics
    ///
    /// Panics if a parser-supplied deferred slot is missing or already resolved.
    pub(super) fn resolve_parts(&mut self, slots: &[usize], message: usize) {
        #[expect(
            clippy::expect_used,
            reason = "only GenerateContent resolves its own deferred parts"
        )]
        let order = self
            .order
            .as_mut()
            .expect("this codec records original part order");
        for slot in slots {
            #[expect(
                clippy::expect_used,
                reason = "the parser retained this order channel's own slot"
            )]
            let location = order.get_mut(*slot).expect("the deferred part slot exists");
            #[expect(
                clippy::unreachable,
                reason = "the user message resolves each deferred slot once"
            )]
            let PartLocation::Deferred { part } = *location else {
                unreachable!("the retained user slot is unresolved");
            };
            *location = PartLocation::Resolved { message, part };
        }
    }

    /// Reads source-order references without copying accepted content or changing inference IR.
    ///
    /// # Panics
    ///
    /// Panics if the parser left an unresolved reference or emitted a nonexistent destination.
    fn parts<'a>(
        &'a self,
        messages: &'a [IrMessage],
    ) -> Box<dyn Iterator<Item = (usize, IrRole, &'a IrPart)> + 'a> {
        match &self.order {
            Some(order) => Box::new(order.iter().map(move |location| {
                #[expect(
                    clippy::unreachable,
                    reason = "the parser resolves every emitted user part"
                )]
                let PartLocation::Resolved { message, part } = *location else {
                    unreachable!("accepted client parts have resolved destinations");
                };
                #[expect(
                    clippy::expect_used,
                    reason = "the parser recorded its own emitted message and part"
                )]
                messages
                    .get(message)
                    .and_then(|resolved| {
                        resolved
                            .parts
                            .get(part)
                            .map(|part| (message, resolved.role, part))
                    })
                    .expect("the accepted part destination exists")
            })),
            None => Box::new(messages.iter().enumerate().flat_map(|(index, message)| {
                message
                    .parts
                    .iter()
                    .map(move |part| (index, message.role, part))
            })),
        }
    }

    /// Projects ordinary accepted conversation messages using explicit typed parts, never
    /// serializing arbitrary IR or envelope controls. Stateful requests need the authenticated
    /// boundary planner to feed their active message/state window separately before selection.
    ///
    /// # Panics
    ///
    /// Panics if parser source records disagree with emitted tool parts or accepted foreign JSON
    /// cannot be parsed: each is a first-party parser contract violation, never a new body error.
    pub(crate) fn project(&self, messages: &[IrMessage]) -> Projector {
        let mut projector = Projector::new();
        self.append(messages, &mut projector);
        projector
    }

    /// Extends an existing projection with actual accepted codec parts in their wire order.
    ///
    /// # Panics
    ///
    /// Panics if parser source records or validated foreign JSON violate their typed contracts.
    fn append(&self, messages: &[IrMessage], projector: &mut Projector) {
        self.append_window(messages, &[], 0, projector);
    }

    /// Extends a validated client-visible suffix while preserving source records and state
    /// positions. The boundary planner supplies only active state items in encounter order;
    /// messages and offsets still refer to the original parsed request, never expanded history.
    /// Gateway items must already have been authenticated by the caller.
    ///
    /// # Panics
    ///
    /// Panics if parser source records, accepted foreign JSON, or state-family contracts fail.
    pub(crate) fn append_window<'a>(
        &self,
        messages: &[IrMessage],
        state_items: impl IntoIterator<Item = &'a StateItem>,
        first_message: usize,
        projector: &mut Projector,
    ) {
        let mut sources = self.tools.iter();
        let mut states = state_items
            .into_iter()
            .filter(|item| item.offset >= first_message)
            .peekable();
        for (message_index, source_role, part) in self.parts(messages) {
            // Even covered invocations/results consume their source fact, so suffix tools never
            // inherit an older call's identity or result shape.
            let source = matches!(part, IrPart::ToolCall { .. } | IrPart::ToolOutput { .. })
                .then(|| sources.next())
                .flatten();
            if message_index < first_message {
                continue;
            }
            while let Some(state) = states.next_if(|item| item.offset <= message_index) {
                self.append_state(state, projector);
            }
            let role = match source_role {
                IrRole::User => Role::User,
                IrRole::Assistant => Role::Assistant,
                IrRole::Tool => Role::Tool,
            };
            match part {
                IrPart::Text { text } => projector.text(role, text.as_bytes()),
                IrPart::Image {
                    image:
                        IrImage::Inline {
                            mime_type,
                            data_base64,
                        },
                } => {
                    projector.inline_image(role, mime_type, data_base64);
                }
                IrPart::Image {
                    image: IrImage::Url(url),
                } => projector.url_image(role, url),
                IrPart::ToolCall {
                    call_id,
                    name,
                    arguments,
                    thought_signature,
                } => {
                    #[expect(
                        clippy::expect_used,
                        reason = "the parser records each emitted tool part"
                    )]
                    let source = source.expect("the accepted invocation has a source");
                    #[expect(
                        clippy::unreachable,
                        reason = "source records align with parser tool-part order"
                    )]
                    let ToolSource::Call { has_wire_id } = source else {
                        unreachable!("an invocation has call source metadata");
                    };
                    #[expect(
                        clippy::expect_used,
                        reason = "accepted invocation arguments are validated foreign JSON"
                    )]
                    let arguments = serde_json::Value::Object(
                        crate::format::arguments_as_object(arguments)
                            .expect("accepted invocation arguments parse"),
                    );
                    projector.call(
                        role,
                        identity(call_id, *has_wire_id),
                        name,
                        &arguments,
                        thought_signature.as_deref(),
                    );
                }
                IrPart::ToolOutput {
                    call_id,
                    content,
                    is_error,
                } => {
                    #[expect(
                        clippy::expect_used,
                        reason = "the parser records each emitted tool part"
                    )]
                    let source = source.expect("the accepted result has a source");
                    #[expect(
                        clippy::unreachable,
                        reason = "source records align with parser tool-part order"
                    )]
                    let ToolSource::Result { has_wire_id, shape } = source else {
                        unreachable!("a result has result source metadata");
                    };
                    let foreign;
                    let content = match shape {
                        ResultShape::Text => ResultContent::Text(content),
                        ResultShape::Json => {
                            #[expect(
                                clippy::expect_used,
                                reason = "the parser serialized this accepted foreign result"
                            )]
                            {
                                foreign = serde_json::from_str(content)
                                    .expect("the foreign result parses");
                            }
                            ResultContent::Json(&foreign)
                        }
                        ResultShape::JsonNull => {
                            foreign = serde_json::Value::Null;
                            ResultContent::Json(&foreign)
                        }
                    };
                    projector.result(role, identity(call_id, *has_wire_id), content, *is_error);
                }
                IrPart::Reasoning {
                    text,
                    signature,
                    opaque,
                } => {
                    if let Some(family) = replay_family(self.client_format) {
                        projector.reasoning(
                            role,
                            family,
                            text,
                            signature.as_deref(),
                            (!opaque.is_null()).then_some(opaque),
                        );
                    }
                }
            }
        }
        for state in states {
            self.append_state(state, projector);
        }
    }

    /// Projects an already accepted active state without copying its payload.
    ///
    /// # Panics
    ///
    /// Panics if a codec that cannot accept state supplied it, a parser contract violation.
    fn append_state(&self, state: &StateItem, projector: &mut Projector) {
        #[expect(
            clippy::expect_used,
            reason = "state-bearing codecs have a replay family"
        )]
        let family = replay_family(self.client_format).expect("accepted state has a replay family");
        let kind = if state.is_gateway {
            StateKind::Gateway
        } else {
            StateKind::Native
        };
        projector.state(family, kind, &state.item);
    }
}

/// The retained reasoning/state replay family of a client codec's wire semantics; Chat
/// Completions discards reasoning and state on replay.
pub(crate) fn replay_family(format: UpstreamFormat) -> Option<ReplayFamily> {
    match format {
        UpstreamFormat::OpenAiChatCompletion => None,
        UpstreamFormat::OpenAiResponse => Some(ReplayFamily::Responses),
        UpstreamFormat::AnthropicMessages => Some(ReplayFamily::Messages),
        UpstreamFormat::GeminiGenerateContent => Some(ReplayFamily::GenerateContent),
        UpstreamFormat::GeminiInteraction => Some(ReplayFamily::Interactions),
    }
}

/// Projects a completed emitted invocation only if its foreign argument string is replayable.
/// An upstream malformed argument suffix disables output anchoring without altering inference.
pub(super) fn project_call(
    projector: &mut Projector,
    call_id: &str,
    name: &str,
    arguments: &str,
    signature: Option<&str>,
) -> bool {
    let Ok(arguments) = crate::format::arguments_as_object(arguments) else {
        return false;
    };
    projector.call(
        Role::Assistant,
        Identity::Wire(call_id),
        name,
        &serde_json::Value::Object(arguments),
        signature,
    );
    true
}

/// Preserves source identity even when a synthetic stand-in happens to equal a real wire ID.
fn identity(call_id: &str, has_wire_id: bool) -> Identity<'_> {
    if has_wire_id {
        Identity::Wire(call_id)
    } else {
        Identity::Synthetic(call_id)
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the content-free source projection: the replay-family table, the wire-source
    //! facts the codecs record beside their parse, the canonicalization of call identities and
    //! result shapes, the wire-order channel of the regrouping codec, and the state-window
    //! splicing — all observed through the projector's digest identities.

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{ResultShape, Sources, project_call, replay_family};
    use crate::{
        affinity::{
            Digest,
            fingerprint::{Identity, Projector, ReplayFamily, ResultContent, Role, StateKind},
        },
        format::UpstreamFormat,
        ir::{IrMessage, IrPart, IrRole, StateFamily, StateItem},
    };
    use serde_json::{Value, json};
    use std::error::Error as StdError;

    /// A user text message.
    fn user_text(text: &str) -> IrMessage {
        IrMessage {
            role: IrRole::User,
            parts: vec![IrPart::Text {
                text: text.to_string(),
            }],
        }
    }

    /// An assistant message carrying one tool invocation with the given wire identity.
    fn assistant_call(call_id: &str, has_signature: bool) -> IrMessage {
        IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::ToolCall {
                call_id: call_id.to_string(),
                name: "find".to_string(),
                arguments: "{\"q\":1}".to_string(),
                thought_signature: has_signature.then(|| "sig".to_string()),
            }],
        }
    }

    /// A tool message answering one invocation.
    fn tool_result(call_id: &str, content: &str) -> IrMessage {
        IrMessage {
            role: IrRole::Tool,
            parts: vec![IrPart::ToolOutput {
                call_id: call_id.to_string(),
                content: content.to_string(),
                is_error: false,
            }],
        }
    }

    /// An assistant message carrying one reasoning block.
    fn reasoning_message() -> IrMessage {
        IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::Reasoning {
                text: "thinking".to_string(),
                signature: None,
                opaque: Value::Null,
            }],
        }
    }

    /// Projects the messages through the source facts and reads the retry digest.
    fn projected_digest(sources: &Sources, messages: &[IrMessage]) -> Option<Digest> {
        sources
            .project(messages)
            .evidence()
            .retry
            .map(|retry| retry.digest)
    }

    /// Projects the messages directly onto a fresh projector and reads the retry digest.
    fn direct_digest(project: impl FnOnce(&mut Projector)) -> Option<Digest> {
        let mut projector = Projector::new();

        project(&mut projector);

        projector.evidence().retry.map(|retry| retry.digest)
    }

    /// Every client family maps onto its replay family, and Chat Completions — which discards
    /// reasoning and state on replay — maps onto none.
    #[test]
    fn replay_family_table() {
        assert!(replay_family(UpstreamFormat::OpenAiChatCompletion).is_none());
        assert!(matches!(
            replay_family(UpstreamFormat::OpenAiResponse),
            Some(ReplayFamily::Responses)
        ));
        assert!(matches!(
            replay_family(UpstreamFormat::AnthropicMessages),
            Some(ReplayFamily::Messages)
        ));
        assert!(matches!(
            replay_family(UpstreamFormat::GeminiGenerateContent),
            Some(ReplayFamily::GenerateContent)
        ));
        assert!(matches!(
            replay_family(UpstreamFormat::GeminiInteraction),
            Some(ReplayFamily::Interactions)
        ));
    }

    /// Asserts two projections carry the same retry digest.
    fn assert_same_digest(
        left: Option<Digest>,
        right: Option<Digest>,
    ) -> Result<(), Box<dyn StdError>> {
        let is_same = match (left, right) {
            (Some(left), Some(right)) => left == right,
            (None, None) => true,
            _ => false,
        };

        if is_same {
            Ok(())
        } else {
            Err("the projections must carry the same retry digest".into())
        }
    }

    /// Asserts two projections carry different retry digests or only one is fingerprintable.
    fn assert_different_digest(
        left: Option<Digest>,
        right: Option<Digest>,
    ) -> Result<(), Box<dyn StdError>> {
        let is_different = match (left, right) {
            (Some(left), Some(right)) => left != right,
            (Some(_), None) | (None, Some(_)) => true,
            (None, None) => false,
        };

        if is_different {
            Ok(())
        } else {
            Err("the projections must not carry the same retry digest".into())
        }
    }

    /// The recorded source format reads back unchanged from both constructors.
    #[test]
    fn sources_record_the_client_format() {
        for format in [
            UpstreamFormat::OpenAiChatCompletion,
            UpstreamFormat::OpenAiResponse,
            UpstreamFormat::GeminiGenerateContent,
        ] {
            assert_eq!(Sources::new(format).client_format(), format);
            assert_eq!(
                Sources::from_wire_messages(format, &[]).client_format(),
                format
            );
        }
    }

    /// The wire-message constructor and the incremental record-and-project path produce identical
    /// fingerprints for the same accepted conversation.
    #[test]
    fn from_wire_messages_matches_incremental_recording() -> Result<(), Box<dyn StdError>> {
        let messages = vec![
            user_text("hi"),
            assistant_call("call_1", true),
            tool_result("call_1", "ok"),
        ];

        let via_wire = Sources::from_wire_messages(UpstreamFormat::OpenAiChatCompletion, &messages);

        assert_same_digest(
            projected_digest(&via_wire, &messages),
            direct_digest(|projector| {
                projector.text(Role::User, b"hi");
                projector.call(
                    Role::Assistant,
                    Identity::Wire("call_1"),
                    "find",
                    &json!({"q": 1}),
                    Some("sig"),
                );
                projector.result(
                    Role::Tool,
                    Identity::Wire("call_1"),
                    ResultContent::Text("ok"),
                    false,
                );
            }),
        )
    }

    /// A codec-synthesized identity never fingerprints as the real wire ID it happens to equal.
    #[test]
    fn synthetic_identity_differs_from_equal_wire_id() -> Result<(), Box<dyn StdError>> {
        let mut wire_sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        wire_sources.call(true);

        let mut synthetic_sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        synthetic_sources.call(false);

        let messages = vec![assistant_call("call_0", false)];

        assert_eq!(
            wire_sources.client_format(),
            synthetic_sources.client_format()
        );

        assert_different_digest(
            projected_digest(&wire_sources, &messages),
            projected_digest(&synthetic_sources, &messages),
        )
    }

    /// A duplicated unresolved wire identity makes the suffix unfingerprintable instead of
    /// double-counting the call.
    #[test]
    fn duplicate_wire_identity_stops_the_projection() {
        let mut sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        sources.call(true);
        sources.call(true);

        let messages = vec![
            assistant_call("call_1", false),
            assistant_call("call_1", false),
        ];

        assert!(projected_digest(&sources, &messages).is_none());
    }

    /// A result recorded as foreign JSON fingerprints differently from bytes with an equal
    /// rendering recorded as plain text: the accepted content type participates in the identity.
    #[test]
    fn result_shape_participates_in_the_identity() -> Result<(), Box<dyn StdError>> {
        // Text hashes the exact bytes `[1, 2]`; the JSON shape canonicalizes to `[1,2]`.
        let messages = vec![
            assistant_call("call_1", false),
            tool_result("call_1", "[1, 2]"),
        ];

        let mut text_sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        text_sources.call(true);
        text_sources.result(true, ResultShape::Text);

        let mut json_sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        json_sources.call(true);
        json_sources.result(true, ResultShape::Json);

        assert_different_digest(
            projected_digest(&text_sources, &messages),
            projected_digest(&json_sources, &messages),
        )
    }

    /// A GenerateContent null result keeps its flattened-null identity instead of hashing like
    /// accepted text.
    #[test]
    fn json_null_result_shape_is_distinct() -> Result<(), Box<dyn StdError>> {
        let messages = vec![assistant_call("call_1", false), tool_result("call_1", "")];

        let mut null_sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        null_sources.call(true);
        null_sources.result(true, ResultShape::JsonNull);

        let mut text_sources = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        text_sources.call(true);
        text_sources.result(true, ResultShape::Text);

        assert_different_digest(
            projected_digest(&null_sources, &messages),
            projected_digest(&text_sources, &messages),
        )
    }

    /// Reasoning participates in the projection only for codecs that preserve its replay; the
    /// Chat Completions projection hashes the same history identically with or without the block.
    #[test]
    fn reasoning_projection_is_gated_on_the_client_family() -> Result<(), Box<dyn StdError>> {
        let chat_with = Sources::new(UpstreamFormat::OpenAiChatCompletion);
        let chat_without = Sources::new(UpstreamFormat::OpenAiChatCompletion);

        let with_reasoning = vec![user_text("q"), reasoning_message()];
        let without_reasoning = vec![user_text("q")];

        assert_same_digest(
            projected_digest(&chat_with, &with_reasoning),
            projected_digest(&chat_without, &without_reasoning),
        )?;

        let responses_with = Sources::new(UpstreamFormat::OpenAiResponse);
        let responses_without = Sources::new(UpstreamFormat::OpenAiResponse);

        assert_different_digest(
            projected_digest(&responses_with, &with_reasoning),
            projected_digest(&responses_without, &without_reasoning),
        )
    }

    /// Deferred user parts project in their original wire order, not in the order their
    /// regrouped destinations were resolved.
    #[test]
    fn deferred_parts_project_in_wire_order() -> Result<(), Box<dyn StdError>> {
        let mut sources = Sources::new(UpstreamFormat::GeminiGenerateContent);

        let later = sources.defer_part(1);
        let earlier = sources.defer_part(0);
        sources.resolve_parts(&[later, earlier], 0);

        let messages = vec![IrMessage {
            role: IrRole::User,
            parts: vec![
                IrPart::Text {
                    text: "a".to_string(),
                },
                IrPart::Text {
                    text: "b".to_string(),
                },
            ],
        }];

        let projected = sources.project(&messages);

        assert_same_digest(
            projected.evidence().retry.map(|retry| retry.digest),
            direct_digest(|projector| {
                projector.text(Role::User, b"b");
                projector.text(Role::User, b"a");
            }),
        )?;

        assert_different_digest(
            projected.evidence().retry.map(|retry| retry.digest),
            direct_digest(|projector| {
                projector.text(Role::User, b"a");
                projector.text(Role::User, b"b");
            }),
        )
    }

    /// State items splice into the canonical history at their message offsets: the item at offset
    /// zero precedes the first message, later offsets land between their neighbors, and items
    /// beyond the last message close the history.
    #[test]
    fn state_items_splice_at_their_offsets() -> Result<(), Box<dyn StdError>> {
        let native_state = |offset: usize| StateItem {
            offset,
            item: json!({"type": "reasoning", "encrypted_content": "enc"}),
            is_gateway: false,
            is_compaction: false,
            family: StateFamily::Responses,
        };

        let messages = vec![user_text("u1"), user_text("u2")];
        let [head, middle, tail] = [native_state(0), native_state(1), native_state(2)];

        let sources = Sources::new(UpstreamFormat::OpenAiResponse);

        let spliced = {
            let mut projector = Projector::new();

            sources.append_window(&messages, [&head, &middle, &tail], 0, &mut projector);

            projector.evidence().retry.map(|retry| retry.digest)
        };

        assert_same_digest(
            spliced,
            direct_digest(|projector| {
                projector.state(ReplayFamily::Responses, StateKind::Native, &head.item);
                projector.text(Role::User, b"u1");
                projector.state(ReplayFamily::Responses, StateKind::Native, &middle.item);
                projector.text(Role::User, b"u2");
                projector.state(ReplayFamily::Responses, StateKind::Native, &tail.item);
            }),
        )
    }

    /// A suffix window drops covered messages and the state items before it, keeps the state at
    /// and after the window head, and never shifts a state item across a message.
    #[test]
    fn suffix_window_respects_the_first_message_boundary() -> Result<(), Box<dyn StdError>> {
        let native_state = |offset: usize| StateItem {
            offset,
            item: json!({"type": "reasoning", "encrypted_content": "enc"}),
            is_gateway: false,
            is_compaction: false,
            family: StateFamily::Responses,
        };

        let messages = vec![user_text("u1"), user_text("u2")];
        let [head, middle, tail] = [native_state(0), native_state(1), native_state(2)];

        let sources = Sources::new(UpstreamFormat::OpenAiResponse);

        let window_at = |first_message: usize| {
            let mut projector = Projector::new();

            sources.append_window(
                &messages,
                [&head, &middle, &tail],
                first_message,
                &mut projector,
            );

            projector.evidence().retry.map(|retry| retry.digest)
        };

        assert_same_digest(
            window_at(1),
            direct_digest(|projector| {
                projector.state(ReplayFamily::Responses, StateKind::Native, &middle.item);
                projector.text(Role::User, b"u2");
                projector.state(ReplayFamily::Responses, StateKind::Native, &tail.item);
            }),
        )?;

        // The full window and the suffix window differ: covered history is genuinely excluded.
        assert_different_digest(window_at(1), window_at(0))
    }

    /// The completed-call projection helper fingerprints a valid invocation and declines — without
    /// altering inference — a call whose foreign argument string is not a JSON object.
    #[test]
    fn project_call_gates_on_object_arguments() -> Result<(), Box<dyn StdError>> {
        let mut projector = Projector::new();

        assert!(project_call(
            &mut projector,
            "call_1",
            "find",
            "{\"q\":1}",
            Some("sig")
        ));

        // A declined call alters nothing: the projector stays at the empty-history identity.
        let mut declined = Projector::new();

        assert!(!project_call(
            &mut declined,
            "call_1",
            "find",
            "[1,2]",
            None
        ));

        let mut malformed = Projector::new();

        assert!(!project_call(
            &mut malformed,
            "call_1",
            "find",
            "not json",
            None
        ));

        let empty = Projector::new();

        assert_same_digest(
            declined.evidence().retry.map(|retry| retry.digest),
            empty.evidence().retry.map(|retry| retry.digest),
        )?;
        assert_same_digest(
            malformed.evidence().retry.map(|retry| retry.digest),
            empty.evidence().retry.map(|retry| retry.digest),
        )?;

        assert_different_digest(
            projector.evidence().retry.map(|retry| retry.digest),
            empty.evidence().retry.map(|retry| retry.digest),
        )
    }
}
