//! The flow begin orchestration of the OAuth protocol engine: both grants through the persisted
//! flow row — the authorization-code grant's authorize URL and the device grant's authorization
//! request (README §5.3, §7.1).
//!
//! Every provider-specific fact — endpoints, client credentials, parameter names, header sets —
//! lives in the definition the flow references; this engine carries the grant mechanics alone.
//! The control-request executor runs each stage's request, and the stage's response descriptor
//! maps its product. The token mechanics a begun flow later runs — the token exchange, the
//! refresh grant, the token-response expiry resolution, and the token-data folding — live in
//! [`super::exchange`].

use super::exchange::DEFAULT_DEVICE_INTERVAL_SECONDS;
use crate::{
    database::Database,
    provider::{
        Error,
        control::{self, ControlFailure, Extracted},
        definition::{
            credential::{Grant, OAuthRecipe, Pkce, SlotGenerator},
            eval::ValueEnv,
            template::ResponseTarget as Target,
        },
    },
    storage::{
        self,
        oauth_flows::{FlowGrant, FlowPurpose, NewFlow, OngoingFlow},
    },
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use reqwest::Client;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

/// Lifetime of a pending authorization-code flow, in database-clock microseconds.
const AUTHORIZATION_CODE_TTL: i64 = 600 * unix_time::MICROS_PER_SEC;

/// Derives the PKCE S256 challenge of a verifier: Base64URL (unpadded) of SHA-256 over the
/// ASCII verifier bytes (RFC 7636 §4.2).
#[must_use]
fn pkce_challenge(verifier: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Generates `byte_count` random bytes encoded as unpadded URL-safe Base64.
#[must_use]
fn random_url_safe(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];

    rand::fill(&mut bytes[..]);

    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Initializes a new flow's working slots from the definition's `initial_slots` (README §5.3):
/// the generators run once per new flow before its first external request, over the target
/// provider's stored slots for a reauthorization, so only the missing names are initialized.
///
/// # Errors
///
/// Returns [`Error::Internal`] when the definition carries no OAuth credential — stored drift,
/// unreachable for a definition the management boundary already checked.
pub(crate) fn initialize_slots(
    definition: &crate::provider::ProviderDefinition,
    stored: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, Error> {
    let recipe = definition.oauth_recipe().ok_or_else(|| Error::Internal {
        message: "the flow's definition carries no OAuth credential".to_string(),
    })?;

    let mut slots = stored.clone();

    for initializer in &recipe.initial_slots {
        if slots.contains_key(&initializer.name) {
            continue;
        }

        let value = match initializer.generator {
            SlotGenerator::Uuid => crate::ids::new_uuid(),

            SlotGenerator::UuidHex => crate::ids::new_uuid().replace('-', ""),
        };

        slots.insert(initializer.name.clone(), value);
    }

    Ok(slots)
}

/// What one flow begin is for, with the purpose's own inputs: a create flow names the provider
/// it will create, a reauthorize flow targets one existing provider's row. The pairs are the
/// flow schema's cross-column checks (README §3) in type form.
pub(crate) enum BeginPurpose {
    /// The flow creates a new provider when it completes.
    Create {
        /// The new provider's name.
        provider_name: String,
        /// Prefix for the new provider's automatically exposed model IDs.
        model_prefix: String,
    },

    /// The flow replaces the credentials — and optionally the definition reference — of one
    /// existing provider.
    Reauthorize {
        /// The reauthorized provider's row identifier.
        target_provider_id: i64,
    },
}

/// The begin facts one grant kind produces: the authorization-code grant's URL the
/// administrator opens, or the device grant's verification facts the administrator approves
/// through while the gateway polls.
pub(crate) enum BeginFacts {
    /// An authorization-code flow: the URL the administrator opens to authorize.
    AuthorizationCode {
        /// URL the administrator opens to authorize.
        authorize_url: String,
    },

    /// A device flow: the verification facts the administrator approves through.
    Device {
        /// The human-readable code the administrator enters at the verification URI.
        user_code: String,

        /// The URI the administrator opens, when the response carried one.
        verification_uri: Option<String>,

        /// The verification URI carrying the user code, when the response carried one.
        verification_uri_complete: Option<String>,
    },
}

/// The engine's outcome of one flow begin: the persisted flow row — the source of truth for
/// every stored fact it carries — beside the grant's begin facts, so the wire surface maps the
/// outcome without recomputing anything.
pub(crate) struct BeganFlow {
    /// The flow row the insert persisted and returned.
    pub(crate) flow: OngoingFlow,

    /// The grant's begin facts.
    pub(crate) facts: BeginFacts,
}

/// The flow row's grant-varying fields: what one grant kind alone determines — its grant type,
/// its PKCE verifier, the device grant's stored codes and schedule, and the row's expiry. The
/// identity fields the grants share are stamped once where the row literal is built.
struct GrantFlowFields {
    /// The grant kind the flow runs.
    grant_type: FlowGrant,

    /// The PKCE verifier of an authorization-code flow.
    code_verifier: Option<String>,

    /// The device code issued by the authorization response.
    device_code: Option<String>,

    /// The human-readable user code.
    device_user_code: Option<String>,

    /// The verification URI, when the response carried one.
    device_verification_uri: Option<String>,

    /// The verification URI carrying the user code, when carried.
    device_verification_uri_complete: Option<String>,

    /// The stored poll interval, in seconds.
    device_poll_interval_seconds: Option<i64>,

    /// The first attempt instant, in UTC microseconds.
    device_next_poll_at: Option<i64>,

    /// The flow's expiry, in UTC microseconds.
    expires_at: i64,
}

/// Begins one flow against its definition (README §5.3, §7.1): mints the state token and the
/// optional PKCE verifier, builds the authorize URL or executes the device authorization,
/// resolves the expiry, and persists the flow row whose stored facts the outcome carries.
///
/// # Errors
///
/// Returns [`Error`] when the authorize-URL evaluation fails, the device authorization fails,
/// a device duration overflows the database clock, or the row insert fails.
///
/// # Panics
///
/// Panics on stored device-flow drift: the authorization response omits `expires_in_seconds`
/// and the definition carries no `expires_in_fallback_seconds` — unreachable for definitions
/// that pass write-time validation. Panics on begin/persist drift when the persisted row reads
/// back as an authorization-code flow while no authorize URL was built — the match arm that
/// stamps the row's grant type is the same arm that builds the URL. Panics on schema-backstop
/// drift when the persisted device row carries no `user_code` — the schema requires a device
/// row to carry its user code, so a row without one is stored drift.
pub(crate) async fn begin_flow(
    database: &Database,
    http: &Client,
    definition: &crate::provider::ProviderDefinition,
    purpose: BeginPurpose,
    base_url: Option<String>,
    slots: BTreeMap<String, String>,
) -> Result<BeganFlow, Error> {
    let recipe = definition.oauth_recipe().ok_or_else(|| Error::Internal {
        message: "the flow's definition carries no OAuth credential".to_string(),
    })?;

    let OAuthRecipe {
        client_id,
        client_secret,
        grant,
        ..
    } = recipe;

    let state_token = random_url_safe(32);

    // The purpose's target fields are the schema's cross-column checks in storage form: a
    // create row carries its new provider's naming, a reauthorize row its target.
    let (purpose, provider_name, model_prefix, target_provider_id) = match purpose {
        BeginPurpose::Create {
            provider_name,
            model_prefix,
        } => (
            FlowPurpose::Create,
            Some(provider_name),
            Some(model_prefix),
            None,
        ),
        BeginPurpose::Reauthorize { target_provider_id } => (
            FlowPurpose::Reauthorize,
            None,
            None,
            Some(target_provider_id),
        ),
    };

    // The per-grant match yields the row's grant-varying fields plus the one fact storage
    // cannot recover: the authorization-code grant's authorize URL. The device grant's
    // verification facts exist only as the row's stored columns, so no fact is computed twice.
    let (grant_fields, authorize_url) = match grant {
        Grant::AuthorizationCode { pkce, .. } => {
            let code_verifier = match pkce {
                Pkce::S256 => Some(random_url_safe(64)),
                Pkce::None => None,
            };

            let env = ValueEnv::for_authorize_url(
                &slots,
                client_id.as_str(),
                client_secret.as_deref(),
                &state_token,
            );

            let authorize_url =
                build_authorize_url(grant, &env, &state_token, code_verifier.as_deref())?;

            let grant_fields = GrantFlowFields {
                grant_type: FlowGrant::AuthorizationCode,
                code_verifier,
                device_code: None,
                device_user_code: None,
                device_verification_uri: None,
                device_verification_uri_complete: None,
                device_poll_interval_seconds: None,
                device_next_poll_at: None,
                expires_at: unix_time::unix_now_micros() + AUTHORIZATION_CODE_TTL,
            };

            (grant_fields, Some(authorize_url))
        }

        Grant::Device {
            authorization: _,
            expires_in_fallback_seconds,
        } => {
            let env = ValueEnv::for_device_authorization(
                &slots,
                base_url.as_deref(),
                client_id.as_str(),
                client_secret.as_deref(),
            );

            let device = execute_device_authorization(http, grant, &env).await?;

            // The expiry comes from the response's lifetime, else the definition's explicit
            // fallback. The write-time validation makes a definition carrying neither
            // unrepresentable: a stored-drift definition must fail fast instead of silently
            // inventing a lifetime.
            #[expect(
                clippy::expect_used,
                reason = "write-time validation requires a reported expiry or the configured \
                          fallback; a definition with neither is stored drift"
            )]
            let expires_in = device
                .expires_in_seconds
                .or_else(|| {
                    expires_in_fallback_seconds
                        .map(|fallback| i64::try_from(fallback.get()).unwrap_or(i64::MAX))
                })
                .expect("device-flow expiry is guaranteed by the write-time validation");

            let interval = device
                .interval_seconds
                .unwrap_or(DEFAULT_DEVICE_INTERVAL_SECONDS);

            let expires_at =
                unix_time::seconds_from_now(expires_in).ok_or_else(|| Error::Unavailable {
                    detail: "malformed device authorization response: `expires_in` overflow"
                        .to_string(),
                })?;

            let device_next_poll_at =
                unix_time::seconds_from_now(interval).ok_or_else(|| Error::Unavailable {
                    detail: "malformed device authorization response: `interval` overflow"
                        .to_string(),
                })?;

            let grant_fields = GrantFlowFields {
                grant_type: FlowGrant::Device,
                code_verifier: None,
                device_code: Some(device.device_code),
                device_user_code: Some(device.user_code),
                device_verification_uri: device.verification_uri,
                device_verification_uri_complete: device.verification_uri_complete,
                device_poll_interval_seconds: Some(interval),
                device_next_poll_at: Some(device_next_poll_at),
                expires_at,
            };

            (grant_fields, None)
        }
    };

    let GrantFlowFields {
        grant_type,
        code_verifier,
        device_code,
        device_user_code,
        device_verification_uri,
        device_verification_uri_complete,
        device_poll_interval_seconds,
        device_next_poll_at,
        expires_at,
    } = grant_fields;

    let new_flow = NewFlow {
        definition_name: definition.name.clone(),
        definition_version: definition.version,
        state_token,
        purpose,
        provider_name,
        model_prefix,
        base_url,
        target_provider_id,
        grant_type,
        slots,
        code_verifier,
        device_code,
        device_user_code,
        device_verification_uri,
        device_verification_uri_complete,
        device_poll_interval_seconds,
        device_next_poll_at,
        expires_at,
    };

    // The stored row is the response's source of truth: the state token, the expiry, and the
    // device facts all read back from the row the insert returns, never from the caller's
    // local copy.
    let flow = storage::oauth_flows::insert_flow(database, new_flow).await?;

    let facts = match flow.grant_type {
        FlowGrant::AuthorizationCode => {
            #[expect(
                clippy::expect_used,
                reason = "the authorization-code arm above always built its URL"
            )]
            let authorize_url = authorize_url.expect("the begin built its authorize URL");

            BeginFacts::AuthorizationCode { authorize_url }
        }

        FlowGrant::Device => {
            // The schema requires a device row to carry its user code; a row without one is
            // stored drift.
            #[expect(
                clippy::expect_used,
                reason = "the schema requires a device row to carry its user code"
            )]
            let user_code = flow
                .device_user_code
                .clone()
                .expect("a device row carries its user code");

            BeginFacts::Device {
                user_code,
                verification_uri: flow.device_verification_uri.clone(),
                verification_uri_complete: flow.device_verification_uri_complete.clone(),
            }
        }
    };

    Ok(BeganFlow { flow, facts })
}

