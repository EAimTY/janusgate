//! Write-time definition validation tests: the accepted and rejected payload surface
//! `DefinitionPayload::validate` enforces at the management boundary — expression staging,
//! credential and grant agreement, provisioning, discovery, quota declarations, and the
//! compaction section (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, resolve, or patch is that test's failure report"
)]

use super::DefinitionPayload;
use serde_json::{Value, json};

/// Parses one payload fixture; a fixture that does not parse is the test bug. Assertions on
/// parse failures name the serde rejection directly at the call site.
fn payload(fixture: Value) -> DefinitionPayload {
    serde_json::from_value(fixture).expect("fixture definition payload parses")
}

/// Parses and validates one payload fixture, expecting acceptance.
fn validates(fixture: Value) -> DefinitionPayload {
    let payload = payload(fixture);

    payload
        .validate()
        .expect("the fixture payload satisfies the write boundary");

    payload
}

/// Validates one payload fixture, expecting the rejection message.
fn rejects(fixture: Value) -> String {
    payload(fixture)
        .validate()
        .expect_err("the fixture payload violates the write boundary")
        .to_string()
}

/// Sets the member `pointer` addresses to `value`, inserting the member when the base fixture
/// omits it — serde reads an absent optional as its default, so a fixture patch both replaces
/// present members and installs absent ones. The pointer's parent object must exist.
fn patched(mut fixture: Value, pointer: &str, value: Value) -> Value {
    let (parent, key) = pointer
        .rsplit_once('/')
        .filter(|(_, key)| !key.is_empty())
        .expect("the fixture pointer addresses a member");

    let key = key.replace("~1", "/").replace("~0", "~");

    let object = fixture
        .pointer_mut(parent)
        .unwrap_or_else(|| panic!("the fixture pointer {parent:?} resolves"))
        .as_object_mut()
        .unwrap_or_else(|| panic!("the fixture pointer {parent:?} addresses an object"));

    object.insert(key, value);

    fixture
}

/// Nulls the value at `pointer` — the absent-optional spelling serde reads as `None`.
fn nulled(mut fixture: Value, pointer: &str) -> Value {
    let resolved = fixture
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("the fixture pointer {pointer:?} resolves"));

    resolved.take();

    fixture
}

/// The literal one-part expression of `text`.
fn literal(text: &str) -> Value {
    json!([{"type": "literal", "value": text}])
}

/// A static-API-key payload over the literal `base` URL.
fn api_key_payload() -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": literal("https://up.test/v1/chat/completions"),
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    })
}

/// The canonical valid authorization-code recipe: a configured client secret, no PKCE, a
/// form-encoded exchange carrying the code, redirect URI, and client identity, a guaranteed
/// refresh-token source, and no refresh grant.
fn code_payload() -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": literal("https://up.test/v1/chat/completions"),
            "upstream_streaming": "match_client",
            "headers": [
                {"name": "Authorization", "value": [{"type": "secret"}]}
            ]
        },
        "credential": {"type": "oauth",
            "client_id": "client-1",
            "client_secret": "secret-1",
            "grant": {
                "type": "authorization_code",
                "authorize_url": literal("https://up.test/authorize"),
                "redirect_uri": "http://localhost:1455/callback",
                "pkce": "none",
                "extra_params": [
                    {"name": "scope", "value": literal("read write")}
                ]
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": literal("https://up.test/token"),
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": literal("authorization_code")},
                        {"name": "code", "value": [{"type": "authorization_code"}]},
                        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
                        {"name": "client_id", "value": [{"type": "client_id"}]},
                        {"name": "client_secret", "value": [{"type": "client_secret"}]}
                    ]}
                },
                "response": {"encoding": "json", "fields": [
                    {"path": ["access_token"], "target": {"type": "access_token"},
                     "required": true},
                    {"path": ["refresh_token"], "target": {"type": "refresh_token"},
                     "required": true},
                    {"path": ["expires_in"], "target": {"type": "expires_in_seconds"},
                     "required": false}
                ]},
                "terminal_codes": ["invalid_grant", "expired_token"]
            },
            "refresh": {"type": "none"}
        }
    })
}

/// The code recipe whose credential renews through a refresh grant, with the exchange request
/// extended by the refresh-token part the grant rule demands indirectly (the grant request
/// carries it) — the base recipe already maps the refresh token required.
fn refresh_grant_payload() -> Value {
    patched(
        code_payload(),
        "/credential/refresh",
        json!({
            "type": "grant",
            "request": {
                "method": "POST",
                "url": literal("https://up.test/token"),
                "body": {"kind": "form", "fields": [
                    {"name": "grant_type", "value": literal("refresh_token")},
                    {"name": "refresh_token", "value": [{"type": "refresh_token"}]},
                    {"name": "client_id", "value": [{"type": "client_id"}]},
                    {"name": "client_secret", "value": [{"type": "client_secret"}]}
                ]}
            },
            "response": {"encoding": "json", "fields": [
                {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
                {"path": ["expires_in"], "target": {"type": "expires_in_seconds"},
                 "required": false}
            ]},
            "terminal_codes": ["invalid_grant"]
        }),
    )
}

/// The canonical valid device-grant recipe: RFC 8628 §3 response mapping with both required
/// codes, one verification URI, a required lifetime, and a device-code-carrying poll.
fn device_payload() -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": literal("https://up.test/v1/chat/completions"),
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
                        "url": literal("https://up.test/device"),
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
                        {"path": ["expires_in"], "target": {"type": "expires_in_seconds"},
                         "required": true},
                        {"path": ["interval"], "target": {"type": "interval_seconds"},
                         "required": false}
                    ]}
                },
                "expires_in_fallback_seconds": 900
            },
            "exchange": {
                "request": {
                    "method": "POST",
                    "url": literal("https://up.test/token"),
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type",
                         "value": literal("urn:ietf:params:oauth:grant-type:device_code")},
                        {"name": "device_code", "value": [{"type": "device_code"}]},
                        {"name": "client_id", "value": [{"type": "client_id"}]},
                        {"name": "client_secret", "value": [{"type": "client_secret"}]}
                    ]}
                },
                "response": {"encoding": "json", "fields": [
                    {"path": ["access_token"], "target": {"type": "access_token"},
                     "required": true},
                    {"path": ["refresh_token"], "target": {"type": "refresh_token"},
                     "required": true}
                ]},
                "terminal_codes": ["expired_token", "access_denied"]
            },
            "refresh": {"type": "none"}
        }
    })
}

/// A static API-key recipe and the canonical OAuth recipes all validate.
#[test]
fn canonical_recipes_validate() {
    validates(api_key_payload());

    validates(code_payload());

    validates(refresh_grant_payload());

    validates(device_payload());
}

/// An inference URL expression must carry at least one part.
#[test]
fn inference_rejects_an_empty_url_expression() {
    let message = rejects(patched(api_key_payload(), "/inference/url", json!([])));

    assert!(
        message.contains("inference URL must carry at least one expression part"),
        "got: {message}"
    );
}

/// A render option outside the carrying format's closed set is rejected, and the
/// `openai_response`-only option validates there.
#[test]
fn render_options_stay_within_the_format_set() {
    let foreign = patched(
        api_key_payload(),
        "/inference/render_options",
        json!(["omit_max_output_tokens"]),
    );

    let message = rejects(foreign);

    assert!(
        message.contains("is not part of the `openai_chat_completion` format's set"),
        "got: {message}"
    );

    let native = patched(
        patched(api_key_payload(), "/format", json!("openai_response")),
        "/inference/render_options",
        json!(["omit_max_output_tokens"]),
    );

    validates(native);
}

