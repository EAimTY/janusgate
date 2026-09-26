//! The provisioning engine: the ordered control-request sequence a definition runs after a
//! successful exchange — and, for the reprovisioning refresh mode, under the refresh lease
//! (README §5.4).
//!
//! Steps run in order; each folds its response extractions into one [`WorkingState`] that the
//! caller commits atomically. A step guarded by `skip_if` runs only while that slot is absent —
//! the conditional arm Antigravity's onboarding needs. A step's long-running operation poll
//! repeats its request until the done path turns true, within the wall-clock budget, and folds
//! the completed operation's result extractions into the same state.
//!
//! This module is also the shared home of the machine-consumed-text validators every OAuth
//! response fold applies before storage (README §7): [`validate_slot_value`] and
//! [`validate_base_url_value`] are the one contract the provisioning fold and the exchange and
//! refresh-grant folds in [`crate::provider::oauth::flow`] share, so a value an upstream
//! response extracts is validated identically whichever stage extracted it.

use crate::{
    provider::{
        control::{self, DecodedBody, Extracted},
        definition::{
            eval::ValueEnv,
            provisioning::{Poll, ProvisionExtraction, ProvisioningStep},
            template::{ResponsePath, ResponseTarget},
        },
        oauth::Error,
    },
    unix_time,
};
use log::debug;
use reqwest::Client;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// The working state one provisioning sequence folds into: the flow's slots, the credential
/// fields a step may supply, and the working base URL. The caller commits the state atomically —
/// the provider row changes only when all steps succeed.
#[derive(Debug, Default)]
pub(crate) struct WorkingState {
    /// The working slots, updated in place.
    pub(crate) slots: BTreeMap<String, String>,

    /// The access token a step supplied, when the exchange left it to provisioning.
    pub(crate) access_token: Option<String>,

    /// The refresh token a step supplied, when provisioning replaces it.
    pub(crate) refresh_token: Option<String>,

    /// The working base URL, replaceable by a `base_url` extraction.
    pub(crate) base_url: Option<String>,

    /// The access token's absolute expiry, in UTC microseconds, scaled from a
    /// `expires_at_unix_seconds` extraction.
    pub(crate) expires_at: Option<i64>,
}

impl WorkingState {
    /// Opens a working state over the given starting slots and base URL.
    #[must_use]
    pub(crate) fn new(slots: BTreeMap<String, String>, base_url: Option<String>) -> Self {
        Self {
            slots,
            base_url,
            ..Self::default()
        }
    }
}

/// The stage-constant values one provisioning sequence's expressions may reference beside the
/// accumulated working state: the credential secret and the configured client credentials —
/// exactly the fields [`ValueEnv::for_provisioning`] pins as available. The caller threads the
/// constants down once; the engine rebuilds the environment itself from them plus the working
/// state before every request, so each step and poll sees what the preceding stages placed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProvisioningEnv<'a> {
    /// The credential secret the stage's `Secret` references render; `None` fails them as
    /// stored drift.
    pub(crate) secret: Option<&'a str>,

    /// The configured OAuth client ID.
    pub(crate) client_id: &'a str,

    /// The configured OAuth client secret.
    pub(crate) client_secret: Option<&'a str>,
}

/// Runs one provisioning sequence to completion.
///
/// The environment is rebuilt per request from the accumulated working state, so each request
/// sees the slots and base URL the preceding steps and polls placed.
///
/// # Errors
///
/// Returns [`Error`] when a step fails: a transport failure, a rejection, a failed required
/// extraction, an exhausted poll budget, or a terminal poll failure payload.
pub(crate) async fn run_steps(
    http: &Client,
    steps: &[ProvisioningStep],
    constants: &ProvisioningEnv<'_>,
    state: &mut WorkingState,
) -> Result<(), Error> {
    for (index, step) in steps.iter().enumerate() {
        if let Some(skip_if) = &step.skip_if
            && state.slots.contains_key(skip_if)
        {
            debug!("provisioning step {index} skipped: slot `{skip_if}` is present");

            continue;
        }

        run_step(http, step, constants, state, index).await?;
    }

    Ok(())
}

/// Builds the stage environment of one request: the working state accumulated so far plus the
/// stage constants.
fn provisioning_env<'a>(constants: &ProvisioningEnv<'a>, state: &'a WorkingState) -> ValueEnv<'a> {
    ValueEnv::for_provisioning(
        &state.slots,
        state.base_url.as_deref(),
        constants.secret,
        constants.client_id,
        constants.client_secret,
    )
}

