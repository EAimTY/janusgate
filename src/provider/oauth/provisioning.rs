//! The provisioning engine: the ordered control-request sequence a definition runs after a
//! successful exchange — and, for the reprovisioning refresh mode, under the refresh lease
//! (`docs/design/providers.md`).
//!
//! Steps run in order; each folds its response extractions into one [`WorkingState`] that the
//! caller commits atomically. A step guarded by `skip_if` runs only while that slot is absent —
//! the conditional arm Antigravity's onboarding needs. A step's long-running operation poll
//! repeats its request until the done path turns true, within the wall-clock budget, and folds
//! the completed operation's result extractions into the same state.
//!
//! The module also carries the machine-consumed-text contract every OAuth response fold applies
//! before storage (`docs/reference/management-api.md`): the fold here shares the validators
//! [`super::exchange::validate_slot_value`], [`super::exchange::validate_token_value`], and
//! [`super::exchange::validate_base_url_value`] with the exchange and refresh-grant folds in
//! [`super::exchange`], so a value an upstream response extracts is validated identically
//! whichever stage extracted it. They live in
//! [`super::exchange`] — the layer's substrate — because every production fold runs inside the
//! oauth engine: a provisioning sequence only ever executes under this engine's stages.

use super::exchange::{validate_base_url_value, validate_slot_value, validate_token_value};
use crate::{
    provider::{
        Error,
        control::{self, DecodedBody, Extracted},
        definition::{
            eval::ValueEnv,
            provisioning::{Poll, ProvisionExtraction, ProvisioningStep},
            template::{ResponsePath, ResponseTarget},
        },
    },
    unix_time,
};
use log::debug;
use reqwest::Client;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[cfg(test)]
mod tests;

/// The working state one provisioning sequence folds into: the flow's slots, the credential
/// fields a step may supply, and the working base URL. The caller commits the state atomically —
/// the provider row changes only when all steps succeed.
#[derive(Debug, Default)]
pub(super) struct WorkingState {
    /// The working slots, updated in place.
    pub(super) slots: BTreeMap<String, String>,

    /// The access token a step supplied, when the exchange left it to provisioning.
    pub(super) access_token: Option<String>,

    /// The refresh token a step supplied, when provisioning replaces it.
    pub(super) refresh_token: Option<String>,

    /// The working base URL, replaceable by a `base_url` extraction.
    pub(super) base_url: Option<String>,

    /// Whether the working base URL is upstream-derived — the exchange's `base_url` fold or a
    /// provisioning `base_url` extraction — rather than the begin-time base the flow echoed in.
    /// Only a derived base replaces the stored one at a reauthorization's provider write
    /// (`docs/reference/management-api.md`); the begin-time echo never does, or the commit would
    /// clobber a base a concurrent `providers:update` legitimately stored while the flow ran.
    pub(super) is_base_url_derived: bool,

    /// The access token's absolute expiry, in UTC microseconds, scaled from a
    /// `expires_at_unix_seconds` extraction.
    pub(super) expires_at: Option<i64>,
}

