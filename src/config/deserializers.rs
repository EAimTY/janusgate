//! Custom deserializers for the configuration schema.

use axum::http::HeaderValue;
use serde::{
    Deserializer,
    de::{Error, SeqAccess, Visitor},
};
use std::fmt::{Error as FmtError, Formatter};
use tower_http::cors::AllowOrigin;

/// Deserializes origin strings into an [`AllowOrigin`].
///
/// Accepts either a single origin string (e.g. `"*"` or
/// `"https://dashboard.example.com"`) or a sequence of origin strings
/// (e.g. `["*"]`, `["https://dashboard.example.com"]`, or `[]`).
///
/// # Errors
///
/// Returns a deserialization error if any origin string is not a valid HTTP header value or the
/// input type is neither a string nor a sequence.
pub(super) fn allowed_origin<'de, D>(deserializer: D) -> Result<AllowOrigin, D::Error>
where
    D: Deserializer<'de>,
{
    /// Visitor that deserializes an [`AllowOrigin`] from either a single origin string or a
    /// sequence of origin strings.
    struct AllowedOriginVisitor;

    impl<'de> Visitor<'de> for AllowedOriginVisitor {
        type Value = AllowOrigin;

        fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
            formatter.write_str("an origin string or a list of origin strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            if value == "*" {
                Ok(AllowOrigin::any())
            } else {
                let header_value = HeaderValue::from_str(value).map_err(Error::custom)?;

                Ok(AllowOrigin::list([header_value]))
            }
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut origins = Vec::new();
            let mut has_wildcard = false;

            while let Some(string) = seq.next_element::<String>()? {
                if string == "*" {
                    has_wildcard = true;
                } else {
                    let header_value = HeaderValue::from_str(&string).map_err(Error::custom)?;

                    origins.push(header_value);
                }
            }

            if has_wildcard {
                Ok(AllowOrigin::any())
            } else {
                Ok(AllowOrigin::list(origins))
            }
        }
    }

    deserializer.deserialize_any(AllowedOriginVisitor)
}
