//! The OAuth protocol engine: flow begin for both grants through the persisted flow row, the
//! token exchange with terminal-code classification, the identity-claim extraction, and the
//! expiry resolution (README §5.3, §7.1).
//!
//! Every provider-specific fact — endpoints, client credentials, parameter names, header sets —
//! lives in the definition the flow references; this engine carries the grant mechanics alone.
//! The control-request executor runs each stage's request, and the stage's response descriptor
//! maps its product. The exchange and refresh-grant fold applies the shared machine-consumed
//! text validators (README §7) before any slot or base-URL value reaches storage, and the
//! identity-token claim absorption applies the slot validator before a claimed value lands.

use super::provisioning::{validate_base_url_value, validate_slot_value};
use crate::{
    database::Database,
    provider::{
        Error,
        control::{self, ControlFailure, DecodedBody, Extracted},
        definition::{
            credential::{
                CredentialAcquisition, Exchange, Grant, IdTokenClaim, OAuthRecipe, Pkce,
                SlotGenerator,
            },
            eval::ValueEnv,
            template::{ResponseField, ResponseTarget as Target},
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

/// The polling interval a device authorization response implies when it reports none: five
/// seconds per RFC 8628 §3.2.
pub(crate) const DEFAULT_DEVICE_INTERVAL_SECONDS: i64 = 5;

/// The permanent interval increase a `slow_down` response applies, in seconds, per RFC 8628
/// §3.5.
pub(crate) const SLOW_DOWN_PENALTY_SECONDS: i64 = 5;

/// The expiry of a token response, resolved per README §5.3: a positive reported `expires_in`,
/// else a future `exp` claim of the access token, else the definition's explicitly configured
/// lifetime, else no expiry at all. A reported non-positive `expires_in` or a non-future `exp`
/// claim is a malformed token response, never silently treated as missing.
pub(crate) fn resolve_expiry(
    expires_in_seconds: Option<i64>,
    access_token: Option<&str>,
    configured_lifetime_seconds: Option<std::num::NonZeroU64>,
) -> Result<Option<i64>, Error> {
    if let Some(lifetime) = expires_in_seconds
        && lifetime <= 0
    {
        return Err(Error::Unavailable {
            detail: format!(
                "malformed token response: `expires_in` must be positive, got {lifetime}"
            ),
        });
    }

    if let Some(lifetime) = expires_in_seconds {
        return Ok(Some(unix_time::seconds_from_now(lifetime).ok_or_else(
            || Error::Unavailable {
                detail: format!("malformed token response: `expires_in` overflow, got {lifetime}"),
            },
        )?));
    }

    if let Some(access_token) = access_token
        && let Some(exp) = jwt_exp_claim(access_token)
    {
        let exp_micros =
            exp.checked_mul(unix_time::MICROS_PER_SEC)
                .ok_or_else(|| Error::Unavailable {
                    detail: format!(
                        "malformed token response: `exp` claim must be in the future, got {exp}"
                    ),
                })?;

        let now = unix_time::unix_now_micros();

        if exp_micros <= now {
            return Err(Error::Unavailable {
                detail: format!(
                    "malformed token response: `exp` claim must be in the future, got {exp}"
                ),
            });
        }

        return Ok(Some(exp_micros));
    }

    let Some(lifetime) =
        configured_lifetime_seconds.and_then(|lifetime| i64::try_from(lifetime.get()).ok())
    else {
        return Ok(None);
    };

    Ok(Some(unix_time::seconds_from_now(lifetime).ok_or_else(
        || Error::Unavailable {
            detail: format!(
                "malformed definition: `access_token_lifetime_seconds` overflow, got {lifetime}"
            ),
        },
    )?))
}

/// Reads the `exp` claim of an access token JWT, or `None` when the token is not a decodable JWT
/// or carries no `exp` claim. RFC 7519 §2 lets a NumericDate carry a fractional part, so any
/// JSON number is accepted and ceiled to the first whole second it still covers; a claim of any
/// other JSON kind reads as absent.
fn jwt_exp_claim(jwt: &str) -> Option<i64> {
    let claims = jwt_claims(jwt)?;

    let number = match claims.get("exp")? {
        serde_json::Value::Number(number) => number,
        _ => return None,
    };

    if let Some(seconds) = number.as_i64() {
        return Some(seconds);
    }

    // A non-integer NumericDate ceils to whole seconds. The `as` cast saturates, so a
    // magnitude beyond the clock stays "in the future", where the downstream `resolve_expiry`
    // overflow guard — not a silent acceptance — is what meets it.
    let seconds = number.as_f64()?;

    if !seconds.is_finite() {
        return None;
    }

    Some(seconds.ceil() as i64)
}

/// Decodes the JSON object a JWT payload carries, or `None` when the token is not a decodable
/// JWT.
#[must_use]
fn jwt_claims(jwt: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let payload = jwt.split('.').nth(1)?;

    serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// Copies the definition's identity-token claims into the working slots: a claim whose path
/// resolves into a string member of the payload names its slot's value, and any other claim is
/// skipped. Every claimed value passes the shared routing-safety validation before it lands —
/// the absorption is a storage boundary like the response-field folds (README §5.4) — so a
/// hostile identity token fails the stage instead of poisoning the persisted slots.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when a resolved claim value falls outside the routing-safe
/// text contract, naming the slot the claim targets.
pub(crate) fn apply_id_token_claims(
    id_token: &str,
    claims: &[IdTokenClaim],
    slots: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    let Some(payload) = jwt_claims(id_token) else {
        return Ok(());
    };

    let payload = serde_json::Value::Object(payload);

    for claim in claims {
        if let Some(text) = claim
            .claim
            .resolve(&payload)
            .and_then(serde_json::Value::as_str)
        {
            validate_slot_value(&claim.slot, text).map_err(|_| Error::Unavailable {
                detail: format!(
                    "malformed token response: id_token claim for slot `{}` is not routing-safe",
                    claim.slot
                ),
            })?;

            slots.insert(claim.slot.clone(), text.to_string());
        }
    }

    Ok(())
}

/// Folds one executed token stage's identity artifacts into the stored working slots: the
/// stored slots stay in place, the identity token's claims copy over, and the stage's own slot
/// targets extend the result (README §5.3). The completion pipeline and the refresh grant share
/// this fold.
///
/// # Errors
///
/// Returns [`Error`] when an identity-token claim value fails the routing-safety validation
/// (README §5.4) — the stage's own slot targets validated at the mapping boundary already.
pub(crate) fn absorb_exchange_slots(
    stored: &BTreeMap<String, String>,
    outcome: &ExchangeOutcome,
    claims: &[IdTokenClaim],
) -> Result<BTreeMap<String, String>, Error> {
    let mut slots = stored.clone();

    if let Some(id_token) = &outcome.id_token {
        apply_id_token_claims(id_token, claims, &mut slots)?;
    }

    slots.extend(outcome.slots.clone());

    Ok(slots)
}

/// Derives the PKCE S256 challenge of a verifier: Base64URL (unpadded) of SHA-256 over the
/// ASCII verifier bytes (RFC 7636 §4.2).
#[must_use]
pub(crate) fn pkce_challenge(verifier: &str) -> String {
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
    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return Err(Error::Internal {
            message: "the flow's definition carries no OAuth credential".to_string(),
        });
    };

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

        /// The new provider's public model name prefix.
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
    let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
        return Err(Error::Internal {
            message: "the flow's definition carries no OAuth credential".to_string(),
        });
    };

    let OAuthRecipe {
        client_id,
        client_secret,
        grant,
        ..
    } = &**recipe;

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

    // The per-grant begin persists the row and yields the one fact storage cannot recover: the
    // authorization-code grant's authorize URL. The device grant's verification facts exist
    // only as the row's stored columns, so no fact is computed twice.
    let (new_flow, authorize_url) = match grant {
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

            let expires_at = unix_time::unix_now_micros() + AUTHORIZATION_CODE_TTL;

            let new_flow = NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token,
                purpose,
                provider_name,
                model_prefix,
                base_url,
                target_provider_id,
                grant_type: FlowGrant::AuthorizationCode,
                slots,
                code_verifier,
                device_code: None,
                device_user_code: None,
                device_verification_uri: None,
                device_verification_uri_complete: None,
                device_poll_interval_seconds: None,
                device_next_poll_at: None,
                expires_at,
            };

            (new_flow, Some(authorize_url))
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

            let new_flow = NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token,
                purpose,
                provider_name,
                model_prefix,
                base_url,
                target_provider_id,
                grant_type: FlowGrant::Device,
                slots,
                code_verifier: None,
                device_code: Some(device.device_code),
                device_user_code: Some(device.user_code),
                device_verification_uri: device.verification_uri,
                device_verification_uri_complete: device.verification_uri_complete,
                device_poll_interval_seconds: Some(interval),
                device_next_poll_at: Some(unix_time::seconds_from_now(interval).ok_or_else(
                    || {
                        Error::Unavailable {
                            detail: "malformed device authorization response: `interval` overflow"
                                .to_string(),
                        }
                    },
                )?),
                expires_at,
            };

            (new_flow, None)
        }
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
        url.push_str(if url.contains('?') { "&" } else { "?" });
        url.push_str(name);
        url.push('=');
        url.push_str(&control::percent_encode_component(value));
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

/// The product of one token exchange (README §5.3).
#[derive(Debug, Clone, Default)]
pub(crate) struct ExchangeOutcome {
    /// The access token, when the response mapped it.
    pub(crate) access_token: Option<String>,

    /// The refresh token, when the response mapped it.
    pub(crate) refresh_token: Option<String>,

    /// The access token's lifetime in seconds, when the response reported it.
    pub(crate) expires_in_seconds: Option<i64>,

    /// The identity token, when the response mapped it.
    pub(crate) id_token: Option<String>,

    /// The upstream base URL the response supplied.
    pub(crate) base_url: Option<String>,

    /// The slots the response placed.
    pub(crate) slots: BTreeMap<String, String>,
}

/// How one executed exchange classifies, combining the transport outcome with the terminal-code
/// configuration (README §5.3 device grant).
pub(crate) enum ExchangeProduct {
    /// The exchange succeeded and its response was mapped.
    Completed(ExchangeOutcome),

    /// The upstream reported `authorization_pending`.
    Pending,

    /// The upstream reported `slow_down`: the flow's interval permanently grows.
    SlowDown,

    /// The upstream reported a configured terminal code: the flow fails.
    Terminal(String),

    /// The upstream reported a failure code outside the configuration: the stage fails without
    /// consuming the flow's schedule.
    Other(String),
}

/// The error code of one token-endpoint rejection body, read in every wire shape the researched
/// upstreams send: a JSON body per [`error_code_of_json`], else the form-encoded `error=` field
/// (RFC 6749 §5.2). A body carrying none yields `None`.
#[must_use]
pub(crate) fn token_error_code(body: &str) -> Option<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(code) = error_code_of_json(&json)
    {
        return Some(code);
    }

    control::parse_form(body).get("error").cloned()
}

