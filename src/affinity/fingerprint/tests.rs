//! Tests for the canonical content-free fingerprint: which parts of a conversation participate in
//! pool identities, the exact and relational projections, call-ID and reference canonicalization,
//! and the completed anchors a codec may register (`docs/design/routing.md` *Conversation
//! affinity*).

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{Identity, Projector, ReplayFamily, ResultContent, Role, StateKind};
use crate::affinity::{Anchor, Evidence, Projection};
use serde_json::{Value, json};

/// The canonical unit prefixes of one evidence value as (unit count, digest bytes) pairs.
fn prefixes(evidence: &Evidence) -> Vec<(u64, [u8; 32])> {
    evidence
        .history
        .iter()
        .map(|prefix| (prefix.units.get(), prefix.digest.0))
        .collect()
}

/// The complete-request retry identity as (projection, digest bytes), when the request was
/// fingerprintable at all.
fn retry(evidence: &Evidence) -> Option<(Projection, [u8; 32])> {
    let retry = evidence.retry.as_ref()?;

    Some((retry.projection, retry.digest.0))
}

/// The projection of the complete-request retry identity, when fingerprintable.
fn retry_projection(evidence: &Evidence) -> Option<Projection> {
    retry(evidence).map(|(projection, _)| projection)
}

/// The history prefix carried by a completed-history anchor, and nothing for an artifact anchor.
fn history_anchor(anchor: &Anchor) -> Option<(u64, [u8; 32])> {
    match anchor {
        Anchor::History(prefix) => Some((prefix.units.get(), prefix.digest.0)),
        Anchor::Artifact(_) => None,
    }
}

/// Projects one minimal user-plus-assistant text conversation; an empty side contributes no text.
fn text_conversation(user: &str, assistant: &str) -> Projector {
    let mut projector = Projector::new();

    projector.text(Role::User, user.as_bytes());
    projector.text(Role::Assistant, assistant.as_bytes());

    projector
}

/// Two identical conversations fingerprint identically, and every content or role change moves
/// the identity: the fingerprint is a content-free but content-sensitive conversation identity.
#[test]
fn identical_conversations_agree_and_content_moves_the_identity() {
    let reference = text_conversation("hello", "hi");
    let replay = text_conversation("hello", "hi");
    let other_text = text_conversation("hello", "bye");
    let other_role = text_conversation("hi", "hello");

    assert_eq!(retry(&reference.evidence()), retry(&replay.evidence()));
    assert_ne!(retry(&reference.evidence()), retry(&other_text.evidence()));
    assert_ne!(retry(&reference.evidence()), retry(&other_role.evidence()));
}

/// Text is hashed incrementally: one chunk and the same bytes split across deltas, including a
/// UTF-8 boundary split, project identically, and an empty delta is no content at all.
#[test]
fn split_text_deltas_project_identically_and_empty_deltas_are_absent() {
    let whole = text_conversation("héllo", "hi");

    let mut split = Projector::new();

    split.text(Role::User, b"h");
    split.text(Role::User, b"\xc3\xa9");
    split.text(Role::User, b"llo");
    split.text(Role::Assistant, b"hi");

    let mut with_empty = text_conversation("héllo", "hi");

    with_empty.text(Role::User, b"");

    assert_eq!(retry(&whole.evidence()), retry(&split.evidence()));
    assert_eq!(retry(&whole.evidence()), retry(&with_empty.evidence()));
    assert_eq!(
        prefixes(&whole.evidence()),
        prefixes(&with_empty.evidence())
    );
}

/// Adjacent same-role text is one maximal run: two user deltas and their concatenation are the
/// same canonical unit, while a role change closes the run and starts another unit.
#[test]
fn same_role_text_merges_and_role_changes_close_runs() {
    let merged = text_conversation("helloworld", "");
    let mut deltas = Projector::new();

    deltas.text(Role::User, b"hello");
    deltas.text(Role::User, b"world");

    let two_roles = text_conversation("hello", "world");

    assert_eq!(retry(&merged.evidence()), retry(&deltas.evidence()));
    assert_eq!(prefixes(&merged.evidence()), prefixes(&deltas.evidence()));
    assert_eq!(prefixes(&merged.evidence()).len(), 1);
    assert_eq!(prefixes(&two_roles.evidence()).len(), 2);
    assert_ne!(retry(&merged.evidence()), retry(&two_roles.evidence()));
}