/// An inference envelope carries the codec body placeholder exactly once: zero and two
/// placeholders are rejected, one is accepted.
#[test]
fn inference_envelopes_carry_exactly_one_body_placeholder() {
    let base = api_key_payload();

    let zero = patched(
        base.clone(),
        "/inference/envelope",
        json!({"type": "object", "fields": {"model": {"type": "string", "expr": literal("m")}}}),
    );

    let message = rejects(zero);

    assert!(
        message.contains("carries 0 body placeholders"),
        "got: {message}"
    );

    let two = patched(
        base.clone(),
        "/inference/envelope",
        json!({"type": "array", "items": [
            {"type": "body"},
            {"type": "object", "fields": {"body": {"type": "body"}}}
        ]}),
    );

    let message = rejects(two);

    assert!(
        message.contains("carries 2 body placeholders"),
        "got: {message}"
    );

    let one = patched(
        base,
        "/inference/envelope",
        json!({"type": "object", "fields": {
            "model": {"type": "string", "expr": literal("m")},
            "request": {"type": "body"}
        }}),
    );

    validates(one);
}

/// The inference stage carries the secret, base, slots, model, and request UUID — and nothing
/// of the OAuth flow state: client identity and state tokens are rejected there.
#[test]
fn inference_expressions_stay_within_the_stage_vocabulary() {
    let base = api_key_payload();

    let foreign = patched(
        base.clone(),
        "/inference/url",
        json!([{"type": "client_id"}]),
    );

    let message = rejects(foreign);

    assert!(
        message.contains("client ID") && message.contains("inference"),
        "got: {message}"
    );

    let state_token = patched(
        base.clone(),
        "/inference/url",
        json!([{"type": "oauth_state"}]),
    );

    let message = rejects(state_token);

    assert!(
        message.contains("state token") && message.contains("inference"),
        "got: {message}"
    );

    let staged = patched(
        base,
        "/inference/url",
        json!([
            {"type": "base"},
            {"type": "literal", "value": "/v1/chat/completions?d="},
            {"type": "request_uuid"},
            {"type": "literal", "value": "&m="},
            {"type": "model"}
        ]),
    );

    validates(staged);
}

/// An empty response-unwrap member name is rejected: it would address nothing.
#[test]
fn inference_rejects_an_empty_response_unwrap_member() {
    let message = rejects(patched(
        api_key_payload(),
        "/inference/response_unwrap",
        json!(""),
    ));

    assert!(
        message.contains("response unwrap member must not be empty"),
        "got: {message}"
    );
}

/// `coalesce` carries direct nonempty arguments only: an empty coalesce and a nested coalesce
/// are both rejected, a flat one accepted.
#[test]
fn coalesce_carries_direct_nonempty_arguments_only() {
    let base = api_key_payload();

    let empty = patched(
        base.clone(),
        "/inference/url",
        json!([{"type": "coalesce", "parts": []}]),
    );

    let message = rejects(empty);

    assert!(message.contains("empty coalesce"), "got: {message}");

    let nested = patched(
        base.clone(),
        "/inference/url",
        json!([{"type": "coalesce", "parts": [
            {"type": "coalesce", "parts": [{"type": "literal", "value": "x"}]}
        ]}]),
    );

    let message = rejects(nested);

    assert!(message.contains("nests a coalesce"), "got: {message}");

    let flat = patched(
        base,
        "/inference/url",
        json!([{"type": "coalesce", "parts": [
            {"type": "slot", "name": "region"},
            {"type": "literal", "value": "default"}
        ]}]),
    );

    let message = rejects(flat);

    assert!(
        message.contains("references the slot `region`"),
        "an unconfigured slot still resolves against the namespace: {message}"
    );
}

/// Header rows carry the HTTP byte contracts: names must be valid header names, literal value
/// fragments must be header-value bytes — a pasted trailing newline is rejected — and empty
/// names never reach the wire.
#[test]
fn header_rows_carry_the_http_byte_contracts() {
    let base = api_key_payload();

    let control_byte = patched(
        base.clone(),
        "/inference/headers",
        json!([{"name": "Authorization", "value": [{"type": "literal", "value": "Bearer x\n"}]}]),
    );

    let message = rejects(control_byte);

    assert!(
        message.contains("invalid HTTP header value"),
        "got: {message}"
    );

    let bad_name = patched(
        base.clone(),
        "/inference/headers",
        json!([{"name": "bad name", "value": literal("v")}]),
    );

    let message = rejects(bad_name);

    assert!(
        message.contains("invalid HTTP header name"),
        "got: {message}"
    );

    let empty_name = patched(
        base,
        "/inference/headers",
        json!([{"name": "", "value": literal("v")}]),
    );

    let message = rejects(empty_name);

    assert!(message.contains("carries an empty name"), "got: {message}");
}