/// The error code one parsed JSON body carries, in either wire shape the researched upstreams
/// send: the RFC 6749 §5.2 string form and the object form carrying the machine-readable name in
/// its `code` member. The payload is upstream-owned, so any other shape reads as no code.
fn error_code_of_json(json: &serde_json::Value) -> Option<String> {
    match json.get("error") {
        Some(serde_json::Value::String(code)) => Some(code.clone()),

        Some(serde_json::Value::Object(object)) => object
            .get("code")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),

        _ => None,
    }
}

/// Executes one token exchange against the definition's exchange request and classifies the
/// outcome (README §5.3).
pub(crate) async fn execute_exchange(
    http: &Client,
    exchange: &Exchange,
    env: &ValueEnv<'_>,
) -> Result<ExchangeProduct, Error> {
    let outcome = control::execute(
        http,
        &exchange.request,
        env,
        exchange.response.encoding,
        "token exchange",
    )
    .await;

    let decoded = match outcome {
        Ok(decoded) => decoded,

        Err(ControlFailure::Rejected { status, body }) => {
            return classify_exchange_rejection(status, &body, &exchange.terminal_codes);
        }

        Err(failure) => return Err(failure.into()),
    };

    // A 2xx body can still carry a failure name: GitHub's device token endpoint reports
    // `authorization_pending` with a success status.
    if let Some(code) = decoded_error_code(&decoded) {
        return Ok(classify_error_code(&code, &exchange.terminal_codes));
    }

    map_exchange_fields(&decoded, &exchange.response.fields, "token exchange")
        .map(ExchangeProduct::Completed)
}

