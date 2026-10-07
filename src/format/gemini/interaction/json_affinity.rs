//! Borrowed Interactions JSON output projection through the accepted replay vocabulary.

use super::{ContentBlock, RenderStep, TypedStep, client_render::JsonResponse};
use crate::affinity::{
    Digest,
    fingerprint::{
        BorrowedJson, Identity, PreparedReasoning, PreparedText, Projector, ReplayFamily, Role,
    },
};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap as _};

/// Projects the actual rendered Interaction resource: emitted ordered steps through borrowed
/// accepted-replay field views.
pub(super) fn project_json_response(response: &JsonResponse, projector: &mut Projector) -> bool {
    response
        .steps()
        .iter()
        .all(|step| project_step(step, projector))
}

/// Accepted native thought fields, borrowing text and opaque annotation fragments.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Step<'a> {
    /// Replayable reasoning; explicit null optional fields normalize to absence.
    Thought {
        /// Ordered summary blocks, including an explicitly empty array.
        #[serde(borrow)]
        summary: Option<Vec<Content<'a>>>,
        /// Existing producer signature.
        signature: Option<&'a str>,
    },
}

/// Accepted thought summary vocabulary, with exactly the input parser's optional fields.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Content<'a> {
    /// Visible reasoning and producer-defined citations.
    Text {
        /// Existing text bytes.
        text: &'a str,
        /// Citation fragments in recursive canonical order.
        #[serde(borrow)]
        annotations: Option<BorrowedJson<'a>>,
    },
    /// Opaque reasoning media; the thought parser preserves these fields without validation.
    Image {
        /// Optional source URL.
        uri: Option<&'a str>,
        /// Optional inline data.
        data: Option<&'a str>,
        /// Optional inline MIME type.
        mime_type: Option<&'a str>,
        /// Optional resolution hint.
        resolution: Option<&'a str>,
    },
}

impl Content<'_> {
    /// Borrows text fragments that ordinary thought parsing concatenates in order.
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Image { .. } => None,
        }
    }
}

/// Borrowed summary content whose known text field is replaced by an independent digest.
struct ContentFingerprint<'a, 'b> {
    /// Existing parser-normalized content.
    content: &'b Content<'a>,
    /// Process-local field hashing, independent of signature arrival order.
    projector: &'b Projector,
}

impl Serialize for ContentFingerprint<'_, '_> {
    /// Emits the input parser's normalized content with lexical member ordering.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self.content {
            Content::Text { text, annotations } => {
                if let Some(annotations) = annotations {
                    map.serialize_entry("annotations", annotations)?;
                }
                map.serialize_entry("text", &self.projector.reasoning_text_digest(text))?;
                map.serialize_entry("type", "text")?;
            }
            Content::Image {
                uri,
                data,
                mime_type,
                resolution,
            } => {
                if let Some(data) = data {
                    map.serialize_entry("data", data)?;
                }
                if let Some(mime_type) = mime_type {
                    map.serialize_entry("mime_type", mime_type)?;
                }
                if let Some(resolution) = resolution {
                    map.serialize_entry("resolution", resolution)?;
                }
                map.serialize_entry("type", "image")?;
                if let Some(uri) = uri {
                    map.serialize_entry("uri", uri)?;
                }
            }
        }
        map.end()
    }
}

/// Canonical known thought payload with separately hashed text and signature components.
#[derive(Serialize)]
struct Thought<'a, 'b> {
    /// Ordered summary shapes, omitted only when absent or explicitly null.
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<Vec<ContentFingerprint<'a, 'b>>>,
    /// The normalized wire discriminator.
    #[serde(rename = "type")]
    kind: ThoughtKind,
}

/// One streamed summary block holding only a finalized text digest.
#[derive(Serialize)]
struct StreamedSummary {
    /// Actual emitted summary text identity.
    text: Digest,
    /// Fixed accepted summary discriminator.
    #[serde(rename = "type")]
    kind: &'static str,
}