/// Runs one provisioning step, including its long-running operation poll.
async fn run_step(
    http: &Client,
    step: &ProvisioningStep,
    constants: &ProvisioningEnv<'_>,
    state: &mut WorkingState,
    index: usize,
) -> Result<(), Error> {
    let what = "provisioning";

    // The environment reads the accumulated working state: a preceding step's slots and base
    // URL are visible to this step's request.
    let env = provisioning_env(constants, state);

    let body = control::execute(http, &step.request, &env, step.response_encoding, what).await?;

    apply_extractions(&body, &step.extract, what, state)?;

    if let Some(poll) = &step.poll {
        run_poll(http, poll, constants, state, index).await?;
    }

    Ok(())
}

/// Repeats one long-running operation poll until its done path turns true, within the budget,
/// then folds the completed operation's result extractions into the working state.
async fn run_poll(
    http: &Client,
    poll: &Poll,
    constants: &ProvisioningEnv<'_>,
    state: &mut WorkingState,
    index: usize,
) -> Result<(), Error> {
    let what = "provisioning poll";

    let interval = Duration::from_millis(poll.interval_ms.get());

    let deadline = Instant::now() + Duration::from_millis(poll.budget_ms.get());

    loop {
        // The budget is checked before every attempt: each attempt is bounded by the
        // control-plane client's total request timeout, but a fixed attempt count alone bounds
        // nothing.
        if Instant::now() >= deadline {
            return Err(Error::Unavailable {
                detail: "provisioning poll did not complete in time".to_string(),
            });
        }

        // The poll request resolves against the working state too: the step's own extractions
        // ran before the poll, so it can reference the operation name they placed.
        let env = provisioning_env(constants, state);

        let body =
            control::execute(http, &poll.request, &env, poll.response_encoding, what).await?;

        // A completed operation carrying a terminal failure payload fails the step with that
        // payload instead of collapsing into a generic missing-done failure.
        if let Some(failure) = error_at(&body, &poll.error_path)? {
            return Err(Error::Unavailable {
                detail: format!("provisioning poll {index} failed upstream: {failure}"),
            });
        }

        if done_at(&body, &poll.done_path)? {
            apply_extractions(&body, &poll.result_extract, what, state)?;

            return Ok(());
        }

        debug!("provisioning poll {index} still running; retrying");

        tokio::time::sleep(interval).await;
    }
}

/// One poll-path value resolved out of a decoded poll response: the JSON value at the path, or
/// the text of the one flat form member the path names (README §5.1: form responses are flat,
/// one member).
enum PollValue<'a> {
    /// The JSON value the path addresses.
    Json(&'a serde_json::Value),

    /// The flat form field's text.
    Form(&'a str),
}

/// Resolves one poll path against a decoded body: the JSON member chain, or the single named
/// member of the flat form encoding. A missing path yields [`None`] — the caller decides
/// whether that is fatal (the done path) or an absent payload (the error path) — and a
/// multi-member path against a form body is a malformed definition and fails.
fn resolve_poll_value<'a>(
    body: &'a DecodedBody,
    path: &ResponsePath,
    role: &str,
) -> Result<Option<PollValue<'a>>, Error> {
    match body {
        DecodedBody::Json(json) => Ok(path.resolve(json).map(PollValue::Json)),

        DecodedBody::Form(fields) => {
            let [member] = path.0.as_slice() else {
                return Err(Error::Unavailable {
                    detail: format!(
                        "provisioning poll {role} path addresses a flat form field and carries \
                         exactly one member"
                    ),
                });
            };

            Ok(fields
                .get(member)
                .map(|text| PollValue::Form(text.as_str())))
        }
    }
}

/// Whether the long-running operation at one done path has completed: on JSON, the shared
/// upstream truthy-scalar contract of [`control::is_truthy_scalar`] decides, and on the flat
/// form encoding, the one addressed member reads `true` or `1` — the string spellings those
/// truthy scalars render in.
fn done_at(body: &DecodedBody, path: &ResponsePath) -> Result<bool, Error> {
    let Some(value) = resolve_poll_value(body, path, "done")? else {
        return Err(Error::Unavailable {
            detail: "provisioning poll response lacks its done path".to_string(),
        });
    };

    Ok(match value {
        PollValue::Json(json) => control::is_truthy_scalar(json),

        // A form field carries only text, so the truthy boolean and number scalars surface in
        // their string spellings.
        PollValue::Form(text) => text == "true" || text == "1",
    })
}