/// A non-text part closes the open text run: text beside an image projects as two text parts in
/// one unit, not as one concatenated run, and image metadata and payload are distinct fields.
#[test]
fn image_parts_split_text_runs() {
    let mut with_image = text_conversation("hello", "");

    with_image.inline_image(Role::User, "image/png", "cHJpdmF0ZQ");
    with_image.text(Role::User, "world".as_bytes());

    let mut other_image = text_conversation("hello", "");

    other_image.inline_image(Role::User, "image/png", "other");
    other_image.text(Role::User, "world".as_bytes());

    let mut url_image = text_conversation("hello", "");

    url_image.url_image(Role::User, "https://example.test/a.png");
    url_image.text(Role::User, "world".as_bytes());

    assert_eq!(prefixes(&with_image.evidence()).len(), 1);
    assert_ne!(
        retry(&with_image.evidence()),
        retry(&other_image.evidence())
    );
    assert_ne!(retry(&with_image.evidence()), retry(&url_image.evidence()));
}

/// A replayable state boundary is its own canonical unit: same-role text cannot merge across it,
/// so user-text/state/user-text is three units, and the payload's members are canonicalized by
/// sorted key, not by wire order.
#[test]
fn state_boundaries_split_units_and_canonicalize_their_payload() {
    let payload = json!({"b": 1, "a": 2});
    let reordered = json!({"a": 2, "b": 1});

    let mut with_state = text_conversation("hello", "");

    with_state.state(ReplayFamily::Responses, StateKind::Native, &payload);
    with_state.text(Role::User, b"world");

    let mut reordered_state = text_conversation("hello", "");

    reordered_state.state(ReplayFamily::Responses, StateKind::Native, &reordered);
    reordered_state.text(Role::User, b"world");

    assert_eq!(prefixes(&with_state.evidence()).len(), 3);
    assert_eq!(
        retry(&with_state.evidence()),
        retry(&reordered_state.evidence())
    );
}