/// Slot references resolve against the recipe's namespace: an inference header referencing a
/// slot no initializer, exchange target, claim, or provisioning target defines is rejected,
/// and one an initial slot defines is accepted.
#[test]
fn slot_references_resolve_against_the_recipe_namespace() {
    let undefined = patched(
        api_key_payload(),
        "/inference/headers",
        json!([{"name": "X-Region", "value": [{"type": "slot", "name": "region"}]}]),
    );

    let message = rejects(undefined);

    assert!(
        message.contains("references the slot `region`"),
        "got: {message}"
    );

    let defined = json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": literal("https://up.test/v1/chat/completions"),
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"},
        "provisioning": [{
            "request": {"method": "GET", "url": literal("https://up.test/region")},
            "response_encoding": "json",
            "extract": [
                {"path": ["region"], "target": {"type": "slot", "name": "region"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]
    });

    // The provisioning sequence needs OAuth, so the api-key form is rejected for the
    // provisioning itself — the only violation, the extract target being the namespace's own
    // definition.
    let message = rejects(defined.clone());

    assert!(
        message.contains("requires an OAuth credential"),
        "got: {message}"
    );

    let oauth_defined = patched(
        patched(
            code_payload(),
            "/inference/headers",
            json!([
                {"name": "X-Region", "value": [{"type": "slot", "name": "region"}]}
            ]),
        ),
        "/credential/initial_slots",
        json!([{"name": "region", "generator": "uuid"}]),
    );

    validates(oauth_defined);
}

/// The OAuth client identity is validated once at the recipe: an empty client ID, an empty
/// configured secret, and header-unsafe identity bytes are all rejected.
#[test]
fn oauth_client_identity_is_validated() {
    let base = code_payload();

    let message = rejects(patched(base.clone(), "/credential/client_id", json!("")));

    assert!(
        message.contains("client ID must not be empty"),
        "got: {message}"
    );

    let message = rejects(patched(
        base.clone(),
        "/credential/client_secret",
        json!(""),
    ));

    assert!(
        message.contains("client secret must not be empty when present"),
        "got: {message}"
    );

    let message = rejects(patched(
        base.clone(),
        "/credential/client_id",
        json!("client\n"),
    ));

    assert!(
        message.contains("not a valid HTTP header value"),
        "got: {message}"
    );

    let message = rejects(patched(base, "/credential/client_secret", json!("s\n")));

    assert!(
        message.contains("not a valid HTTP header value"),
        "got: {message}"
    );
}

/// A credential without a client secret rejects every credential-stage expression still
/// referencing the `client_secret` part — the guaranteed-drift configuration.
#[test]
fn secretless_recipes_reject_stranded_client_secret_references() {
    let base = nulled(code_payload(), "/credential/client_secret");

    let message = rejects(base.clone());

    assert!(
        message.contains("references the `client_secret` value part"),
        "got: {message}"
    );

    // Removing the exchange's secret row leaves a valid secret-less (public-client) recipe.
    let exchange_fields = json!([
        {"name": "grant_type", "value": literal("authorization_code")},
        {"name": "code", "value": [{"type": "authorization_code"}]},
        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
        {"name": "client_id", "value": [{"type": "client_id"}]}
    ]);

    let public_client = patched(
        base,
        "/credential/exchange/request/body/fields",
        exchange_fields,
    );

    validates(public_client);
}

/// A secret-less credential never authenticates, so the exchange and the refresh grant must
/// carry the client ID themselves; the device authorization request carries the same duty.
#[test]
fn secretless_recipes_carry_the_client_id_themselves() {
    let exchange_without_id = json!([
        {"name": "grant_type", "value": literal("authorization_code")},
        {"name": "code", "value": [{"type": "authorization_code"}]},
        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}
    ]);

    let base = patched(
        nulled(code_payload(), "/credential/client_secret"),
        "/credential/exchange/request/body/fields",
        exchange_without_id,
    );

    let message = rejects(base.clone());

    assert!(
        message.contains("requires the token exchange request to reference the `client_id`"),
        "got: {message}"
    );

    // With the exchange carrying the client ID itself, the refresh grant carries the same duty.
    let exchange_with_id = json!([
        {"name": "grant_type", "value": literal("authorization_code")},
        {"name": "code", "value": [{"type": "authorization_code"}]},
        {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
        {"name": "client_id", "value": [{"type": "client_id"}]}
    ]);

    let grant_fields = json!([
        {"name": "grant_type", "value": literal("refresh_token")},
        {"name": "refresh_token", "value": [{"type": "refresh_token"}]}
    ]);

    let refresh_request = json!({
        "type": "grant",
        "request": {
            "method": "POST",
            "url": literal("https://up.test/token"),
            "body": {"kind": "form", "fields": grant_fields}
        },
        "response": {"encoding": "json", "fields": [
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true}
        ]},
        "terminal_codes": ["invalid_grant"]
    });

    let grant_without_id = patched(
        patched(
            nulled(code_payload(), "/credential/client_secret"),
            "/credential/exchange/request/body/fields",
            exchange_with_id,
        ),
        "/credential/refresh",
        refresh_request,
    );

    let message = rejects(grant_without_id);

    assert!(
        message.contains("requires the token refresh request to reference the `client_id`"),
        "got: {message}"
    );
}

/// The authorize URL stage: a nonempty URL and redirect URI, extra parameters that never
/// duplicate the builder-owned names, and a value vocabulary of client identity and state
/// alone.
#[test]
fn authorize_url_rules_are_enforced() {
    let base = code_payload();

    let message = rejects(patched(
        base.clone(),
        "/credential/grant/authorize_url",
        json!([]),
    ));

    assert!(
        message.contains("authorize URL must carry"),
        "got: {message}"
    );

    let message = rejects(patched(
        base.clone(),
        "/credential/grant/redirect_uri",
        json!(""),
    ));

    assert!(
        message.contains("redirect URI must not be empty"),
        "got: {message}"
    );

    for owned in [
        "response_type",
        "client_id",
        "redirect_uri",
        "code_challenge",
        "code_challenge_method",
        "state",
    ] {
        let fixture = patched(
            base.clone(),
            "/credential/grant/extra_params",
            json!([{"name": owned, "value": literal("x")}]),
        );

        let message = rejects(fixture);

        assert!(
            message.contains("duplicates a parameter the authorize-URL builder appends itself"),
            "builder-owned `{owned}` rejected, got: {message}"
        );
    }

    let base_part = patched(
        base.clone(),
        "/credential/grant/authorize_url",
        json!([{"type": "base"}]),
    );

    let message = rejects(base_part);

    assert!(
        message.contains("base URL") && message.contains("authorize URL"),
        "got: {message}"
    );

    let slot_part = patched(
        base.clone(),
        "/credential/grant/authorize_url",
        json!([{"type": "slot", "name": "team"}]),
    );

    let message = rejects(slot_part);

    assert!(
        message.contains("authorize URL stage does not carry"),
        "got: {message}"
    );

    let state_part = patched(
        base,
        "/credential/grant/authorize_url",
        json!([{"type": "oauth_state"}]),
    );

    validates(state_part);
}

/// PKCE agreement: an `s256` grant must present the verifier at the token endpoint, and a
/// `none` grant must never reference it.
#[test]
fn pkce_agreement_between_grant_and_exchange_is_enforced() {
    let s256 = patched(code_payload(), "/credential/grant/pkce", json!("s256"));

    let message = rejects(s256.clone());

    assert!(
        message.contains("requires the token exchange request to reference the `code_verifier`"),
        "got: {message}"
    );

    let with_verifier = patched(
        s256,
        "/credential/exchange/request/body/fields",
        json!([
            {"name": "grant_type", "value": literal("authorization_code")},
            {"name": "code", "value": [{"type": "authorization_code"}]},
            {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
            {"name": "client_id", "value": [{"type": "client_id"}]},
            {"name": "client_secret", "value": [{"type": "client_secret"}]},
            {"name": "code_verifier", "value": [{"type": "code_verifier"}]}
        ]),
    );

    validates(with_verifier);

    let none_with_verifier = patched(
        code_payload(),
        "/credential/exchange/request/body/fields",
        json!([
            {"name": "grant_type", "value": literal("authorization_code")},
            {"name": "code", "value": [{"type": "authorization_code"}]},
            {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
            {"name": "client_id", "value": [{"type": "client_id"}]},
            {"name": "client_secret", "value": [{"type": "client_secret"}]},
            {"name": "code_verifier", "value": [{"type": "code_verifier"}]}
        ]),
    );

    let message = rejects(none_with_verifier);

    assert!(
        message.contains("must not reference the `code_verifier` value part"),
        "got: {message}"
    );
}

/// The authorization-code exchange must reference the pasted code and the redirect URI — the
/// parts the flow burns on its first completion attempt.
#[test]
fn code_exchanges_require_the_code_and_redirect_uri_references() {
    let without_code = patched(
        code_payload(),
        "/credential/exchange/request/body/fields",
        json!([
            {"name": "grant_type", "value": literal("authorization_code")},
            {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]},
            {"name": "client_id", "value": [{"type": "client_id"}]},
            {"name": "client_secret", "value": [{"type": "client_secret"}]}
        ]),
    );

    let message = rejects(without_code);

    assert!(
        message.contains("reference the `authorization_code` value part"),
        "got: {message}"
    );

    let without_redirect = patched(
        code_payload(),
        "/credential/exchange/request/body/fields",
        json!([
            {"name": "grant_type", "value": literal("authorization_code")},
            {"name": "code", "value": [{"type": "authorization_code"}]},
            {"name": "client_id", "value": [{"type": "client_id"}]},
            {"name": "client_secret", "value": [{"type": "client_secret"}]}
        ]),
    );

    let message = rejects(without_redirect);

    assert!(
        message.contains("reference the `redirect_uri` value part"),
        "got: {message}"
    );
}

/// The device grant's response contract: both codes required, at least one verification URI,
/// and exactly one guaranteed lifetime source — a reported `expires_in` the response must
/// carry, or the definition's explicit fallback.
#[test]
fn device_authorization_response_contract_is_enforced() {
    let base = device_payload();

    let fields_pointer = "/credential/grant/authorization/response/fields";

    let fields_without = |target: &str| {
        let fields: Vec<Value> = serde_json::from_value(
            base.pointer(fields_pointer)
                .expect("the fixture carries the device response fields")
                .clone(),
        )
        .expect("the fields are an array");

        let filtered: Vec<Value> = fields
            .into_iter()
            .filter(|field| field.pointer("/target/type").and_then(Value::as_str) != Some(target))
            .collect();

        serde_json::to_value(filtered).expect("the field list renders")
    };

    for dropped in ["device_code", "user_code"] {
        let message = rejects(patched(
            base.clone(),
            fields_pointer,
            fields_without(dropped),
        ));

        assert!(
            message.contains("must map both the `device_code` and the `user_code`"),
            "dropping `{dropped}` rejected, got: {message}"
        );
    }

    let message = rejects(patched(
        base.clone(),
        fields_pointer,
        fields_without("verification_uri"),
    ));

    assert!(
        message.contains("must map at least one verification URI"),
        "got: {message}"
    );

    // An optional device_code is the dead-lifetime shape: the codes are the flow's identity.
    let optional_device_code = patched(
        base.clone(),
        "/credential/grant/authorization/response/fields/0/required",
        json!(false),
    );

    let message = rejects(optional_device_code);

    assert!(
        message.contains("must mark the `device_code` target required"),
        "got: {message}"
    );

    // No expiry mapping at all without the configured fallback is unrepresentable.
    let fallback_nulled = nulled(
        patched(
            base.clone(),
            fields_pointer,
            fields_without("expires_in_seconds"),
        ),
        "/credential/grant/expires_in_fallback_seconds",
    );

    let message = rejects(fallback_nulled);

    assert!(
        message.contains("must configure `expires_in_fallback_seconds`"),
        "got: {message}"
    );

    // The explicit fallback alone satisfies the lifetime guarantee.
    validates(patched(
        base.clone(),
        fields_pointer,
        fields_without("expires_in_seconds"),
    ));

    validates(base);
}

/// A foreign target in a stage mapping is stored drift: the device authorization response
/// never mints tokens, and the exchange never carries device targets.
#[test]
fn stage_foreign_targets_are_rejected() {
    let base = device_payload();

    let device_mints_token = patched(
        base.clone(),
        "/credential/grant/authorization/response/fields",
        json!([
            {"path": ["device_code"], "target": {"type": "device_code"}, "required": true},
            {"path": ["user_code"], "target": {"type": "user_code"}, "required": true},
            {"path": ["verification_uri"], "target": {"type": "verification_uri"},
             "required": true},
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["expires_in"], "target": {"type": "expires_in_seconds"}, "required": true}
        ]),
    );

    let message = rejects(device_mints_token);

    assert!(
        message.contains("target its stage does not produce"),
        "got: {message}"
    );

    let exchange_carries_device_target = patched(
        base,
        "/credential/exchange/response/fields",
        json!([
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true},
            {"path": ["device_code"], "target": {"type": "device_code"}, "required": false}
        ]),
    );

    let message = rejects(exchange_carries_device_target);

    assert!(
        message.contains("target its stage does not produce"),
        "got: {message}"
    );
}

