//! Test support: seeding helpers shared by the integration-style tests across the crate.
//!
//! Every helper panics when its fixed fixture fails to parse, validate, or insert — a test
//! bug by construction, so the panic is the failure report.

#![expect(
    clippy::panic,
    reason = "a fixture that cannot parse, validate, or insert is a test bug, and the panic \
              is the failure report"
)]

use crate::{
    database::Database,
    provider::{Provider, ProviderDefinition, StoredCredentials, definition::DefinitionPayload},
    storage,
};
use serde_json::json;
use std::{num::NonZero, path::PathBuf, time::Duration};

/// File guard cleaning up the temporary SQLite test database on drop.
pub(crate) struct TempDbGuard(PathBuf);

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Directory guard removing the temporary test directory tree on drop.
pub(crate) struct TempDirGuard(pub(crate) PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates an initialized temporary SQLite test database.
///
/// # Panics
///
/// Panics if opening the test database fails — a broken fixture environment is the failure
/// report.
pub(crate) async fn create_test_database() -> (Database, TempDbGuard) {
    let db_path = std::env::temp_dir().join(format!("janusgate-test-{}.db", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero literal")]
    let max_connections = NonZero::new(2).unwrap();

    #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
    let database = Database::open(
        &db_path,
        crate::database::schema::Registry::Main,
        max_connections,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    (database, TempDbGuard(db_path))
}

/// Inserts one static-key definition version 1 of the given format and returns the stored
/// definition.
///
/// # Panics
///
/// Panics if the insert fails, the payload does not parse, or the fixture fails validation —
/// none of which can occur for the fixed fixture payloads.
pub(crate) async fn insert_static_key_definition(
    database: &Database,
    name: &str,
    format: crate::format::UpstreamFormat,
) -> ProviderDefinition {
    let payload: DefinitionPayload = serde_json::from_value(json!({
        "format": format.as_str(),
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"},
        "discovery": {
            "request": {
                "method": "GET",
                "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
            },
            "extract": {"mode": "array", "array_at": ["data"], "id_field": ["id"]}
        }
    }))
    .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"));

    #[expect(
        clippy::unwrap_used,
        reason = "the fixture definition always validates"
    )]
    payload.validate().unwrap();

    storage::provider_definitions::insert_definition(
        database,
        storage::provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload: &payload,
            created_at: 1,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture definition inserts: {error}"))
}

/// Inserts one OAuth definition version 1 whose refresh grant fails against an unreachable
/// endpoint, for the pre-dispatch refresh-failure scenario.
///
/// The exchange references the `redirect_uri` value part because the write boundary requires
/// that of every authorization-code grant (RFC 6749 §4.1.3).
///
/// # Panics
///
/// Panics if the insert fails, the payload does not parse, or the fixture fails validation —
/// none of which can occur for the fixed fixture payloads.
pub(crate) async fn insert_failing_refresh_definition(
    database: &Database,
    name: &str,
) -> ProviderDefinition {
    let payload: DefinitionPayload = serde_json::from_value(json!({
        "format": "gemini_generate_content",
        "inference": {
            "url": [{"type": "base"}],
            "upstream_streaming": "match_client"
        },
        "credential": {
            "type": "oauth",
            "client_id": "fixture-client",
            "initial_slots": [{"name": "project", "generator": "uuid_hex"}],
            "grant": {
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://accounts.example/auth"}],
                "redirect_uri": "http://localhost:1/callback",
                "pkce": "none"
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": "http://127.0.0.1:1/token"}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "authorization_code"}
                        ]},
                        {"name": "code", "value": [{"type": "authorization_code"}]},
                        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}
                    ]}
                },
                "response": {
                    "encoding": "json",
                    "fields": [
                        {
                            "path": ["access_token"],
                            "target": {"type": "access_token"},
                            "required": true
                        },
                        {
                            "path": ["refresh_token"],
                            "target": {"type": "refresh_token"},
                            "required": true
                        }
                    ]
                },
                "terminal_codes": ["invalid_grant"]
            },
            "refresh": {
                "type": "grant",
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": "http://127.0.0.1:1/token"}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "refresh_token"}
                        ]},
                        {"name": "refresh_token", "value": [{"type": "refresh_token"}]}
                    ]}
                },
                "terminal_codes": ["invalid_grant"]
            }
        }
    }))
    .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"));

    #[expect(
        clippy::unwrap_used,
        reason = "the fixture definition always validates"
    )]
    payload.validate().unwrap();

    storage::provider_definitions::insert_definition(
        database,
        storage::provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload: &payload,
            created_at: 1,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture definition inserts: {error}"))
}

/// Inserts one provider row of the given definition and credential, returning the stored row.
///
/// # Panics
///
/// Panics if the insert fails, or if the fixture credentials disagree with the definition's
/// credential kind — a test bug the assertion surfaces.
pub(crate) async fn insert_provider(
    database: &Database,
    definition: &ProviderDefinition,
    name: &str,
    model_prefix: &str,
    base_url: Option<&str>,
    credentials: StoredCredentials,
) -> Provider {
    assert_eq!(
        credentials.kind(),
        definition.credential_kind(),
        "fixture credentials must match the definition's credential kind"
    );

    storage::providers::insert_provider(
        database,
        storage::providers::NewProvider {
            name,
            model_prefix,
            base_url,
            credentials: &credentials,
            definition,
            is_enabled: true,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture provider inserts: {error}"))
}

/// Inserts one OAuth definition version 1 — an authorization-code grant whose exchange maps the
/// access token and whose credential never renews — for the flow-begin and completion tests.
///
/// The exchange references the `redirect_uri` value part because the write boundary requires
/// that of every authorization-code grant (RFC 6749 §4.1.3).
///
/// # Panics
///
/// Panics if the insert fails, the payload does not parse, or the fixture fails validation —
/// none of which can occur for the fixed fixture payloads.
pub(crate) async fn insert_oauth_definition(database: &Database, name: &str) -> ProviderDefinition {
    let payload: DefinitionPayload = serde_json::from_value(json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {
            "type": "oauth",
            "client_id": "fixture-client",
            "grant": {
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://accounts.example/auth"}],
                "redirect_uri": "http://localhost:1/callback",
                "pkce": "none"
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": "http://127.0.0.1:1/token"}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "authorization_code"}
                        ]},
                        {"name": "code", "value": [{"type": "authorization_code"}]},
                        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}
                    ]}
                },
                "response": {
                    "encoding": "json",
                    "fields": [
                        {
                            "path": ["access_token"],
                            "target": {"type": "access_token"},
                            "required": true
                        }
                    ]
                },
                "terminal_codes": ["invalid_grant"]
            },
            "refresh": {"type": "none"}
        }
    }))
    .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"));

    #[expect(
        clippy::unwrap_used,
        reason = "the fixture definition always validates"
    )]
    payload.validate().unwrap();

    storage::provider_definitions::insert_definition(
        database,
        storage::provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload: &payload,
            created_at: 1,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture definition inserts: {error}"))
}
