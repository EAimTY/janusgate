//! Provider entities, credentials, upstream storage, OAuth flows, and model
//! discovery.

use crate::{error::ApiError, format::UpstreamFormat};

pub(crate) mod discovery;
pub(crate) mod endpoints;
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
    /// Every arm enforces its exact field set: an unknown field is rejected
    /// with an [`ApiError`] naming it, because a tolerated extra field — such
    /// as an admin-pasted `expiresAt` — would be silently dropped on storage
    /// and change the credentials' semantics without notice.
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

    /// Upstream wire format; required for `openai_compatible`.
    pub(crate) protocol: Option<UpstreamFormat>,

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
    /// The upstream wire format the provider speaks: the type-fixed format
    /// when the account kind pins one, otherwise the configured `protocol`.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when an `openai_compatible` provider carries no
    /// protocol, which is a configuration invariant violation.
    pub(crate) fn upstream_format(&self) -> Result<UpstreamFormat, ApiError> {
        match self.provider_type.upstream_format() {
            Some(format) => Ok(format),
            None => self.protocol.ok_or_else(|| {
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

/// Rejects a credentials payload carrying fields outside the provider type's
/// exact field set. Non-object payloads are accepted here; they fail the
/// required-field checks that follow.
///
/// # Errors
///
/// Returns [`ApiError`] naming the first unknown field.
fn ensure_exact_credential_fields(
    value: &serde_json::Value,
    allowed_fields: &[&str],
) -> Result<(), ApiError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };

    for field_name in object.keys() {
        if !allowed_fields.contains(&field_name.as_str()) {
            let allowed = allowed_fields.join("`, `");

            return Err(ApiError::invalid_parameter(
                "credentials",
                format!("Credentials carry an unknown `{field_name}` field; allowed: `{allowed}`"),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies both OAuth credential arms reject payloads carrying fields
    /// outside their exact field set, naming the unknown field: a tolerated
    /// extra field — such as an admin-pasted `expiresAt` — would be silently
    /// dropped on storage and change the credentials' semantics without
    /// notice.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn oauth_credentials_reject_unknown_fields() {
        for (provider_type, json) in [
            (
                ProviderType::OpenAiOauth,
                r#"{"accessToken":"a","refreshToken":"r","accountId":"x","expiresAt":123}"#,
            ),
            (
                ProviderType::AntigravityOauth,
                r#"{"accessToken":"a","refreshToken":"r","projectId":"p","expiresAt":123}"#,
            ),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "the extra field must make parsing fail; the failure is the subject"
            )]
            let error = Credentials::parse(provider_type, json)
                .expect_err("an unknown credentials field must be rejected");

            assert_eq!(error.status, 400);
            assert_eq!(error.param.as_deref(), Some("credentials"));
            assert!(
                error.message.contains("expiresAt"),
                "the error must name the unknown field: {error}"
            );
        }
    }

    /// Verifies exact-field OAuth payloads still parse into their credential
    /// values, and the API key arm keeps rejecting extra fields.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn credentials_parse_exact_field_sets() {
        #[expect(
            clippy::expect_used,
            reason = "exact-field credentials are well-formed by construction"
        )]
        let openai = Credentials::parse(
            ProviderType::OpenAiOauth,
            r#"{"accessToken":"a","refreshToken":"r","accountId":"x"}"#,
        )
        .expect("exact OpenAI OAuth credentials must parse");

        assert_eq!(
            openai,
            Credentials::OpenAiOauth {
                access_token: "a".to_string(),
                refresh_token: "r".to_string(),
                account_id: "x".to_string(),
            }
        );

        #[expect(
            clippy::expect_used,
            reason = "exact-field credentials are well-formed by construction"
        )]
        let antigravity = Credentials::parse(
            ProviderType::AntigravityOauth,
            r#"{"accessToken":"a","refreshToken":"r","projectId":"p"}"#,
        )
        .expect("exact Antigravity OAuth credentials must parse");

        assert_eq!(
            antigravity,
            Credentials::AntigravityOauth {
                access_token: "a".to_string(),
                refresh_token: "r".to_string(),
                project_id: "p".to_string(),
            }
        );

        #[expect(
            clippy::expect_used,
            reason = "the extra field must make parsing fail; the failure is the subject"
        )]
        let api_key = Credentials::parse(
            ProviderType::OpenAiCompatible,
            r#"{"apiKey":"k","extra":true}"#,
        )
        .expect_err("api key credentials with extra fields must be rejected");

        assert_eq!(api_key.status, 400);
    }
}
