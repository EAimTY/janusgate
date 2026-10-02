//! Compaction-specific pieces of the Anthropic Messages codec: the outbound gateway carrier
//! block, the tolerant typed view of a vendor compaction block, and the header-overlay helper
//! the compaction operation descriptors compose with.
//!
//! The Messages compaction wire (request parsing, upstream render extensions, upstream capture,
//! client rendering) lives in the family's concern modules beside this one; this module holds
//! only the pieces those directions share.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fixed carrier description rendered as a gateway compaction block's `content` when the
/// sealed envelope carries no readable summary — wrapped native state with no exported text. It
/// is a transport marker, never text sent to a model and never a substitute summary.
pub(crate) const GATEWAY_CARRIER_DESCRIPTION: &str =
    "JanusGate compacted conversation state (encrypted); replay through JanusGate.";

/// The tolerant typed view of a vendor `compaction` content block: every member optional, extra
/// vendor members ignored. The block's shape is the vendor's to define, so upstream parsing
/// captures the raw [`Value`] beside this view for replay fidelity; this struct only decides
/// which members the gateway reads.
#[derive(Debug, Default, Clone, Deserialize)]
pub(super) struct CompactionBlock {
    /// The readable summary content; `None` marks the native null-content no-op block.
    #[serde(default)]
    pub(super) content: Option<String>,

    /// The vendor signature, or the gateway carrier string when the gateway rendered the block.
    #[serde(default)]
    pub(super) signature: Option<String>,

    /// Opaque vendor ciphertext member, when the block carries one.
    #[serde(default)]
    pub(super) encrypted_content: Option<String>,

    /// Tool-advertisement changes the compaction applied, when the block carries any.
    #[serde(default)]
    pub(super) tool_changes: Option<Vec<Value>>,
}

/// The typed outbound gateway carrier block: a protocol-shaped `compaction` block whose
/// `signature` carries the sealed gateway envelope. `content` is the readable summary when the
/// envelope exposes one, else [`GATEWAY_CARRIER_DESCRIPTION`]; no other member is ever added —
/// a gateway block carries no `encrypted_content` and no absent member rendered as null.
///
/// # Panics
///
/// Panics if the first-party block type fails to serialize, which cannot occur.
pub(crate) fn gateway_carrier_block(content: Option<&str>, signature: &str) -> Value {
    #[derive(Debug, Serialize)]
    struct GatewayCarrierBlock<'a> {
        /// Always `compaction`.
        #[serde(rename = "type")]
        kind: &'static str,

        /// The readable summary, else the fixed carrier description.
        content: &'a str,

        /// The sealed gateway carrier.
        signature: &'a str,
    }

    let content = content.unwrap_or(GATEWAY_CARRIER_DESCRIPTION);

    #[expect(
        clippy::expect_used,
        reason = "the carrier block is a first-party type whose serialization is total"
    )]
    serde_json::to_value(&GatewayCarrierBlock {
        kind: "compaction",
        content,
        signature,
    })
    .expect("first-party wire types serialize")
}

/// Merges an operation's overlay header rows over the inference header rows: an overlay row
/// replaces the base row of the same name — the operation's value wins, the base's position is
/// not kept — and the result preserves overlay order first, then the remaining base rows in
/// their order. This is how a compaction operation's pinned header (the `anthropic-beta` beta
/// declaration foremost) overrides the inference header set without reordering the rest.
#[must_use]
pub(crate) fn overlay_header_rows(
    base: &[crate::provider::definition::template::ParamRow],
    overlay: &[crate::provider::definition::template::ParamRow],
) -> Vec<crate::provider::definition::template::ParamRow> {
    let mut rows: Vec<_> = overlay.to_vec();

    for row in base {
        if !overlay
            .iter()
            .any(|overlay_row| overlay_row.name == row.name)
        {
            rows.push(row.clone());
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::{GATEWAY_CARRIER_DESCRIPTION, gateway_carrier_block, overlay_header_rows};
    use crate::provider::definition::template::{ParamRow, StringExpr};
    use serde_json::json;

    /// Builds one literal header row.
    fn row(name: &str, value: &str) -> ParamRow {
        ParamRow {
            name: name.to_string(),
            value: StringExpr::literal(value),
        }
    }

    /// Renders the gateway carrier block for inspection: the readable summary or the fixed
    /// description rides as `content`, the sealed carrier as `signature`, and no absent member
    /// is ever rendered.
    ///
    /// # Panics
    ///
    /// Panics if the rendered block diverges from the carrier contract.
    #[test]
    fn renders_the_gateway_carrier_block_without_absent_members() {
        let block = gateway_carrier_block(Some("the summary"), "janusgate.compaction.v1.k.ct");

        assert_eq!(
            block,
            json!({
                "type": "compaction",
                "content": "the summary",
                "signature": "janusgate.compaction.v1.k.ct",
            })
        );

        // No readable summary: the fixed description rides instead, and no other member is
        // ever fabricated — no `encrypted_content`, no null `content`.
        let block = gateway_carrier_block(None, "carrier");

        assert_eq!(
            block,
            json!({
                "type": "compaction",
                "content": GATEWAY_CARRIER_DESCRIPTION,
                "signature": "carrier",
            })
        );
    }

    /// Overlay rows replace same-named base rows, keep overlay order ahead of the surviving
    /// base rows, and never reorder the untouched base rows.
    ///
    /// # Panics
    ///
    /// Panics if the merged rows diverge from the replace-by-name contract.
    #[test]
    fn overlay_header_rows_replace_by_name_and_preserve_order() {
        let base = vec![
            row("x-api-key", "key"),
            row("anthropic-version", "2023-06-01"),
            row("anthropic-beta", "old-beta"),
        ];
        let overlay = vec![
            row("anthropic-beta", "compact-2026-09-04"),
            row("x-trace", "t1"),
        ];

        let merged = overlay_header_rows(&base, &overlay);

        assert_eq!(
            merged,
            vec![
                // The overlay row replaced the same-named base row and carries its own value.
                row("anthropic-beta", "compact-2026-09-04"),
                row("x-trace", "t1"),
                // The surviving base rows keep their order.
                row("x-api-key", "key"),
                row("anthropic-version", "2023-06-01"),
            ]
        );

        // An empty overlay keeps the base rows untouched in order.
        assert_eq!(overlay_header_rows(&base, &[]), base);
    }
}
