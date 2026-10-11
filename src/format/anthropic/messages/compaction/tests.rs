//! Compaction carrier tests for the Anthropic Messages codec: the outbound gateway carrier
//! block and the tolerant typed view of a vendor compaction block.
#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section on every \
              test would document nothing its name does not"
)]

use serde_json::Value;
use std::error::Error as StdError;

/// The carrier block renders the readable content when the sealed envelope carries one and the
/// fixed transport marker when it carries none, beside its authenticated signature.
#[test]
fn carrier_block_uses_content_or_the_marker() -> Result<(), Box<dyn StdError>> {
    let body = serde_json::to_value(super::gateway_carrier_block(Some("Summary."), "sig"))?;

    assert_eq!(
        body,
        serde_json::json!({"type": "compaction", "content": "Summary.", "signature": "sig"})
    );

    let body = serde_json::to_value(super::gateway_carrier_block(None, "sig"))?;

    assert_eq!(
        body,
        serde_json::json!({
            "type": "compaction",
            "content": super::GATEWAY_CARRIER_DESCRIPTION,
            "signature": "sig"
        })
    );

    Ok(())
}

/// The typed vendor-block view reads only the `content` member: extra vendor members and a
/// missing discriminator are tolerated — the upstream parser probes the discriminator itself —
/// and a non-string content is not a readable block.
#[test]
fn vendor_block_view_reads_content_tolerantly() -> Result<(), Box<dyn StdError>> {
    let view: super::CompactionBlock = serde_json::from_value(serde_json::json!({
        "type": "compaction",
        "content": "Summary.",
        "signature": "sig",
        "encrypted_content": "enc",
        "vendor_extension": {"anything": true}
    }))?;

    assert_eq!(view.content.as_deref(), Some("Summary."));

    for block in [
        serde_json::json!({"type": "compaction"}),
        serde_json::json!({"type": "compaction", "content": null}),
    ] {
        let view: super::CompactionBlock = serde_json::from_value(block)?;

        assert_eq!(view.content, None, "absence and null both mean the no-op");
    }

    // The view reads `content` with or without the discriminator beside it; the upstream
    // parser probes the discriminator itself before consulting this view.
    let view: super::CompactionBlock = serde_json::from_value(serde_json::json!({
        "content": "Summary."
    }))?;

    assert_eq!(view.content.as_deref(), Some("Summary."));

    assert!(
        serde_json::from_value::<super::CompactionBlock>(serde_json::json!({
            "type": "compaction",
            "content": 5
        }))
        .is_err(),
        "a non-string content is not a readable block"
    );

    Ok(())
}

/// The fixed carrier description is a transport marker describing encrypted state, never a
/// summary standing in for one.
#[test]
fn carrier_marker_describes_encrypted_state() {
    assert_eq!(
        super::GATEWAY_CARRIER_DESCRIPTION,
        "JanusGate compacted conversation state (encrypted); replay through JanusGate."
    );
}

/// Sanity pin: the carrier marker renders inside the fixed-marker checkpoint as a `Value`.
#[test]
fn carrier_block_serializes_as_a_checkpoint_value() -> Result<(), Box<dyn StdError>> {
    let checkpoint = super::gateway_carrier_block(None, "sig");

    let value = serde_json::to_value(&checkpoint)?;

    assert_eq!(
        crate::test_support::at(&value, "/content"),
        &Value::String(super::GATEWAY_CARRIER_DESCRIPTION.to_string())
    );

    Ok(())
}