/// Reads the failure payload at one error path, when any: on JSON, an object renders its
/// `status` and `message` members and any other non-`null` value renders verbatim; on the flat
/// form encoding, the one addressed member renders verbatim — it is already text. An absent or
/// `null` payload reads as no failure.
fn error_at(body: &DecodedBody, path: &ResponsePath) -> Result<Option<String>, Error> {
    // The empty path is the configured no-error-inspection: it addresses nothing by design.
    if path.0.is_empty() {
        return Ok(None);
    }

    let Some(value) = resolve_poll_value(body, path, "error")? else {
        return Ok(None);
    };

    Ok(match value {
        // A form field is already text: the flat encoding's only shape renders verbatim.
        PollValue::Form(text) => Some(text.to_string()),

        PollValue::Json(json) => {
            if json.is_null() {
                return Ok(None);
            }

            Some(match json {
                serde_json::Value::Object(object) => {
                    let status = object.get("status").and_then(serde_json::Value::as_str);
                    let message = object.get("message").and_then(serde_json::Value::as_str);

                    match (status, message) {
                        (Some(status), Some(message)) => format!("{status}: {message}"),
                        (Some(status), None) => status.to_string(),
                        (None, Some(message)) => message.to_string(),
                        (None, None) => json.to_string(),
                    }
                }

                other => other.to_string(),
            })
        }
    })
}

/// Folds one extraction list into the working state through the shared control-layer fold: a
/// missing required path fails, a missing optional path leaves its target unchanged, and a
/// present value of the wrong shape fails in the extractor.
fn apply_extractions(
    body: &DecodedBody,
    extractions: &[ProvisionExtraction],
    what: &str,
    state: &mut WorkingState,
) -> Result<(), Error> {
    control::map_response_fields(body, extractions, what, |target, extracted| {
        apply_target(state, target, extracted)
    })
}

/// Folds one extracted value into the working state per its target, validating the values that
/// must stay machine-consumed text (slots and base URLs) at the storage boundary.
fn apply_target(
    state: &mut WorkingState,
    target: &ResponseTarget,
    extracted: Extracted,
) -> Result<(), Error> {
    match target {
        ResponseTarget::Slot { name } => {
            let value = extracted.as_text();

            validate_slot_value(name, &value)?;

            state.slots.insert(name.clone(), value);

            Ok(())
        }

        ResponseTarget::AccessToken => {
            state.access_token = Some(extracted.as_text());

            Ok(())
        }

        ResponseTarget::RefreshToken => {
            state.refresh_token = Some(extracted.as_text());

            Ok(())
        }

        ResponseTarget::BaseUrl => {
            let value = extracted.as_text();

            validate_base_url_value(&value)?;

            state.base_url = Some(value);

            Ok(())
        }

        // Both expiry consult points prefer this extraction over the response's own token
        // evidence (README §5.4), so a stored non-future timestamp is always elapsed for the
        // freshness check and would send every dispatch back into the refresh lease. The
        // contract for consulted evidence is that it must be in the future (README §7.2): a
        // zero, negative, or past timestamp is a malformed provisioning response, the same
        // failure the token path applies to a non-future `exp` claim.
        ResponseTarget::ExpiresAtUnixSeconds => {
            let Some(seconds) = extracted.as_integer() else {
                return Err(Error::Unavailable {
                    detail: "provisioning extraction must carry an integer expiry".to_string(),
                });
            };

            let micros = seconds
                .checked_mul(unix_time::MICROS_PER_SEC)
                .ok_or_else(|| Error::Unavailable {
                    detail: "provisioning extraction carries an overflowing expiry".to_string(),
                })?;

            if micros <= unix_time::unix_now_micros() {
                let detail = format!(
                    "malformed provisioning response: expiry must be in the future, got {seconds}"
                );

                return Err(Error::Unavailable { detail });
            }

            state.expires_at = Some(micros);

            Ok(())
        }

        // The write-time validation rejects an exchange- or device-stage target in a
        // provisioning extraction, so this arm is stored drift and fails fast.
        _ => Err(Error::Internal {
            message: "the provisioning extraction targets a value its stage does not produce"
                .to_string(),
        }),
    }
}

