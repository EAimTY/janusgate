//! Tests for the configuration's custom deserializers: the compaction sealing-key set contract
//! and the CORS origin configuration shape (`docs/reference/configuration.md`).

use super::schema::{CompactionConfig, CorsConfig, SEALING_KEY_BYTES};
use serde_json::Error as SerdeJsonError;
use std::error::Error as StdError;

/// A first sealing key's material: standard base64 of the 32 ascending bytes from `0x2b`.
pub(super) const KEY_MATERIAL_A: &str = "KywtLi8wMTIzNDU2Nzg5Ojs8PT4/QEFCQ0RFRkdISUo=";

/// A second sealing key's material: standard base64 of 32 further bytes, carrying `+` so its
/// URL-safe re-encoding is no longer standard base64.
pub(super) const KEY_MATERIAL_B: &str = "Y2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYI=";

/// Parses a compaction section at the schema boundary.
fn parse_compaction(body: &str) -> Result<CompactionConfig, SerdeJsonError> {
    serde_json::from_str(body)
}

/// Parses a CORS section at the schema boundary.
fn parse_cors(body: &str) -> Result<CorsConfig, SerdeJsonError> {
    serde_json::from_str(body)
}

/// Asserts a compaction section is rejected by a failure naming the violated contract clause.
///
/// # Panics
///
/// Panics when the section is accepted, or its rejection names a different cause.
fn assert_compaction_rejected(body: &str, cause: &str) {
    let failure = parse_compaction(body);

    assert!(failure.is_err(), "the section must be rejected: {body}");

    if let Err(failure) = failure {
        let message = failure.to_string();

        assert!(
            message.contains(cause),
            "the rejection must name `{cause}`: {message}"
        );
    }
}

/// Asserts a CORS section is rejected by a failure naming the violated contract clause.
///
/// # Panics
///
/// Panics when the section is accepted, or its rejection names a different cause.
fn assert_cors_rejected(body: &str, cause: &str) {
    let failure = parse_cors(body);

    assert!(failure.is_err(), "the section must be rejected: {body}");

    if let Err(failure) = failure {
        let message = failure.to_string();

        assert!(
            message.contains(cause),
            "the rejection must name `{cause}`: {message}"
        );
    }
}

/// A valid key set parses, preserving rotation order and decoding every key's material from its
/// standard base64 encoding to exactly [`SEALING_KEY_BYTES`] bytes.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn valid_key_set_parses_in_rotation_order() -> Result<(), Box<dyn StdError>> {
    let section = parse_compaction(&format!(
        r#"{{"keys": [
            {{"id": "active", "key": "{KEY_MATERIAL_A}"}},
            {{"id": "retired", "key": "{KEY_MATERIAL_B}"}}
        ]}}"#
    ))?;

    let keys = section.keys;

    assert_eq!(keys.len(), 2);

    let first = keys.first();

    assert_eq!(first.id, "active");

    // The material is the decoded bytes, never the configuration string.
    let expected: [u8; SEALING_KEY_BYTES] = std::array::from_fn(|index| (0x2b + index) as u8);

    assert_eq!(first.key, expected);

    assert!(
        keys.get(1)
            .is_some_and(|key| key.id == "retired" && key.key != first.key),
        "the second entry must carry its own decoded material"
    );

    Ok(())
}

