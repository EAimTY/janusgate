//! Api-log schema version 1: the initial api-log file layout (`docs/design/storage.md` *ApiLog
//! Store*).
//!
//! Version 1 creates the `api_logs` table and its five indexes in the api-log store's separate
//! registry. Its [`Schema::migration`] is empty: there is no previous version to migrate from.
//! A fresh api-log file receives this version's DDL directly, and every later version's migration
//! transforms it.
//!
//! The version's SQL opens by stamping the file's registry identity into `PRAGMA application_id`
//! — the fourcc `JGAL` — written by version 1's SQL itself and never at runtime. The bootstrap
//! verifies the stamp on any file that already carries a schema version, so a file opened under
//! the wrong registry — or a file predating the stamp, an unreleased development artifact the
//! operator recreates — fails at startup instead of at first query.
//!
//! `api_logs` is request accounting: one row per dispatched inference request:
//!
//! - **Dispatch is the recording boundary**: a row exists exactly when the upstream HTTP request
//!   was issued (`send()` called). Everything before that instant — authentication, routing 404s,
//!   token refresh, upstream payload rendering failures — never reaches this table.
//! - `id` deliberately carries no `AUTOINCREMENT`: no identity reference points at `api_logs` rows
//!   and the id is never published — it stays internal as the per-file tie-break of the list
//!   ordering (`ORDER BY started_at DESC, id DESC`), so its number may be recycled.
//! - No foreign keys: `api-keys:delete`, `providers:delete`, and `models:delete` neither cascade
//!   into nor block log rows. A deleted row's logs remain untouched and keep attributing by id —
//!   sound precisely because the referenced identities (`providers.id`, `api_keys.id`, `models.id`,
//!   `model_routes.id`) are never recycled.
//! - No name snapshot: labels are resolved by the frontend through `api-keys:list`,
//!   `providers:list`, `models:list`, and `model-routes:list`.
//! - The model is referenced by `model_id` — a bare `models.id` integer — for the same reason
//!   `api_key_id` and `provider_id` are bare integers: no FK, so a `models:delete` leaves the log
//!   row intact and attribution by id unchanged.
//! - `model_route_id` separately attributes the explicitly published route, while `model_id` and
//!   `provider_id` identify the physical account attempt.
//! - `upstream_http_status` records the first data-plane response on the attempt, including an
//!   unsupported native-compaction stage; `NULL` means no upstream response was obtained.
//! - `http_status` is `NULL` exactly when no response was ever produced to the client, and
//!   otherwise a status `CHECK`-pinned to 100-999 — the full `StatusCode` domain the gateway
//!   mirrors verbatim from the upstream response, so a recorded status is never rejected at write
//!   time; the per-outcome decision rules live in the recording code. An abandoned quota rejection
//!   keeps `http_status` null even though its upstream status is known, because that account
//!   attempt produced no outward response.
//! - `duration` counts whole milliseconds from the dispatch stamp to the recording instant; the
//!   recording code saturates a wall clock stepped backwards, and the schema `CHECK`-pins the
//!   result non-negative.
//! - The five token columns are `NULL` when the upstream reported no usage; `output_tokens`
//!   includes `reasoning_tokens`.
//! - `started_at` (dispatch-start instant, microseconds) is indexed alone
//!   (`idx_api_logs_started_at`) and in one composite index per attribution id
//!   (`idx_api_logs_api_key_started_at`, `idx_api_logs_provider_started_at`,
//!   `idx_api_logs_model_started_at`, `idx_api_logs_model_route_started_at`): the management
//!   filters pair a high-selectivity id with a time window, while the low-cardinality columns —
//!   `outcome`, `endpoint_type`, `is_stream`, `http_status` — are deliberately not indexed. List
//!   reads order by `started_at DESC, id DESC`.
//! - The requested reasoning effort and any content — prompt, completion, headers, upstream error
//!   bodies — are never recorded.

use super::Schema;

/// The schema of version 1.
///
/// The DDL is hand-written SQL in a raw string literal: every statement is terminated by `;`,
/// the exact shape the migration workflow embeds into its transaction batches. The bootstrap
/// applies it verbatim to a fresh api-log file and walks every later migration forward from it.
pub(super) const SCHEMA: Schema = Schema {
    version: 1,
    schema: r###"
PRAGMA application_id = 0x4A47414C;

CREATE TABLE api_logs (
    id INTEGER PRIMARY KEY NOT NULL,
    started_at INTEGER NOT NULL,
    api_key_id INTEGER NOT NULL,
    provider_id INTEGER NOT NULL,
    model_id INTEGER NOT NULL,
    model_route_id INTEGER NOT NULL,
    upstream_http_status INTEGER NULL CHECK (
        upstream_http_status IS NULL OR upstream_http_status BETWEEN 100 AND 999
    ),
    endpoint_type TEXT NOT NULL CHECK (
        endpoint_type IN (
            'openai_chat_completions',
            'openai_responses',
            'openai_responses_compact',
            'gemini_generate_content',
            'gemini_interactions',
            'anthropic_messages'
        )
    ),
    is_stream INTEGER NOT NULL CHECK (is_stream IN (0, 1)),
    http_status INTEGER NULL CHECK (http_status IS NULL OR http_status BETWEEN 100 AND 999),
    outcome TEXT NOT NULL CHECK (
        outcome IN ('completed', 'client_aborted', 'upstream_error', 'internal')
    ),
    duration INTEGER NOT NULL CHECK (duration >= 0),
    input_tokens INTEGER NULL,
    output_tokens INTEGER NULL,
    cached_tokens INTEGER NULL,
    cache_write_tokens INTEGER NULL,
    reasoning_tokens INTEGER NULL
) STRICT;

CREATE INDEX idx_api_logs_started_at ON api_logs (started_at);

CREATE INDEX idx_api_logs_api_key_started_at ON api_logs (api_key_id, started_at);

CREATE INDEX idx_api_logs_provider_started_at ON api_logs (provider_id, started_at);

CREATE INDEX idx_api_logs_model_started_at ON api_logs (model_id, started_at);

CREATE INDEX idx_api_logs_model_route_started_at ON api_logs (model_route_id, started_at);
"###,
    migration: "",
};
