//! Selects a configured foreign response envelope without constructing a JSON DOM.

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

#[cfg(test)]
mod tests;

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
