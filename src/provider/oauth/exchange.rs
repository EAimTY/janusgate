//! The OAuth token mechanics: the token exchange with terminal-code classification, the refresh
//! grant, the token-response expiry resolution, and the token-data folding into the working
//! slots (`docs/design/providers.md` and `docs/reference/management-api.md`).
//!
//! Every provider-specific fact — endpoints, client credentials, parameter names, header sets —
//! lives in the definition the flow references; this engine carries the grant mechanics alone.
//! The control-request executor runs each stage's request, and the stage's response descriptor
//! maps its product. The exchange and refresh-grant fold applies the shared machine-consumed
//! text validators (`docs/reference/management-api.md`) before any token, slot, or base-URL value
//! reaches storage, and the identity-token claim absorption applies the slot validator before a
//! claimed value lands. Those validators live in this module — the layer's substrate every stage
//! above imports from — so one contract guards one storage boundary whichever stage extracted the
//! value.

use crate::{
    provider::{
        Error,
        control::{self, ControlFailure, DecodedBody, Extracted},
        definition::{
            credential::{Exchange, IdTokenClaim},
            eval::ValueEnv,
            template::{ResponseField, ResponseTarget as Target},
        },
    },
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// The polling interval a device authorization response implies when it reports none: five
/// seconds per RFC 8628 §3.2.
pub(super) const DEFAULT_DEVICE_INTERVAL_SECONDS: i64 = 5;

/// The permanent interval increase a `slow_down` response applies, in seconds, per RFC 8628
/// §3.5.
pub(super) const SLOW_DOWN_PENALTY_SECONDS: i64 = 5;

/// The expiry of a token response, resolved per `docs/design/providers.md`: a positive reported
/// `expires_in`, else a future `exp` claim of the access token, else the definition's explicitly
/// configured lifetime, else no expiry at all. A reported non-positive `expires_in` or a non-future
/// `exp` claim is a malformed token response, never silently treated as missing, and a configured
/// lifetime beyond the database clock's range is a malformed definition, never silently
/// treated as absent.
pub(super) fn resolve_expiry(
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
        // The overflow arm is its own failure class: a magnitude this large may be far in the
        // future — it merely cannot scale to the database clock — so it never renders as a
        // past-expiry violation. The value is omitted because the saturating claim read
        // (`jwt_exp_claim`) cannot report the pre-saturation magnitude truthfully.
        let exp_micros =
            exp.checked_mul(unix_time::MICROS_PER_SEC)
                .ok_or_else(|| Error::Unavailable {
                    detail: "malformed token response: `exp` claim overflow".to_string(),
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

    let Some(lifetime) = configured_lifetime_seconds else {
        return Ok(None);
    };

    // A configured lifetime beyond the `i64` range the database clock scales it into is stored
    // drift: it fails as a malformed definition instead of silently reading as absent and
    // resolving no expiry at all.
    let lifetime = i64::try_from(lifetime.get()).map_err(|_| Error::Unavailable {
        detail: format!(
            "malformed definition: `access_token_lifetime_seconds` is out of range, got {}",
            lifetime.get()
        ),
    })?;

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
    /// The only access-token claim the expiry contract consumes.
    #[derive(Serialize, Deserialize)]
    struct ExpirationClaims {
        /// RFC 7519 NumericDate; a non-numeric value makes this claim unusable.
        exp: Option<serde_json::Number>,
    }
    let payload = jwt.split('.').nth(1)?;
    let bytes = general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: ExpirationClaims = serde_json::from_slice(&bytes).ok()?;
    let number = claims.exp?;

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
/// the absorption is a storage boundary like the response-field folds (`docs/design/providers.md`)
/// — so a hostile identity token fails the stage instead of poisoning the persisted slots.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when a resolved claim value falls outside the routing-safe
/// text contract, naming the slot the claim targets.
fn apply_id_token_claims(
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
/// targets extend the result (`docs/design/providers.md`). The completion pipeline and the refresh
/// grant share this fold.
///
/// # Errors
///
/// Returns [`Error`] when an identity-token claim value fails the routing-safety validation
/// (`docs/design/providers.md`) — the stage's own slot targets validated at the mapping boundary
/// already.
pub(super) fn absorb_exchange_slots(
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

/// The product of one token exchange (`docs/design/providers.md`).
///
/// The outcome serializes: the device poller persists a completed exchange as the flow row's
/// checkpoint the moment it completes, so a restart that recovers the claim resumes from the
/// checkpoint instead of re-presenting the consumed device code (`docs/design/storage.md` and
/// `docs/reference/management-api.md`). The values it carries are already validated — the mapping
/// fold validated the stored tokens, the machine-consumed slots, and the base URL at the storage
/// boundary — and only the gateway writes the checkpoint, so deserialization re-imposes no
/// validation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExchangeOutcome {
    /// The access token, when the response mapped it.
    pub(super) access_token: Option<String>,

    /// The refresh token, when the response mapped it.
    pub(super) refresh_token: Option<String>,

    /// The access token's lifetime in seconds, when the response reported it.
    pub(super) expires_in_seconds: Option<i64>,

    /// The identity token, when the response mapped it.
    id_token: Option<String>,

    /// The upstream base URL the response supplied.
    pub(super) base_url: Option<String>,

    /// The slots the response placed.
    slots: BTreeMap<String, String>,
}

/// How one executed exchange classifies, combining the transport outcome with the terminal-code
/// configuration (`docs/design/providers.md` device grant).
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
fn token_error_code(body: &str) -> Option<String> {
    if let Ok(response) = serde_json::from_str::<TokenErrorResponse>(body)
        && let Some(error) = response.error
    {
        return Some(error.code());
    }
    control::parse_form(body).get("error").cloned()
}

/// The fixed token-endpoint rejection envelope; unrelated fields are ignored.
#[derive(Deserialize)]
struct TokenErrorResponse {
    /// RFC string form or the provider object carrying its machine code.
    error: Option<TokenError>,
}

/// Supported token-endpoint error spellings; other forms establish no machine code.
#[derive(Deserialize)]
#[serde(untagged)]
enum TokenError {
    /// RFC 6749 string error code.
    Text(String),
    /// Provider object form carrying a string code.
    Object {
        /// Exact machine-readable rejection code.
        code: String,
    },
}

impl TokenError {
    /// Extracts the machine code from either supported representation.
    fn code(self) -> String {
        match self {
            Self::Text(code) | Self::Object { code } => code,
        }
    }
}

/// Probes an already decoded arbitrary control response through the fixed error vocabulary.
fn error_code_of_json(json: &serde_json::Value) -> Option<String> {
    TokenErrorResponse::deserialize(json)
        .ok()?
        .error
        .map(TokenError::code)
}

/// Why one executed token exchange failed, split by what a retry can and cannot survive
/// (`docs/design/storage.md` and `docs/reference/management-api.md`): a fold failure after the
/// upstream's decoded 2xx response is a deterministic response-shape violation a retry cannot fix —
/// the single-use device code is consumed upstream whether or not the gateway could use the
/// response — while every other failure happened before any fold and is one the poll schedule
/// absorbs.
#[derive(Debug)]
pub(super) enum ExchangeFailure {
    /// A 2xx response decoded but folding its mapped fields failed: a mapped slot outside the
    /// storage boundary, a non-integer `expires_in`, a missing required field, or a mapping
    /// target the stage does not produce. Retrying cannot fix the response's shape.
    Consumed(Error),

    /// Transport, rejection, and decode failures: the request never produced a decoded 2xx
    /// body, so the fold never ran.
    Retryable(Error),
}

impl From<ExchangeFailure> for Error {
    fn from(failure: ExchangeFailure) -> Self {
        match failure {
            ExchangeFailure::Consumed(error) | ExchangeFailure::Retryable(error) => error,
        }
    }
}

/// Executes one token exchange against the definition's exchange request and classifies the
/// outcome (`docs/design/providers.md`). The synchronous surfaces read the failure as [`Error`]
/// alone; the device poller consumes [`execute_classified_exchange`] to distinguish a consumed
/// grant from a retryable attempt.
///
/// # Errors
///
/// Returns [`Error`] for transport failures, rejections, malformed response bodies, and failed
/// response folds.
pub(crate) async fn execute_exchange(
    http: &Client,
    exchange: &Exchange,
    env: &ValueEnv<'_>,
) -> Result<ExchangeProduct, Error> {
    execute_classified_exchange(http, exchange, env)
        .await
        .map_err(Error::from)
}

/// Executes one token exchange and classifies both the outcome and the failure's retry safety
/// (`docs/design/providers.md` and `docs/reference/management-api.md`): the device poller's resume
/// contract persists a terminal gateway-owned failure for [`ExchangeFailure::Consumed`] — the
/// consumed grant a retry could only re-present — and keeps the retry schedule for
/// [`ExchangeFailure::Retryable`].
///
/// # Errors
///
/// Returns [`ExchangeFailure`] for every failure class [`execute_exchange`] reports, typed by
/// whether the fold already saw a decoded 2xx body.
pub(super) async fn execute_classified_exchange(
    http: &Client,
    exchange: &Exchange,
    env: &ValueEnv<'_>,
) -> Result<ExchangeProduct, ExchangeFailure> {
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
            return classify_exchange_rejection(status, &body, &exchange.terminal_codes)
                .map_err(ExchangeFailure::Retryable);
        }

        Err(failure) => return Err(ExchangeFailure::Retryable(failure.into())),
    };

    // A 2xx body can still carry a failure name: GitHub's device token endpoint reports
    // `authorization_pending` with a success status.
    if let Some(code) = decoded_error_code(&decoded) {
        return Ok(classify_error_code(&code, &exchange.terminal_codes));
    }

    map_exchange_fields(&decoded, &exchange.response.fields, "token exchange")
        .map(ExchangeProduct::Completed)
        .map_err(ExchangeFailure::Consumed)
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
            detail: crate::http_body::truncate_excerpt(&format!("upstream error `{code}`")),
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

/// Whether one upstream failure code is one of the definition's configured terminal codes,
/// matched case-insensitively (`docs/design/providers.md`): a mis-cased configured code must still
/// classify as its configured disposition.
fn is_terminal_code(code: &str, terminal_codes: &[String]) -> bool {
    terminal_codes
        .iter()
        .any(|terminal| terminal.eq_ignore_ascii_case(code))
}

/// Classifies one upstream failure code against the definition's terminal codes: the RFC 8628
/// polling codes are engine-owned and match case-insensitively like the configured terminal
/// codes (`docs/design/providers.md`) — a mis-cased `slow_down` must still apply the slow-down
/// backoff — the configured terminal codes end the flow, and anything else is `Other` carrying the
/// code's original text.
fn classify_error_code(code: &str, terminal_codes: &[String]) -> ExchangeProduct {
    match code.to_ascii_lowercase().as_str() {
        "authorization_pending" => return ExchangeProduct::Pending,
        "slow_down" => return ExchangeProduct::SlowDown,
        _ => {}
    }

    if is_terminal_code(code, terminal_codes) {
        ExchangeProduct::Terminal(code.to_string())
    } else {
        ExchangeProduct::Other(code.to_string())
    }
}

/// Maps one executed token stage's response fields into the outcome per the definition's
/// targets: the exchange and the refresh grant share the same fold, differing only in the
/// mapping slice and the stage label the failures carry. The fold is a storage boundary — the
/// machine-consumed targets (`access_token`, `refresh_token`, `base_url`, `slot:<name>`)
/// validate through the shared contract (`docs/reference/management-api.md`) before any value
/// reaches it.
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
/// that must stay machine-consumed text (the stored tokens, slots, and base URLs) at the
/// storage boundary (`docs/reference/management-api.md`): a hostile token endpoint fails the stage
/// instead of poisoning the persisted routing state.
fn apply_exchange_target(
    outcome: &mut ExchangeOutcome,
    target: &Target,
    extracted: Extracted,
) -> Result<(), Error> {
    match target {
        Target::AccessToken => {
            let value = extracted.as_text();

            validate_token_value("access_token", &value)?;

            outcome.access_token = Some(value);
        }

        Target::RefreshToken => {
            let value = extracted.as_text();

            validate_token_value("refresh_token", &value)?;

            outcome.refresh_token = Some(value);
        }

        // A present lifetime is consumed here or not at all: a non-integer value is a malformed
        // token response (`docs/reference/management-api.md`), never a silent fall through to the
        // weaker expiry sources the consumer consults when the lifetime is absent.
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

/// Validates one stored slot value against the shared routing-safe text contract
/// (`docs/reference/management-api.md`): non-empty printable ASCII without whitespace. An upstream
/// value outside the set fails the stage instead of storing an unusable value. Shared by every
/// OAuth response fold — the exchange, refresh-grant, and identity-token folds here and the
/// provisioning fold in [`super::provisioning`] — so one contract guards one storage boundary.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when the value carries a character outside the allowed set or
/// is empty.
pub(super) fn validate_slot_value(name: &str, value: &str) -> Result<(), Error> {
    if crate::routing_text::is_routing_safe_text(value) {
        Ok(())
    } else {
        Err(Error::Unavailable {
            detail: format!(
                "the upstream reported a `{name}` value outside printable ASCII without whitespace"
            ),
        })
    }
}

/// Validates one stored base-URL value against the shared base-URL contract of
/// [`crate::routing_text::is_base_url`] (`docs/reference/management-api.md`): routing-safe text
/// and a non-empty `http://` or `https://` scheme. Management applies the same contract to
/// operator-supplied URLs. Shared by every OAuth response fold — the exchange and refresh-grant
/// folds here and the provisioning fold in [`super::provisioning`] — so one contract guards one
/// storage boundary.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when the value falls outside the shared base-URL contract.
pub(super) fn validate_base_url_value(value: &str) -> Result<(), Error> {
    if crate::routing_text::is_base_url(value) {
        Ok(())
    } else {
        Err(Error::Unavailable {
            detail: "the upstream reported a `base_url` value outside printable ASCII without \
                     whitespace or without an `http(s)://` scheme"
                .to_string(),
        })
    }
}

/// Validates one stored token value — the access or refresh token — against the HTTP
/// header-value byte contract (`docs/design/providers.md`): the header is the strictest destination
/// either token may face, and the access token's concrete instance is the `Authorization` header
/// every dispatch renders it into (the provisioning stage's `secret` references alongside).
/// The refresh token is never rendered into an `Authorization` header — shipped presets send
/// it only in the refresh-grant request body, and only the refresh-grant stage may reference
/// it in headers. A control byte — a pasted trailing newline foremost — would therefore fail the
/// requests carrying the value at runtime as a transport failure misclassified as upstream
/// unavailability, the same misclassification the recipe's client-credential rule exists for.
/// Shared by every OAuth response fold — the exchange and refresh-grant folds here and the
/// provisioning fold in [`super::provisioning`] — so one contract guards one storage boundary.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when the value falls outside the header-value byte contract.
pub(super) fn validate_token_value(target: &str, value: &str) -> Result<(), Error> {
    if http::HeaderValue::from_str(value).is_ok() {
        Ok(())
    } else {
        Err(Error::Unavailable {
            detail: format!(
                "the upstream reported a `{target}` value outside HTTP header-value bytes"
            ),
        })
    }
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
pub(super) async fn execute_refresh_grant(
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
                && is_terminal_code(&code, terminal_codes)
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
