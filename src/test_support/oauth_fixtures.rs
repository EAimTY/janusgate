//! Test support: the OAuth definition payload fixtures — the shared [`oauth_definition_payload`]
//! skeleton and its delta types, the provider-domain fixture knowledge that lets every OAuth
//! definition fixture in the crate restate only its deltas.
//!
//! Every helper panics when its fixed fixture fails to parse — a test bug by construction, so
//! the panic is the failure report.

#![expect(
    clippy::panic,
    reason = "a fixture that cannot parse is a test bug, and the panic is the failure report"
)]

use crate::{
    format::UpstreamFormat,
    provider::definition::{DefinitionPayload, template::ResponseEncoding},
};
use serde_json::{Value, json};

/// The unreachable fixture token endpoint the fixed OAuth definition fixtures exchange and
/// refresh at: a test that needs a live answer starts its own mock and passes its base URL.
pub(crate) const FIXTURE_TOKEN_URL: &str = "http://127.0.0.1:1/token";

/// The response-mapping target an OAuth definition fixture folds a mapped value into.
pub(crate) enum OAuthTarget {
    /// The stored access token.
    AccessToken,

    /// The stored refresh token.
    RefreshToken,

    /// The stored provider base URL.
    BaseUrl,

    /// The named working slot.
    Slot(&'static str),
}

/// One required response-mapping row of an OAuth definition fixture: the single response member
/// `path` folds into `target`.
pub(crate) struct OAuthResponseField {
    /// The response member the row reads.
    path: &'static str,

    /// The target the member's value folds into.
    target: OAuthTarget,
}

impl OAuthResponseField {
    /// Maps the response member `path` to `target`, required.
    pub(crate) const fn new(path: &'static str, target: OAuthTarget) -> Self {
        Self { path, target }
    }

    /// Renders the row's descriptor JSON.
    fn to_json(&self) -> Value {
        let target = match &self.target {
            OAuthTarget::AccessToken => json!({"type": "access_token"}),
            OAuthTarget::RefreshToken => json!({"type": "refresh_token"}),
            OAuthTarget::BaseUrl => json!({"type": "base_url"}),
            OAuthTarget::Slot(name) => json!({"type": "slot", "name": name}),
        };

        json!({
            "path": [self.path],
            "target": target,
            "required": true
        })
    }
}

/// The grant kind of an OAuth definition fixture, carrying the kind's fixture parameters:
/// the authorization-code grant's authorize URL, redirect URI, and absent PKCE, and the
/// device grant's fixed exchange form and expiry fallback alongside the authorization
/// step's deltas.
pub(crate) enum OAuthGrantFixture {
    /// The authorization-code grant, whose exchange posts the `grant_type`, `code`, and
    /// `redirect_uri` form rows.
    AuthorizationCode,

    /// The device grant, whose exchange posts the `device_code` form row plus the device
    /// `grant_type` literal row when `has_grant_type_field` holds, and whose authorization
    /// step varies by `authorization`.
    Device {
        /// Whether the exchange's form declares the device `grant_type` literal row.
        has_grant_type_field: bool,

        /// The authorization step's deltas over the skeleton's fixed request and row set.
        authorization: OAuthDeviceAuthorizationFixture,
    },
}

/// The device grant's authorization-step deltas: the skeleton's fixed step posts the request
/// carrying the `client_id` form body and maps the required `device_code` and `user_code` rows
/// plus the optional `verification_uri` row off a form-encoded response, and these members vary
/// the request's URL and body, the response's wire encoding, the `verification_uri` row's
/// requiredness, and the optional `expires_in` row.
pub(crate) struct OAuthDeviceAuthorizationFixture {
    /// The literal URL the authorization request posts to.
    pub(crate) request_url: &'static str,

    /// Whether the authorization request declares the `client_id` form body.
    pub(crate) has_client_id_body: bool,

    /// The wire encoding the authorization response arrives in.
    pub(crate) response_encoding: ResponseEncoding,

    /// Whether the `verification_uri` response row is required.
    pub(crate) is_verification_uri_required: bool,

    /// Whether the authorization response declares the optional `expires_in` row mapping the
    /// `expires_in_seconds` target.
    pub(crate) has_expires_in_field: bool,
}

impl Default for OAuthDeviceAuthorizationFixture {
    /// The skeleton's fixed authorization-step parameters.
    fn default() -> Self {
        Self {
            request_url: "http://127.0.0.1:1/device",
            has_client_id_body: true,
            response_encoding: ResponseEncoding::Form,
            is_verification_uri_required: false,
            has_expires_in_field: false,
        }
    }
}

/// The refresh mode of an OAuth definition fixture.
pub(crate) enum OAuthRefreshFixture<'a> {
    /// The tagged never-renews mode.
    None,

