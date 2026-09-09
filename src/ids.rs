//! UUID generation for gateway-issued identifiers.

use uuid::Uuid;

/// Generates a random UUID v4 in its canonical lowercase hyphenated form.
pub(crate) fn new_uuid() -> String {
    Uuid::new_v4().to_string()
}
