//! The upstream request-construction tests that are pure logic: the overlay-header merge the
//! compaction operations apply over the inference descriptor's own header rows.

use super::overlay_header_rows;
use crate::provider::definition::template::{ParamRow, StringExpr, ValueExpr};

/// A literal one-part header row.
fn row(name: &str, value: &str) -> ParamRow {
    ParamRow {
        name: name.to_owned(),
        value: StringExpr(vec![ValueExpr::Literal {
            value: value.to_owned(),
        }]),
    }
}

/// The rendered value of one row.
fn value_of(row: &ParamRow) -> &str {
    match row.value.0.first() {
        Some(crate::provider::definition::template::ValueExpr::Literal { value }) => value,
        _ => "",
    }
}

/// The lowercase names and literal values of one merged row set.
fn rendered(rows: &[ParamRow]) -> Vec<(String, String)> {
    rows.iter()
        .map(|row| (row.name.to_ascii_lowercase(), value_of(row).to_owned()))
        .collect()
}

/// An overlay row replaces the base row of the same name case-insensitively and takes its
/// position first; every non-conflicting base row survives in its order.
#[test]
fn overlay_rows_replace_same_named_base_rows() {
    let base = vec![
        row("anthropic-beta", "base-beta"),
        row("X-Custom", "kept"),
        row("accept-encoding", "identity"),
    ];

    let overlay = vec![
        row("x-custom", "replaced"),
        row("ANTHROPIC-BETA", "operation-beta"),
    ];

    let merged = overlay_header_rows(&base, &overlay);

    assert_eq!(
        rendered(&merged),
        vec![
            ("x-custom".to_owned(), "replaced".to_owned()),
            ("anthropic-beta".to_owned(), "operation-beta".to_owned()),
            ("accept-encoding".to_owned(), "identity".to_owned()),
        ]
    );
}

/// An empty overlay leaves the base rows untouched, and an overlay over no base rows is the
/// overlay itself.
#[test]
fn non_conflicting_rows_survive_in_both_directions() {
    let base = vec![row("x-base", "kept")];
    let overlay = vec![row("x-operation", "added")];

    let merged = overlay_header_rows(&base, &overlay);

    assert_eq!(
        rendered(&merged),
        vec![
            ("x-operation".to_owned(), "added".to_owned()),
            ("x-base".to_owned(), "kept".to_owned()),
        ]
    );

    assert_eq!(rendered(&overlay_header_rows(&base, &[])).len(), 1);

    assert_eq!(rendered(&overlay_header_rows(&[], &overlay)).len(), 1);
}