    /// The refresh grant: the fixed `grant_type`/`refresh_token` form posted to `url`, with the
    /// optional declared response mapping — when absent, the refresh response maps through the
    /// exchange's mapping.
    Grant {
        /// The refresh endpoint.
        url: &'a str,

        /// The declared response mapping rows, or none to inherit the exchange's.
        response: Option<Vec<OAuthResponseField>>,
    },

    /// The reprovision mode: the provisioning steps renew the credential.
    Reprovision,
}

/// The deltas an OAuth definition fixture varies around the shared
/// [`oauth_definition_payload`] skeleton.
pub(crate) struct OAuthDefinitionFixture<'a> {
    /// The upstream format the definition speaks.
    pub(crate) format: UpstreamFormat,

    /// The literal appended to the base URL in the inference URL, or none for the bare base.
    pub(crate) inference_suffix: Option<&'static str>,

    /// The OAuth client id.
    pub(crate) client_id: &'static str,

    /// The `(name, generator)` initial-slot pairs, the generator drawn from the closed
    /// `uuid`/`uuid_hex` set; empty declares none.
    pub(crate) initial_slots: &'static [(&'static str, &'static str)],

    /// The grant kind.
    pub(crate) grant: OAuthGrantFixture,

    /// The endpoint the exchange's control request posts to.
    pub(crate) exchange_url: &'a str,

    /// The exchange response's mapped targets besides the skeleton's always-required
    /// `access_token` row.
    pub(crate) response_fields: Vec<OAuthResponseField>,

    /// The terminal codes of the exchange and of a `Grant` refresh.
    pub(crate) terminal_codes: &'static [&'static str],

    /// The refresh mode.
    pub(crate) refresh: OAuthRefreshFixture<'a>,

    /// The provisioning steps array, verbatim, or none.
    pub(crate) provisioning: Option<Value>,
}