/// Builds the authorize URL of an authorization-code grant: the evaluated base URL, the standard
/// parameters the builder appends itself (`response_type`, `client_id`, `redirect_uri`,
/// `state`, and the PKCE pair when configured), and the definition's extra parameters in order.
/// The parameters join through [`control::append_query_row`], which keeps them in the query
/// component ahead of a `#` fragment in the stored base (RFC 3986 §3) so every authorization
/// parameter reaches the provider.
fn build_authorize_url(
    grant: &Grant,
    env: &ValueEnv<'_>,
    state_token: &str,
    code_verifier: Option<&str>,
) -> Result<String, Error> {
    let Grant::AuthorizationCode {
        authorize_url,
        redirect_uri,
        pkce,
        extra_params,
    } = grant
    else {
        return Err(Error::Internal {
            message: "the flow's definition carries no authorization-code grant".to_string(),
        });
    };

    let mut url = env
        .evaluate(authorize_url, "authorize URL")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    let mut append = |name: &str, value: &str| {
        control::append_query_row(&mut url, name, &control::percent_encode_component(value));
    };

    append("response_type", "code");

    append("client_id", env.client_id.unwrap_or_default());

    append("redirect_uri", redirect_uri);

    if let Some(verifier) = code_verifier
        && matches!(pkce, Pkce::S256)
    {
        append("code_challenge", &pkce_challenge(verifier));
        append("code_challenge_method", "S256");
    }

    for param in extra_params {
        let value = env
            .evaluate(&param.value, "authorize-URL parameter")
            .map_err(|error| Error::Internal {
                message: error.to_string(),
            })?;

        append(&param.name, &value);
    }

    // The builder owns `state`: it is appended last, after every configured parameter.
    append("state", state_token);

    Ok(url)
}