/// Duplicate targets within one response mapping are dead configuration: the fold applies the
/// fields in order, so the earlier mapping can never be observed.
#[test]
fn duplicate_targets_are_rejected() {
    let message = rejects(patched(
        code_payload(),
        "/credential/exchange/response/fields",
        json!([
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["token"], "target": {"type": "access_token"}, "required": false},
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true}
        ]),
    ));

    assert!(
        message.contains("carries the `access_token` target twice"),
        "got: {message}"
    );
}

/// Form-encoded responses are flat: a mapping path into them carries exactly one member.
#[test]
fn form_response_paths_stay_flat() {
    let form_exchange = patched(
        code_payload(),
        "/credential/exchange/response",
        json!({"encoding": "form", "fields": [
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["nested", "refresh_token"], "target": {"type": "refresh_token"},
             "required": true}
        ]}),
    );

    let message = rejects(form_exchange);

    assert!(
        message.contains("flat form fields and carry exactly one member"),
        "got: {message}"
    );
}

/// An OAuth credential must receive an access token before the provider write: from the
/// exchange mapping or a provisioning extraction.
#[test]
fn access_tokens_have_a_guaranteed_source() {
    let no_token_mapping = patched(
        code_payload(),
        "/credential/exchange/response/fields",
        json!([
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true}
        ]),
    );

    let message = rejects(no_token_mapping.clone());

    assert!(
        message.contains("must receive an access token"),
        "got: {message}"
    );

    // A provisioning extraction satisfies the guarantee.
    let provisioned = patched(
        no_token_mapping.clone(),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]),
    );

    validates(provisioned);
}

/// The refresh grant demands a guaranteed refresh-token source: a required exchange mapping
/// or a required extraction in an unconditional provisioning step — never an optional mapping
/// nor a guard-skipped step.
#[test]
fn refresh_grants_require_a_guaranteed_refresh_token_source() {
    let refresh_section =
        |refresh_request: Value| patched(code_payload(), "/credential/refresh", refresh_request);

    let optional_source = patched(
        code_payload(),
        "/credential/exchange/response/fields",
        json!([
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": false}
        ]),
    );
    let grant = refresh_grant_payload()
        .pointer("/credential/refresh")
        .expect("the fixture carries the refresh section")
        .clone();

    // The canonical recipe's exchange mapping is required, so the grant itself validates.
    validates(refresh_section(grant.clone()));

    let message = rejects(patched(
        refresh_section(grant.clone()),
        "/credential/exchange/response/fields",
        json!([
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": false}
        ]),
    ));

    assert!(
        message.contains("requires the credential's refresh token to have a guaranteed source"),
        "got: {message}"
    );

    // A guard-skipped step cannot guarantee the token either.
    let conditional_source = patched(
        patched(
            optional_source,
            "/provisioning",
            json!([{
                "request": {"method": "GET", "url": literal("https://up.test/me")},
                "response_encoding": "json",
                "extract": [
                    {"path": ["token"], "target": {"type": "refresh_token"},
                     "shape": {"type": "string"}, "required": true}
                ],
                "skip_if": "onboarded"
            }]),
        ),
        "/credential/refresh",
        grant,
    );

    let message = rejects(conditional_source);

    assert!(
        message.contains("requires the credential's refresh token to have a guaranteed source"),
        "got: {message}"
    );
}

/// The refresh grant request must reference the stored refresh token — the part whose absence
/// would misdiagnose a healthy provider as a revoked grant.
#[test]
fn refresh_grant_requests_reference_the_refresh_token() {
    let base = refresh_grant_payload();

    let without_token = patched(
        base,
        "/credential/refresh/request/body/fields",
        json!([
            {"name": "grant_type", "value": literal("refresh_token")},
            {"name": "client_id", "value": [{"type": "client_id"}]},
            {"name": "client_secret", "value": [{"type": "client_secret"}]}
        ]),
    );

    let message = rejects(without_token);

    assert!(
        message.contains("reference the `refresh_token` value part"),
        "got: {message}"
    );
}

/// A declared refresh response mapping wholly replaces the exchange's: it must stay within
/// the exchange stage's targets and carry the access token.
#[test]
fn declared_refresh_responses_carry_the_access_token() {
    let base = refresh_grant_payload();

    let without_token = patched(
        base,
        "/credential/refresh/response/fields",
        json!([
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": false},
            {"path": ["expires_in"], "target": {"type": "expires_in_seconds"}, "required": false}
        ]),
    );

    let message = rejects(without_token);

    assert!(
        message.contains("must map the `access_token` target"),
        "got: {message}"
    );
}

/// Without a declared refresh response the exchange mapping is the renewal's fold input, so it
/// must map the access token itself — a provisioning-only token source never renews.
#[test]
fn default_refresh_responses_reuse_a_token_mapping_exchange() {
    let provisioned_token = patched(
        patched(
            code_payload(),
            "/credential/exchange/response/fields",
            json!([
                {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true}
            ]),
        ),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]),
    );
    let mut grant = refresh_grant_payload()
        .pointer("/credential/refresh")
        .expect("the fixture carries the refresh section")
        .clone();
    *grant
        .pointer_mut("/response")
        .expect("the grant fixture declares a response") = Value::Null;

    let message = rejects(patched(provisioned_token, "/credential/refresh", grant));

    assert!(
        message.contains("must map the `access_token` target"),
        "got: {message}"
    );
}

/// Terminal-code lists carry nonempty codes.
#[test]
fn terminal_codes_reject_empty_members() {
    let message = rejects(patched(
        code_payload(),
        "/credential/exchange/terminal_codes",
        json!(["invalid_grant", ""]),
    ));

    assert!(
        message.contains("terminal codes carry an empty code"),
        "got: {message}"
    );
}

