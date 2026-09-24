//! The provisioning engine: the ordered control-request sequence a definition runs after a
//! successful exchange — and, for the reprovisioning refresh mode, under the refresh lease
//! (README §5.4).
//!
//! Steps run in order; each folds its response extractions into one [`WorkingState`] that the
//! caller commits atomically. A step guarded by `skip_if` runs only while that slot is absent —
//! the conditional arm Antigravity's onboarding needs. A step's long-running operation poll
//! repeats its request until the done path turns true, within the wall-clock budget, and folds
//! the completed operation's result extractions into the same state.

use crate::{
    provider::{
        control::{self, DecodedBody, Extracted},
        definition::{
            eval::ValueEnv,
            provisioning::{Poll, ProvisionExtraction, ProvisioningStep},
            template::ResponseTarget,
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

/// Runs one provisioning sequence to completion.
///
/// The environment is rebuilt per step, so each request sees the slots and base URL the preceding
/// steps placed, and the secret the credential currently carries.
///
/// # Errors
///
/// Returns [`Error`] when a step fails: a transport failure, a rejection, a failed required
/// extraction, an exhausted poll budget, or a terminal poll failure payload.
pub(crate) async fn run_steps(
    http: &Client,
    steps: &[ProvisioningStep],
    env: &ValueEnv<'_>,
    state: &mut WorkingState,
) -> Result<(), Error> {
    for (index, step) in steps.iter().enumerate() {
        if let Some(skip_if) = &step.skip_if
            && state.slots.contains_key(skip_if)
        {
            debug!("provisioning step {index} skipped: slot `{skip_if}` is present");

            continue;
        }

        run_step(http, step, env, state, index).await?;
    }

    Ok(())
}

/// Runs one provisioning step, including its long-running operation poll.
async fn run_step(
    http: &Client,
    step: &ProvisioningStep,
    env: &ValueEnv<'_>,
    state: &mut WorkingState,
    index: usize,
) -> Result<(), Error> {
    let what = "provisioning";

    // The environment reads the working state: a preceding step's slots and base URL are visible
    // to this step's request.
    let body = control::execute(http, &step.request, env, step.response_encoding, what).await?;

    apply_extractions(&body, &step.extract, what, state)?;

    if let Some(poll) = &step.poll {
        run_poll(http, poll, env, state, index).await?;
    }

    Ok(())
}

/// Repeats one long-running operation poll until its done path turns true, within the budget,
/// then folds the completed operation's result extractions into the working state.
async fn run_poll(
    http: &Client,
    poll: &Poll,
    env: &ValueEnv<'_>,
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

        let body = control::execute(http, &poll.request, env, poll.response_encoding, what).await?;

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

/// Reads the JSON value one member chain addresses, or `None` when any member is missing.
fn value_at<'a>(
    json: &'a serde_json::Value,
    path: &crate::provider::definition::template::ResponsePath,
) -> Option<&'a serde_json::Value> {
    let mut value = json;

    for member in &path.0 {
        value = value.get(member)?;
    }

    Some(value)
}

/// Whether the long-running operation at one done path has completed: the boolean `true`, the
/// number `1`, or the string `true` all count — the upstream-owned done spelling is one of these
/// truthy shapes.
fn done_at(
    body: &DecodedBody,
    path: &crate::provider::definition::template::ResponsePath,
) -> Result<bool, Error> {
    let DecodedBody::Json(json) = body else {
        return Ok(false);
    };

    let Some(value) = value_at(json, path) else {
        return Err(Error::Unavailable {
            detail: "provisioning poll response lacks its done path".to_string(),
        });
    };

    Ok(match value {
        serde_json::Value::Bool(done) => *done,
        serde_json::Value::Number(number) => number.as_i64() == Some(1),
        serde_json::Value::String(text) => text == "true",
        _ => false,
    })
}

/// Reads the failure payload at one error path, when any: an object renders its `status` and
/// `message` members, any other value renders verbatim.
fn error_at(
    body: &DecodedBody,
    path: &crate::provider::definition::template::ResponsePath,
) -> Result<Option<String>, Error> {
    if path.0.is_empty() {
        return Ok(None);
    }

    let DecodedBody::Json(json) = body else {
        return Ok(None);
    };

    let Some(value) = value_at(json, path) else {
        return Ok(None);
    };

    if value.is_null() {
        return Ok(None);
    }

    Ok(Some(match value {
        serde_json::Value::Object(object) => {
            let status = object.get("status").and_then(serde_json::Value::as_str);
            let message = object.get("message").and_then(serde_json::Value::as_str);

            match (status, message) {
                (Some(status), Some(message)) => format!("{status}: {message}"),
                (Some(status), None) => status.to_string(),
                (None, Some(message)) => message.to_string(),
                (None, None) => value.to_string(),
            }
        }

        other => other.to_string(),
    }))
}

/// Folds one extraction list into the working state: a missing required path fails, a missing
/// optional path leaves its target unchanged, and a present value of the wrong shape fails in
/// the extractor.
fn apply_extractions(
    body: &DecodedBody,
    extractions: &[ProvisionExtraction],
    what: &str,
    state: &mut WorkingState,
) -> Result<(), Error> {
    for extraction in extractions {
        let extracted = control::extract_field(body, &extraction.path, &extraction.shape, what)
            .map_err(|failure| Error::Unavailable {
                detail: failure.to_string(),
            })?;

        let Some(extracted) = extracted else {
            if extraction.required {
                return Err(Error::Unavailable {
                    detail: format!("{what} response lacks a required field"),
                });
            }

            continue;
        };

        apply_target(state, &extraction.target, extracted)?;
    }

    Ok(())
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

            validate_slot_value("base_url", &value)?;

            state.base_url = Some(value);

            Ok(())
        }

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

/// Validates one stored slot or base-URL value: non-empty printable ASCII without whitespace
/// (README §7). An upstream value outside the set fails the step instead of storing an
/// unusable value.
fn validate_slot_value(name: &str, value: &str) -> Result<(), Error> {
    let is_routable = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic());

    if is_routable {
        Ok(())
    } else {
        Err(Error::Unavailable {
            detail: format!(
                "the upstream reported a `{name}` value outside printable ASCII without whitespace"
            ),
        })
    }
}