/// The state kind and carrier framing participate: the same payload projects differently as
/// native state and as a sealed gateway carrier, whose carrier string is framed as JSON text.
#[test]
fn state_kind_and_carrier_framing_participate() {
    let payload = json!({"compaction": true});

    let mut native = Projector::new();

    native.state(ReplayFamily::Responses, StateKind::Native, &payload);

    let mut gateway = Projector::new();

    gateway.state(ReplayFamily::Responses, StateKind::Gateway, &payload);

    let mut carrier = Projector::new();

    carrier.gateway_state(ReplayFamily::Responses, r#"{"compaction":true}"#);

    assert_ne!(retry(&native.evidence()), retry(&gateway.evidence()));
    assert_ne!(retry(&native.evidence()), retry(&carrier.evidence()));
}

/// Wire-identified calls and results participate in both projections: exact and relational
/// digests diverge from the call onward, so the merged history carries both strengths' prefixes
/// while their shared user-turn prefix is counted once.
#[test]
fn wire_calls_diverge_the_projections_from_the_call_onward() {
    let mut projector = Projector::new();

    projector.text(Role::User, b"question");
    projector.call(
        Role::Assistant,
        Identity::Wire("call-1"),
        "get_weather",
        &json!({"city": "Oslo"}),
        None,
    );
    projector.result(
        Role::Tool,
        Identity::Wire("call-1"),
        ResultContent::Text("sunny"),
        false,
    );

    let evidence = projector.evidence();

    assert_eq!(prefixes(&evidence).len(), 5);
    assert_eq!(retry_projection(&evidence), Some(Projection::Exact));
}

/// The relational projection is independent of the actual call-ID strings: conversations whose
/// wire IDs differ share their relational anchor and diverge on the exact one.
#[test]
fn relational_projection_ignores_wire_call_ids() {
    let project = |call_id: &str| {
        let mut projector = text_conversation("question", "");

        projector.call(
            Role::Assistant,
            Identity::Wire(call_id),
            "f",
            &json!({}),
            None,
        );
        projector.result(
            Role::Tool,
            Identity::Wire(call_id),
            ResultContent::Text("r"),
            false,
        );

        projector
    };

    let first = project("aaa");
    let second = project("bbb");

    let first_anchors = first.completed_anchors();
    let second_anchors = second.completed_anchors();

    assert_eq!(first_anchors.len(), 2);
    assert_ne!(
        first_anchors.first().and_then(history_anchor),
        second_anchors.first().and_then(history_anchor)
    );
    assert_eq!(
        first_anchors.get(1).and_then(history_anchor),
        second_anchors.get(1).and_then(history_anchor)
    );
}

/// Synthetic identities — codec-resolved id-less or legacy references — never enter the exact
/// projection: the exact path stops at the call, the relational path stays complete, and the
/// resolved lookup bytes stay internal.
#[test]
fn synthetic_calls_stop_only_the_exact_projection() {
    let mut projector = text_conversation("question", "");

    projector.call(
        Role::Assistant,
        Identity::Synthetic("legacy-0"),
        "f",
        &json!({}),
        None,
    );
    projector.result(
        Role::Tool,
        Identity::Synthetic("legacy-0"),
        ResultContent::Text("r"),
        false,
    );

    let evidence = projector.evidence();

    assert_eq!(prefixes(&evidence).len(), 3);
    assert_eq!(retry_projection(&evidence), Some(Projection::Relational));
}

/// Function arguments are canonical foreign JSON: member order never moves a call identity.
#[test]
fn call_arguments_are_canonicalized() {
    let mut ordered = Projector::new();

    ordered.call(
        Role::Assistant,
        Identity::Wire("c"),
        "f",
        &json!({"a": 1, "b": 2}),
        None,
    );

    let mut reordered = Projector::new();

    reordered.call(
        Role::Assistant,
        Identity::Wire("c"),
        "f",
        &json!({"b": 2, "a": 1}),
        None,
    );

    assert_eq!(retry(&ordered.evidence()), retry(&reordered.evidence()));
}

/// A repeated unresolved call identity poisons both projections terminally: the retry identity
/// disappears and later well-formed content cannot revive either path.
#[test]
fn duplicate_unresolved_call_ids_stop_both_projections() {
    let mut projector = text_conversation("question", "");

    projector.call(Role::Assistant, Identity::Wire("x"), "f", &json!({}), None);
    projector.call(Role::Assistant, Identity::Wire("x"), "f", &json!({}), None);

    let stopped = projector.evidence();

    assert!(retry(&stopped).is_none());
    assert_eq!(prefixes(&stopped).len(), 1);

    let mut continued = projector;

    continued.text(Role::User, b"more");
    continued.call(Role::Assistant, Identity::Wire("y"), "g", &json!({}), None);
    continued.result(
        Role::Tool,
        Identity::Wire("y"),
        ResultContent::Text("r"),
        false,
    );

    let evidence = continued.evidence();

    assert!(retry(&evidence).is_none());
    assert_eq!(prefixes(&evidence).len(), 1);
}

/// A result with no resolved call reference, and a second result for an already-consumed one,
/// each stop both projections.
#[test]
fn unresolved_and_double_results_stop_both_projections() {
    let mut missing = Projector::new();

    missing.text(Role::User, b"q");
    missing.result(
        Role::Tool,
        Identity::Wire("nope"),
        ResultContent::Text("r"),
        false,
    );

    let missing_evidence = missing.evidence();

    assert!(retry(&missing_evidence).is_none());
    assert_eq!(prefixes(&missing_evidence).len(), 1);

    let mut doubled = Projector::new();

    doubled.call(Role::Assistant, Identity::Wire("c"), "f", &json!({}), None);
    doubled.result(
        Role::Tool,
        Identity::Wire("c"),
        ResultContent::Text("r"),
        false,
    );
    doubled.result(
        Role::Tool,
        Identity::Wire("c"),
        ResultContent::Text("r"),
        false,
    );

    let doubled_evidence = doubled.evidence();

    // The duplicated result poisons the still-open tool run wholesale: neither projection can
    // complete a unit boundary around it, so only the earlier call unit survives in each.
    assert!(retry(&doubled_evidence).is_none());
    assert_eq!(prefixes(&doubled_evidence).len(), 2);
}

/// Sequential legacy call/result pairs may reuse one codec-synthesized lookup identity, while two
/// unresolved synthetic calls in a row cannot: the pairing already established by the codec is
/// preserved, never re-derived.
#[test]
fn sequential_synthetic_pairs_are_accepted_and_unmatched_repeats_stop() {
    let mut paired = Projector::new();

    paired.call(
        Role::Assistant,
        Identity::Synthetic("s"),
        "f",
        &json!({}),
        None,
    );
    paired.result(
        Role::Tool,
        Identity::Synthetic("s"),
        ResultContent::Text("1"),
        false,
    );
    paired.call(
        Role::Assistant,
        Identity::Synthetic("s"),
        "f",
        &json!({}),
        None,
    );
    paired.result(
        Role::Tool,
        Identity::Synthetic("s"),
        ResultContent::Text("2"),
        false,
    );

    let paired_evidence = paired.evidence();

    assert_eq!(
        retry_projection(&paired_evidence),
        Some(Projection::Relational)
    );

    // The exact path holds no completed prefix — it stopped at the first call — so the merged
    // history is the relational projection's four units alone.
    assert_eq!(prefixes(&paired_evidence).len(), 4);

    let mut repeated = Projector::new();

    repeated.call(
        Role::Assistant,
        Identity::Synthetic("s"),
        "f",
        &json!({}),
        None,
    );
    repeated.call(
        Role::Assistant,
        Identity::Synthetic("s"),
        "f",
        &json!({}),
        None,
    );

    assert!(retry(&repeated.evidence()).is_none());
}

/// Results match their calls by the codec's resolved reference, not by order: reversed result
/// order still completes both projections, and the two results of one assistant turn stay one
/// canonical unit per projection.
#[test]
fn out_of_order_results_still_complete_both_projections() {
    let mut projector = Projector::new();

    projector.call(Role::Assistant, Identity::Wire("1"), "f", &json!({}), None);
    projector.call(Role::Assistant, Identity::Wire("2"), "g", &json!({}), None);
    projector.result(
        Role::Tool,
        Identity::Wire("2"),
        ResultContent::Text("two"),
        false,
    );
    projector.result(
        Role::Tool,
        Identity::Wire("1"),
        ResultContent::Text("one"),
        false,
    );

    let evidence = projector.evidence();

    // The two calls form one assistant unit and the two results one tool unit per projection,
    // and no unit is shared between the exact and relational strengths here.
    assert_eq!(retry_projection(&evidence), Some(Projection::Exact));
    assert_eq!(prefixes(&evidence).len(), 4);
}

/// Result content and its error flag participate: the same reference with different text, JSON
/// instead of text, or a flipped error flag projects differently.
#[test]
fn result_content_and_error_flag_participate() {
    let project = |content: ResultContent, is_error: bool| {
        let mut projector = Projector::new();

        projector.call(Role::Assistant, Identity::Wire("c"), "f", &json!({}), None);
        projector.result(Role::Tool, Identity::Wire("c"), content, is_error);

        projector
    };

    let text = project(ResultContent::Text("r"), false);
    let other_text = project(ResultContent::Text("s"), false);
    let json_content = project(ResultContent::Json(&json!({"r": true})), false);
    let is_error = project(ResultContent::Text("r"), true);

    assert_ne!(retry(&text.evidence()), retry(&other_text.evidence()));
    assert_ne!(retry(&text.evidence()), retry(&json_content.evidence()));
    assert_ne!(retry(&text.evidence()), retry(&is_error.evidence()));
}

/// A reasoning block without replayable payload is family-neutral: the same text projects
/// identically under every family, while a signature or opaque payload binds the family and
/// participates with its own value.
#[test]
fn reasoning_family_binds_only_with_a_replayable_payload() {
    let text_only = |family| {
        let mut projector = Projector::new();

        projector.reasoning(Role::Assistant, family, "deep", None, None);
        projector
    };

    assert_eq!(
        retry(&text_only(ReplayFamily::Messages).evidence()),
        retry(&text_only(ReplayFamily::Responses).evidence())
    );

    let signed = |family, signature: &str| {
        let mut projector = Projector::new();

        projector.reasoning(Role::Assistant, family, "deep", Some(signature), None);
        projector
    };

    assert_ne!(
        retry(&signed(ReplayFamily::Messages, "sig").evidence()),
        retry(&signed(ReplayFamily::Responses, "sig").evidence())
    );
    assert_ne!(
        retry(&signed(ReplayFamily::Messages, "sig").evidence()),
        retry(&signed(ReplayFamily::Messages, "2").evidence())
    );

    let mut with_opaque = Projector::new();

    with_opaque.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "deep",
        None,
        Some(&json!({"i": 1})),
    );

    assert_ne!(
        retry(&text_only(ReplayFamily::Messages).evidence()),
        retry(&with_opaque.evidence())
    );
}

