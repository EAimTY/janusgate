//! Selects a configured foreign response envelope without constructing a JSON DOM.

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

/// Selects the last occurrence of a configured root member, matching ordinary JSON decoding.
struct EnvelopeMember<'a> {
    /// Provider-authored object member naming the actual protocol document.
    name: &'a str,
}

impl<'de> DeserializeSeed<'de> for EnvelopeMember<'_> {
    type Value = Option<&'de RawValue>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EnvelopeMember<'_> {
    type Value = Option<&'de RawValue>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a provider response envelope")
    }

    fn visit_map<A>(self, mut members: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut selected = None;
        while let Some(name) = members.next_key::<String>()? {
            if name == self.name {
                selected = Some(members.next_value::<&RawValue>()?);
            } else {
                members.next_value::<IgnoredAny>()?;
            }
        }
        Ok(selected)
    }
}

/// Borrows the configured member's JSON bytes, or the original body when it is absent.
/// A non-object root passes through unchanged; a present null member is selected as null.
///
/// # Errors
///
/// Returns a JSON decoding failure when the complete response is malformed.
pub(super) fn unwrap<'a>(
    body: &'a [u8],
    member: Option<&str>,
) -> Result<&'a [u8], serde_json::Error> {
    let Some(member) = member else {
        return Ok(body);
    };
    let raw: &RawValue = serde_json::from_slice(body)?;
    if !raw.get().starts_with('{') {
        return Ok(body);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let selected = EnvelopeMember { name: member }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(selected.map_or(body, |selected| selected.get().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::unwrap;

    /// Envelope selection must preserve raw numbers and nested native payloads, distinguish
    /// present null from missing, consume the whole document, and resolve duplicate keys.
    ///
    /// # Panics
    ///
    /// Panics when configured envelope selection changes its ingestion contract.
    #[test]
    #[expect(
        clippy::expect_used,
        reason = "deliberate envelope fixtures are valid JSON"
    )]
    fn selects_only_the_authored_member_and_preserves_its_wire_bytes() {
        let body = br#"{
            "ignored":{"deep":[1,2]},
            "response":{"n":1.00,"native":[null,{"signature":"x"}]}
        }"#;
        assert_eq!(
            unwrap(body, Some("response")).expect("envelope"),
            br#"{"n":1.00,"native":[null,{"signature":"x"}]}"#
        );
        assert_eq!(
            unwrap(br#"{"response":null}"#, Some("response")).expect("null"),
            b"null"
        );
        for body in [b"[]".as_slice(), br#"{"other":1}"#] {
            assert_eq!(unwrap(body, Some("response")).expect("absent"), body);
        }
        assert_eq!(
            unwrap(br#"{"response":1,"response":2}"#, Some("response")).expect("duplicates"),
            b"2"
        );
        assert!(unwrap(br#"{"response":{}} trailing"#, Some("response")).is_err());
    }
}
