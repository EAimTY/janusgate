//! Canonical role/state placement, tool identity, JSON semantics, and streaming replay invariants.

use super::*;
use serde_json::json;

/// Deterministic process-key substitute for first-party projection invariants.
fn projector() -> Projector {
    Projector::with_key(Key::from_bytes([7; 32]))
}

/// Complete-request identity without exposing its bytes in test reports.
fn retry(projector: &Projector) -> Option<(Projection, Digest)> {
    projector
        .evidence()
        .retry
        .map(|retry| (retry.projection, retry.digest))
}

/// Role runs merge through empty parts, preserve byte concatenation, and break at state items.
/// Snapshots do not close the open text field or turn an assistant extension into a second unit.
///
/// # Panics
///
/// Panics if replay/chunking changes identity or meaningful role/state placement collapses.
#[test]
fn role_runs_and_streamed_input_tail_share_canonical_prefixes() {
    let mut streamed = projector();
    streamed.text(Role::User, b"question");
    streamed.text(Role::Assistant, b"old");
    let before = streamed.evidence();
    streamed.text(Role::User, b"");
    streamed.text(Role::Assistant, b"new");
    streamed.text(Role::Assistant, b"\xe4\xbd");
    streamed.text(Role::Assistant, b"\xa0");
    let mut replayed = projector();
    replayed.text(Role::User, b"question");
    replayed.text(Role::Assistant, "oldnew你".as_bytes());
    assert!(retry(&streamed) == retry(&replayed));
    assert!(
        before.history.first().map(|prefix| prefix.digest)
            == streamed
                .evidence()
                .history
                .first()
                .map(|prefix| prefix.digest)
    );
    assert_eq!(streamed.evidence().history.len(), 2);
    assert_eq!(streamed.completed_anchors().len(), 1);

    let mut interrupted = projector();
    interrupted.text(Role::User, b"question");
    interrupted.text(Role::Assistant, b"old");
    interrupted.state(
        ReplayFamily::Responses,
        StateKind::Native,
        &json!({"token":"x"}),
    );
    interrupted.text(Role::Assistant, "new你".as_bytes());
    assert!(retry(&interrupted) != retry(&replayed));
    assert_eq!(interrupted.evidence().history.len(), 4);
    let mut first = projector();
    first.text(Role::User, b"common");
    first.text(Role::Assistant, b"");
    assert!(first.completed_anchors().is_empty());
}

/// Recursive foreign object ordering is irrelevant; arrays, normal scalar representations,
/// field domains, role, and multimodal part order retain their actual accepted semantics.
///
/// # Panics
///
/// Panics if normalization erases meaningful bytes or retains foreign object insertion order.
#[test]
fn foreign_json_and_typed_parts_preserve_meaningful_distinctions() {
    let arguments = json!({"z":[{"b":2,"a":1}], "a":true});
    let reordered = json!({"a":true,"z":[{"a":1,"b":2}]});
    let mut first = projector();
    first.call(
        Role::Assistant,
        Identity::Wire("call"),
        "tool",
        &arguments,
        None,
    );
    let mut same = projector();
    same.call(
        Role::Assistant,
        Identity::Wire("call"),
        "tool",
        &reordered,
        None,
    );
    assert!(retry(&first) == retry(&same));
    let mut different = projector();
    different.call(
        Role::Assistant,
        Identity::Wire("call"),
        "tool",
        &json!({"a":true,"z":[{"a":1.0,"b":2}]}),
        None,
    );
    assert!(retry(&first) != retry(&different));

    let mut image = projector();
    image.inline_image(Role::User, "image/png", "abc");
    image.text(Role::User, b"url");
    let mut reversed = projector();
    reversed.text(Role::User, b"url");
    reversed.inline_image(Role::User, "image/png", "abc");
    assert!(retry(&image) != retry(&reversed));
    let mut url = projector();
    url.url_image(Role::User, "abc");
    assert!(retry(&url) != retry(&image));
    let mut text = projector();
    text.text(Role::User, b"abc");
    assert!(retry(&url) != retry(&text));
    let mut other_key = Projector::with_key(Key::from_bytes([8; 32]));
    other_key.text(Role::User, b"abc");
    assert!(retry(&text) != retry(&other_key));

    let mut reasoning = projector();
    reasoning.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "thought",
        Some("signature"),
        Some(&json!({"b":2,"a":1})),
    );
    let mut same_reasoning = projector();
    same_reasoning.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "thought",
        Some("signature"),
        Some(&json!({"a":1,"b":2})),
    );
    assert!(retry(&reasoning) == retry(&same_reasoning));
    let mut foreign_reasoning = projector();
    foreign_reasoning.reasoning(
        Role::Assistant,
        ReplayFamily::Interactions,
        "thought",
        Some("signature"),
        Some(&json!({"a":1,"b":2})),
    );
    assert!(retry(&reasoning) != retry(&foreign_reasoning));
    // Unknown payloads keep their complete canonical content, including fields named signature.
    let mut changed_foreign = projector();
    changed_foreign.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "thought",
        Some("signature"),
        Some(&json!({"a":1,"b":2,"signature":"foreign"})),
    );
    assert!(retry(&reasoning) != retry(&changed_foreign));
    // Known payload text and emitted signature are independent identities; neither is discarded.
    let mut known = projector();
    known.reasoning(
        Role::Assistant,
        ReplayFamily::Messages,
        "thought",
        Some("signature"),
        Some(&json!({"type":"thinking","thinking":"thought","signature":"signature"})),
    );
    for (text, signature) in [("changed", "signature"), ("thought", "changed")] {
        let mut changed = projector();
        changed.reasoning(
            Role::Assistant,
            ReplayFamily::Messages,
            text,
            Some(signature),
            Some(&json!({"type":"thinking","thinking":text,"signature":signature})),
        );
        assert!(retry(&known) != retry(&changed));
    }
    let mut native = projector();
    native.state(
        ReplayFamily::Responses,
        StateKind::Native,
        &json!({"x":[1,2]}),
    );
    let mut reordered_array = projector();
    reordered_array.state(
        ReplayFamily::Responses,
        StateKind::Native,
        &json!({"x":[2,1]}),
    );
    assert!(retry(&native) != retry(&reordered_array));
    let mut gateway = projector();
    gateway.state(
        ReplayFamily::Responses,
        StateKind::Gateway,
        &json!({"x":[1,2]}),
    );
    assert!(retry(&native) != retry(&gateway));
}