/// Validates one stored slot value against the shared routing-safe text contract (README §7):
/// non-empty printable ASCII without whitespace. An upstream value outside the set fails the
/// stage instead of storing an unusable value. Shared by every OAuth response fold — the
/// provisioning fold here and the exchange and refresh-grant folds in
/// [`crate::provider::oauth::flow`] — so one contract guards one storage boundary.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when the value carries a character outside the allowed set or
/// is empty.
pub(crate) fn validate_slot_value(name: &str, value: &str) -> Result<(), Error> {
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
/// [`crate::routing_text::is_base_url`] (README §7): routing-safe text plus the non-empty
/// `http://` or `https://` scheme — the same composition the management boundary enforces on
/// operator-supplied URLs. Shared by every OAuth response fold — the provisioning fold here and
/// the exchange and refresh-grant folds in [`crate::provider::oauth::flow`] — so one contract
/// guards one storage boundary.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when the value falls outside the shared base-URL contract.
pub(crate) fn validate_base_url_value(value: &str) -> Result<(), Error> {
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

    use super::{ResponsePath, ResponseTarget, WorkingState, apply_target, done_at, error_at};
    use crate::provider::control::{DecodedBody, Extracted};
    use std::collections::BTreeMap;

    /// The provisioning fold shares the exchange fold's storage boundary (README §7): slot text
    /// must be routing-safe machine-consumed text, and a mapped `base_url` must additionally
    /// carry the non-empty `http(s)://` scheme — an upstream value outside the contracts fails
    /// the step instead of storing an unusable value, and valid values fold in verbatim.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[test]
    fn the_provisioning_fold_validates_slot_and_base_url_targets() {
        let Err(base_url_error) = fold(
            &mut WorkingState::default(),
            &ResponseTarget::BaseUrl,
            "upstream.example",
        ) else {
            panic!("a scheme-less base_url must fail the provisioning fold");
        };

        assert!(
            base_url_error.to_string().contains("base_url"),
            "{base_url_error}"
        );

        let Err(_) = fold(
            &mut WorkingState::default(),
            &ResponseTarget::BaseUrl,
            "ftp://upstream.example",
        ) else {
            panic!("a non-http base_url scheme must fail the provisioning fold");
        };

        let mut state = WorkingState::default();

        fold(
            &mut state,
            &ResponseTarget::BaseUrl,
            "https://upstream.example/v1",
        )
        .unwrap();

        assert_eq!(
            state.base_url.as_deref(),
            Some("https://upstream.example/v1"),
            "a valid base_url stores"
        );

        let Err(slot_error) = fold(
            &mut WorkingState::default(),
            &ResponseTarget::Slot {
                name: "stage".to_string(),
            },
            "has space",
        ) else {
            panic!("slot text with whitespace must fail the provisioning fold");
        };

        assert!(slot_error.to_string().contains("stage"), "{slot_error}");

        let mut state = WorkingState::default();

        fold(
            &mut state,
            &ResponseTarget::Slot {
                name: "stage".to_string(),
            },
            "clean-value",
        )
        .unwrap();

        assert_eq!(
            state.slots.get("stage").map(String::as_str),
            Some("clean-value"),
            "valid slot text stores"
        );
    }

    /// Folds one text value into the state through the given target, returning the fold's
    /// result so the test can pin its failure side.
    fn fold(
        state: &mut WorkingState,
        target: &ResponseTarget,
        value: &str,
    ) -> Result<(), super::Error> {
        apply_target(state, target, Extracted::Text(value.to_string()))
    }

    /// A flat form response carries no nesting (README §5.1), so a poll path addressing it
    /// names exactly one member: a multi-member done or error path is a malformed definition
    /// that fast-fails the poll — never a missing value read as still-running (the done path)
    /// or as no failure (the error path).
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[test]
    fn a_multi_member_poll_path_against_a_form_body_fast_fails() {
        let body = DecodedBody::Form(BTreeMap::from([(
            "status".to_string(),
            "running".to_string(),
        )]));

        let Err(done_error) = done_at(
            &body,
            &ResponsePath(vec!["result".to_string(), "done".to_string()]),
        ) else {
            panic!("a two-member done path against a form body must fail the poll");
        };

        assert!(
            done_error
                .to_string()
                .contains("done path addresses a flat form field and carries exactly one member"),
            "{done_error}"
        );

        let Err(error_path_error) = error_at(
            &body,
            &ResponsePath(vec!["result".to_string(), "error".to_string()]),
        ) else {
            panic!("a two-member error path against a form body must fail the poll");
        };

        assert!(
            error_path_error
                .to_string()
                .contains("error path addresses a flat form field and carries exactly one member"),
            "{error_path_error}"
        );
    }
}
