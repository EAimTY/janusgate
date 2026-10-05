//! The serde adapters of the two-state field rule (README §7.0 rule 2): a wire field is either
//! always present — required, and nullable only when it mirrors a nullable storage column — or
//! absent-keeps, and never both `null` and a value at once.
//!
//! The two adapters are crate-neutral on purpose: the management API payloads declare both, and
//! the storage tier's shared [`UsageFilter`](crate::storage::api_logs::UsageFilter) declares the
//! absent-keeps deserializer too, so neither layer owns the other's internals. The management
//! API version modules re-export them, so their payloads name the adapters through their own
//! version module.
//!
//! [`Nullable`] is the required-nullable field adapter; [`absent_keeps`] is the absent-keeps
//! field adapter, consumed as a `deserialize_with` function.

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as DeError, Visitor},
};
use std::{
    fmt::{Error as FmtError, Formatter},
    marker::PhantomData,
};

/// Reads a foreign field only when it carries a string, consuming every other JSON shape.
/// This is for tolerant upstream probes, never for authored configuration or client contracts.
///
/// # Errors
///
/// Returns the deserializer's error for malformed input.
pub(crate) fn string_or_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    /// A probe consumes uninterpreted external values without constructing arbitrary JSON.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Field {
        /// The string representation the caller actually consumes.
        Text(String),
        /// Every other representation, discarded after complete decoding.
        Ignored(serde::de::IgnoredAny),
    }
    Ok(match Field::deserialize(deserializer)? {
        Field::Text(text) => Some(text),
        Field::Ignored(_) => None,
    })
}

/// A required, nullable wire field (README §7.0 rule 2): its value may be `null`, mirroring a
/// nullable storage column exactly, and the TypeScript bindings render the key as required (`T |
/// null`). A field of this type carries `#[ts(type = "<T's TypeScript spelling> | null")]`, because
/// the generic parameter has no TypeScript rendering of its own.
///
/// One tolerance is documented rather than fought: serde routes both a missing key and an explicit
/// `null` through the same none path, so the parser reads an omitted key as `null`. Under
/// full-state update semantics (rule 3) the two spellings share the one meaning — "no override" —
/// and the bindings keep well-typed clients honest by requiring the key.
///
/// The struct is `pub` only because it is the field type of publicly reachable wire payloads;
/// the defining module stays private, so the name is reachable exclusively through the
/// re-exports of the surfaces that declare it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nullable<T>(pub Option<T>);

impl<T> Serialize for Nullable<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.0 {
            Some(value) => value.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }
}

impl<'de, T> Deserialize<'de> for Nullable<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Accepts a value or `null`; see the type's documentation for the missing-key tolerance.
        struct NullableVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for NullableVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = Nullable<T>;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
                formatter.write_str("a value or null")
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize(deserializer).map(|value| Nullable(Some(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(Nullable(None))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(Nullable(None))
            }
        }

        deserializer.deserialize_option(NullableVisitor(PhantomData))
    }
}

/// The field adapter of an absent-keeps field (README §7.0 rule 2): the key may be omitted, but an
/// explicit `null` is rejected — absence and `null` are not two spellings of one state. Pair with
/// `#[serde(default)]` so a missing key defaults rather than errors, and `#[ts(optional)]` so the
/// bindings render `field?: T`.
pub(crate) mod absent_keeps {
    use serde::{
        Deserialize, Deserializer,
        de::{Error, Visitor},
    };
    use std::{
        fmt::{Error as FmtError, Formatter},
        marker::PhantomData,
    };

    /// Deserializes a present value into `Some`, rejecting an explicit `null`.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's error when the value is `null` or malformed.
    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        /// Accepts a value, never `null`.
        struct AbsentKeepsVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for AbsentKeepsVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = Option<T>;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
                formatter.write_str("a value, not `null` — omit the field to keep the stored one")
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize(deserializer).map(Some)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Err(Error::custom(
                    "`null` is not accepted here — omit the field to keep the stored value",
                ))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Err(Error::custom(
                    "`null` is not accepted here — omit the field to keep the stored value",
                ))
            }
        }

        deserializer.deserialize_option(AbsentKeepsVisitor(PhantomData))
    }
}