/// Codec-owned Messages reasoning payloads hash their visible text and exclude their signature
/// member: two payloads differing only in the signature value share one identity, while a text
/// or foreign-member change moves it, and unknown fragments stay fully canonical.
#[test]
fn messages_reasoning_payloads_exclude_signatures_and_digest_text() {
    let thought = |thinking: &str, signature: &str| {
        json!({
            "type": "thinking",
            "thinking": thinking,
            "signature": signature,
            "detail": "kept",
        })
    };

    let reasoning = |opaque: &Value| {
        let mut projector = Projector::new();

        projector.reasoning(
            Role::Assistant,
            ReplayFamily::Messages,
            "deep",
            None,
            Some(opaque),
        );
        projector
    };

    assert_eq!(
        retry(&reasoning(&thought("t", "AAA")).evidence()),
        retry(&reasoning(&thought("t", "BBB")).evidence())
    );
    assert_ne!(
        retry(&reasoning(&thought("t", "AAA")).evidence()),
        retry(&reasoning(&thought("other", "AAA")).evidence())
    );
    assert_ne!(
        retry(&reasoning(&thought("t", "AAA")).evidence()),
        retry(
            &reasoning(&json!({
                "type": "thinking",
                "thinking": "t",
                "signature": "AAA",
                "detail": "changed",
            }))
            .evidence()
        )
    );

    let mut foreign_first = Projector::new();

    foreign_first.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "deep",
        None,
        Some(&json!({"type": "custom", "signature": "AAA"})),
    );

    let mut foreign_second = Projector::new();

    foreign_second.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "deep",
        None,
        Some(&json!({"type": "custom", "signature": "BBB"})),
    );

    assert_ne!(
        retry(&foreign_first.evidence()),
        retry(&foreign_second.evidence())
    );
}