/// The decoded product of a device authorization response (README §5.3).
#[derive(Debug, Clone)]
pub(crate) struct DeviceAuthorization {
    /// The device code the polling exchange presents.
    pub(crate) device_code: String,

    /// The human-readable code the administrator enters at the verification URI.
    pub(crate) user_code: String,

    /// The URI the administrator opens, when the response carries one.
    pub(crate) verification_uri: Option<String>,

    /// The verification URI carrying the user code, when the response carries one.
    pub(crate) verification_uri_complete: Option<String>,

    /// The device code's lifetime in seconds, when the response reports it.
    pub(crate) expires_in_seconds: Option<i64>,

    /// The polling interval in seconds, when the response requires one.
    pub(crate) interval_seconds: Option<i64>,
}

/// Executes the device authorization request of a definition and decodes its response.
pub(crate) async fn execute_device_authorization(
    http: &Client,
    grant: &Grant,
    env: &ValueEnv<'_>,
) -> Result<DeviceAuthorization, Error> {
    let Grant::Device { authorization, .. } = grant else {
        return Err(Error::Internal {
            message: "the flow's definition carries no device grant".to_string(),
        });
    };

    let body = control::execute(
        http,
        &authorization.request,
        env,
        authorization.response.encoding,
        "device authorization",
    )
    .await?;

    let required =
        |field: Result<Option<Extracted>, ControlFailure>, name: &str| -> Result<String, Error> {
            match field {
                Ok(Some(Extracted::Text(text))) => Ok(text),
                Ok(Some(Extracted::Integer(value))) => Ok(value.to_string()),
                Ok(None) => Err(Error::Unavailable {
                    detail: format!("device authorization response lacks a `{name}` value"),
                }),
                Err(failure) => Err(failure.into()),
            }
        };

    let optional =
        |field: Result<Option<Extracted>, ControlFailure>| -> Result<Option<String>, Error> {
            match field {
                Ok(Some(extracted)) => Ok(Some(extracted.as_text())),
                Ok(None) => Ok(None),
                Err(failure) => Err(failure.into()),
            }
        };

    let find = |target: &Target| {
        authorization
            .response
            .fields
            .iter()
            .find(|field| &field.target == target)
            .map(|field| {
                control::extract_field(
                    &body,
                    &field.path,
                    &control::TEXT_FIELD_SHAPE,
                    "device authorization",
                )
            })
            .unwrap_or(Ok(None))
    };

    let device_code = required(find(&Target::DeviceCode), "device_code")?;
    let user_code = required(find(&Target::UserCode), "user_code")?;

    let verification_uri = optional(find(&Target::VerificationUri))?;
    let verification_uri_complete = optional(find(&Target::VerificationUriComplete))?;

    if verification_uri.is_none() && verification_uri_complete.is_none() {
        return Err(Error::Unavailable {
            detail: "device authorization response lacks a verification URI".to_string(),
        });
    }

    // RFC 8628 §3.2 requires a positive integer for both durations: a non-positive `interval`
    // would make the poller due every tick and a non-positive `expires_in` a dead-on-arrival
    // flow, so either is a malformed device-authorization response. Absence still falls back
    // per the documented rules (the five-second default; `expires_in_fallback_seconds`).
    let expires_in_seconds = match optional(find(&Target::ExpiresInSeconds))? {
        Some(text) => Some(positive_duration(text, "expires_in")?),

        None => None,
    };

    let interval_seconds = match optional(find(&Target::IntervalSeconds))? {
        Some(text) => Some(positive_duration(text, "interval")?),

        None => None,
    };

    Ok(DeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in_seconds,
        interval_seconds,
    })
}