impl WorkingState {
    /// Opens a working state over the given starting slots and base URL.
    #[must_use]
    pub(super) fn new(slots: BTreeMap<String, String>, base_url: Option<String>) -> Self {
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
pub(super) struct ProvisioningEnv<'a> {
    /// The credential secret the stage's `Secret` references render; `None` fails them as
    /// stored drift.
    pub(super) secret: Option<&'a str>,

    /// The configured OAuth client ID.
    pub(super) client_id: &'a str,

    /// The configured OAuth client secret.
    pub(super) client_secret: Option<&'a str>,
}

/// Runs one provisioning sequence to completion.
///
/// The environment is rebuilt per request from the accumulated working state, so each request
/// sees the slots and base URL the preceding steps and polls placed.
///
/// # Errors
///
/// Returns [`Error`] when a step fails: a transport failure, a rejection, a failed required
/// extraction, an exhausted poll budget, or a terminal poll failure at the configured error
/// path. Every failure the stage raises is gateway-authored; upstream response content from a
/// failure payload stays in the log excerpt alone.
pub(super) async fn run_steps(
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

        // A completed operation carrying a terminal failure payload fails the step. The payload
        // is upstream response content: its bounded excerpt is logged for diagnosis, while the
        // error the stage raises is fully gateway-authored, so the terminal failure pair a
        // device flow persists (`docs/design/storage.md` and `docs/reference/management-api.md`)
        // never embeds an upstream body — the contract the poller's
        // `provisioning_failure_message` renders under.
        if let Some(failure) = error_at(&body, &poll.error_path)? {
            debug!("provisioning poll {index} failed upstream: {failure}");

            return Err(Error::Unavailable {
                detail: format!("provisioning poll {index} failed upstream"),
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
/// the text of the one flat form member the path names (`docs/design/providers.md`: form responses
/// are flat, one member).
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
/// `status` and `message` members, a bare string renders its text — never the JSON-quoted
/// spelling — and any other non-`null` value renders its JSON text; on the flat form encoding,
/// the one addressed member renders verbatim — it is already text. Every rendering is truncated
/// to the shared excerpt bound (`docs/reference/configuration.md`), so a hostile payload can never
/// stream unbounded text into a message. An absent or `null` payload reads as no failure.
///
/// The rendering is diagnostic text for the log alone: the error the poll raises names the
/// failed step without embedding it, so no upstream body content can reach a stored terminal
/// failure (`docs/design/storage.md` and `docs/reference/management-api.md`).
fn error_at(body: &DecodedBody, path: &ResponsePath) -> Result<Option<String>, Error> {
    /// Known display fields inside an otherwise arbitrary upstream failure fragment.
    #[derive(Deserialize)]
    struct ErrorDescription {
        /// Machine status, used only when the upstream supplied text.
        #[serde(default, deserialize_with = "crate::serde_helpers::string_or_none")]
        status: Option<String>,
        /// Human-readable detail, used only when the upstream supplied text.
        #[serde(default, deserialize_with = "crate::serde_helpers::string_or_none")]
        message: Option<String>,
    }
    // The empty path is the configured no-error-inspection: it addresses nothing by design.
    if path.0.is_empty() {
        return Ok(None);
    }

    let Some(value) = resolve_poll_value(body, path, "error")? else {
        return Ok(None);
    };

    Ok(match value {
        // A form field is already text: the flat encoding's only shape renders verbatim.
        PollValue::Form(text) => Some(crate::http_body::truncate_excerpt(text)),

        PollValue::Json(json) => {
            if json.is_null() {
                return Ok(None);
            }

            let description = ErrorDescription::deserialize(json).ok();
            let (status, message) = description
                .map(|description| (description.status, description.message))
                .unwrap_or_default();

            let rendered = match (status, message) {
                (Some(status), Some(message)) => format!("{status}: {message}"),
                (Some(status), None) => status,
                (None, Some(message)) => message,

                // A bare string renders its text — never the JSON-quoted spelling — and any
                // other shape renders its JSON text.
                (None, None) => json
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| json.to_string()),
            };

            Some(crate::http_body::truncate_excerpt(&rendered))
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
/// must stay machine-consumed text (the stored tokens, slots, and base URLs) at the storage
/// boundary.
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
            let value = extracted.as_text();

            validate_token_value("access_token", &value)?;

            state.access_token = Some(value);

            Ok(())
        }

        ResponseTarget::RefreshToken => {
            let value = extracted.as_text();

            validate_token_value("refresh_token", &value)?;

            state.refresh_token = Some(value);

            Ok(())
        }

        ResponseTarget::BaseUrl => {
            let value = extracted.as_text();

            validate_base_url_value(&value)?;

            state.base_url = Some(value);

            // The extraction is the provenance mark (`docs/reference/management-api.md`): the base
            // now aboard is upstream-derived, the only kind a reauthorization's write
            // commits.
            state.is_base_url_derived = true;

            Ok(())
        }

        // Both expiry consult points prefer this extraction over the response's own token
        // evidence (`docs/design/providers.md`), so a stored non-future timestamp is always elapsed
        // for the freshness check and would send every dispatch back into the refresh
        // lease. The contract for consulted evidence is that it must be in the future
        // (`docs/reference/management-api.md`): a zero, negative, or past timestamp is a
        // malformed provisioning response, the same failure the token path applies to a
        // non-future `exp` claim.
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
