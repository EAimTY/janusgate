//! Provider definitions: the immutable, one-format connection recipes every provider instance and
//! OAuth flow references (README §3, §5).
//!
//! A definition decomposes into wire format (code, the five format codecs), connection
//! configuration (data, the descriptors in this module), and credentials (data, the provider
//! rows). The engines never special-case a provider; these descriptor vocabularies are the only
//! extension surface, and the management write boundary validates a payload once, at write time
//! ([`DefinitionPayload::validate`]).

pub(crate) mod compaction;
pub(crate) mod credential;
pub(crate) mod discovery;
pub(crate) mod eval;
mod expressions;
pub(crate) mod inference;
pub(crate) mod provisioning;
pub(crate) mod quota;
pub(crate) mod quota_exhaustion;
mod quota_validation;
pub(crate) mod template;
pub(crate) mod validate;

use crate::format::UpstreamFormat;
use serde::{Deserialize, Serialize};
use std::num::NonZeroI64;
pub(crate) use validate::ValidationError;

/// The payload of one provider definition version: the data `provider-definitions:create` and
/// `provider-definitions:update` carry, and what the `format`, `inference`, `credential`,
/// `provisioning`, `discovery`, and `compaction` columns of the `provider_definitions` table
/// store.
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

    /// The optional read-only upstream account quota query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<quota::QuotaDescriptor>,

    /// The optional remote-compaction section (README §5): strategy, budgets, per-mechanism
    /// declarations, and the operations executing them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<compaction::CompactionSection>,
}

impl DefinitionPayload {
    /// Whether any descriptor expression of this payload references the instance base URL — the
    /// [`template::ValueExpr::Base`] part (README §3). A definition so addressed evaluates every
    /// such expression against the bound provider row's stored base URL, so a provider bound to
    /// it must carry one.
    ///
    /// The walk covers every expression member whose evaluation environment carries the base
    /// URL — the inference and streaming URLs, the inference header-row values, the envelope's
    /// string leaves, every control request's full expression surface (URL, header-row
    /// values, query-row values, form-field values, and JSON string leaves) across the
    /// credential section's device-authorization, token-exchange, and refresh-grant requests,
    /// the provisioning steps and their polls, the discovery and quota requests, and the
    /// compaction section's declared mechanisms through [`compaction::MechanismDeclarations::
    /// enumerated`] (a declaration member joins this walk through it) plus the token-count
    /// operation — and must be extended alongside any new member of the manually walked set:
    /// the requirement checks consult only this predicate, so a missed expression is accepted
    /// with no base URL and every later evaluation of it fails as stored drift. The authorize
    /// URL and its extra parameters are excluded: the write-time validation bars the base
    /// reference at the authorize-URL stage (README §5.1), so those expressions can never
    /// evaluate against a base URL.
    #[must_use]
    pub(crate) fn references_base(&self) -> bool {
        self.base_capable_surfaces()
            .into_iter()
            .any(|surface| surface.references(&template::ValueExpr::Base))
    }

    /// Enumerates each descriptor surface that can use a stored or working base URL.
    /// Authorization URLs and extra parameters are excluded because their stage forbids Base.
    fn base_capable_surfaces(&self) -> Vec<expressions::ExpressionSurface<'_>> {
        use compaction::Declaration;
        use expressions::ExpressionSurface;
        let mut surfaces = vec![
            ExpressionSurface::String(&self.inference.url),
            ExpressionSurface::Headers(&self.inference.headers),
        ];
        surfaces.extend(
            self.inference
                .stream_url
                .as_ref()
                .map(ExpressionSurface::String),
        );
        surfaces.extend(
            self.inference
                .envelope
                .as_ref()
                .map(ExpressionSurface::Template),
        );
        if let credential::CredentialAcquisition::OAuth(recipe) = &self.credential {
            if let credential::Grant::Device { authorization, .. } = &recipe.grant {
                surfaces.push(ExpressionSurface::Control(&authorization.request));
            }
            surfaces.push(ExpressionSurface::Control(&recipe.exchange.request));
            if let credential::TokenRefresh::Grant { request, .. } = &recipe.refresh {
                surfaces.push(ExpressionSurface::Control(request));
            }
        }
        if let Some(steps) = &self.provisioning {
            for step in steps {
                surfaces.push(ExpressionSurface::Control(&step.request));
                surfaces.extend(
                    step.poll
                        .as_ref()
                        .map(|poll| ExpressionSurface::Control(&poll.request)),
                );
            }
        }
        surfaces.extend(
            self.discovery
                .as_ref()
                .map(|discovery| ExpressionSurface::Control(&discovery.request)),
        );
        surfaces.extend(
            self.quota
                .as_ref()
                .map(|quota| ExpressionSurface::Control(&quota.request)),
        );
        if let Some(compaction) = &self.compaction {
            for (_, declaration) in compaction.mechanisms.enumerated() {
                match declaration {
                    Some(Declaration::Standalone(standalone)) => {
                        if let Some(operation) = &standalone.operation {
                            surfaces.push(ExpressionSurface::String(&operation.url));
                            surfaces.push(ExpressionSurface::Headers(&operation.headers));
                            surfaces.extend(
                                operation.envelope.as_ref().map(ExpressionSurface::Template),
                            );
                        }
                    }

                    Some(Declaration::Overlay(overlay)) => {
                        if let Some(headers) = overlay.overlay_headers.as_deref() {
                            surfaces.push(ExpressionSurface::Headers(headers));
                        }
                    }

                    // The ride-along declarations carry no operation data: the trigger and the
                    // automatic controls ride the ordinary inference request.
                    Some(Declaration::RideAlong(_)) | None => {}
                }
            }

            if let Some(operation) = &compaction.count_tokens {
                surfaces.push(ExpressionSurface::String(&operation.url));
                surfaces.push(ExpressionSurface::Headers(&operation.headers));
                surfaces.extend(operation.envelope.as_ref().map(ExpressionSurface::Template));
            }
        }
        surfaces
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

