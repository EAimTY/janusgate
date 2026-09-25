//! Provider definitions: the immutable, one-format connection recipes every provider instance and
//! OAuth flow references (README §3, §5).
//!
//! A definition decomposes into wire format (code, the five format codecs), connection
//! configuration (data, the descriptors in this module), and credentials (data, the provider
//! rows). The engines never special-case a provider; these descriptor vocabularies are the only
//! extension surface, and the management write boundary validates a payload once, at write time
//! ([`validate::DefinitionPayload::validate`]).

pub(crate) mod credential;
pub(crate) mod discovery;
pub(crate) mod eval;
pub(crate) mod inference;
pub(crate) mod provisioning;
pub(crate) mod template;
pub(crate) mod validate;

use crate::format::UpstreamFormat;
use serde::{Deserialize, Serialize};
use std::num::NonZeroI64;
pub(crate) use validate::ValidationError;

/// The payload of one provider definition version: the data `provider-definitions:create` and
/// `provider-definitions:update` carry, and what the `format`, `inference`, `credential`,
/// `provisioning`, and `discovery` columns of the `provider_definitions` table store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DefinitionPayload {
    /// The one upstream wire format every inference of this definition speaks.
    pub format: UpstreamFormat,

    /// The inference request descriptor.
    pub inference: inference::Inference,

    /// The credential-acquisition descriptor.
    pub credential: credential::CredentialAcquisition,

    /// The optional ordered provisioning sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisioning: Option<Vec<provisioning::ProvisioningStep>>,

    /// The optional upstream model-list discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<discovery::Discovery>,
}

impl DefinitionPayload {
    /// Whether any descriptor URL of this payload references the instance base URL — the
    /// [`template::ValueExpr::Base`] part (README §3). A definition so addressed evaluates every
    /// such URL against the bound provider row's stored base URL, so a provider bound to it must
    /// carry one.
    ///
    /// The walk covers every URL-bearing member whose evaluation environment carries the base
    /// URL — the inference and streaming URLs, the discovery and provisioning request URLs, and
    /// the credential section's device-authorization, token-exchange, and refresh-grant request
    /// URLs — and must be extended alongside any new member of that set: the requirement checks
    /// consult only this predicate, so a missed URL is accepted with no base URL and every later
    /// evaluation of it fails as stored drift. The authorize URL is excluded: the write-time
    /// validation bars the base reference at the authorize-URL stage (README §5.1), so that
    /// expression can never evaluate against a base URL.
    #[must_use]
    pub(crate) fn references_base(&self) -> bool {
        /// Whether one string expression carries the base reference, descending into a
        /// `Coalesce`'s arguments — the one compound part, which evaluation recurses into.
        fn references_base(expr: &template::StringExpr) -> bool {
            /// Whether one expression part is, or contains, the base reference.
            fn part_references_base(part: &template::ValueExpr) -> bool {
                match part {
                    template::ValueExpr::Base => true,

                    template::ValueExpr::Coalesce { parts } => {
                        parts.iter().any(part_references_base)
                    }

                    _ => false,
                }
            }

            expr.0.iter().any(part_references_base)
        }

        /// Whether one control request's URL carries the base reference.
        fn references_request_base(request: &template::ControlRequest) -> bool {
            references_base(&request.url)
        }

        let mut is_addressed = references_base(&self.inference.url)
            || self
                .inference
                .stream_url
                .as_ref()
                .is_some_and(references_base);

        // The credential section's OAuth recipe: every stage here evaluates against the flow's
        // working base URL or the stored provider base URL, except the authorize-URL build,
        // whose stage bars the base reference outright.
        if let credential::CredentialAcquisition::OAuth(recipe) = &self.credential {
            is_addressed = is_addressed
                || match &recipe.grant {
                    credential::Grant::AuthorizationCode { .. } => false,
                    credential::Grant::Device { authorization, .. } => {
                        references_request_base(&authorization.request)
                    }
                };

            is_addressed = is_addressed || references_request_base(&recipe.exchange.request);

            if let credential::TokenRefresh::Grant { request, .. } = &recipe.refresh {
                is_addressed = is_addressed || references_request_base(request);
            }
        }

        if let Some(steps) = &self.provisioning {
            is_addressed = is_addressed
                || steps.iter().any(|step| {
                    references_request_base(&step.request)
                        || step
                            .poll
                            .as_ref()
                            .is_some_and(|poll| references_request_base(&poll.request))
                });
        }

        if let Some(discovery) = &self.discovery {
            is_addressed = is_addressed || references_request_base(&discovery.request);
        }

        is_addressed
    }
}

