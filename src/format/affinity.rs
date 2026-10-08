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
