//! Main schema with automatic pool model exposure and pool-owned naming prefixes.

use super::Schema;

/// Complete main database layout, applied only to a fresh database.
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
    quota_generation INTEGER NOT NULL DEFAULT 0 CHECK (quota_generation >= 0),
    quota_revision INTEGER NOT NULL DEFAULT 0 CHECK (quota_revision >= 0),
    model_prefix TEXT NOT NULL DEFAULT '',
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


CREATE TABLE provider_pools (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL UNIQUE,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    model_prefix TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE TABLE provider_pool_members (
    pool_id INTEGER NOT NULL REFERENCES provider_pools (id) ON DELETE CASCADE,
    provider_id INTEGER NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    priority INTEGER NOT NULL CHECK (priority >= 0),
    PRIMARY KEY (pool_id, provider_id),
    UNIQUE (pool_id, priority)
) STRICT;

CREATE TABLE model_routes (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    public_id TEXT NOT NULL UNIQUE,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    is_automatic INTEGER NOT NULL DEFAULT 0 CHECK (is_automatic IN (0, 1)),
    model_id INTEGER NULL REFERENCES models (id) ON DELETE CASCADE,
    pool_id INTEGER NULL REFERENCES provider_pools (id) ON DELETE RESTRICT,
    upstream_id TEXT NULL,
    CHECK (
        (model_id IS NOT NULL AND pool_id IS NULL AND upstream_id IS NULL)
        OR
        (model_id IS NULL AND pool_id IS NOT NULL AND upstream_id IS NOT NULL)
    )
) STRICT;

CREATE UNIQUE INDEX idx_model_routes_automatic_model_id
    ON model_routes (model_id) WHERE is_automatic = 1 AND model_id IS NOT NULL;

CREATE UNIQUE INDEX idx_model_routes_automatic_pool_model
    ON model_routes (pool_id, upstream_id) WHERE is_automatic = 1 AND pool_id IS NOT NULL;

CREATE INDEX idx_model_routes_model_id ON model_routes (model_id);
CREATE INDEX idx_model_routes_pool_id ON model_routes (pool_id);
CREATE INDEX idx_provider_pool_members_provider_id ON provider_pool_members (provider_id);

CREATE TABLE provider_quota_blocks (
    provider_id INTEGER NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    restriction_key TEXT NOT NULL CHECK (json_valid(restriction_key)),
    applies_to TEXT NOT NULL CHECK (json_valid(applies_to)),
    observed_at INTEGER NOT NULL,
    blocked_until INTEGER NULL,
    PRIMARY KEY (provider_id, restriction_key),
    CHECK (blocked_until IS NULL OR blocked_until > observed_at)
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
    migration: r###"
ALTER TABLE provider_pools ADD COLUMN model_prefix TEXT NOT NULL DEFAULT '';
UPDATE provider_pools SET model_prefix = 'pool-' || id || '-';
CREATE TEMP TABLE model_route_sequence (previous_sequence INTEGER NOT NULL) STRICT;
INSERT INTO model_route_sequence SELECT seq FROM sqlite_sequence WHERE name = 'model_routes';
CREATE TABLE model_routes_next (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    public_id TEXT NOT NULL UNIQUE,
    is_enabled INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    is_automatic INTEGER NOT NULL DEFAULT 0 CHECK (is_automatic IN (0, 1)),
    model_id INTEGER NULL REFERENCES models (id) ON DELETE CASCADE,
    pool_id INTEGER NULL REFERENCES provider_pools (id) ON DELETE RESTRICT,
    upstream_id TEXT NULL,
    CHECK (
        (model_id IS NOT NULL AND pool_id IS NULL AND upstream_id IS NULL)
        OR
        (model_id IS NULL AND pool_id IS NOT NULL AND upstream_id IS NOT NULL)
    )
) STRICT;


INSERT INTO model_routes_next SELECT * FROM model_routes;
DROP TABLE model_routes;
ALTER TABLE model_routes_next RENAME TO model_routes;
UPDATE sqlite_sequence SET seq = MAX(seq, COALESCE((SELECT previous_sequence FROM
model_route_sequence), 0)) WHERE name = 'model_routes';
DROP TABLE model_route_sequence;
CREATE UNIQUE INDEX idx_model_routes_automatic_model_id
    ON model_routes (model_id) WHERE is_automatic = 1 AND model_id IS NOT NULL;

CREATE UNIQUE INDEX idx_model_routes_automatic_pool_model
    ON model_routes (pool_id, upstream_id) WHERE is_automatic = 1 AND pool_id IS NOT NULL;

CREATE INDEX idx_model_routes_model_id ON model_routes (model_id);
CREATE INDEX idx_model_routes_pool_id ON model_routes (pool_id);

DELETE FROM model_routes WHERE is_automatic = 1 AND model_id IN (SELECT model.id FROM models
model JOIN provider_pool_members member ON member.provider_id = model.provider_id);
INSERT INTO model_routes (public_id, model_id, is_automatic)
SELECT provider.model_prefix || model.upstream_id, model.id, 1 FROM models model JOIN
providers provider ON provider.id = model.provider_id
WHERE NOT EXISTS (SELECT 1 FROM provider_pool_members member WHERE member.provider_id =
provider.id)
AND NOT EXISTS (SELECT 1 FROM model_routes route WHERE route.model_id = model.id AND
route.is_automatic = 1);
INSERT INTO model_routes (public_id, pool_id, upstream_id, is_automatic)
SELECT DISTINCT pool.model_prefix || model.upstream_id, pool.id, model.upstream_id, 1 FROM
provider_pools pool JOIN provider_pool_members member ON member.pool_id = pool.id JOIN models
model ON model.provider_id = member.provider_id;
"###,
};
