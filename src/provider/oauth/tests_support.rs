//! Shared OAuth-engine test fixtures: the connection-recipe payloads whose engines the flow,
//! exchange, provisioning, and refresh test families drive against the mock upstream.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a fixture that cannot parse is that test's failure report"
)]

use crate::provider::definition::DefinitionPayload;
use serde_json::{Value, json};

/// Parses one recipe payload fixture.
pub(crate) fn parse(payload: Value) -> DefinitionPayload {
    serde_json::from_value(payload).expect("fixture OAuth payload parses")
}

/// Replaces the value at `pointer` in one payload fixture.
pub(crate) fn tap_pointer(mut payload: Value, pointer: &str, value: Value) -> Value {
    *payload
        .pointer_mut(pointer)
        .expect("the fixture pointer resolves") = value;

    payload
}

/// Inserts `value` under `key` into the object at `pointer` — the spelling for members the
/// base fixture omits because serde reads their absence as the default.
pub(crate) fn with_member(mut payload: Value, pointer: &str, key: &str, value: Value) -> Value {
    payload
        .pointer_mut(pointer)
        .expect("the fixture pointer resolves")
        .as_object_mut()
        .expect("the fixture pointer addresses an object")
        .insert(key.to_string(), value);

    payload
}

/// Sets the provisioning sequence of one payload fixture.
pub(crate) fn with_provisioning(mut payload: Value, steps: Value) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "the fixture payload is an object by construction"
    )]
    payload
        .as_object_mut()
        .expect("the fixture payload is an object")
        .insert("provisioning".to_string(), steps);

    payload
}

/// The form fields of the fixture token exchange: grant type, code, redirect URI, and the
/// client identity — the wire shape `build_request` renders and the mock records.
pub(crate) fn exchange_fields() -> Value {
    json!([
        {"name": "grant_type", "value": [{"type": "literal", "value": "authorization_code"}]},
        {"name": "code", "value": [{"type": "authorization_code"}]},
        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
        {"name": "client_id", "value": [{"type": "client_id"}]},
        {"name": "client_secret", "value": [{"type": "client_secret"}]}
    ])
}

/// The exchange response mapping of the fixture recipes: the two tokens, the lifetime, the
/// derived base URL, and the `project` slot.
pub(crate) fn exchange_response() -> Value {
    json!({
        "encoding": "json",
        "fields": [
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true},
            {"path": ["expires_in"], "target": {"type": "expires_in_seconds"}, "required": false},
            {"path": ["base_url"], "target": {"type": "base_url"}, "required": false},
            {"path": ["project"], "target": {"type": "slot", "name": "project"}, "required": false}
        ]
    })
}

/// The authorization-code recipe whose exchange and refresh grant both address the mock
/// upstream under `base`.
pub(crate) fn code_payload(base: &str) -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/v1/chat/completions"}],
            "upstream_streaming": "match_client",
            "headers": [{"name": "Authorization", "value": [{"type": "secret"}]}]
        },
        "credential": {"type": "oauth",
            "client_id": "client-1",
            "client_secret": "secret-1",
            "grant": {
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://up.test/authorize"}],
                "redirect_uri": "http://localhost:1455/callback",
                "pkce": "none"
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": format!("{base}/token")}],
                    "body": {"kind": "form", "fields": exchange_fields()}
                },
                "response": exchange_response(),
                "terminal_codes": ["invalid_grant", "expired_token"]
            },
            "refresh": {"type": "grant",
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": format!("{base}/token")}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type",
                         "value": [{"type": "literal", "value": "refresh_token"}]},
                        {"name": "refresh_token", "value": [{"type": "refresh_token"}]},
                        {"name": "client_id", "value": [{"type": "client_id"}]},
                        {"name": "client_secret", "value": [{"type": "client_secret"}]}
                    ]}
                },
                "response": {"encoding": "json", "fields": [
                    {"path": ["access_token"], "target": {"type": "access_token"},
                     "required": true},
                    {"path": ["refresh_token"], "target": {"type": "refresh_token"},
                     "required": false},
                    {"path": ["expires_in"], "target": {"type": "expires_in_seconds"},
                     "required": false}
                ]},
                "terminal_codes": ["invalid_grant", "expired_token"]
            }
        }
    })
}

