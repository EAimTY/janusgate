//! Borrowed replay projection of the actual standalone compact response's canonical window.

use crate::affinity::fingerprint::{
    BorrowedJson as Foreign, Identity, Projector, ResultContent, Role, StateFamily, StateKind,
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

/// Accepted conversation fields borrowed from the already rendered vendor-owned output.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Item<'a> {
    /// Text/media conversation turn; presentation fields carry no history identity.
    Message {
        /// Ignored resource identity.
        #[serde(rename = "id")]
        _id: Option<IgnoredAny>,
        /// Ordinary role or instruction role.
        role: &'a str,
        /// Accepted text and image parts.
        #[serde(borrow)]
        content: Content<'a, Part<'a>>,
        /// Ignored presentation phase.
        #[serde(rename = "phase")]
        _phase: Option<IgnoredAny>,
        /// Ignored lifecycle state.
        #[serde(rename = "status")]
        _status: Option<IgnoredAny>,
    },
    /// Real wire invocation identity and exact accepted foreign argument object.
    FunctionCall {
        /// Ignored resource identity.
        #[serde(rename = "id")]
        _id: Option<IgnoredAny>,
        /// Invocation identity.
        call_id: &'a str,
        /// Function name.
        name: &'a str,
        /// Empty arguments normalize to an empty object on ordinary replay.
        #[serde(default)]
        arguments: &'a str,
        /// Ignored lifecycle state.
        #[serde(rename = "status")]
        _status: Option<IgnoredAny>,
        /// Replayable invocation signature.
        thought_signature: Option<&'a str>,
    },
    /// Ordinary tool result, with the codec's text-array normalization.
    FunctionCallOutput {
        /// Ignored resource identity.
        #[serde(rename = "id")]
        _id: Option<IgnoredAny>,
        /// Invocation being answered.
        call_id: &'a str,
        /// Accepted plain text or text-only part array.
        #[serde(borrow)]
        output: Content<'a, ToolPart<'a>>,
        /// Ignored lifecycle state.
        #[serde(rename = "status")]
        _status: Option<IgnoredAny>,
    },
    /// Only encrypted reasoning becomes replayable state.
    #[serde(borrow)]
    Reasoning(Reasoning<'a>),
    /// Complete provider state or a newly sealed gateway carrier.
    #[serde(borrow, alias = "compaction_summary")]
    Compaction(Compaction<'a>),
}

/// A wire string or ordered part array, without an owned string fallback.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content<'a, P> {
    /// Exact existing text bytes.
    Text(#[serde(borrow)] &'a str),
    /// Borrowed typed parts and their small ordering metadata.
    Parts(Vec<P>),
}

/// The ordinary Responses content-part vocabulary.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Part<'a> {
    /// Client text.
    InputText {
        /// Exact text bytes.
        text: &'a str,
    },
    /// Retained assistant text.
    OutputText {
        /// Exact text bytes.
        text: &'a str,
        /// Accepted, ignored annotations.
        #[serde(rename = "annotations")]
        _annotations: Option<IgnoredAny>,
        /// Accepted, ignored probabilities.
        #[serde(rename = "logprobs")]
        _logprobs: Option<IgnoredAny>,
    },
    /// Accepted inline or remote image reference.
    InputImage {
        /// Exact image URL bytes.
        image_url: &'a str,
        /// Accepted, ignored detail hint.
        #[serde(rename = "detail")]
        _detail: Option<IgnoredAny>,
    },
}

/// Tool results accept only text parts; unsupported media cannot create a history anchor.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ToolPart<'a> {
    /// Exact existing result bytes.
    InputText {
        /// Borrowed accepted result text.
        text: &'a str,
    },
}

/// Reasoning's accepted replay fields, declared in canonical member order. Unknown members
/// are ignored, exactly as in the ordinary input codec.
#[derive(Serialize, Deserialize)]
struct Reasoning<'a> {
    /// Foreign reasoning content, when present.
    #[serde(borrow, skip_serializing_if = "Option::is_none")]
    content: Option<Vec<Foreign<'a>>>,
    /// Presence makes this item an opaque state boundary.
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted_content: Option<&'a str>,
    /// Existing reasoning identity; null normalizes to absence on input.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
    /// Ordinary replay always supplies a summary array, even when omitted upstream.
    #[serde(borrow, default)]
    summary: Vec<Foreign<'a>>,
}

/// Optional native text whose explicit null must survive ordinary replay.
#[derive(Default, Serialize, Deserialize)]
#[serde(untagged)]
enum OptionalText<'a> {
    /// Absent wire member.
    #[default]
    #[serde(skip)]
    Missing,
    /// Present string or null.
    Present(#[serde(borrow)] Option<&'a str>),
}

impl OptionalText<'_> {
    /// Whether ordinary serialization omits this member.
    const fn is_absent(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

/// Strict native state fields, declared in canonical member order.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Compaction<'a> {
    /// Optional authoring attribution, preserving explicit null.
    #[serde(borrow, default, skip_serializing_if = "OptionalText::is_absent")]
    created_by: OptionalText<'a>,
    /// Exact replay payload bytes.
    encrypted_content: &'a str,
    /// Optional native identity, preserving explicit null.
    #[serde(borrow, default, skip_serializing_if = "OptionalText::is_absent")]
    id: OptionalText<'a>,
}

/// Codec-normalized state tag, after every lexically earlier replay member.
#[derive(Serialize)]
struct Tagged<'a, T> {
    /// Already canonical typed fields and sorted foreign fragments.
    #[serde(flatten)]
    fields: &'a T,
    /// Canonical ordinary replay discriminator, including alias normalization.
    #[serde(rename = "type")]
    kind: &'static str,
}