/// Parses one upstream-reported device-authorization duration: a non-integer spelling and a
/// non-positive value are both malformed device-authorization responses (RFC 8628 §3.2) — a
/// non-positive `interval` would make the poller due every tick and a non-positive `expires_in`
/// a dead-on-arrival flow. Absence falls back per the documented rules at the call site.
fn positive_duration(text: String, field: &str) -> Result<i64, Error> {
    let seconds = text.parse::<i64>().map_err(|_| Error::Unavailable {
        detail: format!("device authorization response carries a non-integer `{field}`"),
    })?;

    if seconds <= 0 {
        return Err(Error::Unavailable {
            detail: format!(
                "malformed device authorization response: `{field}` must be positive, \
                 got {seconds}"
            ),
        });
    }

    Ok(seconds)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{
        AUTHORIZATION_CODE_TTL, BeginFacts, BeginPurpose, begin_flow, build_authorize_url,
    };
    use crate::{
        format::UpstreamFormat,
        provider::definition::{
            credential::{CredentialAcquisition, Grant, Pkce},
            eval::ValueEnv,
            template::{ParamRow, StringExpr},
        },
        storage::provider_definitions::{self, NewDefinition},
        test_support::{
            self, MockUpstream, OAuthDefinitionFixture, OAuthDeviceAuthorizationFixture,
            OAuthGrantFixture, OAuthRefreshFixture,
        },
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    /// The device grant one begin test swaps into the shared OAuth skeleton: the authorization
    /// request posts the `client_id` form row — RFC 8628 §3.1, and the write boundary requires
    /// it of the fixture's secret-less credential — to the given base and maps the required
    /// codes plus the verification URI, reporting neither `expires_in` nor `interval` — so the
    /// begin resolves the documented fallbacks, the definition's expiry fallback and the
    /// five-second RFC 8628 default.
    ///
    /// # Panics
    ///
    /// Panics when the fixture grant fails to parse — a broken fixture is the failure report.
    fn device_begin_grant(base: &str) -> Grant {
        #[expect(clippy::unwrap_used, reason = "the fixture grant always parses")]
        serde_json::from_value(json!({
            "type": "device",
            "authorization": {
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": format!("{base}/device/code")}],
                    "body": {"kind": "form", "fields": [
                        {"name": "client_id", "value": [{"type": "client_id"}]}
                    ]}
                },
                "response": {
                    "encoding": "json",
                    "fields": [
                        {
                            "path": ["device_code"],
                            "target": {"type": "device_code"},
                            "required": true
                        },
                        {
                            "path": ["user_code"],
                            "target": {"type": "user_code"},
                            "required": true
                        },
                        {
                            "path": ["verification_uri"],
                            "target": {"type": "verification_uri"},
                            "required": false
                        }
                    ]
                }
            },
            "expires_in_fallback_seconds": 900
        }))
        .unwrap()
    }

    /// The authorization-code grant one URL-build test swaps in: the given authorize base, the
    /// loopback redirect, no PKCE, and one extra parameter whose value carries a space — so the
    /// rendered URL exercises the standard parameters, the extra parameters, and the strict
    /// percent encoding in one byte-exact expectation.
    fn code_grant(authorize_base: &str) -> Grant {
        Grant::AuthorizationCode {
            authorize_url: StringExpr::literal(authorize_base),
            redirect_uri: "https://gateway.example/callback".to_string(),
            pkce: Pkce::None,
            extra_params: vec![ParamRow {
                name: "scope".to_string(),
                value: StringExpr::literal("read write"),
            }],
        }
    }

    /// The authorize-URL builder joins every appended parameter into the evaluated URL's query
    /// component — before a `#` fragment when the stored base carries one — and re-attaches the
    /// fragment after the last parameter (RFC 3986 §3): a parameter appended after the first
    /// `#` would fold into the fragment and silently drop from the authorization request, so
    /// `response_type`, `client_id`, `redirect_uri`, the extra parameters, and the trailing
    /// `state` must all precede it. The fragment case is byte-exact.
    ///
    /// # Panics
    ///
    /// Panics when the built URL diverges from the byte-exact expectation.
    #[test]
    fn authorize_url_joins_parameters_before_a_stored_fragment() {
        let slots = BTreeMap::new();

        let env = ValueEnv::for_authorize_url(&slots, "client-id", None, "state-token");

        let url = build_authorize_url(
            &code_grant("https://auth.example/authorize#tenant"),
            &env,
            "state-token",
            None,
        )
        .unwrap();

        assert_eq!(
            url,
            "https://auth.example/authorize?response_type=code&client_id=client-id\
             &redirect_uri=https%3A%2F%2Fgateway.example%2Fcallback&scope=read%20write\
             &state=state-token#tenant"
        );
    }

    /// The fragment-free rendering is byte-identical to the builder's documented shape: the
    /// standard parameters in the builder's order (`response_type`, `client_id`,
    /// `redirect_uri`, the extra parameters in order, `state` appended last), every value
    /// percent-encoded with the strict RFC 3986 unreserved set, and the plain `?` join.
    ///
    /// # Panics
    ///
    /// Panics when the built URL diverges from the byte-exact expectation.
    #[test]
    fn authorize_url_without_a_fragment_keeps_its_documented_byte_shape() {
        let slots = BTreeMap::new();

        let env = ValueEnv::for_authorize_url(&slots, "client-id", None, "state-token");

        let url = build_authorize_url(
            &code_grant("https://auth.example/authorize"),
            &env,
            "state-token",
            None,
        )
        .unwrap();

        assert_eq!(
            url,
            "https://auth.example/authorize?response_type=code&client_id=client-id\
             &redirect_uri=https%3A%2F%2Fgateway.example%2Fcallback&scope=read%20write\
             &state=state-token"
        );
    }

    /// The engine's begin orchestration persists the flow row its outcome carries (README §5.3,
    /// §7.1): an authorization-code begin stores the row with the fixed ten-minute expiry and
    /// its create purpose's naming, and the begin facts carry the authorize URL — carrying the
    /// minted state — the one fact storage cannot recover.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[tokio::test]
    async fn begin_flow_persists_an_authorization_code_flow_and_carries_its_url() {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = test_support::insert_oauth_definition(&database, "oauth-def").await;

        let now = crate::unix_time::unix_now_micros();

        let began = begin_flow(
            &database,
            &crate::http_client::control_plane(),
            &definition,
            BeginPurpose::Create {
                model_prefix: String::new(),
                provider_name: "created".to_string(),
            },
            Some("https://upstream.example".to_string()),
            BTreeMap::new(),
        )
        .await
        .unwrap();

        #[expect(
            clippy::panic,
            reason = "a divergent facts variant is the failure report"
        )]
        let BeginFacts::AuthorizationCode { authorize_url } = began.facts else {
            panic!("an authorization-code definition begins an authorization-code flow");
        };

        assert!(
            authorize_url.starts_with("https://accounts.example/auth?"),
            "{authorize_url}"
        );

        assert!(
            authorize_url.ends_with(&format!("&state={}", began.flow.state_token)),
            "{authorize_url}"
        );

        // The expiry is the fixed ten-minute TTL, read from the persisted row.
        assert!(
            began.flow.expires_at - now >= AUTHORIZATION_CODE_TTL,
            "{}",
            began.flow.expires_at - now
        );

        // The row is persisted and carries its purpose's target fields.
        #[expect(clippy::expect_used, reason = "the began flow row is persisted")]
        let stored = crate::storage::oauth_flows::find_flow_by_state(
            &database,
            &began.flow.state_token,
            now,
        )
        .await
        .unwrap()
        .expect("the began flow row is persisted");

        assert_eq!(stored.id, began.flow.id);
        assert_eq!(
            stored.grant_type,
            crate::storage::oauth_flows::FlowGrant::AuthorizationCode
        );
        assert_eq!(
            stored.purpose,
            crate::storage::oauth_flows::FlowPurpose::Create
        );
        assert_eq!(stored.provider_name.as_deref(), Some("created"));
        assert_eq!(stored.base_url.as_deref(), Some("https://upstream.example"));
    }

    /// The engine's device begin persists the flow row on its stored schedule (README §5.3,
    /// §7.1): the row carries the response's device facts, the definition's expiry fallback
    /// scaled to the database clock, and the five-second default interval the response implied
    /// by reporting none — and the begin facts carry the verification facts.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[tokio::test]
    async fn begin_flow_persists_a_device_flow_on_its_stored_schedule() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "device_code": "device-code",
                "user_code": "user-code",
                "verification_uri": "https://verify.example",
            }))
            .unwrap(),
        ])
        .await;

        let exchange_url = mock.base();

        // The shared OAuth skeleton with the grant's authorization redirected at the mock: the
        // skeleton's own exchange references the device_code value part, so the payload
        // validates with the swapped grant too.
        let mut payload = test_support::oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture-client",
            initial_slots: &[],
            grant: OAuthGrantFixture::Device {
                has_grant_type_field: true,
                authorization: OAuthDeviceAuthorizationFixture::default(),
            },
            exchange_url: &exchange_url,
            response_fields: vec![],
            terminal_codes: &["invalid_grant"],
            refresh: OAuthRefreshFixture::None,
            provisioning: None,
        });

        #[expect(
            clippy::panic,
            reason = "the fixture payload always carries an OAuth credential"
        )]
        let CredentialAcquisition::OAuth(recipe) = &mut payload.credential else {
            panic!("the OAuth fixture payload carries an OAuth credential");
        };

        recipe.grant = device_begin_grant(&exchange_url);

        payload.validate().unwrap();

        let (database, _guard) = test_support::create_test_database().await;

        let definition = provider_definitions::insert_definition(
            &database,
            NewDefinition {
                name: "device-def",
                base_version: None,
                payload: &payload,
                created_at: 1,
            },
        )
        .await
        .unwrap();

        let now = crate::unix_time::unix_now_micros();

        let began = begin_flow(
            &database,
            &crate::http_client::control_plane(),
            &definition,
            BeginPurpose::Reauthorize {
                target_provider_id: 1,
            },
            None,
            BTreeMap::new(),
        )
        .await
        .unwrap();

        #[expect(
            clippy::panic,
            reason = "a divergent facts variant is the failure report"
        )]
        let BeginFacts::Device {
            user_code,
            verification_uri,
            verification_uri_complete,
        } = began.facts
        else {
            panic!("a device definition begins a device flow");
        };

        assert_eq!(user_code, "user-code");
        assert_eq!(verification_uri.as_deref(), Some("https://verify.example"));
        assert_eq!(verification_uri_complete, None);

        // The stored schedule: the response implied no interval, so the five-second RFC 8628
        // default persists, both instants resolve from the fallback lifetime and the default,
        // and the row carries its reauthorize target.
        assert_eq!(began.flow.device_poll_interval_seconds, Some(5));
        assert!(began.flow.device_next_poll_at.unwrap() > now);
        assert!(began.flow.expires_at > now);

        assert_eq!(began.flow.device_code.as_deref(), Some("device-code"));
        assert_eq!(began.flow.device_user_code.as_deref(), Some("user-code"));
        assert_eq!(
            began.flow.purpose,
            crate::storage::oauth_flows::FlowPurpose::Reauthorize
        );
        assert_eq!(began.flow.provider_name, None);
        assert_eq!(began.flow.target_provider_id, Some(1));

        #[expect(clippy::expect_used, reason = "the began flow row is persisted")]
        let stored = crate::storage::oauth_flows::find_flow_by_state(
            &database,
            &began.flow.state_token,
            now,
        )
        .await
        .unwrap()
        .expect("the began flow row is persisted");

        assert_eq!(stored.id, began.flow.id);
    }
}