/// Builds one OAuth [`DefinitionPayload`] fixture from the shared skeleton — the given format
/// and inference URL, the grant's fixture parameters, a form exchange whose response maps
/// the `access_token` target required plus [`OAuthDefinitionFixture::response_fields`] under
/// [`OAuthDefinitionFixture::terminal_codes`], and the refresh mode — so every OAuth definition
/// fixture in the crate restates only its deltas.
///
/// The skeleton's exchange references the `redirect_uri` value part because the write boundary
/// requires that of every authorization-code grant (RFC 6749 §4.1.3); the same boundary
/// requires the device grant's exchange to reference the `device_code` value part (RFC 8628
/// §3.4), which the device rows carry. The skeleton credential never carries a client secret,
/// so both grants' exchanges and the `Grant` refresh's request carry the `client_id` row the
/// same boundary requires of a secret-less token or renewal request (RFC 6749 §4.1.3, §3.2.1,
/// §6).
///
/// The payload is returned unvalidated: some fixtures are deliberately invalid, so validation
/// stays the caller's decision.
///
/// # Panics
///
/// Panics if the assembled payload does not parse — the skeleton is fixed, so a parse failure is
/// a test bug, and the panic is the failure report.
pub(crate) fn oauth_definition_payload(fixture: OAuthDefinitionFixture<'_>) -> DefinitionPayload {
    let inference_url = match fixture.inference_suffix {
        Some(suffix) => json!([{"type": "base"}, {"type": "literal", "value": suffix}]),
        None => json!([{"type": "base"}]),
    };

    let (grant, exchange_fields) = match fixture.grant {
        OAuthGrantFixture::AuthorizationCode => (
            json!({
                "type": "authorization_code",
                "authorize_url": [{"type": "literal", "value": "https://accounts.example/auth"}],
                "redirect_uri": "http://localhost:1/callback",
                "pkce": "none"
            }),
            vec![
                json!({"name": "grant_type", "value": [
                    {"type": "literal", "value": "authorization_code"}
                ]}),
                json!({"name": "code", "value": [{"type": "authorization_code"}]}),
                json!({"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}),
                json!({"name": "client_id", "value": [{"type": "client_id"}]}),
            ],
        ),

        OAuthGrantFixture::Device {
            has_grant_type_field,
            authorization,
        } => {
            let response_encoding = match authorization.response_encoding {
                ResponseEncoding::Form => "form",
                ResponseEncoding::Json => "json",
            };

            let authorization_request = match authorization.has_client_id_body {
                true => json!({
                    "method": "POST",
                    "url": [{"type": "literal", "value": authorization.request_url}],
                    "body": {"kind": "form", "fields": [
                        {"name": "client_id", "value": [{"type": "client_id"}]}
                    ]}
                }),

                false => json!({
                    "method": "POST",
                    "url": [{"type": "literal", "value": authorization.request_url}]
                }),
            };

            let mut authorization_response_fields = vec![
                json!({
                    "path": ["device_code"],
                    "target": {"type": "device_code"},
                    "required": true
                }),
                json!({
                    "path": ["user_code"],
                    "target": {"type": "user_code"},
                    "required": true
                }),
                json!({
                    "path": ["verification_uri"],
                    "target": {"type": "verification_uri"},
                    "required": authorization.is_verification_uri_required
                }),
            ];

            if authorization.has_expires_in_field {
                authorization_response_fields.push(json!({
                    "path": ["expires_in"],
                    "target": {"type": "expires_in_seconds"},
                    "required": false
                }));
            }

            let mut exchange_fields = Vec::new();

            if has_grant_type_field {
                exchange_fields.push(json!({"name": "grant_type", "value": [
                    {"type": "literal", "value": "urn:ietf:params:oauth:grant-type:device_code"}
                ]}));
            }

            exchange_fields.push(json!({"name": "client_id", "value": [{"type": "client_id"}]}));

            exchange_fields
                .push(json!({"name": "device_code", "value": [{"type": "device_code"}]}));

            (
                json!({
                    "type": "device",
                    "authorization": {
                        "request": authorization_request,
                        "response": {
                            "encoding": response_encoding,
                            "fields": authorization_response_fields
                        }
                    },
                    "expires_in_fallback_seconds": 900
                }),
                exchange_fields,
            )
        }
    };

    let exchange = json!({
        "request": {
            "method": "POST",
            "url": [{"type": "literal", "value": fixture.exchange_url}],
            "body": {"kind": "form", "fields": exchange_fields}
        },
        "response": {
            "encoding": "json",
            "fields": std::iter::once(OAuthResponseField::new(
                "access_token",
                OAuthTarget::AccessToken,
            ))
            .chain(fixture.response_fields)
            .map(|field| field.to_json())
            .collect::<Vec<_>>()
        },
        "terminal_codes": fixture.terminal_codes
    });

    let refresh = match fixture.refresh {
        OAuthRefreshFixture::None => json!({"type": "none"}),

        OAuthRefreshFixture::Reprovision => json!({"type": "reprovision"}),

        OAuthRefreshFixture::Grant { url, response } => {
            let mut refresh = json!({
                "type": "grant",
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": url}],
                    "body": {"kind": "form", "fields": [
                        {"name": "grant_type", "value": [
                            {"type": "literal", "value": "refresh_token"}
                        ]},
                        {"name": "refresh_token", "value": [{"type": "refresh_token"}]},
                        {"name": "client_id", "value": [{"type": "client_id"}]}
                    ]}
                }
            });

            #[expect(
                clippy::expect_used,
                reason = "the refresh skeleton literal is always an object"
            )]
            let refresh_object = refresh
                .as_object_mut()
                .expect("the refresh skeleton is an object");

            if let Some(fields) = response {
                refresh_object.insert(
                    "response".to_string(),
                    json!({
                        "encoding": "json",
                        "fields": fields
                            .iter()
                            .map(|field| field.to_json())
                            .collect::<Vec<_>>()
                    }),
                );
            }

            refresh_object.insert("terminal_codes".to_string(), json!(fixture.terminal_codes));

            refresh
        }
    };

    let mut credential = json!({
        "type": "oauth",
        "client_id": fixture.client_id,
        "grant": grant,
        "exchange": exchange,
        "refresh": refresh
    });

    if !fixture.initial_slots.is_empty() {
        #[expect(
            clippy::expect_used,
            reason = "the credential skeleton literal is always an object"
        )]
        let credential_object = credential
            .as_object_mut()
            .expect("the credential skeleton is an object");

        credential_object.insert(
            "initial_slots".to_string(),
            json!(
                fixture
                    .initial_slots
                    .iter()
                    .map(|(name, generator)| json!({
                        "name": name,
                        "generator": generator
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }

    let mut payload = json!({
        "format": fixture.format.as_str(),
        "inference": {
            "url": inference_url,
            "upstream_streaming": "match_client"
        },
        "credential": credential
    });

    if let Some(steps) = fixture.provisioning {
        #[expect(
            clippy::expect_used,
            reason = "the payload skeleton literal is always an object"
        )]
        let payload_object = payload
            .as_object_mut()
            .expect("the payload skeleton is an object");

        payload_object.insert("provisioning".to_string(), steps);
    }

    serde_json::from_value(payload)
        .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"))
}
