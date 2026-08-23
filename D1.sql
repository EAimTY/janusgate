CREATE TABLE providers (
    -- Internal surrogate key; never expose through an API
    id Integer PRIMARY KEY,

    -- Immutable external identifier
    name Text NOT NULL UNIQUE,

    -- Seeded from name at creation; an empty string disables prefixing
    model_prefix Text NOT NULL DEFAULT '',

    type Text NOT NULL,
    protocol Text NULL,
    base_url Text NULL,

    -- AES-256-GCM value laid out as IV || ciphertext || tag
    encrypted_credentials Blob NOT NULL,

    -- Unix seconds
    expires_at Integer NULL,
    refresh_started_at Integer NULL,

    enabled Integer NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE models (
    id Integer PRIMARY KEY,

    name Text NOT NULL UNIQUE,
    provider_id Integer NOT NULL
        REFERENCES providers(id)
        ON DELETE RESTRICT,
    upstream_name Text NOT NULL,

    -- JSON array of canonical thinking levels; NULL selects adapter defaults
    thinking_levels Text NULL,

    enabled Integer NOT NULL DEFAULT 1
) STRICT;

CREATE INDEX idx_models_provider_id
    ON models(provider_id);

CREATE TABLE oauth_states (
    id Integer PRIMARY KEY,

    type Text NOT NULL,
    state Text NOT NULL UNIQUE,
    verifier Text NOT NULL,

    -- NULL for new-provider flows; set for reauthentication flows
    provider_id Integer NULL
        REFERENCES providers(id)
        ON DELETE CASCADE,

    -- Unix seconds
    expires_at Integer NOT NULL
) STRICT;

CREATE INDEX idx_oauth_states_expires_at
    ON oauth_states(expires_at);
