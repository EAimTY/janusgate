//! The OAuth protocol engine: flow begin (both grants), token exchange with terminal-code
//! classification, identity-claim extraction, and expiry resolution (README §5.3).
//!
//! Every provider-specific fact — endpoints, client credentials, parameter names, header sets —
//! lives in the definition the flow references; this engine carries the grant mechanics alone.
//! The control-request executor runs each stage's request, and the stage's response descriptor
//! maps its product.

use super::Error;
use crate::{
    provider::{
        control::{self, ControlFailure, DecodedBody, Extracted},
        definition::{
            credential::{Exchange, Grant, Pkce},
            eval::ValueEnv,
            template::ResponseTarget as Target,
        },
    },
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use reqwest::Client;
use serde::Deserialize;
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
        let micros = lifetime
            .checked_mul(unix_time::MICROS_PER_SEC)
            .ok_or_else(|| Error::Unavailable {
                detail: format!("malformed token response: `expires_in` overflow, got {lifetime}"),
            })?;

        return Ok(Some(
            unix_time::unix_now_micros()
                .checked_add(micros)
                .ok_or_else(|| Error::Unavailable {
                    detail: format!(
                        "malformed token response: `expires_in` overflow, got {lifetime}"
                    ),
                })?,
        ));
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

    let micros = lifetime
        .checked_mul(unix_time::MICROS_PER_SEC)
        .ok_or_else(|| Error::Unavailable {
            detail: format!(
                "malformed definition: `access_token_lifetime_seconds` overflow, got {lifetime}"
            ),
        })?;

    Ok(Some(
        unix_time::unix_now_micros()
            .checked_add(micros)
            .ok_or_else(|| Error::Unavailable {
                detail: format!(
                    "malformed definition: `access_token_lifetime_seconds` overflow, got {lifetime}"
                ),
            })?,
    ))
}

/// Reads the `exp` claim of an access token JWT, or `None` when the token is not a decodable JWT
/// or carries no `exp` claim.
fn jwt_exp_claim(jwt: &str) -> Option<i64> {
    jwt_claims(jwt)?.get("exp")?.as_i64()
}

/// Decodes the JSON object a JWT payload carries, or `None` when the token is not a decodable
/// JWT.
#[must_use]
pub(crate) fn jwt_claims(jwt: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let payload = jwt.split('.').nth(1)?;

    serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// Copies the definition's identity-token claims into the working slots.
pub(crate) fn apply_id_token_claims(
    id_token: &str,
    claims: &[crate::provider::definition::credential::IdTokenClaim],
    slots: &mut BTreeMap<String, String>,
) {
    let Some(payload) = jwt_claims(id_token) else {
        return;
    };

    for claim in claims {
        let mut value = serde_json::Value::Object(payload.clone());

        let mut resolved = true;

        for member in &claim.claim.0 {
            match value.get(member) {
                Some(found) => value = found.clone(),
                None => {
                    resolved = false;
                    break;
                }
            }
        }

        if resolved && let Some(text) = value.as_str() {
            slots.insert(claim.slot.clone(), text.to_string());
        }
    }
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

/// Percent-encodes one authorize-URL query component: every byte outside the RFC 3986 unreserved
/// set renders as `%XX`, so scopes render their spaces as `%20` exactly like the mirrored CLIs.
#[must_use]
pub(crate) fn authorize_url_encode(component: &str) -> String {
    let mut encoded = String::with_capacity(component.len());

    for byte in component.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }

            other => {
                encoded.push_str(&format!("%{other:02X}"));
            }
        }
    }

    encoded
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
        url.push_str(&authorize_url_encode(value));
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
                control::extract_field(&body, &field.path, &FIELD_SHAPE, "device authorization")
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

    let expires_in_seconds = match optional(find(&Target::ExpiresInSeconds))? {
        Some(text) => Some(text.parse::<i64>().map_err(|_| Error::Unavailable {
            detail: "device authorization response carries a non-integer `expires_in`".to_string(),
        })?),
        None => None,
    };

    let interval_seconds = match optional(find(&Target::IntervalSeconds))? {
        Some(text) => Some(text.parse::<i64>().map_err(|_| Error::Unavailable {
            detail: "device authorization response carries a non-integer `interval`".to_string(),
        })?),
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

/// The extraction shape of an exchange or device field: every such value is read as text, with
/// the lifetime targets decoded by their consumers.
const FIELD_SHAPE: crate::provider::definition::template::ExtractionShape =
    crate::provider::definition::template::ExtractionShape::String;

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
/// upstreams send: the RFC 6749 §5.2 string form, the object form carrying the code in a `code`
/// member, and the form-encoded `error=` field. A body carrying none yields `None`.
#[must_use]
pub(crate) fn token_error_code(body: &str) -> Option<String> {
    if let Ok(decoded) = serde_json::from_str::<TokenErrorResponse>(body)
        && let Some(code) = decoded.error_code()
    {
        return Some(code);
    }

    if let Ok(fields) = serde_json::from_str::<BTreeMap<String, String>>(body)
        && let Some(code) = fields.get("error")
    {
        return Some(code.clone());
    }

    let form = control::parse_form(body);

    form.get("error").cloned()
}

/// The rejection body's error field in either JSON wire shape.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    /// Machine-readable failure name, in either wire shape.
    error: Option<TokenErrorName>,
}

