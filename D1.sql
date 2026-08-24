CREATE TABLE providers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    model_prefix TEXT NOT NULL DEFAULT '',
    type TEXT NOT NULL,
    protocol TEXT NULL,
    base_url TEXT NULL,
    encrypted_credentials BLOB NOT NULL,
    expires_at INTEGER NULL,
    refresh_started_at INTEGER NULL,
    enabled INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE models (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    provider_id INTEGER NOT NULL REFERENCES providers(id) ON DELETE RESTRICT,
    upstream_name TEXT NOT NULL,
    thinking_levels TEXT NULL,
    enabled INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE INDEX idx_models_provider_id ON models(provider_id);

CREATE TABLE oauth_states (
    id INTEGER PRIMARY KEY,
    type TEXT NOT NULL,
    state TEXT NOT NULL UNIQUE,
    verifier TEXT NOT NULL,
    provider_id INTEGER NULL REFERENCES providers(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_oauth_states_expires_at ON oauth_states(expires_at);