/// Classifies a non-2xx token-endpoint rejection: a configured terminal code fails the flow, the
/// RFC polling codes classify as their schedule outcomes, and anything else is a generic
/// rejection carrying the status.
fn classify_exchange_rejection(
    status: u16,
    body: &str,
    terminal_codes: &[String],
) -> Result<ExchangeProduct, Error> {
    let Some(code) = token_error_code(body) else {
        return Err(Error::Rejected {
            status,
            detail: crate::http_body::truncate_excerpt(body),
        });
    };

    match classify_error_code(&code, terminal_codes) {
        ExchangeProduct::Other(code) => Err(Error::Rejected {
            status,
            detail: crate::http_body::truncate_excerpt(&format!("upstream error `{code}`"))
                .to_string(),
        }),

        classified => Ok(classified),
    }
}

/// Reads the failure code a decoded 2xx body carries, when any: the same closed set of shapes the
/// rejection bodies use, consulted on the decoded tree.
fn decoded_error_code(decoded: &DecodedBody) -> Option<String> {
    match decoded {
        DecodedBody::Json(json) => error_code_of_json(json),

        DecodedBody::Form(fields) => fields.get("error").cloned(),
    }
}

/// Classifies one upstream failure code against the definition's terminal codes: the RFC 8628
/// polling codes are engine-owned, the configured terminal codes end the flow, and anything else
/// is `Other`.
fn classify_error_code(code: &str, terminal_codes: &[String]) -> ExchangeProduct {
    match code {
        "authorization_pending" => return ExchangeProduct::Pending,
        "slow_down" => return ExchangeProduct::SlowDown,
        _ => {}
    }

    let is_terminal = terminal_codes
        .iter()
        .any(|terminal| terminal.eq_ignore_ascii_case(code));

    if is_terminal {
        ExchangeProduct::Terminal(code.to_string())
    } else {
        ExchangeProduct::Other(code.to_string())
    }
}

