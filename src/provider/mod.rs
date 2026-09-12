//! Provider entities, credentials, upstream storage, OAuth flows, and model
//! discovery.

use self::storage::Error as StorageError;
use crate::format::UpstreamFormat;
use turso::{Value, params::IntoValue};

pub(crate) mod discovery;
pub(crate) mod endpoints;
pub(crate) mod http_body;
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

impl IntoValue for ProviderType {
    fn into_value(self) -> Result<Value, turso::Error> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl TryFrom<&Value> for ProviderType {
    type Error = StorageError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Text(text) => Self::parse(text).ok_or_else(|| StorageError::Drift {
                message: format!("unknown provider type `{text}`"),
            }),
            _ => Err(StorageError::Drift {
                message: "provider type column must be a text value".to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for ProviderType {
    type Error = StorageError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

/// Kinds of upstream provider accounts that use OAuth authorization flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OAuthProviderType {
    /// ChatGPT account speaking the Codex Responses protocol.
    OpenAiOauth,

    /// Google account speaking Gemini GenerateContent via Code Assist.
    AntigravityOauth,
}

impl OAuthProviderType {
    /// Converts this OAuth flow kind into its corresponding provider type.
    #[must_use]
    pub(crate) const fn as_provider_type(self) -> ProviderType {
        match self {
            Self::OpenAiOauth => ProviderType::OpenAiOauth,
            Self::AntigravityOauth => ProviderType::AntigravityOauth,
        }
    }

    /// Parses the canonical OAuth provider type name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_oauth" => Some(Self::OpenAiOauth),
            "antigravity_oauth" => Some(Self::AntigravityOauth),
            _ => None,
        }
    }

    /// Renders the canonical OAuth provider type name.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiOauth => "openai_oauth",
            Self::AntigravityOauth => "antigravity_oauth",
        }
    }
}

impl IntoValue for OAuthProviderType {
    fn into_value(self) -> Result<Value, turso::Error> {
        Ok(Value::Text(self.as_str().to_string()))
    }
}

