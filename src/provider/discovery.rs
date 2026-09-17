//! Upstream model discovery per provider type.

use super::{
    Credentials, Provider, ProviderType, endpoints, http_body, http_body::ExchangeFailure, oauth,
};
use crate::format::UpstreamFormat;
use reqwest::Client;
use serde::{Deserialize, Serialize, de::IgnoredAny};
use std::collections::BTreeMap;

/// Upstream model discovery failures. These paths never forward foreign statuses, so an upstream
/// rejection pins 502 with the status embedded in the message.
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

    /// The upstream failed without a usable response, or answered with a payload that does not
    /// carry any recognizable model list.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// Discovery failed on JanusGate's side: the stored provider configuration violates an
    /// invariant.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message.
        message: String,
    },
}

/// The `fetchAvailableModels` request body (README §5.2): the action takes no inputs, so the body
/// is the empty object.
#[derive(Debug, Serialize)]
struct FetchAvailableModelsRequest {}

/// A Codex model list (README §5.1): the vendor serves the model entries under `items` or `models`.
/// The payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct CodexModelsResponse {
    /// Model entries the payload carries.
    #[serde(alias = "models")]
    items: Option<Vec<CodexModelEntry>>,
}

/// One model entry of a Codex model list.
#[derive(Debug, Deserialize)]
struct CodexModelEntry {
    /// Upstream model identifier the entry names.
    slug: Option<String>,
}

/// An Antigravity `fetchAvailableModels` response (README §5.2): the available models keyed by
/// model name. The payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct AvailableModelsResponse {
    /// The available models. Each value is a provider-specific model descriptor the gateway never
    /// reads — discovery takes only the map's keys as `upstream_id` values — so its shape is not
    /// the gateway's to define.
    models: Option<BTreeMap<String, IgnoredAny>>,
}

/// An OpenAI-compatible model list (README §5.3): the OpenAI protocols serve the model entries
/// under `data`. The payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct CompatibleModelsResponse {
    /// Model entries the payload carries.
    data: Option<Vec<CompatibleModelEntry>>,
}

/// One model entry of an OpenAI-compatible model list.
#[derive(Debug, Deserialize)]
struct CompatibleModelEntry {
    /// Upstream model identifier the entry names.
    id: Option<String>,
}

/// A Gemini `models.list` response (README §5.3): the entries are served under `models`. The
/// payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct GeminiModelsResponse {
    /// Model entries the payload carries.
    models: Option<Vec<GeminiModelEntry>>,
}

/// One model entry of a Gemini model list.
#[derive(Debug, Deserialize)]
struct GeminiModelEntry {
    /// Model resource name, `models/<id>`.
    name: Option<String>,
}

/// Fetches the upstream model identifiers (`upstream_id` values) of a provider.
///
/// # Errors
///
/// Returns [`Error`] when the upstream call fails or the payload shape is not recognized.
pub(crate) async fn discover_upstream_models(
    http: &Client,
    provider: &Provider,
) -> Result<Vec<String>, Error> {
    // Every credential kind carries an upstream secret; the codex path below still fails with an
    // explicit internal error when the credentials are not of the kind its provider type requires.
    let secret = provider.credentials.secret();

    match provider.provider_type {
        ProviderType::OpenAiOauth => discover_codex(http, provider, secret).await,

        ProviderType::AntigravityOauth => discover_antigravity(http, secret).await,

        ProviderType::OpenAiCompatible => discover_compatible(http, provider, secret).await,
    }
}

/// Converts the shared upstream exchange failure into the discovery error domain: the rejection
/// detail is the body excerpt under the one truncation policy ([`http_body::truncate_excerpt`]).
impl From<ExchangeFailure> for Error {
    fn from(failure: ExchangeFailure) -> Self {
        match failure {
            ExchangeFailure::Rejected { status, body } => Self::Rejected {
                status,
                detail: http_body::truncate_excerpt(&body),
            },

            ExchangeFailure::Unavailable { detail } => Self::Unavailable { detail },
        }
    }
}

/// Discovers Codex models from the ChatGPT backend.
///
/// The payload must contain an `items` or `models` array whose entries carry a `slug` string.
async fn discover_codex(
    http: &Client,
    provider: &Provider,
    secret: &str,
) -> Result<Vec<String>, Error> {
    let account_id = match &provider.credentials {
        Credentials::OpenAiOauth { account_id, .. } => account_id.clone(),

        _ => {
            return Err(Error::Internal {
                message: "codex discovery requires OpenAI OAuth credentials".to_string(),
            });
        }
    };

    let request = http
        .get(endpoints::CODEX_MODELS_URL)
        .query(&[("client_version", endpoints::CODEX_CLIENT_VERSION)])
        .bearer_auth(secret)
        .header(endpoints::CODEX_ACCOUNT_ID_HEADER, account_id);

    let response: CodexModelsResponse = http_body::send_and_parse_json(request, "upstream").await?;

    let entries = response.items.ok_or_else(|| Error::Unavailable {
        detail: "model discovery payload lacks an `items` or `models` array".to_string(),
    })?;

    entries
        .into_iter()
        .map(|entry| {
            entry.slug.ok_or_else(|| Error::Unavailable {
                detail: "model discovery entry lacks a `slug` string".to_string(),
            })
        })
        .collect()
}

