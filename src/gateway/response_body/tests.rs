//! The foreign-response-envelope selection tests: the configured member is chosen without a
//! JSON DOM, with ordinary JSON decoding's last-occurrence rule, while non-object roots,
//! absent members, and malformed bodies keep or fail their documented shapes.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section would \
              document nothing its name does not"
)]

use super::unwrap;
use std::error::Error as StdError;

/// Selects `member` out of `body`, failing the test on a decoding error.
fn selected(body: &str, member: &str) -> Result<String, Box<dyn StdError>> {
    let bytes = unwrap(body.as_bytes(), Some(member))?;

    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// A configured member present in an object root is selected verbatim.
#[test]
fn a_present_member_is_selected_verbatim() -> Result<(), Box<dyn StdError>> {
    assert_eq!(
        selected(
            r#"{"trace": 1, "result": {"choices": [], "usage": {}}}"#,
            "result"
        )?,
        r#"{"choices": [], "usage": {}}"#
    );

    Ok(())
}

/// A repeated member selects the last occurrence, matching ordinary JSON decoding.
#[test]
fn a_repeated_member_selects_the_last_occurrence() -> Result<(), Box<dyn StdError>> {
    assert_eq!(
        selected(r#"{"result": {"n": 1}, "result": {"n": 2}}"#, "result")?,
        r#"{"n": 2}"#
    );

    Ok(())
}

/// A present null member is selected as null, never silently replaced by the whole body.
#[test]
fn a_present_null_member_selects_null() -> Result<(), Box<dyn StdError>> {
    assert_eq!(selected(r#"{"result": null}"#, "result")?, "null");

    Ok(())
}

/// An absent member and a non-object root both pass the body through unchanged.
#[test]
fn an_absent_member_or_non_object_root_passes_through() -> Result<(), Box<dyn StdError>> {
    assert_eq!(selected(r#"{"other": 1}"#, "result")?, r#"{"other": 1}"#);

    assert_eq!(selected("[1, 2, 3]", "result")?, "[1, 2, 3]");

    assert_eq!(selected("\"scalar\"", "result")?, "\"scalar\"");

    Ok(())
}

/// A malformed body fails the decoding instead of passing through.
#[test]
fn a_malformed_body_fails_the_selection() {
    assert!(selected("{\"result\": ", "result").is_err());

    // Trailing garbage after a well-formed member is still a malformed response.
    assert!(selected(r#"{"result": 1} trailing"#, "result").is_err());
}

/// No configured member returns the body unchanged without touching the parser.
#[test]
fn no_configured_member_returns_the_body_unchanged() -> Result<(), Box<dyn StdError>> {
    let bytes = unwrap(b"not even json", None)?;

    assert_eq!(String::from_utf8_lossy(bytes), "not even json");

    Ok(())
}