impl TryFrom<&Value> for OAuthProviderType {
    type Error = StorageError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Text(text) => Self::parse(text).ok_or_else(|| StorageError::Drift {
                message: format!("unknown OAuth provider type `{text}`"),
            }),
            _ => Err(StorageError::Drift {
                message: "OAuth state type column must be a text value".to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for OAuthProviderType {
    type Error = StorageError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from(&value)
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

/// A credentials parse failure, typed so each boundary renders its own error
/// domain: management input validation maps it to a 400 invalid request
/// naming `credentials`, while storage row decoding wraps it once as a 500
/// internal error — stored credentials are written only through the validated
/// management path, so a payload that fails there is database drift and never
/// a client fault.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ParseError {
    /// The payload is not valid JSON.
    #[error("Credentials are not valid JSON: {0}")]
    Json(serde_json::Error),

    /// A required field is absent or not a string.
    #[error("Credentials must carry a string `{0}` field")]
    MissingField(String),

    /// The payload carries a field outside the provider type's exact field
    /// set.
    #[error("Credentials carry an unknown `{0}` field; allowed: `{1}`")]
    UnknownField(String, String),
}

impl Credentials {
    /// Parses credentials JSON for the given provider type.
    ///
    /// Every arm enforces its exact field set: an unknown field is rejected
    /// naming it, because a tolerated extra field — such as an admin-pasted
    /// `expiresAt` — would be silently dropped on storage and change the
    /// credentials' semantics without notice.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the payload does not match the shape
    /// required by the provider type.
    pub(crate) fn parse(provider_type: ProviderType, json: &str) -> Result<Self, ParseError> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(ParseError::Json)?;

        let field = |name: &str| -> Result<String, ParseError> {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ParseError::MissingField(name.to_string()))
        };

        match provider_type {
            ProviderType::OpenAiOauth => {
                ensure_exact_credential_fields(
                    &value,
                    &["accessToken", "refreshToken", "accountId"],
                )?;

                Ok(Self::OpenAiOauth {
                    access_token: field("accessToken")?,
                    refresh_token: field("refreshToken")?,
                    account_id: field("accountId")?,
                })
            }

            ProviderType::AntigravityOauth => {
                ensure_exact_credential_fields(
                    &value,
                    &["accessToken", "refreshToken", "projectId"],
                )?;

                Ok(Self::AntigravityOauth {
                    access_token: field("accessToken")?,
                    refresh_token: field("refreshToken")?,
                    project_id: field("projectId")?,
                })
            }

            ProviderType::OpenAiCompatible => {
                ensure_exact_credential_fields(&value, &["apiKey"])?;

                Ok(Self::ApiKey(field("apiKey")?))
            }
        }
    }

    /// The secret these credentials present to an upstream: the OAuth access
    /// token for OAuth accounts, the static API key for key-based accounts.
    /// Depending on the wire format it is sent as a bearer token or as an
    /// API key header.
    pub(crate) fn secret(&self) -> &str {
        match self {
            Self::OpenAiOauth { access_token, .. }
            | Self::AntigravityOauth { access_token, .. } => access_token,

            Self::ApiKey(api_key) => api_key,
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

    /// Upstream wire format; required for `openai_compatible`.
    pub(crate) protocol: Option<UpstreamFormat>,

    /// Upstream base URL; required for `openai_compatible`.
    pub(crate) base_url: Option<&'a str>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: &'a Credentials,

    /// Unix timestamp when the OAuth access token expires.
    pub(crate) expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,
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

    /// Upstream wire format; required for `openai_compatible`.
    pub(crate) protocol: Option<UpstreamFormat>,

    /// Upstream base URL; required for `openai_compatible`.
    pub(crate) base_url: Option<String>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: Credentials,

    /// Unix timestamp when the OAuth access token expires.
    pub(crate) expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,
}

impl Provider {
    /// The upstream wire format the provider speaks: the type-fixed format
    /// when the account kind pins one, otherwise the configured `protocol`.
    /// `None` means an `openai_compatible` provider carries no protocol,
    /// which is a configuration invariant violation the caller fails fast
    /// on.
    pub(crate) fn upstream_format(&self) -> Option<UpstreamFormat> {
        match self.provider_type.upstream_format() {
            Some(format) => Some(format),
            None => self.protocol,
        }
    }
}

/// A stored model row.
#[derive(Debug, Clone)]
pub(crate) struct Model {
    /// Owning provider row identifier.
    pub(crate) provider_id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_id: String,

    /// Public routing ID, maintained at write time.
    pub(crate) id: String,

    /// Non-`None` explicitly sets the public routing ID directly, bypassing
    /// the provider's `model_prefix`.
    pub(crate) id_override: Option<String>,

    /// Whether the model is enabled to serve traffic under its provider.
    pub(crate) is_enabled: bool,

    /// JSON array of canonical thinking levels, when filtering is configured.
    pub(crate) thinking_levels_override: Option<String>,
}

/// A provider row joined with the model row a public ID resolved to.
#[derive(Debug, Clone)]
pub(crate) struct ModelRouting {
    /// Owning provider.
    pub(crate) provider: Provider,

    /// Resolved model row.
    pub(crate) model: Model,

    /// Whether the model effectively serves traffic
    /// ([`Model::is_effectively_enabled`]).
    pub(crate) is_enabled: bool,
}

impl Model {
    /// The public routing ID the model carries: an explicit override directly
    /// replaces the ID without prefix, else `model_prefix + upstream_id`.
    pub(crate) fn public_id(
        model_prefix: &str,
        upstream_id: &str,
        id_override: Option<&str>,
    ) -> String {
        match id_override {
            Some(id) => id.to_string(),
            None => format!("{model_prefix}{upstream_id}"),
        }
    }

    /// The model's effective serving status: true if and only if both the
    /// owning provider and the model itself are enabled (README §3).
    ///
    /// The provider's `enabled` flag serves as the master switch: disabling a
    /// provider turns off all its models unconditionally.
    pub(crate) fn is_effectively_enabled(&self, is_provider_enabled: bool) -> bool {
        is_provider_enabled && self.is_enabled
    }
}

/// Rejects a credentials payload carrying fields outside the provider type's
/// exact field set. Non-object payloads are accepted here; they fail the
/// required-field checks that follow.
///
/// # Errors
///
/// Returns [`ParseError::UnknownField`] naming the first unknown field.
fn ensure_exact_credential_fields(
    value: &serde_json::Value,
    allowed_fields: &[&str],
) -> Result<(), ParseError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };

    for field_name in object.keys() {
        if !allowed_fields.contains(&field_name.as_str()) {
            let allowed = allowed_fields.join("`, `");

            return Err(ParseError::UnknownField(field_name.to_string(), allowed));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `Model::public_id` uses `model_prefix + upstream_id` when
    /// unoverridden, and uses `id_override` directly without `model_prefix`
    /// when an override is present.
    ///
    /// # Panics
    ///
    /// Panics if public ID composition assertions fail.
    #[test]
    fn composes_public_id() {
        let default_id = Model::public_id("codex/", "gpt-4o", None);
        assert_eq!(default_id, "codex/gpt-4o");

        let overridden_id = Model::public_id("codex/", "gpt-4o", Some("custom-gpt-4o"));
        assert_eq!(overridden_id, "custom-gpt-4o");

        let empty_prefix_id = Model::public_id("", "gemini-pro", None);
        assert_eq!(empty_prefix_id, "gemini-pro");
    }

    /// Verifies that `Model::is_effectively_enabled` requires both the provider
    /// and the model to be enabled.
    ///
    /// # Panics
    ///
    /// Panics if effective enabled assertions fail.
    #[test]
    fn resolves_effective_enabled_status() {
        let model_enabled = Model {
            provider_id: 1,
            upstream_id: "m1".to_string(),
            id: "m1".to_string(),
            id_override: None,
            is_enabled: true,
            thinking_levels_override: None,
        };
        assert!(model_enabled.is_effectively_enabled(true));
        assert!(!model_enabled.is_effectively_enabled(false));

        let model_disabled = Model {
            provider_id: 1,
            upstream_id: "m2".to_string(),
            id: "m2".to_string(),
            id_override: None,
            is_enabled: false,
            thinking_levels_override: None,
        };
        assert!(!model_disabled.is_effectively_enabled(true));
        assert!(!model_disabled.is_effectively_enabled(false));
    }

    /// Verifies that `OAuthProviderType` accurately maps to `ProviderType`
    /// without relying on an implicit `From` implementation.
    ///
    /// # Panics
    ///
    /// Panics if provider type mapping assertions fail.
    #[test]
    fn maps_oauth_provider_type_to_provider_type() {
        assert_eq!(
            OAuthProviderType::OpenAiOauth.as_provider_type(),
            ProviderType::OpenAiOauth
        );
        assert_eq!(
            OAuthProviderType::AntigravityOauth.as_provider_type(),
            ProviderType::AntigravityOauth
        );
    }

    /// Verifies value conversions between database values and provider types.
    ///
    /// # Panics
    ///
    /// Panics if value serialization or deserialization fails.
    #[test]
    fn converts_provider_type_database_values() {
        for provider_type in [
            ProviderType::OpenAiOauth,
            ProviderType::AntigravityOauth,
            ProviderType::OpenAiCompatible,
        ] {
            #[expect(clippy::unwrap_used, reason = "into_value returns Ok for ProviderType")]
            let value = provider_type.into_value().unwrap();
            assert_eq!(value, Value::Text(provider_type.as_str().to_string()));

            #[expect(clippy::unwrap_used, reason = "try_from returns Ok for valid Text")]
            let parsed = ProviderType::try_from(&value).unwrap();
            assert_eq!(parsed, provider_type);
        }

        let invalid = Value::Text("unknown_type".to_string());
        assert!(ProviderType::try_from(&invalid).is_err());

        let non_text = Value::Integer(42);
        assert!(ProviderType::try_from(&non_text).is_err());
    }

    /// Verifies value conversions between database values and OAuth provider
    /// types.
    ///
    /// # Panics
    ///
    /// Panics if value serialization or deserialization fails.
    #[test]
    fn converts_oauth_provider_type_database_values() {
        for oauth_type in [
            OAuthProviderType::OpenAiOauth,
            OAuthProviderType::AntigravityOauth,
        ] {
            #[expect(
                clippy::unwrap_used,
                reason = "into_value returns Ok for OAuthProviderType"
            )]
            let value = oauth_type.into_value().unwrap();
            assert_eq!(value, Value::Text(oauth_type.as_str().to_string()));

            #[expect(clippy::unwrap_used, reason = "try_from returns Ok for valid Text")]
            let parsed = OAuthProviderType::try_from(&value).unwrap();
            assert_eq!(parsed, oauth_type);
        }

        let incompatible = Value::Text("openai_compatible".to_string());
        assert!(OAuthProviderType::try_from(&incompatible).is_err());

        let invalid = Value::Text("unknown_type".to_string());
        assert!(OAuthProviderType::try_from(&invalid).is_err());

        let non_text = Value::Integer(42);
        assert!(OAuthProviderType::try_from(&non_text).is_err());
    }
}
