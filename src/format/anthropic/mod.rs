//! The Anthropic protocol's Messages family.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module holds the Messages family ([`messages`]) — the one codec over
//! the Anthropic client protocol and upstream wire (`docs/design/architecture.md` and
//! `docs/design/providers.md` and `docs/reference/inference.md`); the outward error render its
//! surface shares lives in [`crate::api::error::anthropic`] (`docs/reference/inference.md`).

pub(crate) mod messages;