/// The device-grant recipe whose device authorization and polling exchange address the mock
/// upstream under `base`. The lifetime falls back to the configured 900 seconds.
pub(crate) fn device_payload(base: &str) -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/v1/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "oauth",
            "client_id": "client-1",
            "client_secret": "secret-1",
            "grant": {
                "type": "device",
                "authorization": {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": format!("{base}/device")}],
                        "body": {"kind": "form", "fields": [
                            {"name": "client_id", "value": [{"type": "client_id"}]}
                        ]}
                    },
                    "response": {"encoding": "json", "fields": [
                        {"path": ["device_code"], "target": {"type": "device_code"},
                         "required": true},
                        {"path": ["user_code"], "target": {"type": "user_code"}, "required": true},
                        {"path": ["verification_uri"], "target": {"type": "verification_uri"},
                         "required": true},
                        {"path": ["verification_uri_complete"],
                         "target": {"type": "verification_uri_complete"}, "required": false},
                        {"path": ["expires_in"], "target": {"type": "expires_in_seconds"},
                         "required": false},
                        {"path": ["interval"], "target": {"type": "interval_seconds"},
                         "required": false}
                    ]}
                },
                "expires_in_fallback_seconds": 900
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": format!("{base}/token")}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal",
                             "value": "urn:ietf:params:oauth:grant-type:device_code"}
                        ]},
                        {"name": "device_code", "value": [{"type": "device_code"}]},
                        {"name": "client_id", "value": [{"type": "client_id"}]},
                        {"name": "client_secret", "value": [{"type": "client_secret"}]}
                    ]}
                },
                "response": exchange_response(),
                "terminal_codes": ["expired_token", "access_denied"]
            },
            "refresh": {"type": "none"}
        }
    })
}

/// The reprovisioning recipe: an authorization-code flow whose renewal re-runs one
/// provisioning step extracting a fresh token and absolute expiry from the mock upstream.
pub(crate) fn reprovision_payload(base: &str) -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/v1/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "oauth",
            "client_id": "client-1",
            "client_secret": "secret-1",
            "grant": {
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://up.test/authorize"}],
                "redirect_uri": "http://localhost:1455/callback",
                "pkce": "none"
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": format!("{base}/token")}],
                    "body": {"kind": "form", "fields": exchange_fields()}
                },
                "response": {"encoding": "json", "fields": [
                    {"path": ["access_token"], "target": {"type": "access_token"},
                     "required": true},
                    {"path": ["refresh_token"], "target": {"type": "refresh_token"},
                     "required": true},
                    {"path": ["project"], "target": {"type": "slot", "name": "project"},
                     "required": false}
                ]},
                "terminal_codes": ["invalid_grant"]
            },
            "refresh": {"type": "reprovision"}
        },
        "provisioning": [{
            "request": {
                "method": "GET",
                "url": [{"type": "literal", "value": format!("{base}/session")}],
                "headers": [{"name": "Authorization", "value": [{"type": "secret"}]}]
            },
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true},
                {"path": ["expiry"], "target": {"type": "expires_at_unix_seconds"},
                 "shape": {"type": "integer"}, "required": true}
            ]
        }]
    })
}

/// The provisioning-only token source recipe: the exchange maps no access token, one
/// unconditional step supplies it together with the absolute expiry.
pub(crate) fn provisioned_token_payload(base: &str) -> Value {
    let mut payload = reprovision_payload(base);

    *payload
        .pointer_mut("/credential/refresh")
        .expect("the fixture carries the refresh section") = json!({"type": "none"});

    payload
}