impl TokenErrorResponse {
    /// The failure name this body carries.
    fn error_code(&self) -> Option<String> {
        match &self.error {
            Some(TokenErrorName::Name(code)) => Some(code.clone()),
            Some(TokenErrorName::Object(object)) => object.code.clone(),
            None => None,
        }
    }
}

/// One token-endpoint failure name in either wire shape it arrives in.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TokenErrorName {
    /// RFC 6749 §5.2 string form, such as `invalid_grant`.
    Name(String),

    /// Object form, carrying the failure in its `code` field.
    Object(TokenErrorObject),
}

/// The object-form failure: the machine-readable failure is its `code` field. The payload is
/// upstream-owned, so only the code is modeled and unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct TokenErrorObject {
    /// Machine-readable failure code, such as `token_expired`.
    code: Option<String>,
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

    map_exchange_fields(&decoded, exchange)
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
            detail: crate::provider::http_body::truncate_excerpt(body),
        });
    };

    match classify_error_code(&code, terminal_codes) {
        ExchangeProduct::Other(code) => Err(Error::Rejected {
            status,
            detail: crate::provider::http_body::truncate_excerpt(&format!(
                "upstream error `{code}`"
            ))
            .to_string(),
        }),

        classified => Ok(classified),
    }
}

/// Reads the failure code a decoded 2xx body carries, when any: the same closed set of shapes the
/// rejection bodies use, consulted on the decoded tree.
fn decoded_error_code(decoded: &DecodedBody) -> Option<String> {
    match decoded {
        DecodedBody::Json(json) => match json.get("error") {
            Some(serde_json::Value::String(code)) => Some(code.clone()),
            Some(serde_json::Value::Object(object)) => object
                .get("code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        },

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

/// Maps the exchange response's fields into the outcome per the definition's targets.
fn map_exchange_fields(
    decoded: &DecodedBody,
    exchange: &Exchange,
) -> Result<ExchangeProduct, Error> {
    let mut outcome = ExchangeOutcome::default();

    for field in &exchange.response.fields {
        let extracted =
            control::extract_field(decoded, &field.path, &FIELD_SHAPE, "token exchange")?;

        let Some(extracted) = extracted else {
            if field.required {
                return Err(Error::Unavailable {
                    detail: "token exchange response lacks a required field".to_string(),
                });
            }

            continue;
        };

        apply_exchange_target(&mut outcome, &field.target, extracted)?;
    }

    Ok(ExchangeProduct::Completed(outcome))
}

/// Folds one extracted exchange value into the outcome per its target.
fn apply_exchange_target(
    outcome: &mut ExchangeOutcome,
    target: &Target,
    extracted: Extracted,
) -> Result<(), Error> {
    match target {
        Target::AccessToken => outcome.access_token = Some(extracted.as_text()),

        Target::RefreshToken => outcome.refresh_token = Some(extracted.as_text()),

        Target::ExpiresInSeconds => {
            outcome.expires_in_seconds = extracted
                .as_integer()
                .or_else(|| extracted.as_text().parse::<i64>().ok());
        }

        Target::IdToken => outcome.id_token = Some(extracted.as_text()),

        Target::BaseUrl => outcome.base_url = Some(extracted.as_text()),

        Target::Slot { name } => {
            outcome.slots.insert(name.clone(), extracted.as_text());
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
                detail: crate::provider::http_body::truncate_excerpt(&body),
            });
        }

        Err(failure) => return Err(failure.into()),
    };

    let mut mapped = ExchangeOutcome::default();

    for field in &response.fields {
        let extracted =
            control::extract_field(&decoded, &field.path, &FIELD_SHAPE, "token refresh").map_err(
                |failure| Error::Unavailable {
                    detail: failure.to_string(),
                },
            )?;

        let Some(extracted) = extracted else {
            if field.required {
                return Err(Error::Unavailable {
                    detail: "token refresh response lacks a required field".to_string(),
                });
            }

            continue;
        };

        apply_exchange_target(&mut mapped, &field.target, extracted)?;
    }

    Ok(mapped)
}