/// Interactions thought payloads digest their ordered summary text and exclude the thought-level
/// signature; a non-text summary block stays fully canonical.
#[test]
fn interactions_thought_payloads_digest_summary_text_and_exclude_signatures() {
    let thought = |second: &str, signature: &str| {
        json!({
            "type": "thought",
            "signature": signature,
            "summary": [
                {"type": "text", "text": "first"},
                {"type": "text", "text": second},
            ],
        })
    };

    let reasoning = |opaque: &Value| {
        let mut projector = Projector::new();

        projector.reasoning(
            Role::Assistant,
            ReplayFamily::Interactions,
            "deep",
            None,
            Some(opaque),
        );
        projector
    };

    assert_eq!(
        retry(&reasoning(&thought("s", "AAA")).evidence()),
        retry(&reasoning(&thought("s", "BBB")).evidence())
    );
    assert_ne!(
        retry(&reasoning(&thought("s", "AAA")).evidence()),
        retry(&reasoning(&thought("other", "AAA")).evidence())
    );
    assert_ne!(
        retry(&reasoning(&thought("s", "AAA")).evidence()),
        retry(
            &reasoning(&json!({
                "type": "thought",
                "signature": "AAA",
                "summary": [
                    {"type": "text", "text": "first"},
                    {"type": "other", "value": "s"},
                ],
            }))
            .evidence()
        )
    );
}

/// Only a completed assistant output or replayable state anchors a conversation: a user-only
/// history establishes nothing, while completed output yields one deduplicated anchor carrying
/// the full-history prefix.
#[test]
fn anchors_require_completed_output_or_state() {
    let user_only = text_conversation("hello", "");

    assert!(user_only.completed_anchors().is_empty());

    let completed = text_conversation("hello", "hi");
    let anchors = completed.completed_anchors();
    let evidence = completed.evidence();

    assert_eq!(anchors.len(), 1);
    assert_eq!(
        anchors.first().and_then(history_anchor),
        prefixes(&evidence).last().copied()
    );

    let mut state_only = text_conversation("hello", "");

    state_only.state(ReplayFamily::Responses, StateKind::Native, &json!({"s": 1}));

    assert_eq!(state_only.completed_anchors().len(), 1);
}

/// Wire-identified calls split the exact and relational anchors into two distinct strengths,
/// while a synthetic call leaves only the relational strength anchorable.
#[test]
fn call_strengths_split_the_completed_anchors() {
    let mut wire = text_conversation("q", "");

    wire.call(Role::Assistant, Identity::Wire("c"), "f", &json!({}), None);
    wire.result(
        Role::Tool,
        Identity::Wire("c"),
        ResultContent::Text("r"),
        false,
    );

    assert_eq!(wire.completed_anchors().len(), 2);

    let mut synthetic = text_conversation("q", "");

    synthetic.call(
        Role::Assistant,
        Identity::Synthetic("s"),
        "f",
        &json!({}),
        None,
    );
    synthetic.result(
        Role::Tool,
        Identity::Synthetic("s"),
        ResultContent::Text("r"),
        false,
    );

    let anchors = synthetic.completed_anchors();
    let evidence = synthetic.evidence();

    assert_eq!(anchors.len(), 1);
    assert_eq!(
        anchors.first().and_then(history_anchor),
        prefixes(&evidence).last().copied()
    );
}