/// Maps one executed token stage's response fields into the outcome per the definition's
/// targets: the exchange and the refresh grant share the same fold, differing only in the
/// mapping slice and the stage label the failures carry. The fold is a storage boundary — the
/// machine-consumed targets (`base_url`, `slot:<name>`) validate through the shared contract
/// (README §7) before any value reaches it.
fn map_exchange_fields(
    decoded: &DecodedBody,
    fields: &[ResponseField],
    what: &str,
) -> Result<ExchangeOutcome, Error> {
    let mut outcome = ExchangeOutcome::default();

    control::map_response_fields(decoded, fields, what, |target, extracted| {
        apply_exchange_target(&mut outcome, target, extracted)
    })?;

    Ok(outcome)
}

/// Folds one extracted exchange value into the outcome per its target, validating the values
/// that must stay machine-consumed text (slots and base URLs) at the storage boundary (README
/// §7): a hostile token endpoint fails the stage instead of poisoning the persisted routing
/// state.
fn apply_exchange_target(
    outcome: &mut ExchangeOutcome,
    target: &Target,
    extracted: Extracted,
) -> Result<(), Error> {
    match target {
        Target::AccessToken => outcome.access_token = Some(extracted.as_text()),

        Target::RefreshToken => outcome.refresh_token = Some(extracted.as_text()),

        // A present lifetime is consumed here or not at all: a non-integer value is a malformed
        // token response (README §7.2), never a silent fall through to the weaker expiry
        // sources the consumer consults when the lifetime is absent.
        Target::ExpiresInSeconds => match extracted {
            Extracted::Integer(seconds) => outcome.expires_in_seconds = Some(seconds),

            Extracted::Text(text) => match text.parse::<i64>() {
                Ok(seconds) => outcome.expires_in_seconds = Some(seconds),

                Err(_) => {
                    return Err(Error::Unavailable {
                        detail: "token response carries a non-integer `expires_in`".to_string(),
                    });
                }
            },
        },

        Target::IdToken => outcome.id_token = Some(extracted.as_text()),

        Target::BaseUrl => {
            let value = extracted.as_text();

            validate_base_url_value(&value)?;

            outcome.base_url = Some(value);
        }

        Target::Slot { name } => {
            let value = extracted.as_text();

            validate_slot_value(name, &value)?;

            outcome.slots.insert(name.clone(), value);
        }

        // The write-time validation rejects a device-authorization or provisioning target in an
        // exchange mapping, so these arms are stored drift and fail fast.
        _ => {
            return Err(Error::Internal {
                message: "the exchange response maps a target its stage does not produce"
                    .to_string(),
            });
        }
    }

    Ok(())
}

