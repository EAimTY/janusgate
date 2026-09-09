//! Provider entities, credentials, upstream storage, OAuth flows, and model
//! discovery.

use crate::{error::ApiError, format::UpstreamFormat};

pub(crate) mod discovery;
pub(crate) mod oauth;
pub(crate) mod storage;

/// Kinds of upstream provider accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderType {
    /// ChatGPT account speaking the Codex Responses protocol.
    OpenAiOauth,

    /// Google account speaking Gemini GenerateContent via Code Assist.
    AntigravityOauth,

    /// Static API key account speaking a configured protocol.
    OpenAiCompatible,
}

impl ProviderType {
    /// Parses the canonical provider type name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_oauth" => Some(Self::OpenAiOauth),
            "antigravity_oauth" => Some(Self::AntigravityOauth),
            "openai_compatible" => Some(Self::OpenAiCompatible),
            _ => None,
        }
    }

    /// Renders the canonical provider type name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiOauth => "openai_oauth",
            Self::AntigravityOauth => "antigravity_oauth",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }

    /// The upstream wire format the provider type always speaks, when fixed.
    pub(crate) fn upstream_format(self) -> Option<UpstreamFormat> {
        match self {
            Self::OpenAiOauth => Some(UpstreamFormat::OpenAiResponse),
            Self::AntigravityOauth => Some(UpstreamFormat::GeminiGenerateContent),
            Self::OpenAiCompatible => None,
        }
    }
}

/// Upstream wire protocol of an `openai_compatible` provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpstreamProtocol {
    /// OpenAI Responses protocol.
    OpenAiResponse,

    /// OpenAI Chat Completions protocol.
    OpenAiChatCompletion,

    /// Gemini GenerateContent protocol.
    GeminiGenerateContent,
}

impl UpstreamProtocol {
    /// Parses the canonical protocol name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_response" => Some(Self::OpenAiResponse),
            "openai_chat_completion" => Some(Self::OpenAiChatCompletion),
            "gemini_generate_content" => Some(Self::GeminiGenerateContent),
            _ => None,
        }
    }

    /// Renders the canonical protocol name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponse => "openai_response",
            Self::OpenAiChatCompletion => "openai_chat_completion",
            Self::GeminiGenerateContent => "gemini_generate_content",
        }
    }

    /// The wire format this protocol speaks.
    pub(crate) fn upstream_format(self) -> UpstreamFormat {
        match self {
            Self::OpenAiResponse => UpstreamFormat::OpenAiResponse,
            Self::OpenAiChatCompletion => UpstreamFormat::OpenAiChatCompletion,
            Self::GeminiGenerateContent => UpstreamFormat::GeminiGenerateContent,
        }
    }
}

/// Upstream credentials stored as plaintext JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credentials {
    /// ChatGPT OAuth credentials; the account ID accompanies every upstream
    /// request as the `chatgpt-account-id` header.
    OpenAiOauth {
        /// OAuth access token.
        access_token: String,

        /// OAuth refresh token.
        refresh_token: String,

        /// ChatGPT account ID from the `id_token`.
        account_id: String,
    },

    /// Google OAuth credentials; the Cloud Code Assist project ID accompanies
    /// the Antigravity envelope.
    AntigravityOauth {
        /// OAuth access token.
        access_token: String,

        /// OAuth refresh token.
        refresh_token: String,

        /// Cloud Code Assist project ID.
        project_id: String,
    },

    /// Static API key.
    ApiKey(String),
}

impl Credentials {
    /// Parses credentials JSON for the given provider type.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the payload does not match the shape
    /// required by the provider type.
    pub(crate) fn parse(provider_type: ProviderType, json: &str) -> Result<Self, ApiError> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|error| {
            ApiError::invalid_parameter(
                "credentials",
                format!("Credentials are not valid JSON: {error}"),
            )
        })?;

        let field = |name: &str| -> Result<String, ApiError> {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    ApiError::invalid_parameter(
                        "credentials",
                        format!("Credentials must carry a string `{name}` field"),
                    )
                })
        };

        match provider_type {
            ProviderType::OpenAiOauth => Ok(Self::OpenAiOauth {
                access_token: field("accessToken")?,
                refresh_token: field("refreshToken")?,
                account_id: field("accountId")?,
            }),

            ProviderType::AntigravityOauth => Ok(Self::AntigravityOauth {
                access_token: field("accessToken")?,
                refresh_token: field("refreshToken")?,
                project_id: field("projectId")?,
            }),

            ProviderType::OpenAiCompatible => {
                let api_key = field("apiKey")?;

                if value.as_object().is_some_and(|object| object.len() != 1) {
                    return Err(ApiError::invalid_parameter(
                        "credentials",
                        "API key credentials must carry only an `apiKey` field",
                    ));
                }

                Ok(Self::ApiKey(api_key))
            }
        }
    }

    /// Renders the credentials into their plaintext JSON storage form.
    pub(crate) fn to_json(&self) -> String {
        match self {
            Self::OpenAiOauth {
                access_token,
                refresh_token,
                account_id,
            } => serde_json::json!({
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "accountId": account_id,
            })
            .to_string(),

            Self::AntigravityOauth {
                access_token,
                refresh_token,
                project_id,
            } => serde_json::json!({
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "projectId": project_id,
            })
            .to_string(),

            Self::ApiKey(api_key) => serde_json::json!({"apiKey": api_key}).to_string(),
        }
    }
}

