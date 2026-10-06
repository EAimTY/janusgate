//! Custom deserializers for the configuration schema.

use super::schema::{CompactionKeyConfig, SEALING_KEY_BYTES};
use crate::compaction_model::sealing::KEY_ID_SEPARATOR;
use axum::http::HeaderValue;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{
    Deserialize as _, Deserializer,
    de::{Error, SeqAccess, Visitor},
};
use std::fmt::{Error as FmtError, Formatter};
use tower_http::cors::AllowOrigin;
use vec1::Vec1;

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

/// Deserializes a compaction sealing key's carrier identifier: a non-empty string free of the
/// carrier's key-id separator ([`KEY_ID_SEPARATOR`], whose carrier-format invariant that
/// constant states).
///
/// # Errors
///
/// Returns a deserialization error for an empty identifier or one carrying the separator.
pub(super) fn sealing_key_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let id = String::deserialize(deserializer)?;

    if id.is_empty() {
        return Err(Error::custom("a compaction key id cannot be empty"));
    }

    if id.contains(KEY_ID_SEPARATOR) {
        return Err(Error::custom(format!(
            "the compaction key id `{id}` cannot carry the `{KEY_ID_SEPARATOR}` carrier separator"
        )));
    }

    Ok(id)
}

/// Deserializes a compaction sealing key's material: standard base64 decoding to exactly
/// [`SEALING_KEY_BYTES`] bytes.
///
/// # Errors
///
/// Returns a deserialization error when the value is not standard base64 or does not decode to
/// exactly [`SEALING_KEY_BYTES`] bytes.
pub(super) fn sealing_key_material<'de, D>(
    deserializer: D,
) -> Result<[u8; SEALING_KEY_BYTES], D::Error>
where
    D: Deserializer<'de>,
{
    let encoded = String::deserialize(deserializer)?;

    let decoded = STANDARD.decode(encoded.as_bytes()).map_err(Error::custom)?;

    <[u8; SEALING_KEY_BYTES]>::try_from(decoded.as_slice()).map_err(|_mismatch| {
        Error::custom(format!(
            "a compaction sealing key decodes to {} bytes, not {SEALING_KEY_BYTES}",
            decoded.len()
        ))
    })
}

/// Deserializes the compaction sealing-key set: a non-empty list of keys whose identifiers are
/// unique.
///
/// # Errors
///
/// Returns a deserialization error for an empty list or a repeated key identifier.
pub(super) fn compaction_keys<'de, D>(
    deserializer: D,
) -> Result<Vec1<CompactionKeyConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    /// Visitor that deserializes the sealing-key sequence and enforces the set contract.
    struct CompactionKeysVisitor;

    impl<'de> Visitor<'de> for CompactionKeysVisitor {
        type Value = Vec1<CompactionKeyConfig>;

        fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
            formatter.write_str("a list of compaction sealing keys")
        }

        fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let first = access
                .next_element::<CompactionKeyConfig>()?
                .ok_or_else(|| Error::custom("the compaction section declares no sealing key"))?;

            let mut keys = Vec1::new(first);

            while let Some(key) = access.next_element::<CompactionKeyConfig>()? {
                if keys.iter().any(|existing| existing.id == key.id) {
                    return Err(Error::custom(format!(
                        "the compaction key id `{}` is repeated",
                        key.id
                    )));
                }

                keys.push(key);
            }

            Ok(keys)
        }
    }

    deserializer.deserialize_seq(CompactionKeysVisitor)
}
