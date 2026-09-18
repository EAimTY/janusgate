//! Schema version 1: the initial database layout.
//!
//! Version 1 creates every current object — the `providers`, `models`, and `oauth_states` tables
//! and the `idx_oauth_states_expires_at` index — so its [`Schema::migration`] is empty: there is no
//! previous version to migrate from. A fresh database receives this version's DDL directly, and
//! every later version's migration transforms it.

use super::Schema;

/// The schema of version 1.
///
/// The DDL is hand-written SQL in a raw string literal: every statement is terminated by `;`,
/// the exact shape the migration workflow embeds into its transaction batches. Its validity is
/// enforced by tests: the `tests` module in this file applies it to a fresh database, and the
/// parent module walks every migration forward into its declared schema.
pub(super) const SCHEMA: Schema = Schema {
    version: 1,
    schema: SCHEMA_SQL,
    migration: MIGRATION_SQL,
};

/// Complete DDL of version 1, applied to a fresh database.
const SCHEMA_SQL: &str = r###"
CREATE TABLE providers (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE CHECK (name != ''),
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    model_prefix TEXT NOT NULL,
    type TEXT NOT NULL CHECK (
        type IN ('openai_oauth', 'antigravity_oauth', 'openai_compatible')
    ),
    protocol TEXT NULL CHECK (
        protocol ISNULL
        OR protocol IN ('openai_response', 'openai_chat_completion', 'gemini_generate_content')
    ),
    base_url TEXT NULL,
    credentials TEXT NOT NULL,
    expires_at INTEGER NULL,
    refresh_started_at INTEGER NULL
) STRICT;

CREATE TABLE models (
    provider_id INTEGER NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    upstream_id TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    id TEXT NOT NULL UNIQUE,
    id_override TEXT NULL,
    thinking_levels_override TEXT NULL,
    PRIMARY KEY (provider_id, upstream_id)
) STRICT;

CREATE TABLE oauth_states (
    id INTEGER PRIMARY KEY NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('openai_oauth', 'antigravity_oauth')),
    state TEXT NOT NULL UNIQUE,
    verifier TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_oauth_states_expires_at ON oauth_states (expires_at);
"###;

/// Migration upgrading the previous version into version 1: empty, as version 1 has no
/// predecessor.
const MIGRATION_SQL: &str = "";

#[cfg(test)]
mod tests {
    use super::{super::open_memory_connection, SCHEMA};
    use std::error::Error as StdError;

    /// Verifies that this version's DDL applies cleanly to a fresh in-memory database.
    ///
    /// # Panics
    ///
    /// Panics if opening the in-memory database fails.
    #[tokio::test]
    async fn schema_applies_to_a_fresh_database() -> Result<(), Box<dyn StdError>> {
        open_memory_connection()
            .await
            .execute_batch(SCHEMA.schema)
            .await?;

        Ok(())
    }
}
