//! The OAuth protocol engine: flow begin (both grants), token exchange with terminal-code
//! classification, identity-claim extraction, and expiry resolution (README §5.3).
//!
//! Every provider-specific fact — endpoints, client credentials, parameter names, header sets —
//! lives in the definition the flow references; this engine carries the grant mechanics alone.
//! The control-request executor runs each stage's request, and the stage's response descriptor
//! maps its product. The exchange and refresh-grant fold applies the shared machine-consumed
//! text validators (README §7) before any slot or base-URL value reaches storage, and the
//! identity-token claim absorption applies the slot validator before a claimed value lands.

use super::{
    Error,
    provisioning::{validate_base_url_value, validate_slot_value},
};
use crate::{
    provider::{
        control::{self, ControlFailure, DecodedBody, Extracted},
        definition::{
            credential::{
                CredentialAcquisition, Exchange, Grant, IdTokenClaim, Pkce, SlotGenerator,
            },
            eval::ValueEnv,
            template::{ResponseField, ResponseTarget as Target},
        },
    },
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use reqwest::Client;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

/// Lifetime of a pending authorization-code flow, in database-clock microseconds.
pub(crate) const AUTHORIZATION_CODE_TTL: i64 = 600 * unix_time::MICROS_PER_SEC;

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
        return Ok(Some(seconds_from_now(lifetime, || Error::Unavailable {
            detail: format!("malformed token response: `expires_in` overflow, got {lifetime}"),
        })?));
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

    Ok(Some(seconds_from_now(lifetime, || Error::Unavailable {
        detail: format!(
            "malformed definition: `access_token_lifetime_seconds` overflow, got {lifetime}"
        ),
    })?))
}

/// Scales one upstream-reported duration in seconds to the absolute instant that many seconds
/// from the current database clock, mapping an overflow to the caller's upstream-unavailable
/// failure: the magnitude originates in an upstream response, so one this large is a malformed
/// response — the same guard [`resolve_expiry`] applies to the token path's expiry.
///
/// # Errors
///
/// Returns the `overflow` failure when the scaled instant overflows the database clock.
pub(crate) fn seconds_from_now(seconds: i64, overflow: impl Fn() -> Error) -> Result<i64, Error> {
    let micros = seconds
        .checked_mul(unix_time::MICROS_PER_SEC)
        .ok_or_else(&overflow)?;

    unix_time::unix_now_micros()
        .checked_add(micros)
        .ok_or_else(overflow)
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
pub(crate) fn random_url_safe(byte_count: usize) -> String {
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

/// Builds the authorize URL of an authorization-code grant: the evaluated base URL, the standard
/// parameters the builder appends itself (`response_type`, `client_id`, `redirect_uri`,
/// `state`, and the PKCE pair when configured), and the definition's extra parameters in order.
pub(crate) fn build_authorize_url(
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
        ExchangeOutcome, IdTokenClaim, Target, absorb_exchange_slots, apply_exchange_target,
        jwt_exp_claim, token_error_code,
    };
    use crate::provider::{control::Extracted, definition::template::ResponsePath};
    use base64::{Engine as _, engine::general_purpose};
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
}