/// The reprovision refresh mode needs a nonempty provisioning sequence producing a fresh
/// absolute expiry.
#[test]
fn reprovision_requires_provisioning_with_an_expiry_extraction() {
    let reprovision = patched(
        code_payload(),
        "/credential/refresh",
        json!({"type": "reprovision"}),
    );

    let message = rejects(reprovision.clone());

    assert!(
        message.contains("requires a non-empty provisioning list"),
        "got: {message}"
    );

    let without_expiry = patched(
        reprovision,
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]),
    );

    let message = rejects(without_expiry);

    assert!(
        message
            .contains("requires the provisioning sequence to extract an `expires_at_unix_seconds`"),
        "got: {message}"
    );

    let with_expiry = patched(
        patched(
            code_payload(),
            "/credential/refresh",
            json!({"type": "reprovision"}),
        ),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true},
                {"path": ["expiry"], "target": {"type": "expires_at_unix_seconds"},
                 "shape": {"type": "integer"}, "required": true}
            ]
        }]),
    );

    validates(with_expiry);
}

/// The provisioning stage's `secret` is the exchange's access token: a step referencing it
/// without a mapped exchange token is guaranteed drift.
#[test]
fn provisioning_secret_references_require_a_mapped_exchange_token() {
    let base = patched(
        code_payload(),
        "/credential/exchange/response/fields",
        json!([
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true}
        ]),
    );

    let fixture = patched(
        base,
        "/provisioning",
        json!([{
            "request": {
                "method": "GET",
                "url": literal("https://up.test/me"),
                "headers": [{"name": "Authorization", "value": [{"type": "secret"}]}]
            },
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]),
    );

    let message = rejects(fixture);

    assert!(
        message.contains("provisioning references the `secret` value part"),
        "got: {message}"
    );
}

/// Provisioning runs after a token exchange: it requires an OAuth credential and a nonempty
/// sequence, and its slot guards resolve against the namespace.
#[test]
fn provisioning_requires_oauth_and_defined_guards() {
    let on_api_key = patched(
        api_key_payload(),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "json",
            "extract": []
        }]),
    );

    let message = rejects(on_api_key);

    assert!(
        message.contains("requires an OAuth credential"),
        "got: {message}"
    );

    let empty = patched(code_payload(), "/provisioning", json!([]));

    let message = rejects(empty);

    assert!(
        message.contains("must carry at least one step when present"),
        "got: {message}"
    );

    let undefined_guard = patched(
        code_payload(),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "json",
            "extract": [],
            "skip_if": "onboarded"
        }]),
    );

    let message = rejects(undefined_guard);

    assert!(
        message.contains("references the slot `onboarded`"),
        "got: {message}"
    );
}

/// The provisioning extraction contracts: stage-owned targets, unique targets, declared shape
/// members, and the poll's nonempty done path.
#[test]
fn provisioning_extraction_contracts_are_enforced() {
    let step = |extract: Value| {
        patched(
            code_payload(),
            "/provisioning",
            json!([{
                "request": {"method": "GET", "url": literal("https://up.test/me")},
                "response_encoding": "json",
                "extract": extract
            }]),
        )
    };

    let message = rejects(step(json!([
        {"path": ["a"], "target": {"type": "slot", "name": "x"},
         "shape": {"type": "string"}, "required": true},
        {"path": ["b"], "target": {"type": "slot", "name": "x"},
         "shape": {"type": "string"}, "required": false}
    ])));

    assert!(
        message.contains("carries the `slot:x` target twice"),
        "got: {message}"
    );

    let message = rejects(step(json!([
        {"path": ["a"], "target": {"type": "expires_in_seconds"},
         "shape": {"type": "string"}, "required": true}
    ])));

    assert!(
        message.contains("target its stage does not produce"),
        "got: {message}"
    );

    let message = rejects(step(json!([
        {"path": [""], "target": {"type": "slot", "name": "x"},
         "shape": {"type": "string"}, "required": true}
    ])));

    assert!(
        message.contains("carries an empty member"),
        "got: {message}"
    );

    let message = rejects(step(json!([
        {"path": ["a"], "target": {"type": "slot", "name": "x"},
         "shape": {"type": "string_or_field", "field": ""}, "required": true}
    ])));

    assert!(message.contains("names an empty member"), "got: {message}");

    let with_poll = |done_path: Value| {
        patched(
            code_payload(),
            "/provisioning",
            json!([{
                "request": {"method": "GET", "url": literal("https://up.test/operations/1")},
                "response_encoding": "json",
                "extract": [],
                "poll": {
                    "request": {"method": "GET", "url": literal("https://up.test/operations/1")},
                    "response_encoding": "json",
                    "done_path": done_path,
                    "error_path": [],
                    "interval_ms": 10,
                    "budget_ms": 1000
                }
            }]),
        )
    };

    let message = rejects(with_poll(json!([])));

    assert!(
        message.contains("done path must not be empty"),
        "got: {message}"
    );

    validates(with_poll(json!(["done"])));
}

/// The device exchange re-posts the authorization's token request as its poll, so it must
/// reference the device code.
#[test]
fn device_exchanges_reference_the_device_code() {
    let without_device_code = patched(
        device_payload(),
        "/credential/exchange/request/body/fields",
        json!([
            {"name": "grant_type",
             "value": literal("urn:ietf:params:oauth:grant-type:device_code")},
            {"name": "client_id", "value": [{"type": "client_id"}]},
            {"name": "client_secret", "value": [{"type": "client_secret"}]}
        ]),
    );

    let message = rejects(without_device_code);

    assert!(
        message.contains("reference the `device_code` value part"),
        "got: {message}"
    );
}

/// A secret-less device grant must carry its client ID on the device authorization request.
#[test]
fn secretless_device_grants_carry_the_client_id() {
    // The secret-less credential cannot reference the secret anywhere: the exchange row drops
    // with it, so the device-authorization client ID duty is the only violation.
    let base = patched(
        nulled(device_payload(), "/credential/client_secret"),
        "/credential/exchange/request/body/fields",
        json!([
            {"name": "grant_type",
             "value": literal("urn:ietf:params:oauth:grant-type:device_code")},
            {"name": "device_code", "value": [{"type": "device_code"}]},
            {"name": "client_id", "value": [{"type": "client_id"}]}
        ]),
    );

    let without_id = patched(
        base,
        "/credential/grant/authorization/request/body/fields",
        json!([{"name": "scope", "value": literal("read")}]),
    );

    let message = rejects(without_id);

    assert!(
        message.contains("requires the device authorization request to reference the `client_id`"),
        "got: {message}"
    );
}

/// The discovery descriptor shapes: nonempty paths, identifier fields, prefixes, and the
/// pagination parameter and continuation paths.
#[test]
fn discovery_descriptor_shapes_are_validated() {
    let discovery = |extract: Value, pagination: Value| {
        patched(
            api_key_payload(),
            "/discovery",
            json!({
                "request": {"method": "GET", "url": literal("https://up.test/v1/models")},
                "extract": extract,
                "pagination": pagination
            }),
        )
    };

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data", ""], "id_field": ["id"]}),
        json!({"type": "none"}),
    ));

    assert!(message.contains("empty member"), "got: {message}");

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": []}),
        json!({"type": "none"}),
    ));

    assert!(
        message.contains("identifier field path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": ["id"], "strip_prefix": ""}),
        json!({"type": "none"}),
    ));

    assert!(
        message.contains("prefix must not be empty when present"),
        "got: {message}"
    );

    let message = rejects(discovery(
        json!({"mode": "map_keys", "at": ["data", ""]}),
        json!({"type": "none"}),
    ));

    assert!(message.contains("empty member"), "got: {message}");

    // The empty chain addresses the response root, a legitimate object to read keys from.
    validates(discovery(
        json!({"mode": "map_keys", "at": []}),
        json!({"type": "none"}),
    ));

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": ["id"]}),
        json!({"type": "token", "request_param": "", "response_path": ["next"], "max_pages": 3}),
    ));

    assert!(
        message.contains("pagination request parameter must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": ["id"]}),
        json!({"type": "token", "request_param": "page", "response_path": [], "max_pages": 3}),
    ));

    assert!(
        message.contains("pagination response path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": ["id"]}),
        json!({
            "type": "cursor",
            "has_more_path": [],
            "last_id_path": ["last"],
            "request_param": "after",
            "max_pages": 3
        }),
    ));

    assert!(
        message.contains("has-more path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": ["id"]}),
        json!({
            "type": "cursor",
            "has_more_path": ["has_more"],
            "last_id_path": [],
            "request_param": "after",
            "max_pages": 3
        }),
    ));

    assert!(
        message.contains("last-identifier path must not be empty"),
        "got: {message}"
    );

    validates(discovery(
        json!({"mode": "array", "array_at": ["data"], "id_field": ["id"]}),
        json!({
            "type": "cursor",
            "has_more_path": ["has_more"],
            "last_id_path": ["last_id"],
            "request_param": "after",
            "max_pages": 3
        }),
    ));
}