/// Discovers Antigravity models.
///
/// The reference (`fetchAvailableModels` in `src/plugin/quota.ts` at rev `16e0056`) posts
/// `{project}` to the production base URL; the gateway posts the empty object to the daily base
/// URL — both shapes are accepted, and the discovery is verified live.
async fn discover_antigravity(http: &Client, secret: &str) -> Result<Vec<String>, Error> {
    let response: AvailableModelsResponse = oauth::post_antigravity(
        http,
        endpoints::ANTIGRAVITY_FETCH_AVAILABLE_MODELS_ACTION,
        secret,
        &FetchAvailableModelsRequest {},
    )
    .await?;

    let models = response.models.ok_or_else(|| Error::Unavailable {
        detail: "model discovery payload lacks a `models` object".to_string(),
    })?;

    Ok(models.into_keys().collect())
}

/// Discovers models from an OpenAI-compatible upstream, shaped by its protocol.
async fn discover_compatible(
    http: &Client,
    provider: &Provider,
    secret: &str,
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

    match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            let request = http.get(format!("{base_url}/models")).bearer_auth(secret);

            let response: CompatibleModelsResponse =
                http_body::send_and_parse_json(request, "upstream").await?;

            let entries = response.data.ok_or_else(|| Error::Unavailable {
                detail: "model discovery payload lacks a `data` array".to_string(),
            })?;

            entries
                .into_iter()
                .map(|entry| {
                    entry.id.ok_or_else(|| Error::Unavailable {
                        detail: "model discovery entry lacks an `id` string".to_string(),
                    })
                })
                .collect()
        }

        UpstreamFormat::GeminiGenerateContent => {
            let request = http
                .get(format!("{base_url}/models"))
                .header(endpoints::GEMINI_API_KEY_HEADER, secret);

            let response: GeminiModelsResponse =
                http_body::send_and_parse_json(request, "upstream").await?;

            let entries = response.models.ok_or_else(|| Error::Unavailable {
                detail: "model discovery payload lacks a `models` array".to_string(),
            })?;

            entries
                .into_iter()
                .map(|entry| {
                    let name = entry.name.ok_or_else(|| Error::Unavailable {
                        detail: "model discovery entry lacks a `name` string".to_string(),
                    })?;

                    Ok(name.strip_prefix("models/").unwrap_or(&name).to_string())
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `fetchAvailableModels` action takes no inputs, so its body is the empty JSON object
    /// (README §5.2): the request keeps its shape however the request struct is refactored.
    ///
    /// # Panics
    ///
    /// Panics if the empty request body does not render as JSON.
    #[test]
    fn renders_the_empty_model_discovery_body() {
        #[expect(
            clippy::unwrap_used,
            reason = "the empty request body always serializes"
        )]
        let body = serde_json::to_string(&FetchAvailableModelsRequest {}).unwrap();

        assert_eq!(body, "{}");
    }

    /// The vendor serves the Codex model list under `items` or `models` (README §5.1), and the
    /// upstream payload carries fields the gateway does not model: both key spellings yield the
    /// same entries, and unknown fields are ignored rather than failing discovery.
    ///
    /// # Panics
    ///
    /// Panics if the payload or its entries do not parse.
    #[test]
    fn codex_model_list_accepts_both_key_spellings() {
        for json in [
            r#"{"items": [{"slug": "gpt-5", "unmodeled": true}], "unknown": 1}"#,
            r#"{"models": [{"slug": "gpt-5", "unmodeled": true}], "unknown": 1}"#,
        ] {
            #[expect(
                clippy::unwrap_used,
                reason = "both payload spellings are constructed to parse"
            )]
            let response: CodexModelsResponse = serde_json::from_str(json).unwrap();

            #[expect(
                clippy::unwrap_used,
                reason = "both payloads carry the items array and each entry a slug"
            )]
            let slugs = response
                .items
                .unwrap()
                .into_iter()
                .map(|entry| entry.slug.unwrap())
                .collect::<Vec<_>>();

            assert_eq!(slugs, ["gpt-5"]);
        }
    }
}