/// One immutable provider definition version: the stored `(name, version)` row plus its parsed
/// payload (README §3).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderDefinition {
    /// The definition's name; versions accumulate under it.
    pub(crate) name: String,

    /// The definition's positive, monotonically increasing version.
    pub(crate) version: NonZeroI64,

    /// The definition's parsed payload.
    pub(crate) payload: DefinitionPayload,

    /// Creation timestamp in UTC microseconds.
    pub(crate) created_at: i64,
}

impl ProviderDefinition {
    /// The credential kind this definition's recipe describes, shared with the provider row's
    /// credential discriminator.
    #[must_use]
    pub(crate) fn credential_kind(&self) -> crate::provider::CredentialKind {
        match self.payload.credential {
            credential::CredentialAcquisition::ApiKey => crate::provider::CredentialKind::ApiKey,
            credential::CredentialAcquisition::OAuth { .. } => {
                crate::provider::CredentialKind::OAuth
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DefinitionPayload;

    /// Parses one test payload, panicking on a fixture that cannot parse.
    ///
    /// # Panics
    ///
    /// Panics when the fixture payload does not deserialize.
    #[expect(
        clippy::unwrap_used,
        reason = "the fixture payload is constructed to parse"
    )]
    fn parses(payload: serde_json::Value) -> DefinitionPayload {
        serde_json::from_value(payload).unwrap()
    }

    /// The parts of one literal string expression.
    fn literal(value: &str) -> serde_json::Value {
        serde_json::json!([{ "type": "literal", "value": value }])
    }

    /// The parts of one bare `Base` reference.
    fn base() -> serde_json::Value {
        serde_json::json!([{ "type": "base" }])
    }

    /// The parts of one `Coalesce` over a bare `Base` reference — the one compound part, which
    /// the predicate descends into.
    fn coalesced_base() -> serde_json::Value {
        serde_json::json!([{ "type": "coalesce", "parts": [{ "type": "base" }] }])
    }

    /// Builds one static-key payload wholesale with the given URL expressions: the inference
    /// URL, the optional streaming URL, the discovery and provisioning request URLs, and the
    /// optional provisioning poll URL.
    fn static_payload(
        inference_url: serde_json::Value,
        stream_url: Option<serde_json::Value>,
        discovery_url: serde_json::Value,
        step_url: serde_json::Value,
        poll_url: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let inference = match stream_url {
            Some(stream_url) => serde_json::json!({
                "url": inference_url,
                "stream_url": stream_url,
                "upstream_streaming": "match_client",
            }),

            None => serde_json::json!({
                "url": inference_url,
                "upstream_streaming": "match_client",
            }),
        };

        let step = match poll_url {
            Some(poll_url) => serde_json::json!({
                "request": {"method": "GET", "url": step_url},
                "response_encoding": "json",
                "poll": {
                    "request": {"method": "GET", "url": poll_url},
                    "response_encoding": "json",
                    "done_path": ["done"],
                    "error_path": ["error"],
                    "interval_ms": 1000,
                    "budget_ms": 60000,
                },
            }),

            None => serde_json::json!({
                "request": {"method": "GET", "url": step_url},
                "response_encoding": "json",
            }),
        };

        serde_json::json!({
            "format": "openai_chat_completion",
            "inference": inference,
            "credential": {"type": "api_key"},
            "provisioning": [step],
            "discovery": {
                "request": {"method": "GET", "url": discovery_url},
                "extract": {"mode": "array", "array_at": ["data"], "id_field": ["id"]},
            },
        })
    }

    /// Builds one form-body token-endpoint control request with the given URL expression.
    fn token_request(url: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "method": "POST",
            "url": url,
            "body": {
                "kind": "form",
                "fields": [
                    {
                        "name": "grant_type",
                        "value": [{ "type": "literal", "value": "refresh_token" }],
                    },
                ],
            },
        })
    }

    /// Builds one device-grant OAuth payload wholesale with the given credential-stage URL
    /// expressions: the device authorization request URL, the token exchange request URL, and
    /// the refresh grant request URL. The inference URL stays literal so only the named
    /// credential member decides the predicate.
    fn device_payload(
        authorization_url: serde_json::Value,
        exchange_url: serde_json::Value,
        refresh_url: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": literal("https://upstream.example"),
                "upstream_streaming": "match_client",
            },
            "credential": {
                "type": "oauth",
                "client_id": "client-id",
                "grant": {
                    "type": "device",
                    "authorization": {
                        "request": {"method": "POST", "url": authorization_url},
                        "response": {
                            "encoding": "form",
                            "fields": [
                                {
                                    "path": ["device_code"],
                                    "target": {"type": "device_code"},
                                    "required": true,
                                },
                                {
                                    "path": ["user_code"],
                                    "target": {"type": "user_code"},
                                    "required": true,
                                },
                                {
                                    "path": ["verification_uri"],
                                    "target": {"type": "verification_uri"},
                                    "required": false,
                                },
                            ],
                        },
                    },
                    "expires_in_fallback_seconds": 900,
                },
                "exchange": {
                    "request": token_request(exchange_url),
                    "response": {
                        "encoding": "json",
                        "fields": [
                            {
                                "path": ["access_token"],
                                "target": {"type": "access_token"},
                                "required": true,
                            },
                            {
                                "path": ["refresh_token"],
                                "target": {"type": "refresh_token"},
                                "required": true,
                            },
                        ],
                    },
                    "terminal_codes": ["invalid_grant"],
                },
                "refresh": {
                    "type": "grant",
                    "request": token_request(refresh_url),
                    "terminal_codes": ["invalid_grant"],
                },
            },
        })
    }

    /// Builds one authorization-code OAuth payload wholesale with the given authorize URL — the
    /// one URL-bearing member whose stage the write-time validation bars from the base
    /// reference.
    fn authorization_code_payload(authorize_url: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": literal("https://upstream.example"),
                "upstream_streaming": "match_client",
            },
            "credential": {
                "type": "oauth",
                "client_id": "client-id",
                "grant": {
                    "type": "authorization_code",
                    "authorize_url": authorize_url,
                    "redirect_uri": "http://localhost:1/callback",
                    "pkce": "none",
                },
                "exchange": {
                    "request": token_request(literal("https://oauth.example/token")),
                    "response": {
                        "encoding": "json",
                        "fields": [
                            {
                                "path": ["access_token"],
                                "target": {"type": "access_token"},
                                "required": true,
                            },
                        ],
                    },
                    "terminal_codes": ["invalid_grant"],
                },
                "refresh": {"type": "none"},
            },
        })
    }

    /// Verifies the base-reference predicate against every URL-bearing descriptor member whose
    /// evaluation environment carries the base URL: the inference and streaming URLs, the
    /// discovery, provisioning, and poll request URLs, and the credential section's
    /// device-authorization, token-exchange, and refresh-grant request URLs — directly or
    /// inside a `Coalesce`. The authorize URL is the one excluded member: its stage bars the
    /// base reference at write time (README §5.1), so a base reference there never counts.
    ///
    /// # Panics
    ///
    /// Panics when a fixture payload does not parse or the predicate diverges.
    #[test]
    fn detects_the_base_reference_across_every_url_bearing_member() {
        let literal_url = literal("https://upstream.example");

        let cases = [
            (
                "every URL literal",
                static_payload(
                    literal_url.clone(),
                    None,
                    literal_url.clone(),
                    literal_url.clone(),
                    None,
                ),
                false,
            ),
            (
                "inference URL",
                static_payload(base(), None, literal_url.clone(), literal_url.clone(), None),
                true,
            ),
            (
                "inference URL inside a coalesce",
                static_payload(
                    coalesced_base(),
                    None,
                    literal_url.clone(),
                    literal_url.clone(),
                    None,
                ),
                true,
            ),
            (
                "streaming URL",
                static_payload(
                    literal_url.clone(),
                    Some(base()),
                    literal_url.clone(),
                    literal_url.clone(),
                    None,
                ),
                true,
            ),
            (
                "discovery request URL",
                static_payload(literal_url.clone(), None, base(), literal_url.clone(), None),
                true,
            ),
            (
                "provisioning request URL",
                static_payload(literal_url.clone(), None, literal_url.clone(), base(), None),
                true,
            ),
            (
                "provisioning poll request URL",
                static_payload(
                    literal_url.clone(),
                    None,
                    literal_url.clone(),
                    literal_url.clone(),
                    Some(base()),
                ),
                true,
            ),
            (
                "device authorization request URL",
                device_payload(base(), literal_url.clone(), literal_url.clone()),
                true,
            ),
            (
                "exchange request URL",
                device_payload(literal_url.clone(), base(), literal_url.clone()),
                true,
            ),
            (
                "refresh grant request URL",
                device_payload(literal_url.clone(), literal_url.clone(), base()),
                true,
            ),
            (
                "refresh grant request URL inside a coalesce",
                device_payload(literal_url.clone(), literal_url.clone(), coalesced_base()),
                true,
            ),
            (
                "authorize URL — the stage barred from the base reference",
                authorization_code_payload(base()),
                false,
            ),
        ];

        for (name, payload, is_expected) in cases {
            assert_eq!(
                parses(payload).references_base(),
                is_expected,
                "case `{name}` diverges"
            );
        }
    }
}