/// Projects only a completely replayable actual output window, starting a new history root
/// independently of the covered request. A gateway marker requires our sealer-proven sidecar.
pub(super) fn project(
    output: &[Value],
    has_gateway_provenance: bool,
    should_start_at_boundary: bool,
) -> Option<Projector> {
    let mut projector = Projector::new();
    let first_item = if should_start_at_boundary {
        output
            .iter()
            .rposition(|value| matches!(Item::deserialize(value), Ok(Item::Compaction(_))))?
    } else {
        0
    };
    for value in output.iter().skip(first_item) {
        let item = Item::deserialize(value).ok()?;
        match item {
            Item::Message { role, content, .. } => {
                let role = match role {
                    "user" => Some(Role::User),
                    "assistant" => Some(Role::Assistant),
                    "system" | "developer" => None,
                    _ => return None,
                };
                match content {
                    Content::Text(text) => {
                        if let Some(role) = role {
                            projector.text(role, text.as_bytes());
                        }
                    }
                    Content::Parts(parts) => {
                        for part in parts {
                            match part {
                                Part::InputText { text } | Part::OutputText { text, .. } => {
                                    if let Some(role) = role {
                                        projector.text(role, text.as_bytes());
                                    }
                                }
                                Part::InputImage { image_url, .. } => {
                                    image(&mut projector, role?, image_url)?;
                                }
                            }
                        }
                    }
                }
            }
            Item::FunctionCall {
                call_id,
                name,
                arguments,
                thought_signature,
                ..
            } => {
                let arguments = crate::format::arguments_as_object(arguments).ok()?;
                projector.call(
                    Role::Assistant,
                    Identity::Wire(call_id),
                    name,
                    &Value::Object(arguments),
                    thought_signature,
                );
            }
            Item::FunctionCallOutput {
                call_id, output, ..
            } => match output {
                Content::Text(text) => {
                    projector.result(
                        Role::Tool,
                        Identity::Wire(call_id),
                        ResultContent::Text(text),
                        false,
                    );
                }
                Content::Parts(parts) => {
                    let chunks = parts
                        .iter()
                        .map(|ToolPart::InputText { text }| *text)
                        .skip_while(|text| text.is_empty())
                        .enumerate()
                        .flat_map(|(index, text)| [if index == 0 { "" } else { "\n\n" }, text]);
                    projector.result_text_chunks(
                        Role::Tool,
                        Identity::Wire(call_id),
                        chunks,
                        false,
                    );
                }
            },
            Item::Reasoning(reasoning) => {
                if reasoning.encrypted_content.is_some() {
                    projector.state_canonical(
                        StateFamily::Responses,
                        StateKind::Native,
                        &Tagged {
                            fields: &reasoning,
                            kind: "reasoning",
                        },
                    );
                }
            }
            Item::Compaction(compaction) => {
                if crate::compaction::sealing::is_gateway_marker(compaction.encrypted_content) {
                    if !has_gateway_provenance {
                        return None;
                    }
                    projector.gateway_state(StateFamily::Responses, compaction.encrypted_content);
                } else {
                    projector.state_canonical(
                        StateFamily::Responses,
                        StateKind::Native,
                        &Tagged {
                            fields: &compaction,
                            kind: "compaction",
                        },
                    );
                }
            }
        }
    }
    Some(projector)
}

/// Borrows the ordinary codec's accepted URL decomposition without copying MIME, data, or URL.
fn image(projector: &mut Projector, role: Role, url: &str) -> Option<()> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (metadata, data) = rest.split_once(',')?;
        let mime_type = metadata.strip_suffix(";base64")?;
        projector.inline_image(role, mime_type, data);
    } else if url.starts_with("http://") || url.starts_with("https://") {
        projector.url_image(role, url);
    } else {
        return None;
    }
    Some(())
}

/// Borrows and normalizes one completed native state exactly as ordinary Responses replay.
/// A reflected gateway marker lacks authenticated provenance and cannot use this native path.
pub(super) fn prepare_native_state(
    value: &Value,
    projector: &Projector,
) -> Option<crate::affinity::fingerprint::PreparedState> {
    match Item::deserialize(value).ok()? {
        Item::Reasoning(reasoning) if reasoning.encrypted_content.is_some() => {
            Some(projector.prepare_state_canonical(
                StateFamily::Responses,
                StateKind::Native,
                &Tagged {
                    fields: &reasoning,
                    kind: "reasoning",
                },
            ))
        }
        Item::Compaction(compaction)
            if !crate::compaction::sealing::is_gateway_marker(compaction.encrypted_content) =>
        {
            Some(projector.prepare_state_canonical(
                StateFamily::Responses,
                StateKind::Native,
                &Tagged {
                    fields: &compaction,
                    kind: "compaction",
                },
            ))
        }
        _ => None,
    }
}

/// Normalizes one fully replayable raw native checkpoint and its independent active-window hints.
/// Gateway markers need their sealer-authenticated sidecar and cannot use this native path.
pub(super) fn prepare_native_checkpoint(
    value: &Value,
    projector: &Projector,
) -> Option<(
    crate::affinity::fingerprint::PreparedState,
    Vec<crate::affinity::Anchor>,
)> {
    let artifact = super::compaction::native_artifact_anchor(value, projector)?;
    let state = prepare_native_state(value, projector)?;
    let mut window = Projector::new();
    window.state_prepared(state);
    let mut anchors = window.completed_anchors();
    anchors.push(artifact);
    Some((state, anchors))
}
