//! Compaction affinity tests for the Anthropic Messages codec: the native no-op rule, the
//! readable-state gate, gateway provenance, and the canonical identity normalization.
#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section on every \
              test would document nothing its name does not"
)]

use crate::{
    affinity::{Anchor, fingerprint::Projector},
    format::compaction_wire::{Checkpoint, MessagesCheckpoint, ResponsesCheckpoint},
};

/// The no-op rule: absent or null content is native no-op state unless the signature is a
/// gateway marker — authentication cannot be skipped through missing text — and a fragment
/// without the block discriminator is no state at all.
#[test]
fn is_native_no_op_distinguishes_native_and_gateway_blocks() {
    for block in [
        serde_json::json!({"type": "compaction"}),
        serde_json::json!({"type": "compaction", "content": null}),
        serde_json::json!({"type": "compaction", "content": null, "signature": "native"}),
    ] {
        assert!(
            super::is_native_no_op(&block),
            "`{block}` is native no-op state"
        );
    }

    for block in [
        serde_json::json!({"type": "compaction", "content": "Summary."}),
        serde_json::json!({"type": "compaction", "content": null,
            "signature": "janusgate.compaction.v1.primary.ABCD"}),
        serde_json::json!({"content": "Summary."}),
    ] {
        assert!(
            !super::is_native_no_op(&block),
            "`{block}` is not native no-op state"
        );
    }
}

/// A native block projects as completed state only when it carries readable content under a
/// non-gateway signature; null content, a gateway marker, and non-block fragments all decline.
#[test]
fn completed_projection_requires_readable_native_state() {
    #[expect(clippy::expect_used, reason = "the readable native block must project")]
    let readable = super::completed_projection(
        &serde_json::json!({
            "type": "compaction",
            "content": "Summary.",
            "signature": "native"
        }),
        None,
    )
    .expect("a readable native block projects as completed state");

    assert!(!readable.1.is_empty(), "the projection carries its anchors");

    for block in [
        serde_json::json!({"type": "compaction", "content": null, "signature": "native"}),
        serde_json::json!({"type": "compaction", "content": "Summary.",
            "signature": "janusgate.compaction.v1.primary.ABCD"}),
        serde_json::json!({"content": "Summary."}),
    ] {
        assert!(
            super::completed_projection(&block, None).is_none(),
            "`{block}` carries no projectable native state"
        );
    }
}

/// Gateway provenance requires the sealer's artifact identity: with one, the projection binds
/// the carrier signature and the anchors close with the identity digest of that artifact.
#[test]
fn gateway_provenance_binds_the_carrier_and_artifact() {
    #[expect(
        clippy::expect_used,
        reason = "a signed block projects under an artifact id"
    )]
    let (projector, anchors) = super::completed_projection(
        &serde_json::json!({
            "type": "compaction",
            "content": "Summary.",
            "signature": "janusgate.compaction.v1.primary.ABCD"
        }),
        Some("artifact-1"),
    )
    .expect("the gateway block projects with its artifact identity");

    let expected = Projector::new().gateway_artifact("artifact-1");

    assert!(
        anchors
            .iter()
            .any(|anchor| matches!(anchor, Anchor::Artifact(digest) if *digest == expected)),
        "the anchors close with the artifact identity digest"
    );

    // The projection hashes the authenticated carrier token, not the visible content.
    let mut reference = Projector::new();

    reference.gateway_state(
        crate::affinity::fingerprint::ReplayFamily::Messages,
        "janusgate.compaction.v1.primary.ABCD",
    );

    assert!(
        projector.evidence().retry.map(|retry| retry.digest)
            == reference.evidence().retry.map(|retry| retry.digest),
        "the gateway projection hashes only the carrier token"
    );
}

/// The canonical identity of a native block is independent of member order and ignores the
/// caching hint, while different summary content hashes differently.
#[test]
fn canonical_identity_normalizes_members_and_ignores_cache_control() {
    let project = |block: &serde_json::Value| {
        #[expect(
            clippy::expect_used,
            reason = "every fixture block is readable native state"
        )]
        super::completed_projection(block, None).expect("the fixture block projects")
    };

    let left = project(&serde_json::json!({
        "type": "compaction",
        "content": "Summary.",
        "signature": "native",
        "cache_control": {"type": "ephemeral"}
    }));

    let right = project(&serde_json::json!({
        "cache_control": {"type": "ephemeral"},
        "signature": "native",
        "content": "Summary.",
        "type": "compaction"
    }));

    assert!(
        left.0.evidence().retry.map(|retry| retry.digest)
            == right.0.evidence().retry.map(|retry| retry.digest),
        "member order and the caching hint do not change the identity"
    );

    let different = project(&serde_json::json!({
        "type": "compaction",
        "content": "Other summary.",
        "signature": "native"
    }));

    assert!(
        left.0.evidence().retry.map(|retry| retry.digest)
            != different.0.evidence().retry.map(|retry| retry.digest),
        "different summary content hashes differently"
    );
}

/// The checkpoint projection covers each concrete variant: a Messages checkpoint projects only
/// with its artifact identity, a native value delegates to the native gate, and a Responses
/// checkpoint never projects on this family.
#[test]
fn checkpoint_projection_covers_each_variant() {
    let artifact_id = "artifact-1";
    let messages = Checkpoint::Messages(MessagesCheckpoint {
        content: "Summary.".to_string(),
        signature: "janusgate.compaction.v1.primary.ABCD".to_string(),
    });

    assert!(super::checkpoint_projection(&messages, None).is_none());

    #[expect(clippy::expect_used, reason = "the checkpoint carries its artifact id")]
    let (_, anchors) = super::checkpoint_projection(&messages, Some(artifact_id))
        .expect("the Messages checkpoint projects with its artifact id");

    let expected = Projector::new().gateway_artifact(artifact_id);

    assert!(
        anchors
            .iter()
            .any(|anchor| matches!(anchor, Anchor::Artifact(digest) if *digest == expected))
    );

    assert_eq!(
        super::completed_anchors(&messages, Some(artifact_id)).len(),
        anchors.len(),
        "the anchors helper matches the projection"
    );

    let native = serde_json::json!({
        "type": "compaction",
        "content": "Summary.",
        "signature": "native"
    });

    assert!(
        !super::completed_anchors(&Checkpoint::Native(native.clone()), None).is_empty(),
        "the native block carries its canonical artifact"
    );

    assert!(
        super::checkpoint_projection(&Checkpoint::Native(native), None).is_some(),
        "the native value delegates to the native gate"
    );

    let responses = Checkpoint::Responses(ResponsesCheckpoint {
        id: "item-1".to_string(),
        encrypted_content: "enc".to_string(),
        created_by: None,
    });

    assert!(super::checkpoint_projection(&responses, Some(artifact_id)).is_none());

    assert!(super::completed_anchors(&responses, Some(artifact_id)).is_empty());
}
