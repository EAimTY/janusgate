//! The OAuth token mechanics: the token exchange with terminal-code classification, the refresh
//! grant, the token-response expiry resolution, and the token-data folding into the working
//! slots (README §5.3, §7.1).
//!
//! Every provider-specific fact — endpoints, client credentials, parameter names, header sets —
//! lives in the definition the flow references; this engine carries the grant mechanics alone.
//! The control-request executor runs each stage's request, and the stage's response descriptor
//! maps its product. The exchange and refresh-grant fold applies the shared machine-consumed
//! text validators (README §7) before any slot or base-URL value reaches storage, and the
//! identity-token claim absorption applies the slot validator before a claimed value lands.

use super::provisioning::{validate_base_url_value, validate_slot_value};
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

/// The polling interval a device authorization response implies when it reports none: five
/// seconds per RFC 8628 §3.2.
pub(super) const DEFAULT_DEVICE_INTERVAL_SECONDS: i64 = 5;

/// The permanent interval increase a `slow_down` response applies, in seconds, per RFC 8628
/// §3.5.
pub(super) const SLOW_DOWN_PENALTY_SECONDS: i64 = 5;

/// The expiry of a token response, resolved per README §5.3: a positive reported `expires_in`,
/// else a future `exp` claim of the access token, else the definition's explicitly configured
/// lifetime, else no expiry at all. A reported non-positive `expires_in` or a non-future `exp`
/// claim is a malformed token response, never silently treated as missing.
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
/// targets extend the result (README §5.3). The completion pipeline and the refresh grant share
/// this fold.
///
/// # Errors
///
/// Returns [`Error`] when an identity-token claim value fails the routing-safety validation
/// (README §5.4) — the stage's own slot targets validated at the mapping boundary already.
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

/// The product of one token exchange (README §5.3).
///
/// The outcome serializes: the device poller persists a completed exchange as the flow row's
/// checkpoint the moment it completes, so a restart that recovers the claim resumes from the
/// checkpoint instead of re-presenting the consumed device code (README §3, §7.2). The values
/// it carries are already validated — the mapping fold validated the machine-consumed slots and
/// base URL at the storage boundary — and only the gateway writes the checkpoint, so
/// deserialization re-imposes no validation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
fn token_error_code(body: &str) -> Option<String> {
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
        #[expect(
            clippy::panic,
            reason = "a scheme-less base_url passing the fold is the failure report"
        )]
        let Err(base_url_error) = fold_target(&Target::BaseUrl, "upstream.example/v1") else {
            panic!("a scheme-less base_url must fail the fold");
        };

        assert!(
            base_url_error.to_string().contains("base_url"),
            "{base_url_error}"
        );

        #[expect(
            clippy::panic,
            reason = "a non-http base_url scheme passing the fold is the failure report"
        )]
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

        #[expect(
            clippy::panic,
            reason = "whitespace slot text passing the fold is the failure report"
        )]
        let Err(slot_error) = fold_target(
            &Target::Slot {
                name: "stage".to_string(),
            },
            "has space",
        ) else {
            panic!("slot text with whitespace must fail the fold");
        };

        assert!(slot_error.to_string().contains("stage"), "{slot_error}");

        #[expect(
            clippy::panic,
            reason = "non-ASCII slot text passing the fold is the failure report"
        )]
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
            #[expect(
                clippy::panic,
                reason = "a non-routing-safe claim landing in its slot is the failure report"
            )]
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
