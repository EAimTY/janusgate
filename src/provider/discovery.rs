//! Upstream model discovery per provider type.

use super::{Provider, ProviderType, endpoints, http_body, oauth};
use crate::{error::ApiError, format::UpstreamFormat};
use reqwest::Client;
use serde_json::Value;

/// Fetches the upstream model identifiers (`upstream_name` values) of a
/// provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the upstream call fails or the payload shape is
/// not recognized.
pub(crate) async fn discover_upstream_models(
    http: &Client,
    provider: &Provider,
) -> Result<Vec<String>, ApiError> {
    // Every credential kind carries an upstream secret; the codex path below
    // still fails with an explicit internal error when the credentials are
    // not of the kind its provider type requires.
    let secret = provider.credentials.secret();

    match provider.provider_type {
        ProviderType::OpenAiOauth => discover_codex(http, provider, secret).await,

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
    provider: &Provider,
    access_token: &str,
) -> Result<Vec<String>, ApiError> {
    let account_id = match &provider.credentials {
        super::Credentials::OpenAiOauth { account_id, .. } => account_id.clone(),
        _ => {
            return Err(ApiError::internal(
                "codex discovery requires OpenAI OAuth credentials",
            ));
        }
    };

    let response = http
        .get(endpoints::CODEX_MODELS_URL)
        .bearer_auth(access_token)
        .header("chatgpt-account-id", account_id)
        .send()
        .await
        .map_err(|error| {
            ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
        })?;

    let value = parse_success_body(response).await?;

    let entries = value
        .get("items")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::upstream_unavailable(
                "model discovery payload lacks an `items` or `models` array",
            )
        })?;

    entries
        .iter()
        .map(|entry| {
            entry
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    ApiError::upstream_unavailable("model discovery entry lacks a `slug` string")
                })
        })
        .collect()
}

/// Discovers Antigravity models from Cloud Code Assist.
async fn discover_antigravity(http: &Client, access_token: &str) -> Result<Vec<String>, ApiError> {
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
        .ok_or_else(|| {
            ApiError::upstream_unavailable("model discovery payload lacks a `models` object")
        })?;

    Ok(models.keys().cloned().collect())
}

/// Discovers models from an OpenAI-compatible upstream, shaped by its
/// protocol.
async fn discover_compatible(
    http: &Client,
    provider: &Provider,
    access_token: &str,
) -> Result<Vec<String>, ApiError> {
    let protocol = provider.protocol.ok_or_else(|| {
        ApiError::internal("openai_compatible provider lacks an upstream protocol")
    })?;

    let base_url = provider
        .base_url
        .as_deref()
        .ok_or_else(|| ApiError::internal("openai_compatible provider lacks a base URL"))?;

    let base_url = base_url.trim_end_matches('/');

    let mut request = http.get(format!("{base_url}/models"));

    request = match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            request.bearer_auth(access_token)
        }

        UpstreamFormat::GeminiGenerateContent => request.header("x-goog-api-key", access_token),
    };

    let response = request.send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
    })?;

    let value = parse_success_body(response).await?;

    match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            let entries = value.get("data").and_then(Value::as_array).ok_or_else(|| {
                ApiError::upstream_unavailable("model discovery payload lacks a `data` array")
            })?;

            entries
                .iter()
                .map(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| {
                            ApiError::upstream_unavailable(
                                "model discovery entry lacks an `id` string",
                            )
                        })
                })
                .collect()
        }

        UpstreamFormat::GeminiGenerateContent => {
            let entries = value
                .get("models")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ApiError::upstream_unavailable("model discovery payload lacks a `models` array")
                })?;

            entries
                .iter()
                .map(|entry| {
                    let name = entry.get("name").and_then(Value::as_str).ok_or_else(|| {
                        ApiError::upstream_unavailable(
                            "model discovery entry lacks a `name` string",
                        )
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
/// Returns [`ApiError`] for non-success statuses and malformed bodies.
async fn parse_success_body(response: reqwest::Response) -> Result<Value, ApiError> {
    let status = response.status();

    if !status.is_success() {
        let body = http_body::read_error_body(response).await;

        return Err(ApiError::upstream_rejected(
            status.as_u16(),
            http_body::truncate_excerpt(&body),
        ));
    }

    let body = response.text().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream read failed: {error}"))
    })?;

    serde_json::from_str(&body).map_err(|error| {
        ApiError::upstream_unavailable(format!("malformed upstream response: {error}"))
    })
}