/// The capability extraction paths name members, never the addressed entry itself.
#[test]
fn capability_extraction_paths_name_members() {
    let discovery = |capabilities: Value| {
        patched(
            api_key_payload(),
            "/discovery",
            json!({
                "request": {"method": "GET", "url": literal("https://up.test/v1/models")},
                "extract": {
                    "mode": "array",
                    "array_at": ["data"],
                    "id_field": ["id"],
                    "capabilities": capabilities
                }
            }),
        )
    };

    let message = rejects(discovery(json!({
        "thinking_levels": {"shape": "member_array", "array_at": ["levels"], "level_field": []}
    })));

    assert!(
        message.contains("member path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(json!({
        "thinking_levels": {"shape": "flagged_keys", "object_at": ["effort"], "flag_field": []}
    })));

    assert!(
        message.contains("flag path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(json!({
        "max_input_tokens": []
    })));

    assert!(
        message.contains("max-input-tokens member path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(json!({
        "max_output_tokens": [""]
    })));

    assert!(message.contains("empty member"), "got: {message}");

    let message = rejects(discovery(json!({
        "compaction": {
            "messages_on_demand": {"object_at": [], "flag_field": ["supported"]}
        }
    })));

    assert!(
        message.contains("compaction object path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(json!({
        "compaction": {
            "messages_threshold": {"object_at": ["cm"], "flag_field": []}
        }
    })));

    assert!(
        message.contains("compaction flag path must not be empty"),
        "got: {message}"
    );

    let message = rejects(discovery(json!({
        "compaction": {
            "messages_threshold": {"object_at": ["cm"], "flag_field": [""]}
        }
    })));

    assert!(
        message.contains("compaction flag path carries an empty member"),
        "got: {message}"
    );
}

/// Quota source contracts: unique stable keys, nonempty configuration, entry-key uniqueness
/// per source, and the map-key context confined to map sources.
#[test]
fn quota_source_contracts_are_enforced() {
    let quota = |sources: Value| {
        patched(
            api_key_payload(),
            "/quota",
            json!({"request": {
            "method": "GET",
            "url": literal("https://up.test/quota")
        }, "sources": sources}),
        )
    };

    let entry = |key: &str| {
        json!({
            "key": key,
            "label": "Credits",
            "at": ["credits"],
            "is_required": true,
            "window": {"type": "unknown"},
            "measurement": {"type": "unknown"},
            "reset": {"type": "unknown"}
        })
    };

    let message = rejects(quota(json!([
        {"type": "object", "key": "plan", "at": ["plan"], "is_required": true,
         "entries": [entry("credits")]},
        {"type": "object", "key": "plan", "at": ["other"], "is_required": true,
         "entries": [entry("credits")]}
    ])));

    assert!(
        message.contains("source keys must be unique"),
        "got: {message}"
    );

    let message = rejects(quota(json!([
        {"type": "object", "key": "plan", "at": ["plan"], "is_required": true,
         "entries": [entry("credits"), entry("credits")]}
    ])));

    assert!(
        message.contains("entry keys must be unique within a source"),
        "got: {message}"
    );

    let message = rejects(quota(json!([
        {"type": "object", "key": "plan", "at": ["plan"], "is_required": true,
         "scope": {"type": "map_key"},
         "entries": [entry("credits")]}
    ])));

    assert!(
        message.contains("map_key requires a map source"),
        "got: {message}"
    );

    let message = rejects(quota(json!([
        {"type": "array", "key": "plans", "at": ["plans"], "is_required": true,
         "identity": [{"type": "literal", "value": "seg"}],
         "entries": [entry("credits")]}
    ])));

    assert!(
        message.contains("array identity requires an item field"),
        "got: {message}"
    );

    let message = rejects(quota(json!([
        {"type": "array", "key": "plans", "at": ["plans"], "is_required": true,
         "identity": [{"type": "field",
                       "field": {"base": "item", "path": ["id"], "is_required": false},
                       "encoding": "string"}],
         "entries": [entry("credits")]}
    ])));

    assert!(
        message.contains("array identity fields must be required"),
        "got: {message}"
    );

    let message = rejects(quota(json!([
        {"type": "array", "key": "plans", "at": ["plans"], "is_required": true,
         "identity": [{"type": "map_key"}],
         "entries": [entry("credits")]}
    ])));

    assert!(
        message.contains("array identity cannot use map_key"),
        "got: {message}"
    );

    let message = rejects(quota(json!([
        {"type": "array", "key": "plans", "at": ["plans"], "is_required": true,
         "identity": [{"type": "field",
                       "field": {"base": "entry", "path": ["id"], "is_required": true},
                       "encoding": "string"}],
         "entries": [entry("credits")]}
    ])));

    assert!(message.contains("cannot reference entry"), "got: {message}");

    let message = rejects(quota(json!([
        {"type": "object", "key": "", "at": ["plan"], "is_required": true,
         "entries": [entry("credits")]}
    ])));

    assert!(message.contains("must not be empty"), "got: {message}");

    validates(quota(json!([
        {"type": "object", "key": "plan", "at": ["plan"], "is_required": true,
         "entries": [entry("credits")]}
    ])));
}

