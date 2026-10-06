//! The Anthropic protocol's Messages family.
//!
//! The format tree is a protocol hierarchy: the [`crate::format`] root holds only cross-protocol
//! items, and each protocol parent holds the families speaking that one protocol plus the items
//! its families share. This module holds the Messages family ([`messages`]) — the one codec over
//! the Anthropic client protocol and upstream wire (README §1, §5.8, §6); the outward error
//! render its surface shares lives in [`crate::api::error::anthropic`] (README §6.7).

pub(crate) mod messages;