/// Digest-only known thought framing, preserving absent versus emitted-empty summaries.
#[derive(Serialize)]
struct StreamedThought {
    /// The renderer coalesces summary deltas into exactly one text block.
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<[StreamedSummary; 1]>,
    /// Fixed accepted reasoning discriminator.
    #[serde(rename = "type")]
    kind: ThoughtKind,
}

/// Closed discriminator of a normalized thought payload.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThoughtKind {
    /// Model reasoning.
    Thought,
}

/// Projects existing output fields without another serialized step or owned replay IR.
/// Unreplayable output withholds its anchor while preserving inference behavior.
///
/// # Panics
///
/// Panics if accepted canonical fields cannot serialize to the infallible hashing sink or a
/// resident history exhausts its invocation ordinal domain.
pub(super) fn project_step(step: &RenderStep, projector: &mut Projector) -> bool {
    match step {
        RenderStep::Typed(TypedStep::ModelOutput { content }) => {
            for ContentBlock::Text { text } in content {
                projector.text(Role::Assistant, text.as_bytes());
            }
        }
        RenderStep::Typed(TypedStep::FunctionCall {
            id,
            name,
            arguments,
        }) => {
            // A rendered function_call always carries the parsed arguments object; a stored
            // divergence is unreplayable withholds its anchor.
            if !arguments.is_object() {
                return false;
            }

            projector.call(Role::Assistant, Identity::Wire(id), name, arguments, None);
        }
        RenderStep::Typed(TypedStep::Thought { summary, signature }) => {
            let summary = (!summary.is_empty()).then(|| {
                summary
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => Content::Text {
                            text,
                            annotations: None,
                        },
                    })
                    .collect()
            });
            project_thought(projector, summary, signature.as_deref());
        }
        RenderStep::Verbatim(value) => {
            let Ok(Step::Thought { summary, signature }) = Step::deserialize(value) else {
                return false;
            };
            project_thought(projector, summary, signature);
        }
    }
    true
}

/// Prepares an emitted signature-only step without introducing a summary member.
///
/// # Panics
///
/// Panics if the digest-only known payload cannot serialize.
pub(super) fn prepare_signature_thought(
    projector: &Projector,
    signature: Option<&str>,
) -> PreparedReasoning {
    let opaque = StreamedThought {
        summary: None,
        kind: ThoughtKind::Thought,
    };
    projector.prepare_reasoning_canonical(ReplayFamily::Interactions, "", signature, &opaque)
}

/// Completes a coalesced summary from actual text state and independently emitted signature.
///
/// # Panics
///
/// Panics if the digest-only known payload cannot serialize.
pub(super) fn prepare_stream_thought(
    projector: &Projector,
    field: &PreparedText,
    signature: Option<&str>,
) -> PreparedReasoning {
    let opaque = StreamedThought {
        summary: Some([StreamedSummary {
            text: field.digest(),
            kind: "text",
        }]),
        kind: ThoughtKind::Thought,
    };
    projector.prepare_reasoning_fields(ReplayFamily::Interactions, field, signature, &opaque)
}

/// Hashes normalized summary fragments and their canonical opaque payload without joining text.
fn project_thought(
    projector: &mut Projector,
    summary: Option<Vec<Content<'_>>>,
    signature: Option<&str>,
) {
    let opaque = Thought {
        summary: summary.as_ref().map(|summary| {
            summary
                .iter()
                .map(|content| ContentFingerprint {
                    content,
                    projector: &*projector,
                })
                .collect()
        }),
        kind: ThoughtKind::Thought,
    };
    let mut field = projector.prepare_text(Role::Assistant, false);
    for text in summary
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(Content::text)
    {
        field.update(text.as_bytes());
    }
    let reasoning =
        projector.prepare_reasoning_fields(ReplayFamily::Interactions, &field, signature, &opaque);
    projector.reasoning_prepared(Role::Assistant, reasoning);
}
