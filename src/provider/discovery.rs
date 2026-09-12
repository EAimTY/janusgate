//! Upstream model discovery per provider type.

use super::{Provider, ProviderType, endpoints, http_body, oauth, oauth::Error as OAuthError};
use crate::format::UpstreamFormat;
use reqwest::Client;
use serde_json::Value;

/// Upstream model discovery failures. These paths never forward foreign
/// statuses, so an upstream rejection pins 502 with the status embedded in
/// the message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The upstream answered a non-2xx status.
    #[error("Upstream request failed with status {status}: {detail}")]
    Rejected {
        /// Upstream status code, embedded in the message.
        status: u16,

        /// Truncated upstream body excerpt.
        detail: String,
    },

    /// The upstream failed without a usable response, or answered with a
    /// payload that does not carry any recognizable model list.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// Discovery failed on JanusGate's side: the stored provider
    /// configuration violates an invariant.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message.
        message: String,
    },
}

impl From<OAuthError> for Error {
    fn from(error: OAuthError) -> Self {
        match error {
            OAuthError::Rejected { status, detail } => Self::Rejected { status, detail },
            OAuthError::Unavailable { detail } => Self::Unavailable { detail },
            other => Self::Internal {
                message: other.to_string(),
            },
        }
    }
}

/// Fetches the upstream model identifiers (`upstream_id` values) of a
/// provider.
///
/// # Errors
///
/// Returns [`Error`] when the upstream call fails or the payload shape is
/// not recognized.
pub(crate) async fn discover_upstream_models(
    http: &Client,
    provider: &Provider,
) -> Result<Vec<String>, Error> {
    // Every credential kind carries an upstream secret; the codex path below
    // still fails with an explicit internal error when the credentials are
    // not of the kind its provider type requires.
    let secret = provider.credentials.secret();

    match provider.provider_type {
        ProviderType::OpenAiOauth => {
            discover_codex(http, endpoints::CODEX_MODELS_URL, provider, secret).await
        }

        ProviderType::AntigravityOauth => discover_antigravity(http, secret).await,

        ProviderType::OpenAiCompatible => discover_compatible(http, provider, secret).await,
    }
}

/// Discovers Codex models from the ChatGPT backend.
///
/// The payload must contain an `items` or `models` array whose entries carry
/// a `slug` string.
async fn discover_codex(
    http: &Client,
    models_url: &str,
    provider: &Provider,
    access_token: &str,
) -> Result<Vec<String>, Error> {
    let account_id = match &provider.credentials {
        super::Credentials::OpenAiOauth { account_id, .. } => account_id.clone(),
        _ => {
            return Err(Error::Internal {
                message: "codex discovery requires OpenAI OAuth credentials".to_string(),
            });
        }
    };

    let response = http
        .get(models_url)
        .query(&[("client_version", endpoints::CODEX_CLIENT_VERSION)])
        .bearer_auth(access_token)
        .header("chatgpt-account-id", account_id)
        .send()
        .await
        .map_err(|error| Error::Unavailable {
            detail: format!("upstream unreachable: {error}"),
        })?;

    let value = parse_success_body(response).await?;

    let entries = value
        .get("items")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Unavailable {
            detail: "model discovery payload lacks an `items` or `models` array".to_string(),
        })?;

    entries
        .iter()
        .map(|entry| {
            entry
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error::Unavailable {
                    detail: "model discovery entry lacks a `slug` string".to_string(),
                })
        })
        .collect()
}

/// Discovers Antigravity models from Cloud Code Assist.
async fn discover_antigravity(http: &Client, access_token: &str) -> Result<Vec<String>, Error> {
    let value: Value = oauth::post_code_assist(
        http,
        endpoints::ANTIGRAVITY_FETCH_AVAILABLE_MODELS_ACTION,
        access_token,
        &serde_json::json!({}),
    )
    .await?;

    let models = value
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Unavailable {
            detail: "model discovery payload lacks a `models` object".to_string(),
        })?;

    Ok(models.keys().cloned().collect())
}

/// Discovers models from an OpenAI-compatible upstream, shaped by its
/// protocol.
async fn discover_compatible(
    http: &Client,
    provider: &Provider,
    access_token: &str,
) -> Result<Vec<String>, Error> {
    let protocol = provider.protocol.ok_or_else(|| Error::Internal {
        message: "openai_compatible provider lacks an upstream protocol".to_string(),
    })?;

    let base_url = provider
        .base_url
        .as_deref()
        .ok_or_else(|| Error::Internal {
            message: "openai_compatible provider lacks a base URL".to_string(),
        })?;

    let base_url = base_url.trim_end_matches('/');

    let mut request = http.get(format!("{base_url}/models"));

    request = match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            request.bearer_auth(access_token)
        }

        UpstreamFormat::GeminiGenerateContent => request.header("x-goog-api-key", access_token),
    };

    let response = request.send().await.map_err(|error| Error::Unavailable {
        detail: format!("upstream unreachable: {error}"),
    })?;

    let value = parse_success_body(response).await?;

    match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            let entries =
                value
                    .get("data")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::Unavailable {
                        detail: "model discovery payload lacks a `data` array".to_string(),
                    })?;

            entries
                .iter()
                .map(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| Error::Unavailable {
                            detail: "model discovery entry lacks an `id` string".to_string(),
                        })
                })
                .collect()
        }

        UpstreamFormat::GeminiGenerateContent => {
            let entries = value
                .get("models")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Unavailable {
                    detail: "model discovery payload lacks a `models` array".to_string(),
                })?;

            entries
                .iter()
                .map(|entry| {
                    let name = entry.get("name").and_then(Value::as_str).ok_or_else(|| {
                        Error::Unavailable {
                            detail: "model discovery entry lacks a `name` string".to_string(),
                        }
                    })?;

                    Ok(name.strip_prefix("models/").unwrap_or(name).to_string())
                })
                .collect()
        }
    }
}

/// Verifies a successful status and parses the JSON body.
///
/// Only the error path is read through the bounded error-body read: a success
/// body is legitimate discovery JSON of unbounded size and is read in full.
///
/// # Errors
///
/// Returns [`Error`] for non-success statuses and malformed bodies.
async fn parse_success_body(response: reqwest::Response) -> Result<Value, Error> {
    let status = response.status();

    if !status.is_success() {
        let body = http_body::read_error_body(response).await;

        return Err(Error::Rejected {
            status: status.as_u16(),
            detail: http_body::truncate_excerpt(&body),
        });
    }

    let body = response.text().await.map_err(|error| Error::Unavailable {
        detail: format!("upstream read failed: {error}"),
    })?;

    serde_json::from_str(&body).map_err(|error| Error::Unavailable {
        detail: format!("malformed upstream response: {error}"),
    })
}