/// Actual IDs keep independent exact histories apart while relational identities permit replay
/// with omitted IDs; generated internal IDs never influence a complete routing fingerprint.
///
/// # Panics
///
/// Panics if exact/relational availability or tool result linkage changes.
#[test]
fn wire_identity_and_result_relationships_have_independent_strength() {
    let mut one = projector();
    let mut two = projector();
    for (projector, id) in [(&mut one, "one"), (&mut two, "two")] {
        projector.text(Role::User, b"question");
        projector.call(
            Role::Assistant,
            Identity::Wire(id),
            "tool",
            &json!({}),
            Some("signature"),
        );
        projector.result(
            Role::Tool,
            Identity::Wire(id),
            ResultContent::Text("answer"),
            false,
        );
    }
    assert!(retry(&one) != retry(&two));
    let relational = |projector: &Projector| {
        projector
            .evidence()
            .history
            .into_iter()
            .rfind(|prefix| prefix.projection == Projection::Relational)
            .map(|prefix| prefix.digest)
    };
    assert!(relational(&one) == relational(&two));
    let mut idless = projector();
    let mut idless_replay = projector();
    for (projector, id) in [(&mut idless, "random-1"), (&mut idless_replay, "random-2")] {
        projector.text(Role::User, b"question");
        projector.call(
            Role::Assistant,
            Identity::Synthetic(id),
            "tool",
            &json!({}),
            Some("signature"),
        );
        projector.result(
            Role::Tool,
            Identity::Synthetic(id),
            ResultContent::Text("answer"),
            false,
        );
    }
    assert!(retry(&idless) == retry(&idless_replay));
    assert!(retry(&idless).is_some_and(|(projection, _)| projection == Projection::Relational));
    assert!(relational(&one) == relational(&idless));
    let mut foreign = projector();
    foreign.call(
        Role::Assistant,
        Identity::Wire("one"),
        "tool",
        &json!({}),
        None,
    );
    foreign.result(
        Role::Tool,
        Identity::Wire("one"),
        ResultContent::Json(&json!("answer")),
        false,
    );
    let mut plain = projector();
    plain.call(
        Role::Assistant,
        Identity::Wire("one"),
        "tool",
        &json!({}),
        None,
    );
    plain.result(
        Role::Tool,
        Identity::Wire("one"),
        ResultContent::Text("answer"),
        false,
    );
    assert!(retry(&foreign) != retry(&plain));
}

/// Unresolved or duplicate relationships stop the affected suffix without rejecting inference;
/// usable earlier unit prefixes remain. Sequential legacy calls reuse their codec-resolved ID.
///
/// # Panics
///
/// Panics if malformed suffixes erase earlier evidence or fabricate a retry/completed anchor.
#[test]
fn unresolved_suffix_keeps_earlier_evidence_and_legacy_pairing() {
    let mut broken = projector();
    broken.text(Role::User, b"question");
    broken.text(Role::Assistant, b"answer");
    broken.text(Role::User, b"next");
    let previous = broken.evidence().history;
    broken.result(
        Role::Tool,
        Identity::Wire("missing"),
        ResultContent::Text("x"),
        false,
    );
    assert!(retry(&broken).is_none());
    assert_eq!(broken.evidence().history.len(), 3);
    assert!(
        broken
            .evidence()
            .history
            .iter()
            .zip(previous)
            .all(|(new, old)| new.digest == old.digest)
    );
    assert!(broken.completed_anchors().is_empty());

    let mut duplicate = projector();
    duplicate.text(Role::User, b"question");
    duplicate.call(
        Role::Assistant,
        Identity::Wire("same"),
        "tool",
        &json!({}),
        None,
    );
    duplicate.call(
        Role::Assistant,
        Identity::Wire("same"),
        "tool",
        &json!({}),
        None,
    );
    assert!(retry(&duplicate).is_none());
    assert_eq!(duplicate.evidence().history.len(), 1);
    let mut legacy = projector();
    for _ in 0..2 {
        legacy.call(
            Role::Assistant,
            Identity::Synthetic("legacy"),
            "tool",
            &json!({}),
            None,
        );
        legacy.result(
            Role::Tool,
            Identity::Synthetic("legacy"),
            ResultContent::Text("ok"),
            false,
        );
    }
    assert!(retry(&legacy).is_some());
    assert_eq!(legacy.evidence().history.len(), 4);
}