/// An empty key set is rejected: the section must declare at least one key.
#[test]
fn empty_key_set_is_rejected() {
    assert_compaction_rejected(r#"{"keys": []}"#, "declares no sealing key");
}

/// A repeated key identifier is rejected: identifiers are unique in the set.
#[test]
fn repeated_key_id_is_rejected() {
    assert_compaction_rejected(
        &format!(
            concat!(
                r#"{{"keys": [{{"id": "a", "key": "{}"}}, "#,
                r#"{{"id": "a", "key": "{}"}}]}}"#
            ),
            KEY_MATERIAL_A, KEY_MATERIAL_B,
        ),
        "is repeated",
    );
}

/// A key identifier carrying the carrier's `.` separator is rejected.
#[test]
fn key_id_with_separator_is_rejected() {
    assert_compaction_rejected(
        &format!(r#"{{"keys": [{{"id": "a.b", "key": "{KEY_MATERIAL_A}"}}]}}"#),
        "cannot carry",
    );
}

/// An empty key identifier is rejected.
#[test]
fn empty_key_id_is_rejected() {
    assert_compaction_rejected(
        &format!(r#"{{"keys": [{{"id": "", "key": "{KEY_MATERIAL_A}"}}]}}"#),
        "cannot be empty",
    );
}

/// Key material that is not standard base64 is rejected: URL-safe encodings, unpadded
/// encodings, and non-base64 input all fail the standard decoding.
#[test]
fn non_standard_base64_material_is_rejected() {
    let url_safe = KEY_MATERIAL_B.replace('+', "-");
    let unpadded = KEY_MATERIAL_A.trim_end_matches('=');

    assert_compaction_rejected(
        &format!(r#"{{"keys": [{{"id": "a", "key": "{url_safe}"}}]}}"#),
        "not standard base64",
    );

    assert_compaction_rejected(
        &format!(r#"{{"keys": [{{"id": "a", "key": "{unpadded}"}}]}}"#),
        "not standard base64",
    );

    assert_compaction_rejected(
        r#"{"keys": [{"id": "a", "key": "not base64!!"}]}"#,
        "not standard base64",
    );
}

/// Key material decoding to a length other than [`SEALING_KEY_BYTES`] is rejected.
#[test]
fn wrong_length_material_is_rejected() {
    assert_compaction_rejected(
        r#"{"keys": [{"id": "a", "key": "AAAAAAAAAAAAAAAAAAAAAA=="}]}"#,
        "bytes, not 32",
    );

    assert_compaction_rejected(r#"{"keys": [{"id": "a", "key": "AA=="}]}"#, "bytes, not 32");
}

/// A key entry carries exactly its `id` and `key` fields, and the set itself is a list.
#[test]
fn key_set_shape_is_strict() {
    assert_compaction_rejected(r#"{"keys": [{"id": "a"}]}"#, "missing field");

    assert_compaction_rejected(
        &format!(r#"{{"keys": [{{"key": "{KEY_MATERIAL_A}"}}]}}"#),
        "missing field",
    );

    assert_compaction_rejected(
        &format!(r#"{{"keys": [{{"id": "a", "key": "{KEY_MATERIAL_A}", "extra": 1}}]}}"#),
        "unknown field",
    );

    assert_compaction_rejected(
        r#"{"keys": {"id": "a", "key": "AA=="}}"#,
        "a list of compaction sealing keys",
    );
}

/// Origins are accepted as one string, as a list, as the wildcard in either spelling, and as the
/// empty list.
#[test]
fn cors_origin_shapes_parse() -> Result<(), Box<dyn StdError>> {
    parse_cors(r#"{"allowed_origin": "*"}"#)?;
    parse_cors(r#"{"allowed_origin": ["*"]}"#)?;
    parse_cors(r#"{"allowed_origin": "https://console.example"}"#)?;
    parse_cors(r#"{"allowed_origin": ["https://console.example", "https://admin.example"]}"#)?;
    parse_cors(r#"{"allowed_origin": []}"#)?;

    Ok(())
}

/// Private Network Access defaults to disabled and parses when set.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn cors_private_network_defaults_false() -> Result<(), Box<dyn StdError>> {
    assert!(!parse_cors("{}")?.allow_private_network);
    assert!(!parse_cors(r#"{"allowed_origin": "*"}"#)?.allow_private_network);
    assert!(!parse_cors(r#"{"allow_private_network": false}"#)?.allow_private_network);
    assert!(parse_cors(r#"{"allow_private_network": true}"#)?.allow_private_network);

    Ok(())
}

/// Origins that are not valid HTTP header values are rejected, alone or inside a list.
#[test]
fn invalid_origin_values_are_rejected() {
    assert_cors_rejected(r#"{"allowed_origin": "bad\norigin"}"#, "header");

    assert_cors_rejected(
        r#"{"allowed_origin": ["https://ok.example", "bad\norigin"]}"#,
        "header",
    );
}

/// Origins and the Private Network Access switch of the wrong JSON type are rejected.
#[test]
fn wrong_type_cors_values_are_rejected() {
    assert_cors_rejected(r#"{"allowed_origin": 42}"#, "invalid type");

    assert_cors_rejected(r#"{"allowed_origin": [42]}"#, "invalid type");

    assert_cors_rejected(r#"{"allow_private_network": "yes"}"#, "invalid type");
}
