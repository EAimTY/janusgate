//! Schema version 2: nullable quota recipes without changing existing provider bindings.

use super::Schema;

/// Complete quota-aware schema and its foreign-key-safe additive migration.
pub(super) const SCHEMA: Schema = Schema {
    version: 2,
    schema: r###"
PRAGMA application_id = 0x4A474D44;

CREATE TABLE provider_definitions (
    name TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    format TEXT NOT NULL CHECK (
        format IN (
            'openai_response',
            'openai_chat_completion',
            'gemini_generate_content',
            'gemini_interaction',
            'anthropic_messages'
        )
    ),
    inference TEXT NOT NULL CHECK (json_valid(inference)),
    credential TEXT NOT NULL CHECK (json_valid(credential)),
    provisioning TEXT NULL CHECK (provisioning IS NULL OR json_valid(provisioning)),
    discovery TEXT NULL CHECK (discovery IS NULL OR json_valid(discovery)),
    compaction TEXT NULL CHECK (compaction IS NULL OR json_valid(compaction)),
    created_at INTEGER NOT NULL,
    quota TEXT NULL CHECK (quota IS NULL OR json_valid(quota)),
    PRIMARY KEY (name, version)
) STRICT;

CREATE TABLE providers (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL UNIQUE,
    provider_definition_name TEXT NOT NULL,
    provider_definition_version INTEGER NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    model_prefix TEXT NOT NULL,
    base_url TEXT NULL,
    credentials TEXT NOT NULL CHECK (json_valid(credentials)),
    FOREIGN KEY (provider_definition_name, provider_definition_version)
        REFERENCES provider_definitions (name, version) ON DELETE RESTRICT
) STRICT;

CREATE TABLE models (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    provider_id INTEGER NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    upstream_id TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    discovery_state TEXT NOT NULL CHECK (
        discovery_state IN ('discovered', 'vanished', 'manual')
    ),
    public_id TEXT NOT NULL UNIQUE,
    id_override TEXT NULL,
    thinking_levels TEXT NULL CHECK (thinking_levels IS NULL OR json_valid(thinking_levels)),
    max_input_tokens INTEGER NULL CHECK (max_input_tokens IS NULL OR max_input_tokens > 0),
    max_output_tokens INTEGER NULL CHECK (max_output_tokens IS NULL OR max_output_tokens > 0),
    thinking_levels_override TEXT NULL CHECK (
        thinking_levels_override IS NULL OR json_valid(thinking_levels_override)
    ),
    max_input_tokens_override INTEGER NULL CHECK (
        max_input_tokens_override IS NULL OR max_input_tokens_override > 0
    ),
    max_output_tokens_override INTEGER NULL CHECK (
        max_output_tokens_override IS NULL OR max_output_tokens_override > 0
    ),
    compaction_support TEXT NULL CHECK (
        compaction_support IS NULL OR json_valid(compaction_support)
    ),
    compaction_support_override TEXT NULL CHECK (
        compaction_support_override IS NULL OR json_valid(compaction_support_override)
    ),
    UNIQUE (provider_id, upstream_id)
) STRICT;

CREATE TABLE ongoing_oauth_flows (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    provider_definition_name TEXT NOT NULL,
    provider_definition_version INTEGER NOT NULL,
    state_token TEXT NOT NULL UNIQUE,
    purpose TEXT NOT NULL CHECK (purpose IN ('create', 'reauthorize')),
    provider_name TEXT NULL,
    model_prefix TEXT NULL,
    base_url TEXT NULL,
    target_provider_id INTEGER NULL CHECK (
        target_provider_id IS NULL OR target_provider_id > 0
    ),
    grant_type TEXT NOT NULL CHECK (grant_type IN ('authorization_code', 'device')),
    slots TEXT NOT NULL CHECK (json_valid(slots)),
    code_verifier TEXT NULL,
    device_code TEXT NULL,
    device_user_code TEXT NULL,
    device_verification_uri TEXT NULL,
    device_verification_uri_complete TEXT NULL,
    device_poll_interval_seconds INTEGER NULL CHECK (
        device_poll_interval_seconds IS NULL OR device_poll_interval_seconds > 0
    ),
    device_next_poll_at INTEGER NULL,
    device_poll_started_at INTEGER NULL,
    device_exchange_checkpoint TEXT NULL CHECK (
        device_exchange_checkpoint IS NULL OR json_valid(device_exchange_checkpoint)
    ),
    outcome TEXT NULL CHECK (outcome IN ('succeeded', 'failed')),
    result_provider_id INTEGER NULL CHECK (
        result_provider_id IS NULL OR result_provider_id > 0
    ),
    failure_code TEXT NULL,
    failure_message TEXT NULL,
    expires_at INTEGER NOT NULL,
    FOREIGN KEY (provider_definition_name, provider_definition_version)
        REFERENCES provider_definitions (name, version) ON DELETE RESTRICT,
    CHECK (
        (purpose = 'create' AND provider_name IS NOT NULL AND model_prefix IS NOT NULL
         AND target_provider_id IS NULL)
        OR (purpose = 'reauthorize' AND provider_name IS NULL AND model_prefix IS NULL
            AND target_provider_id IS NOT NULL)
    ),
    CHECK (
        (grant_type = 'authorization_code' AND device_code IS NULL
         AND device_user_code IS NULL AND device_verification_uri IS NULL
         AND device_verification_uri_complete IS NULL AND device_poll_interval_seconds IS NULL
         AND device_next_poll_at IS NULL AND device_poll_started_at IS NULL)
        OR (grant_type = 'device' AND code_verifier IS NULL
            AND device_code IS NOT NULL AND device_user_code IS NOT NULL
            AND (device_verification_uri IS NOT NULL
                 OR device_verification_uri_complete IS NOT NULL)
            AND device_poll_interval_seconds IS NOT NULL
            AND ((outcome IS NULL AND device_next_poll_at IS NOT NULL)
                 OR (outcome IS NOT NULL AND device_next_poll_at IS NULL)))
    ),
    CHECK (outcome IS NULL OR device_poll_started_at IS NULL),
    CHECK (outcome IS NULL OR grant_type = 'device'),
    CHECK (device_exchange_checkpoint IS NULL OR grant_type = 'device'),
    CHECK (device_exchange_checkpoint IS NULL OR device_poll_started_at IS NOT NULL),
    CHECK (outcome IS NULL OR device_exchange_checkpoint IS NULL),
    CHECK (
        (outcome IS NULL AND result_provider_id IS NULL
         AND failure_code IS NULL AND failure_message IS NULL)
        OR (outcome = 'succeeded' AND result_provider_id IS NOT NULL
            AND failure_code IS NULL AND failure_message IS NULL)
        OR (outcome = 'failed' AND result_provider_id IS NULL
            AND failure_code IS NOT NULL AND failure_message IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_ongoing_oauth_flows_expires_at ON ongoing_oauth_flows (expires_at);

CREATE INDEX idx_ongoing_oauth_flows_device_next_poll_at
    ON ongoing_oauth_flows (device_next_poll_at);

CREATE TABLE api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL,
    key TEXT NOT NULL UNIQUE,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;
"###,
    migration: "ALTER TABLE provider_definitions ADD COLUMN quota TEXT NULL \
                CHECK (quota IS NULL OR json_valid(quota));",
};