/// The fields of a provider about to be inserted.
#[derive(Debug, Clone)]
pub(crate) struct NewProvider<'a> {
    /// Unique immutable identifier.
    pub(crate) name: &'a str,

    /// Prefix prepended to every public model name.
    pub(crate) model_prefix: &'a str,

    /// Account kind.
    pub(crate) provider_type: ProviderType,

    /// Upstream protocol; required for `openai_compatible`.
    pub(crate) protocol: Option<UpstreamProtocol>,

    /// Upstream base URL; required for `openai_compatible`.
    pub(crate) base_url: Option<&'a str>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: &'a Credentials,

    /// Unix timestamp when the OAuth access token expires.
    pub(crate) expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub(crate) enabled: bool,
}

/// A stored upstream provider with its parsed credentials.
#[derive(Debug, Clone)]
pub(crate) struct Provider {
    /// Row identifier.
    pub(crate) id: i64,

    /// Unique immutable identifier.
    pub(crate) name: String,

    /// Prefix prepended to every public model name.
    pub(crate) model_prefix: String,

    /// Account kind.
    pub(crate) provider_type: ProviderType,

    /// Upstream protocol; required for `openai_compatible`.
    pub(crate) protocol: Option<UpstreamProtocol>,

    /// Upstream base URL; required for `openai_compatible`.
    pub(crate) base_url: Option<String>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: Credentials,

    /// Unix timestamp when the OAuth access token expires.
    pub(crate) expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub(crate) enabled: bool,
}

impl Provider {
    /// The upstream wire format the provider speaks.
    pub(crate) fn upstream_format(&self) -> Result<UpstreamFormat, ApiError> {
        match self.provider_type.upstream_format() {
            Some(format) => Ok(format),
            None => self
                .protocol
                .map(UpstreamProtocol::upstream_format)
                .ok_or_else(|| {
                    ApiError::internal(format!(
                        "provider `{}` has no upstream protocol configured",
                        self.name
                    ))
                }),
        }
    }
}

/// A stored model row.
#[derive(Debug, Clone)]
pub(crate) struct Model {
    /// Owning provider row identifier.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_name: String,

    /// Public routing name, maintained at write time.
    pub(crate) name: String,

    /// Non-`None` replaces the upstream name as the base of the public name.
    pub(crate) name_override: Option<String>,

    /// Non-`None` pins the serving state instead of the discovery default.
    pub(crate) enabled_override: Option<bool>,

    /// JSON array of canonical thinking levels, when filtering is configured.
    pub(crate) thinking_levels_override: Option<String>,
}

/// A provider row joined with the model row a public name resolved to.
#[derive(Debug, Clone)]
pub(crate) struct ModelRouting {
    /// Owning provider.
    pub(crate) provider: Provider,

    /// Resolved model row.
    pub(crate) model: Model,

    /// Whether the model effectively serves traffic
    /// (`enabled_override ?? provider.enabled`).
    pub(crate) is_enabled: bool,
}

impl Model {
    /// The public routing name the model would carry under the given prefix.
    pub(crate) fn public_name(
        model_prefix: &str,
        upstream_name: &str,
        name_override: Option<&str>,
    ) -> String {
        format!("{model_prefix}{}", name_override.unwrap_or(upstream_name))
    }
}

/// Validates a provider identifier against `[a-z0-9_-]{1,64}`.
///
/// # Errors
///
/// Returns [`ApiError`] when the name is invalid.
pub(crate) fn validate_provider_name(name: &str) -> Result<(), ApiError> {
    let is_valid = (1..=64).contains(&name.len())
        && name.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        });

    if is_valid {
        Ok(())
    } else {
        Err(ApiError::invalid_parameter(
            "name",
            "Provider names must match [a-z0-9_-]{1,64}",
        ))
    }
}