/// Executes one refresh grant and maps its response through the carried descriptor — the
/// definition's refresh mapping, or the exchange mapping when the grant declares none.
///
/// A rejection whose failure code is in the grant's terminal list diagnoses the dead refresh
/// token — the grant is revoked or expired and the provider needs re-authorization — while any
/// other rejection keeps the generic upstream rendering with the status embedded.
///
/// # Errors
///
/// Returns [`Error`] for transport failures, rejections, and malformed response bodies.
pub(crate) async fn execute_refresh_grant(
    http: &Client,
    request: &crate::provider::definition::template::ControlRequest,
    env: &ValueEnv<'_>,
    response: &crate::provider::definition::template::ResponseDescriptor,
    terminal_codes: &[String],
) -> Result<ExchangeOutcome, Error> {
    let outcome = control::execute(http, request, env, response.encoding, "token refresh").await;

    let decoded = match outcome {
        Ok(decoded) => decoded,

        Err(ControlFailure::Rejected { status, body }) => {
            if let Some(code) = token_error_code(&body)
                && terminal_codes
                    .iter()
                    .any(|terminal| terminal.eq_ignore_ascii_case(&code))
            {
                return Err(Error::Unavailable {
                    detail: format!(
                        "the token endpoint rejected the refresh token with `{code}`: the grant \
                         is revoked or expired and the provider needs re-authorization"
                    ),
                });
            }

            return Err(Error::Rejected {
                status,
                detail: crate::http_body::truncate_excerpt(&body),
            });
        }

        Err(failure) => return Err(failure.into()),
    };

    map_exchange_fields(&decoded, &response.fields, "token refresh")
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]
    #![expect(
        clippy::panic,
        reason = "a fixture landing on the wrong side of its check is the failure report"
    )]

    use super::{
        AUTHORIZATION_CODE_TTL, BeginFacts, BeginPurpose, ExchangeOutcome, IdTokenClaim, Target,
        absorb_exchange_slots, apply_exchange_target, begin_flow, jwt_exp_claim, token_error_code,
    };
    use crate::{
        format::UpstreamFormat,
        provider::{
            control::Extracted,
            definition::{
                credential::{CredentialAcquisition, Grant},
                template::ResponsePath,
            },
        },
        storage::provider_definitions::{self, NewDefinition},
        test_support::{
            self, MockUpstream, OAuthDefinitionFixture, OAuthGrantFixture, OAuthRefreshFixture,
        },
    };
    use base64::{Engine as _, engine::general_purpose};
    use serde_json::json;
    use std::collections::BTreeMap;

    /// Pins the token-endpoint error-code reader across every wire shape the researched upstreams
    /// send: the RFC 6749 §5.2 string form, the object form carrying the code in `error.code`,
    /// and the form-encoded `error=` field. A sibling member of a non-string JSON kind is
    /// upstream-owned payload the reader does not model and must never blind the read of the
    /// `error` member, and a body carrying no recognizable shape reads as no code.
    ///
    /// # Panics
    ///
    /// Panics when a body reads as anything but its pinned code.
    #[test]
    fn token_error_codes_read_in_every_wire_shape() {
        assert_eq!(
            token_error_code(r#"{"error": "invalid_grant"}"#),
            Some("invalid_grant".to_string())
        );

        assert_eq!(
            token_error_code(r#"{"error": {"code": "token_expired"}}"#),
            Some("token_expired".to_string())
        );

        assert_eq!(
            token_error_code("error=invalid_grant"),
            Some("invalid_grant".to_string())
        );

        assert_eq!(
            token_error_code(r#"{"error": "invalid_grant", "expires_in": 3600}"#),
            Some("invalid_grant".to_string()),
            "a non-string sibling member must not blind the `error` read"
        );

        assert_eq!(token_error_code("neither code nor shape"), None);
    }

    /// Folds one text value into a fresh outcome through the given target, returning the fold's
    /// result so a test can pin its failure side.
    fn fold_target(target: &Target, value: &str) -> Result<(), super::Error> {
        let mut outcome = ExchangeOutcome::default();

        apply_exchange_target(&mut outcome, target, Extracted::Text(value.to_string()))
    }

    /// The exchange fold is a storage boundary (README §7): a mapped `base_url` must carry the
    /// non-empty `http(s)://` scheme and mapped slot text must be routing-safe machine-consumed
    /// text, so a hostile token endpoint fails the stage instead of poisoning the persisted
    /// routing state; valid values fold in verbatim.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[test]
    fn the_exchange_fold_validates_base_url_and_slot_targets() {
        let Err(base_url_error) = fold_target(&Target::BaseUrl, "upstream.example/v1") else {
            panic!("a scheme-less base_url must fail the fold");
        };

        assert!(
            base_url_error.to_string().contains("base_url"),
            "{base_url_error}"
        );

        let Err(_) = fold_target(&Target::BaseUrl, "ftp://upstream.example") else {
            panic!("a non-http base_url scheme must fail the fold");
        };

        let mut outcome = ExchangeOutcome::default();

        apply_exchange_target(
            &mut outcome,
            &Target::BaseUrl,
            Extracted::Text("https://upstream.example/v1".to_string()),
        )
        .unwrap();

        assert_eq!(
            outcome.base_url.as_deref(),
            Some("https://upstream.example/v1"),
            "a valid base_url stores"
        );

        let Err(slot_error) = fold_target(
            &Target::Slot {
                name: "stage".to_string(),
            },
            "has space",
        ) else {
            panic!("slot text with whitespace must fail the fold");
        };

        assert!(slot_error.to_string().contains("stage"), "{slot_error}");

        let Err(_) = fold_target(
            &Target::Slot {
                name: "stage".to_string(),
            },
            "日本語",
        ) else {
            panic!("non-ASCII slot text must fail the fold");
        };

        let mut outcome = ExchangeOutcome::default();

        apply_exchange_target(
            &mut outcome,
            &Target::Slot {
                name: "stage".to_string(),
            },
            Extracted::Text("clean-value".to_string()),
        )
        .unwrap();

        assert_eq!(
            outcome.slots.get("stage").map(String::as_str),
            Some("clean-value"),
            "valid slot text stores"
        );
    }

    /// Builds one JWT whose payload carries exactly the given claims.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to serialize — a broken fixture is the failure report.
    fn jwt_with_payload(payload: serde_json::Value) -> String {
        #[expect(clippy::unwrap_used, reason = "a JSON object always serializes")]
        let payload = serde_json::to_vec(&payload).unwrap();

        format!(
            "header.{}.signature",
            general_purpose::URL_SAFE_NO_PAD.encode(payload)
        )
    }

    /// Builds one JWT whose payload carries only the `exp` claim.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to serialize — a broken fixture is the failure report.
    fn jwt_with_exp(exp: serde_json::Value) -> String {
        jwt_with_payload(serde_json::json!({ "exp": exp }))
    }

    /// The identity-token claim absorption is the same storage boundary as the exchange fold
    /// (README §5.4): a claimed value outside routing-safe machine-consumed text — whitespace,
    /// control characters, non-ASCII — fails the fold naming its slot instead of poisoning the
    /// persisted slots, and a valid claim lands in its slot verbatim.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[test]
    fn the_id_token_claim_absorption_validates_routing_safety() {
        let jwt = jwt_with_payload(serde_json::json!({
            "project": "has space",
            "org": "日本語",
            "team": "tab\tstopped",
            "account": "clean-value",
        }));

        let outcome = ExchangeOutcome {
            id_token: Some(jwt.clone()),
            ..ExchangeOutcome::default()
        };

        let claimed = |member: &str, slot: &str| IdTokenClaim {
            claim: ResponsePath(vec![member.to_string()]),
            slot: slot.to_string(),
        };

        for (member, slot) in [("project", "project"), ("org", "org"), ("team", "team")] {
            let Err(error) =
                absorb_exchange_slots(&BTreeMap::new(), &outcome, &[claimed(member, slot)])
            else {
                panic!("a non-routing-safe id_token claim must fail the fold (slot `{slot}`)");
            };

            assert!(error.to_string().contains(slot), "{error}");
        }

        let slots =
            absorb_exchange_slots(&BTreeMap::new(), &outcome, &[claimed("account", "account")])
                .unwrap();

        assert_eq!(
            slots.get("account").map(String::as_str),
            Some("clean-value"),
            "a valid id_token claim stores"
        );
    }

    /// RFC 7519 §2 lets a NumericDate carry a fractional part: the claim ceils to the first
    /// whole second it still covers, so the non-integer spelling decodes exactly like the
    /// integer one, while a claim of another JSON kind reads as absent.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[test]
    fn jwt_exp_claims_read_every_numeric_date_spelling() {
        assert_eq!(
            jwt_exp_claim(&jwt_with_exp(serde_json::json!(1_750_000_000))),
            Some(1_750_000_000)
        );

        assert_eq!(
            jwt_exp_claim(&jwt_with_exp(serde_json::json!(1_750_000_000.5))),
            Some(1_750_000_001)
        );

        assert_eq!(
            jwt_exp_claim(&jwt_with_exp(serde_json::json!(1_750_000_000.25))),
            Some(1_750_000_001)
        );

        assert_eq!(
            jwt_exp_claim(&jwt_with_exp(serde_json::json!(-0.5))),
            Some(0)
        );

        assert_eq!(
            jwt_exp_claim(&jwt_with_exp(serde_json::json!("soon"))),
            None
        );
    }

    /// The device grant one begin test swaps into the shared OAuth skeleton: the authorization
    /// request posts to the given base and maps the required codes plus the verification URI,
    /// reporting neither `expires_in` nor `interval` — so the begin resolves the documented
    /// fallbacks, the definition's expiry fallback and the five-second RFC 8628 default.
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
                    "url": [{"type": "literal", "value": format!("{base}/device/code")}]
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
                provider_name: "created".to_string(),
                model_prefix: String::new(),
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
        assert_eq!(stored.model_prefix.as_deref(), Some(""));
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