    /// The payload's OAuth recipe, when the credential acquisition is OAuth — the single
    /// deconstruction every OAuth engine stage shares. The accessor itself only reports
    /// absence: stored drift — an OAuth flow or renewal whose definition carries no OAuth
    /// credential — lives in the caller's error domain.
    #[must_use]
    pub(crate) fn oauth_recipe(&self) -> Option<&credential::OAuthRecipe> {
        match &self.payload.credential {
            credential::CredentialAcquisition::OAuth(recipe) => Some(recipe),
            credential::CredentialAcquisition::ApiKey => None,
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

    /// Builds one static-key payload wholesale with the given inference header rows and the
    /// optional envelope: the inference URL stays literal, so only the carried non-URL members
    /// decide the predicate.
    fn inference_payload(
        headers: serde_json::Value,
        envelope: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let inference = match envelope {
            Some(envelope) => serde_json::json!({
                "url": literal("https://upstream.example"),
                "headers": headers,
                "upstream_streaming": "match_client",
                "envelope": envelope,
            }),

            None => serde_json::json!({
                "url": literal("https://upstream.example"),
                "headers": headers,
                "upstream_streaming": "match_client",
            }),
        };

        serde_json::json!({
            "format": "openai_chat_completion",
            "inference": inference,
            "credential": {"type": "api_key"},
        })
    }

    /// Builds one device-grant OAuth payload wholesale with the given token-exchange control
    /// request: every other expression stays literal, so only the exchange request decides the
    /// predicate. The refresh mode stays `none`.
    fn exchange_payload(exchange_request: serde_json::Value) -> serde_json::Value {
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
                        "request": {
                            "method": "POST",
                            "url": literal("https://oauth.example/device"),
                        },
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
                    "request": exchange_request,
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

    /// Builds one static-key `openai_response` payload wholesale with the given compaction
    /// section document: every other expression stays literal, so only the carried compaction
    /// members decide the predicate.
    fn compaction_payload(compaction: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "format": "openai_response",
            "inference": {
                "url": literal("https://upstream.example"),
                "upstream_streaming": "match_client",
            },
            "credential": {"type": "api_key"},
            "compaction": compaction,
        })
    }

    /// Builds one `anthropic_messages` compaction section carrying the given overlay headers
    /// on both Messages declarations.
    fn overlay_section(headers: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "mechanisms": {
                "messages_on_demand_compaction": {
                    "default": "supported",
                    "overlay_headers": headers,
                },
                "messages_threshold_compaction": {
                    "default": "supported",
                    "overlay_headers": headers,
                },
            },
        })
    }

    /// Verifies the base-reference predicate against every compaction-section expression
    /// surface — the standalone operation's URL, header rows, and envelope string leaves, the
    /// token-count operation's surface, and both Messages declarations' overlay-header values —
    /// directly or inside a `Coalesce`.
    ///
    /// # Panics
    ///
    /// Panics when a fixture payload does not parse or the predicate diverges.
    #[test]
    fn detects_the_base_reference_across_the_compaction_surfaces() {
        let standalone = |operation: serde_json::Value| {
            compaction_payload(serde_json::json!({
                "mechanisms": {
                    "responses_standalone_compact": {
                        "default": "supported",
                        "operation": operation,
                    },
                },
            }))
        };

        let cases = [
            (
                "standalone operation URL",
                standalone(serde_json::json!({ "url": base() })),
                true,
            ),
            (
                "standalone operation URL inside a coalesce",
                standalone(serde_json::json!({ "url": coalesced_base() })),
                true,
            ),
            (
                "standalone operation header value",
                standalone(serde_json::json!({
                    "url": literal("https://upstream.example/compact"),
                    "headers": [{ "name": "X-Endpoint", "value": base() }],
                })),
                true,
            ),
            (
                "standalone operation envelope string leaf",
                standalone(serde_json::json!({
                    "url": literal("https://upstream.example/compact"),
                    "envelope": {
                        "type": "object",
                        "fields": {
                            "endpoint": { "type": "string", "expr": base() },
                        },
                    },
                })),
                true,
            ),
            (
                "literal JSON never creates a reference or a body placeholder",
                standalone(serde_json::json!({
                    "url": literal("https://upstream.example/compact"),
                    "envelope": {
                        "type": "json",
                        "value": { "type": "base", "body": { "type": "body" } },
                    },
                })),
                false,
            ),
            (
                "every standalone surface literal",
                standalone(serde_json::json!({
                    "url": literal("https://upstream.example/compact"),
                })),
                false,
            ),
            (
                "token-count operation URL",
                compaction_payload(serde_json::json!({
                    "count_tokens": { "url": base() },
                })),
                true,
            ),
            (
                "token-count operation header value",
                compaction_payload(serde_json::json!({
                    "count_tokens": {
                        "url": literal("https://upstream.example/count"),
                        "headers": [{ "name": "X-Endpoint", "value": base() }],
                    },
                })),
                true,
            ),
            (
                "token-count operation envelope string leaf",
                compaction_payload(serde_json::json!({
                    "count_tokens": {
                        "url": literal("https://upstream.example/count"),
                        "envelope": {
                            "type": "object",
                            "fields": {
                                "endpoint": { "type": "string", "expr": coalesced_base() },
                            },
                        },
                    },
                })),
                true,
            ),
            (
                "on-demand overlay header value",
                compaction_payload(overlay_section(serde_json::json!([
                    { "name": "X-Endpoint", "value": base() },
                ]))),
                true,
            ),
            (
                "threshold overlay header value",
                compaction_payload(overlay_section(serde_json::json!([
                    { "name": "X-Endpoint", "value": coalesced_base() },
                ]))),
                true,
            ),
            (
                "every overlay header value literal",
                compaction_payload(overlay_section(serde_json::json!([
                    { "name": "X-Endpoint", "value": literal("https://upstream.example") },
                ]))),
                false,
            ),
            (
                "an empty section",
                compaction_payload(serde_json::json!({})),
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

    /// Verifies the base-reference predicate against every descriptor member whose evaluation
    /// environment carries the base URL — the inference and streaming URLs, the inference
    /// header-row values, the envelope's string leaves, and every control request's URL,
    /// header-row values, query-row values, form-field values, and JSON string leaves across
    /// the device-authorization, token-exchange, refresh-grant, provisioning, poll, and
    /// discovery requests — directly or inside a `Coalesce`. The authorize URL is the one
    /// excluded member: its stage bars the base reference at write time (README §5.1), so a
    /// base reference there never counts.
    ///
    /// # Panics
    ///
    /// Panics when a fixture payload does not parse or the predicate diverges.
    #[test]
    fn detects_the_base_reference_across_every_expression_surface() {
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
                "literal inference header value",
                inference_payload(
                    serde_json::json!([
                        { "name": "X-Endpoint", "value": literal("https://upstream.example/v1") },
                    ]),
                    None,
                ),
                false,
            ),
            (
                "inference header value",
                inference_payload(
                    serde_json::json!([{ "name": "X-Endpoint", "value": base() }]),
                    None,
                ),
                true,
            ),
            (
                "inference envelope string leaf",
                inference_payload(
                    serde_json::json!([]),
                    Some(serde_json::json!({
                        "type": "object",
                        "fields": {
                            "request": {
                                "type": "array",
                                "items": [{ "type": "string", "expr": base() }],
                            },
                        },
                    })),
                ),
                true,
            ),
            (
                "control request header value",
                exchange_payload(serde_json::json!({
                    "method": "POST",
                    "url": literal("https://oauth.example/token"),
                    "headers": [{ "name": "X-Endpoint", "value": base() }],
                })),
                true,
            ),
            (
                "control request query-row value",
                exchange_payload(serde_json::json!({
                    "method": "POST",
                    "url": literal("https://oauth.example/token"),
                    "query": [{ "name": "endpoint", "value": base() }],
                })),
                true,
            ),
            (
                "control request form-field value",
                exchange_payload(serde_json::json!({
                    "method": "POST",
                    "url": literal("https://oauth.example/token"),
                    "body": {
                        "kind": "form",
                        "fields": [{ "name": "endpoint", "value": base() }],
                    },
                })),
                true,
            ),
            (
                "control request form-field value inside a coalesce",
                exchange_payload(serde_json::json!({
                    "method": "POST",
                    "url": literal("https://oauth.example/token"),
                    "body": {
                        "kind": "form",
                        "fields": [{ "name": "endpoint", "value": coalesced_base() }],
                    },
                })),
                true,
            ),
            (
                "control request JSON string leaf",
                exchange_payload(serde_json::json!({
                    "method": "POST",
                    "url": literal("https://oauth.example/token"),
                    "body": {
                        "kind": "json",
                        "node": {
                            "type": "object",
                            "fields": {
                                "endpoint": { "type": "string", "expr": base() },
                            },
                        },
                    },
                })),
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