/// Quota entry fact contracts: routing selectors resolve to exact tokens and required fields,
/// entry-context paths are available to the entry's own facts but never to source-scope
/// selections, and configured text (units, labels, paths) is nonempty.
#[test]
fn quota_entry_fact_contracts_are_enforced() {
    let entry_base = json!({
        "key": "credits",
        "label": "Credits",
        "at": ["credits"],
        "is_required": true,
        "window": {"type": "unknown"},
        "measurement": {"type": "unknown"},
        "reset": {"type": "unknown"}
    });

    let quota_with = |entry_patch: Value| {
        let mut entry = entry_base.clone();
        entry
            .as_object_mut()
            .expect("the entry fixture is an object")
            .extend(
                entry_patch
                    .as_object()
                    .expect("the patch is an object")
                    .clone(),
            );

        patched(
            api_key_payload(),
            "/quota",
            json!({"request": {
                "method": "GET",
                "url": literal("https://up.test/quota")
            }, "sources": [
                {"type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                 "entries": [entry]}
            ]}),
        )
    };

    let message = rejects(quota_with(json!({
        "routing": {"type": "models", "upstream_ids": [
            {"type": "literal", "value": "not a routing token"}
        ]}
    })));

    assert!(
        message.contains("must be exact routing tokens"),
        "got: {message}"
    );

    let message = rejects(quota_with(json!({
        "routing": {"type": "models", "upstream_ids": [
            {"type": "field",
             "field": {"base": "item", "path": ["id"], "is_required": false},
             "encoding": "string"}
        ]}
    })));

    assert!(
        message.contains("routing model fields must be required"),
        "got: {message}"
    );

    // The entry context is available to the entry's own measurement and unlimited facts.
    validates(quota_with(json!({
        "measurement": {"type": "ratio",
                        "value": {"field": {"base": "entry", "path": ["ratio"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "scale": "fraction", "direction": "used"},
        "is_unlimited": {"base": "entry", "path": ["unlimited"], "is_required": true},
        "window": {"type": "duration_field",
                   "value": {"field": {"base": "entry", "path": ["period"],
                                       "is_required": true},
                             "encoding": "number"},
                   "unit": "hours"}
    })));

    // But never to a source-scope selection: the entry object does not exist yet there.
    let scoped = |scope: Value| {
        patched(
            api_key_payload(),
            "/quota",
            json!({"request": {
                "method": "GET",
                "url": literal("https://up.test/quota")
            }, "sources": [
                {"type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                 "scope": scope,
                 "entries": [entry_base.clone()]}
            ]}),
        )
    };

    let message = rejects(scoped(json!({"type": "field",
        "field": {"base": "entry", "path": ["team"], "is_required": true},
        "encoding": "string"
    })));

    assert!(message.contains("cannot reference entry"), "got: {message}");

    // Configured text is nonempty: the counter unit, the label, and every path member.
    let counter_unit = json!({
        "measurement": {"type": "counter",
                        "value": {"field": {"base": "entry", "path": ["used"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "limit": {"field": {"base": "entry", "path": ["limit"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "direction": "used", "unit": ""}
    });

    let message = rejects(quota_with(counter_unit));

    assert!(message.contains("must not be empty"), "got: {message}");

    let message = rejects(quota_with(json!({"label": ""})));

    assert!(message.contains("must not be empty"), "got: {message}");

    let message = rejects(quota_with(json!({
        "reset": {"type": "field",
                  "field": {"base": "entry", "path": [""], "is_required": true},
                  "encoding": "unix_seconds"}
    })));

    assert!(message.contains("must not be empty"), "got: {message}");

    let message = rejects(quota_with(json!({
        "is_unlimited": {"base": "entry", "path": [""], "is_required": true}
    })));

    assert!(message.contains("must not be empty"), "got: {message}");
}

/// The quota exhaustion classifier: unique nonempty rule names, nonempty status sets,
/// nonempty code paths, and nonempty restriction segments. A success status is unrepresentable
/// at parse.
#[test]
fn quota_exhaustion_classifiers_are_validated() {
    let rule = |patch: Value| {
        let mut base = json!({
            "name": "plan",
            "statuses": [429],
            "code": {"at": ["error"], "value": "quota_exceeded"},
            "restriction_key": [{"type": "literal", "value": "plan"}],
            "applies_to": {"type": "all_models"}
        });
        base.as_object_mut()
            .expect("the rule fixture is an object")
            .extend(patch.as_object().expect("the patch is an object").clone());

        patched(
            api_key_payload(),
            "/inference/quota_exhaustion",
            json!({"rules": [base]}),
        )
    };

    let message = rejects(rule(json!({"name": ""})));

    assert!(
        message.contains("rule names must be nonempty and unique"),
        "got: {message}"
    );

    let message = rejects(rule(json!({"statuses": []})));

    assert!(
        message.contains("statuses must not be empty"),
        "got: {message}"
    );

    let message = rejects(rule(json!({"code": {"at": [], "value": "x"}})));

    assert!(
        message.contains("requires a nonempty object-member path"),
        "got: {message}"
    );

    let message = rejects(rule(
        json!({"restriction_key": [{"type": "literal", "value": ""}]}),
    ));

    assert!(
        message.contains("require nonempty segments"),
        "got: {message}"
    );

    let success_status = rule(json!({"statuses": [200]}));

    assert!(
        serde_json::from_value::<DefinitionPayload>(success_status).is_err(),
        "a success status is unrepresentable at parse"
    );

    validates(rule(json!({})));
}

/// Compaction declarations are format-gated: Responses mechanisms speak `openai_response`,
/// Messages mechanisms `anthropic_messages`, and the token-count operation is
/// Messages-only.
#[test]
fn compaction_declarations_are_format_gated() {
    let chat = api_key_payload();

    let message = rejects(patched(
        chat.clone(),
        "/compaction",
        json!({"mechanisms": {"messages_on_demand_compaction": {"default": "unknown"}}}),
    ));

    assert!(
        message.contains("requires the `anthropic_messages` format"),
        "got: {message}"
    );

    let message = rejects(patched(
        chat.clone(),
        "/compaction",
        json!({"count_tokens": {"url": [{"type": "literal", "value": "https://up.test/count"}]}}),
    ));

    assert!(
        message.contains("`count_tokens` compaction operation requires the `anthropic_messages`"),
        "got: {message}"
    );

    let responses = patched(chat, "/format", json!("openai_response"));

    let message = rejects(patched(
        responses,
        "/compaction",
        json!({"mechanisms": {"messages_threshold_compaction": {"default": "unknown"}}}),
    ));

    assert!(
        message.contains("requires the `anthropic_messages` format"),
        "got: {message}"
    );
}

/// A definition cannot advertise native support without configuring how to execute it: the
/// standalone declaration requires its operation and the overlay declarations their headers —
/// while unknown defaults carry no such requirement, and present operation data validates even
/// when the default is unknown.
#[test]
fn supported_declarations_require_their_operation_data() {
    let responses = patched(api_key_payload(), "/format", json!("openai_response"));

    let standalone = |declaration: Value| {
        patched(
            responses.clone(),
            "/compaction",
            json!({"mechanisms": {"responses_standalone_compact": declaration}}),
        )
    };

    let message = rejects(standalone(json!({"default": "supported"})));

    assert!(
        message.contains("declares `supported` without an operation"),
        "got: {message}"
    );

    let operation = json!({
        "default": "supported",
        "operation": {
            "url": literal("https://up.test/v1/responses/compact"),
            "envelope": {"type": "body"}
        }
    });

    validates(standalone(operation.clone()));

    let unknown_with_bad_operation = patched(
        standalone(operation),
        "/compaction/mechanisms/responses_standalone_compact/operation/url",
        json!([]),
    );

    let message = rejects(unknown_with_bad_operation);

    assert!(
        message.contains("URL must carry at least one expression part"),
        "operation data validates even when the default is unknown: {message}"
    );

    let messages = patched(api_key_payload(), "/format", json!("anthropic_messages"));

    let overlay = |declaration: Value| {
        patched(
            messages.clone(),
            "/compaction",
            json!({"mechanisms": {"messages_on_demand_compaction": declaration}}),
        )
    };

    let message = rejects(overlay(json!({"default": "supported"})));

    assert!(
        message.contains("declares `supported` without overlay headers"),
        "got: {message}"
    );

    let message = rejects(overlay(json!({
        "default": "supported",
        "overlay_headers": []
    })));

    assert!(
        message.contains("declares `supported` without overlay headers"),
        "got: {message}"
    );

    validates(overlay(json!({
        "default": "supported",
        "overlay_headers": [{"name": "anthropic-beta", "value": literal("compaction-2026")}]
    })));
}

/// Unsupported classifiers carry nonempty codes and parameters only — statuses arrive
/// pre-validated at parse.
#[test]
fn unsupported_classifiers_reject_empty_criteria() {
    let responses = patched(api_key_payload(), "/format", json!("openai_response"));

    let standalone = |unsupported: Value| {
        patched(
            responses.clone(),
            "/compaction",
            json!({"mechanisms": {"responses_standalone_compact": {
                "default": "unknown",
                "unsupported": unsupported
            }}}),
        )
    };

    let message = rejects(standalone(json!({"codes": [""], "parameters": []})));

    assert!(message.contains("empty code"), "got: {message}");

    let message = rejects(standalone(json!({"codes": [], "parameters": [""]})));

    assert!(message.contains("empty parameter"), "got: {message}");

    validates(standalone(
        json!({"codes": ["not_supported"], "parameters": ["tool"]}),
    ));
}

/// Control-request shapes: a GET never carries a body, and a control JSON body never carries
/// the inference body placeholder.
#[test]
fn control_request_shapes_are_enforced() {
    let quota_with = |request: Value| {
        patched(
            api_key_payload(),
            "/quota",
            json!({
                "request": request,
                "sources": [{"type": "object", "key": "plan", "at": ["plan"],
                             "is_required": true, "entries": [{
                    "key": "credits", "label": "Credits", "at": ["credits"],
                    "is_required": true, "window": {"type": "unknown"},
                    "measurement": {"type": "unknown"}, "reset": {"type": "unknown"}
                }]}]
            }),
        )
    };

    let get_with_body = quota_with(json!({
        "method": "GET",
        "url": literal("https://up.test/quota"),
        "body": {"kind": "form", "fields": [{"name": "a", "value": literal("b")}]}
    }));

    let message = rejects(get_with_body);

    assert!(
        message.contains("GET control request never carries a body"),
        "got: {message}"
    );

    let json_with_placeholder = quota_with(json!({
        "method": "POST",
        "url": literal("https://up.test/quota"),
        "body": {"kind": "json", "node": {"type": "body"}}
    }));

    let message = rejects(json_with_placeholder);

    assert!(
        message.contains("never carries the inference body placeholder"),
        "got: {message}"
    );

    let bodyless_post = quota_with(json!({
        "method": "POST",
        "url": literal("https://up.test/quota")
    }));

    validates(bodyless_post);
}

/// The base-URL requirement predicate walks every base-capable surface: the inference URLs and
/// headers, the control requests of every stage, and the compaction operations — while the
/// authorize URL, whose stage bars the base, never contributes.
#[test]
fn base_references_are_detected_across_surfaces() {
    let base_part = json!([{"type": "base"}]);

    let with_base = |pointer: &str| patched(code_payload(), pointer, base_part.clone());

    assert!(validates(with_base("/inference/url")).references_base());

    assert!(validates(with_base("/inference/headers/0/value")).references_base());

    let discovery_with_base = patched(
        code_payload(),
        "/discovery",
        json!({
            "request": {"method": "GET", "url": base_part.clone()},
            "extract": {"mode": "map_keys", "at": ["models"]}
        }),
    );

    assert!(validates(discovery_with_base).references_base());

    let provisioning_with_base = patched(
        code_payload(),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": base_part.clone()},
            "response_encoding": "json",
            "extract": [
                {"path": ["token"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]),
    );

    assert!(validates(provisioning_with_base).references_base());

    // The authorize URL bars the base at its stage, so the predicate never reports it — a
    // payload carrying one there is rejected outright instead.
    let authorize_with_base = with_base("/credential/grant/authorize_url");

    assert!(payload(authorize_with_base).validate().is_err());
}

/// The identity-token claims: an empty claim path and a repeated claim slot are dead
/// configuration the write boundary rejects, while a valid claim defines its slot for the
/// recipe's own references.
#[test]
fn identity_token_claims_follow_their_contract() {
    let with_claims =
        |claims: Value| patched(code_payload(), "/credential/id_token_claims", claims);

    let message = rejects(with_claims(json!([{"claim": [], "slot": "team"}])));

    assert!(
        message.contains("identity-token claim path must not be empty"),
        "got: {message}"
    );

    let message = rejects(with_claims(json!([
        {"claim": ["sub"], "slot": "team"},
        {"claim": ["org", "team"], "slot": "team"}
    ])));

    assert!(
        message.contains("store into the slot `team` twice"),
        "got: {message}"
    );

    // The resolved claim defines its slot: an inference header referencing it validates.
    let referenced = patched(
        with_claims(json!([{"claim": ["org", "team"], "slot": "team"}])),
        "/inference/headers",
        json!([{"name": "X-Team", "value": [{"type": "slot", "name": "team"}]}]),
    );

    validates(referenced);
}

/// Two initial slots of one name are dead configuration: only one initializer could ever run.
#[test]
fn initial_slots_define_a_unique_namespace() {
    let message = rejects(patched(
        code_payload(),
        "/credential/initial_slots",
        json!([
            {"name": "region", "generator": "uuid"},
            {"name": "region", "generator": "uuid_hex"}
        ]),
    ));

    assert!(
        message.contains("defines the slot `region` twice"),
        "got: {message}"
    );
}

/// The streaming URL obeys the inference surface's rules: a nonempty expression over the
/// inference stage's vocabulary.
#[test]
fn streaming_urls_follow_the_inference_rules() {
    let message = rejects(patched(
        api_key_payload(),
        "/inference/stream_url",
        json!([]),
    ));

    assert!(
        message.contains("inference streaming URL must carry at least one expression part"),
        "got: {message}"
    );

    let message = rejects(patched(
        api_key_payload(),
        "/inference/stream_url",
        json!([{"type": "client_id"}]),
    ));

    assert!(
        message.contains("inference streaming URL references `client ID`"),
        "got: {message}"
    );

    validates(patched(
        api_key_payload(),
        "/inference/stream_url",
        json!([{"type": "base"}, {"type": "literal", "value": "/v1/stream"}]),
    ));
}

/// A control request carries a nonempty URL expression: the quota query over an empty URL is
/// rejected.
#[test]
fn control_requests_carry_a_url_expression() {
    let message = rejects(patched(
        api_key_payload(),
        "/quota",
        json!({
            "request": {"method": "GET", "url": []},
            "sources": [{
                "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                "entries": [{
                    "key": "credits", "label": "Credits", "at": ["credits"],
                    "is_required": true, "window": {"type": "unknown"},
                    "measurement": {"type": "unknown"}, "reset": {"type": "unknown"}
                }]
            }]
        }),
    ));

    assert!(
        message.contains("control request must carry at least one expression part"),
        "got: {message}"
    );
}

/// Response paths never carry empty members — while the empty chain legitimately addresses the
/// response root — and a form-encoded provisioning extraction stays flat.
#[test]
fn response_paths_carry_members_and_form_extractions_stay_flat() {
    let message = rejects(patched(
        code_payload(),
        "/credential/exchange/response/fields/0/path",
        json!([""]),
    ));

    assert!(
        message.contains("token exchange path carries an empty member"),
        "got: {message}"
    );

    let message = rejects(patched(
        code_payload(),
        "/provisioning",
        json!([{
            "request": {"method": "GET", "url": literal("https://up.test/me")},
            "response_encoding": "form",
            "extract": [
                {"path": ["a", "b"], "target": {"type": "access_token"},
                 "shape": {"type": "string"}, "required": true}
            ]
        }]),
    ));

    assert!(
        message.contains("paths address flat form fields"),
        "got: {message}"
    );
}

/// The exchange may map the identity token: a target its stage produces is accepted, while the
/// id-token claim vocabulary still applies to it.
#[test]
fn exchanges_may_map_the_identity_token() {
    validates(patched(
        code_payload(),
        "/credential/exchange/response/fields",
        json!([
            {"path": ["access_token"], "target": {"type": "access_token"}, "required": true},
            {"path": ["refresh_token"], "target": {"type": "refresh_token"}, "required": true},
            {"path": ["id_token"], "target": {"type": "id_token"}, "required": false}
        ]),
    ));
}

/// Quota routing selectors confine their map-key context to map sources: a `map_key` model
/// segment on an object source is rejected.
#[test]
fn quota_routing_map_key_requires_a_map_source() {
    let message = rejects(patched(
        api_key_payload(),
        "/quota",
        json!({"request": {
            "method": "GET",
            "url": literal("https://up.test/quota")
        }, "sources": [{
            "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
            "entries": [{
                "key": "credits", "label": "Credits", "at": ["credits"],
                "is_required": true, "window": {"type": "unknown"},
                "measurement": {"type": "unknown"}, "reset": {"type": "unknown"},
                "routing": {"type": "models", "upstream_ids": [{"type": "map_key"}]}
            }]
        }]}),
    ));

    assert!(
        message.contains("quota routing map_key requires a map source"),
        "got: {message}"
    );
}
