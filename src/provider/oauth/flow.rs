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
            credential::{self, Grant, OAuthRecipe, Pkce, SlotGenerator},
            eval::ValueEnv,
            template::{ParamRow, ResponseTarget as Target, StringExpr},
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
        Grant::AuthorizationCode {
            authorize_url,
            redirect_uri,
            pkce,
            extra_params,
        } => {
            // The verifier is minted exactly when the grant declares PKCE S256, so its
            // presence is the only marker the URL builder needs to append the challenge pair.
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

            let authorize_url = build_authorize_url(
                &env,
                client_id.as_str(),
                &state_token,
                authorize_url,
                redirect_uri,
                extra_params,
                code_verifier.as_deref(),
            )?;

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
            authorization,
            expires_in_fallback_seconds,
        } => {
            let env = ValueEnv::for_device_authorization(
                &slots,
                base_url.as_deref(),
                client_id.as_str(),
                client_secret.as_deref(),
            );

            let device = execute_device_authorization_stage(http, authorization, &env).await?;

            // The expiry comes from the response's lifetime, else the definition's explicit
            // fallback. The write-time validation makes a definition carrying neither
            // unrepresentable: a stored-drift definition must fail fast instead of silently
            // inventing a lifetime. A fallback beyond the `i64` range the database clock
            // scales it into is the same stored drift — a malformed definition, never a
            // silently clamped magnitude.
            let expires_in = match device.expires_in_seconds {
                Some(expires_in) => expires_in,

                None => {
                    #[expect(
                        clippy::expect_used,
                        reason = "write-time validation requires a reported expiry or the \
                                  configured fallback; a definition with neither is stored \
                                  drift"
                    )]
                    let fallback = expires_in_fallback_seconds
                        .expect("device-flow expiry is guaranteed by the write-time validation");

                    i64::try_from(fallback.get()).map_err(|_| Error::Unavailable {
                        detail: format!(
                            "malformed definition: `expires_in_fallback_seconds` is out of \
                             range, got {}",
                            fallback.get()
                        ),
                    })?
                }
            };

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
/// `state`, and the PKCE pair when the caller minted a verifier), and the grant's extra
/// parameters in order. The caller destructures the grant arm, so every parameter arrives as
/// its own stage datum. The parameters join through [`control::append_query_row`], which keeps
/// them in the query component ahead of a `#` fragment in the stored base (RFC 3986 §3) so
/// every authorization parameter reaches the provider.
fn build_authorize_url(
    env: &ValueEnv<'_>,
    client_id: &str,
    state_token: &str,
    authorize_url: &StringExpr,
    redirect_uri: &str,
    extra_params: &[ParamRow],
    code_verifier: Option<&str>,
) -> Result<String, Error> {
    let mut url = env
        .evaluate(authorize_url, "authorize URL")
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    let mut append = |name: &str, value: &str| {
        control::append_query_row(&mut url, name, &control::percent_encode_component(value));
    };

    append("response_type", "code");

    append("client_id", client_id);

    append("redirect_uri", redirect_uri);

    // The caller mints a verifier exactly when the grant declares PKCE S256 (RFC 7636 §4.2),
    // so the verifier's presence is the challenge pair's marker and the method is S256 by the
    // same construction.
    if let Some(verifier) = code_verifier {
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
struct DeviceAuthorization {
    /// The device code the polling exchange presents.
    device_code: String,

    /// The human-readable code the administrator enters at the verification URI.
    user_code: String,

    /// The URI the administrator opens, when the response carries one.
    verification_uri: Option<String>,

    /// The verification URI carrying the user code, when the response carries one.
    verification_uri_complete: Option<String>,

    /// The device code's lifetime in seconds, when the response reports it.
    expires_in_seconds: Option<i64>,

    /// The polling interval in seconds, when the response requires one.
    interval_seconds: Option<i64>,
}

/// Executes the device authorization stage of a device grant and decodes its response. The
/// caller destructures the grant arm, so the stage arrives directly — a grant of another kind
/// is unrepresentable here.
///
/// # Errors
///
/// Returns [`Error`] when the device authorization request fails or its response is malformed.
async fn execute_device_authorization_stage(
    http: &Client,
    authorization: &credential::DeviceAuthorization,
    env: &ValueEnv<'_>,
) -> Result<DeviceAuthorization, Error> {
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
