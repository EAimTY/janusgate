//! Provider entities, credentials, upstream storage, OAuth flows, and model discovery.

use crate::format::UpstreamFormat;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, IgnoredAny},
};
use serde_json::Error as JsonError;
use std::collections::BTreeMap;

pub(crate) mod discovery;
pub(crate) mod endpoints;
pub(crate) mod http_body;
pub(crate) mod oauth;
pub(crate) mod storage;

/// Kinds of upstream providers: one per upstream product — a static-key wire
/// format or an OAuth login — so the type pins the wire format outright and no
/// second column qualifies it.
///
/// The serde and TypeScript names are the management wire names, shared with
/// `ProviderType::as_str` — the storage form and the wire form are one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum ProviderType {
    /// Static API key speaking the OpenAI Responses protocol.
    #[serde(rename = "openai_response")]
    OpenAiResponse,

    /// Static API key speaking the OpenAI Chat Completions protocol.
    #[serde(rename = "openai_chat_completion")]
    OpenAiChatCompletion,

    /// Static API key speaking the bare Gemini GenerateContent protocol.
    #[serde(rename = "gemini_generate_content")]
    GeminiGenerateContent,

    /// ChatGPT account speaking the Codex Responses protocol.
    #[serde(rename = "codex_oauth")]
    CodexOauth,

    /// Google account speaking Gemini GenerateContent via Antigravity.
    #[serde(rename = "antigravity_oauth")]
    AntigravityOauth,
}

impl ProviderType {
    /// Parses the canonical provider type name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_response" => Some(Self::OpenAiResponse),
            "openai_chat_completion" => Some(Self::OpenAiChatCompletion),
            "gemini_generate_content" => Some(Self::GeminiGenerateContent),
            "codex_oauth" => Some(Self::CodexOauth),
            "antigravity_oauth" => Some(Self::AntigravityOauth),
            _ => None,
        }
    }

    /// Renders the canonical provider type name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponse => "openai_response",
            Self::OpenAiChatCompletion => "openai_chat_completion",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::CodexOauth => "codex_oauth",
            Self::AntigravityOauth => "antigravity_oauth",
        }
    }

    /// Whether the provider authorizes through an OAuth flow with gateway-fixed
    /// endpoints, as opposed to a static key against a caller-supplied base URL.
    #[must_use]
    pub(crate) const fn is_oauth(self) -> bool {
        match self {
            Self::CodexOauth | Self::AntigravityOauth => true,
            Self::OpenAiResponse | Self::OpenAiChatCompletion | Self::GeminiGenerateContent => {
                false
            }
        }
    }

    /// The upstream wire format the provider type speaks.
    #[must_use]
    pub(crate) const fn upstream_format(self) -> UpstreamFormat {
        match self {
            Self::OpenAiResponse | Self::CodexOauth => UpstreamFormat::OpenAiResponse,
            Self::OpenAiChatCompletion => UpstreamFormat::OpenAiChatCompletion,
            Self::GeminiGenerateContent | Self::AntigravityOauth => {
                UpstreamFormat::GeminiGenerateContent
            }
        }
    }
}

/// Kinds of upstream provider accounts that use OAuth authorization flows.
///
/// The serde and TypeScript names are the management wire names, shared with
/// `OAuthProviderType::as_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum OAuthProviderType {
    /// ChatGPT account speaking the Codex Responses protocol.
    #[serde(rename = "codex_oauth")]
    CodexOauth,

    /// Google account speaking Gemini GenerateContent via Antigravity.
    #[serde(rename = "antigravity_oauth")]
    AntigravityOauth,
}

impl OAuthProviderType {
    /// Converts this OAuth flow kind into its corresponding provider type.
    #[must_use]
    pub(crate) const fn as_provider_type(self) -> ProviderType {
        match self {
            Self::CodexOauth => ProviderType::CodexOauth,
            Self::AntigravityOauth => ProviderType::AntigravityOauth,
        }
    }

    /// Parses the canonical OAuth provider type name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "codex_oauth" => Some(Self::CodexOauth),
            "antigravity_oauth" => Some(Self::AntigravityOauth),
            _ => None,
        }
    }

    /// Renders the canonical OAuth provider type name.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CodexOauth => "codex_oauth",
            Self::AntigravityOauth => "antigravity_oauth",
        }
    }
}

/// Upstream credentials stored as plaintext JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credentials {
    /// ChatGPT OAuth credentials; the account ID accompanies every upstream request as the
    /// `chatgpt-account-id` header.
    CodexOauth {
        /// OAuth access token.
        access_token: String,

        /// OAuth refresh token.
        refresh_token: String,

        /// ChatGPT account ID from the `id_token`.
        account_id: String,
    },

    /// Google OAuth credentials; the Antigravity project ID accompanies the Antigravity
    /// envelope.
    AntigravityOauth {
        /// OAuth access token.
        access_token: String,

        /// OAuth refresh token.
        refresh_token: String,

        /// Antigravity project ID.
        project_id: String,
    },

    /// Static API key.
    ApiKey(String),
}

/// The plaintext storage form of `codex_oauth` credentials (README §3): `{"accessToken",
/// "refreshToken", "accountId"}` and nothing else, so a stored payload can never drift from the
/// documented shape.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredCodexOauthCredentials<'a> {
    /// OAuth access token.
    access_token: &'a str,

    /// OAuth refresh token.
    refresh_token: &'a str,

    /// ChatGPT account ID from the `id_token`.
    account_id: &'a str,
}

/// The plaintext storage form of `antigravity_oauth` credentials (README §3): `{"accessToken",
/// "refreshToken", "projectId"}` and nothing else.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredAntigravityOauthCredentials<'a> {
    /// OAuth access token.
    access_token: &'a str,

    /// OAuth refresh token.
    refresh_token: &'a str,

    /// Antigravity project ID.
    project_id: &'a str,
}

/// The plaintext storage form of static-key credentials (README §3): `{"apiKey"}` and
/// nothing else.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredApiKeyCredentials<'a> {
    /// Static API key.
    api_key: &'a str,
}

/// The discovery lifecycle state of a model row (README §3): which role produced the row and
/// what the most recent successful discovery said about it.
///
/// The serde and TypeScript names are the management wire names, shared with
/// `DiscoveryState::as_str` — the storage form and the wire form are one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum DiscoveryState {
    /// Listed by the most recent successful discovery of its provider.
    #[serde(rename = "discovered")]
    Discovered,

    /// Was listed by an earlier discovery but missing from the most recent one: service is
    /// suspended (the three-input effective status is false) until the upstream lists it again or
    /// an administrator claims it through `models:create`.
    #[serde(rename = "vanished")]
    Vanished,

    /// Created — or claimed out of a vanished row — through `models:create`; a discovery that
    /// lists its identifier flips it to [`Discovered`](Self::Discovered), one that does not leaves
    /// it alone: a manual row is never automatically marked `vanished`.
    #[serde(rename = "manual")]
    Manual,
}

impl DiscoveryState {
    /// Parses the canonical discovery state name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "discovered" => Some(Self::Discovered),
            "vanished" => Some(Self::Vanished),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }

    /// Renders the canonical discovery state name.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Vanished => "vanished",
            Self::Manual => "manual",
        }
    }
}

/// A stored credentials value: the string it carries, or any other JSON value, which carries no
/// string. [`Credentials::parse`] rejects a field carrying a non-string value exactly like a field
/// the payload omits, so both read as no string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StoredFieldValue {
    /// The field carries a JSON string.
    Text(String),

    /// The field carries any other JSON value — `null`, a number, a boolean, an array, or an object
    /// — and therefore no string.
    NonString(IgnoredAny),
}

impl StoredFieldValue {
    /// The string the value carries, when it carries one.
    fn into_string(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            Self::NonString(_) => None,
        }
    }
}

/// A stored credentials payload (README §3): the plaintext JSON object of one provider kind's
/// lenient field collection, or any other JSON value, which carries no fields and fails the
/// required-field verdict exactly like an empty object.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StoredCredentialsPayload<T> {
    /// The stored credentials object.
    Object(T),

    /// Any JSON value that is not an object.
    NonObject(IgnoredAny),
}

/// The lenient read form of `codex_oauth` credentials (README §3): each stored field's string
/// value — absent, `null`, and non-string values all reading as absent — plus every key the payload
/// carries outside the exact field set. Collecting never rejects anything; the verdict is
/// [`Credentials::parse`]'s.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCodexOauthFields {
    /// `accessToken` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    access_token: Option<String>,

    /// `refreshToken` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    refresh_token: Option<String>,

    /// `accountId` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    account_id: Option<String>,

    /// Every key the payload carries outside the exact stored field set.
    #[serde(flatten)]
    extra: BTreeMap<String, IgnoredAny>,
}

/// The lenient read form of `antigravity_oauth` credentials (README §3); see
/// [`StoredCodexOauthFields`] for the reading rules.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAntigravityOauthFields {
    /// `accessToken` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    access_token: Option<String>,

    /// `refreshToken` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    refresh_token: Option<String>,

    /// `projectId` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    project_id: Option<String>,

    /// Every key the payload carries outside the exact stored field set.
    #[serde(flatten)]
    extra: BTreeMap<String, IgnoredAny>,
}

/// The lenient read form of static-key credentials (README §3); see
/// [`StoredCodexOauthFields`] for the reading rules.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredApiKeyFields {
    /// `apiKey` string value, when the payload carries one.
    #[serde(default, deserialize_with = "string_or_absent")]
    api_key: Option<String>,

    /// Every key the payload carries outside the exact stored field set.
    #[serde(flatten)]
    extra: BTreeMap<String, IgnoredAny>,
}

/// Reads one stored field's value into its string, when it carries one: through
/// [`StoredFieldValue`], a value of any other JSON shape carries no string and reads as an absent
/// field, exactly like the key being omitted.
fn string_or_absent<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    StoredFieldValue::deserialize(deserializer).map(StoredFieldValue::into_string)
}

/// A credentials parse failure, typed so each boundary renders its own error domain: management
/// input validation maps it to a 400 invalid request naming `credentials`, while storage row
/// decoding wraps it once as a 500 internal error — stored credentials are written only through the
/// validated management path, so a payload that fails there is database drift and never a client
/// fault.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ParseError {
    /// The payload is not valid JSON.
    #[error("Credentials are not valid JSON: {0}")]
    Json(JsonError),

    /// A required field is absent or not a string.
    #[error("Credentials must carry a string `{0}` field")]
    MissingField(String),

    /// The payload carries a field outside the provider type's exact field set.
    #[error("Credentials carry an unknown `{0}` field; allowed: `{1}`")]
    UnknownField(String, String),
}

impl Credentials {
    /// Parses credentials JSON for the given provider type.
    ///
    /// The payload is first read into the provider kind's lenient field collection — each stored
    /// field's string value, with an absent, `null`, and non-string field all reading as absent,
    /// and every key outside the kind's exact field set collected — and the verdict is then
    /// rendered here: an unknown field is rejected naming it, because a tolerated extra field —
    /// such as an admin-pasted `expiresAt` — would be silently dropped on storage and change the
    /// credentials' semantics without notice.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the payload does not match the shape required by the provider
    /// type.
    pub(crate) fn parse(provider_type: ProviderType, json: &str) -> Result<Self, ParseError> {
        match provider_type {
            ProviderType::CodexOauth => {
                let fields = read_lenient_fields::<StoredCodexOauthFields>(json)?;

                reject_extra_key(&fields.extra, &["accessToken", "refreshToken", "accountId"])?;

                Ok(Self::CodexOauth {
                    access_token: fields.access_token.ok_or_else(|| missing("accessToken"))?,
                    refresh_token: fields
                        .refresh_token
                        .ok_or_else(|| missing("refreshToken"))?,
                    account_id: fields.account_id.ok_or_else(|| missing("accountId"))?,
                })
            }

            ProviderType::AntigravityOauth => {
                let fields = read_lenient_fields::<StoredAntigravityOauthFields>(json)?;

                reject_extra_key(&fields.extra, &["accessToken", "refreshToken", "projectId"])?;

                Ok(Self::AntigravityOauth {
                    access_token: fields.access_token.ok_or_else(|| missing("accessToken"))?,
                    refresh_token: fields
                        .refresh_token
                        .ok_or_else(|| missing("refreshToken"))?,
                    project_id: fields.project_id.ok_or_else(|| missing("projectId"))?,
                })
            }

            ProviderType::OpenAiResponse
            | ProviderType::OpenAiChatCompletion
            | ProviderType::GeminiGenerateContent => {
                let fields = read_lenient_fields::<StoredApiKeyFields>(json)?;

                reject_extra_key(&fields.extra, &["apiKey"])?;

                Ok(Self::ApiKey(
                    fields.api_key.ok_or_else(|| missing("apiKey"))?,
                ))
            }
        }
    }

    /// The secret these credentials present to an upstream: the OAuth access token for OAuth
    /// accounts, the static API key for key-based accounts. Depending on the wire format it is sent
    /// as a bearer token or as an API key header.
    pub(crate) fn secret(&self) -> &str {
        match self {
            Self::CodexOauth { access_token, .. } | Self::AntigravityOauth { access_token, .. } => {
                access_token
            }

            Self::ApiKey(api_key) => api_key,
        }
    }

    /// Renders the credentials into their plaintext JSON storage form (README §3).
    ///
    /// # Panics
    ///
    /// Panics if serialization fails, which cannot occur: every stored form is a struct of strings,
    /// a shape serde_json always renders.
    pub(crate) fn to_json(&self) -> String {
        match self {
            Self::CodexOauth {
                access_token,
                refresh_token,
                account_id,
            } => serialize_stored_credentials(&StoredCodexOauthCredentials {
                access_token,
                refresh_token,
                account_id,
            }),

            Self::AntigravityOauth {
                access_token,
                refresh_token,
                project_id,
            } => serialize_stored_credentials(&StoredAntigravityOauthCredentials {
                access_token,
                refresh_token,
                project_id,
            }),

            Self::ApiKey(api_key) => {
                serialize_stored_credentials(&StoredApiKeyCredentials { api_key })
            }
        }
    }
}

/// Reads a stored credentials payload (README §3) into one provider kind's lenient field
/// collection. A payload that is not a JSON object carries no fields and reads as the empty
/// collection, failing the required-field verdict exactly like an empty object.
///
/// # Errors
///
/// Returns [`ParseError::Json`] when the payload is not valid JSON.
fn read_lenient_fields<T>(json: &str) -> Result<T, ParseError>
where
    T: Default + DeserializeOwned,
{
    Ok(
        match serde_json::from_str::<StoredCredentialsPayload<T>>(json).map_err(ParseError::Json)? {
            StoredCredentialsPayload::Object(fields) => fields,
            StoredCredentialsPayload::NonObject(_) => T::default(),
        },
    )
}

/// Rejects a collected extra key — one outside the provider kind's exact stored field set — naming
/// it and the allowed set. Of several extra keys the lexically first is named: the collected keys
/// form an ordered map, not a payload-ordered list.
///
/// # Errors
///
/// Returns [`ParseError::UnknownField`] when any extra key was collected.
fn reject_extra_key(
    extra: &BTreeMap<String, IgnoredAny>,
    allowed: &[&str],
) -> Result<(), ParseError> {
    match extra.first_key_value() {
        Some((key, _)) => Err(ParseError::UnknownField(key.clone(), allowed.join("`, `"))),
        None => Ok(()),
    }
}

/// Builds the missing-field rejection naming the field.
fn missing(field: &str) -> ParseError {
    ParseError::MissingField(field.to_string())
}

/// The fields of a provider about to be inserted.
#[derive(Debug, Clone)]
pub(crate) struct NewProvider<'a> {
    /// Unique, non-empty, renameable label (README §3); a later `providers:update` request may
    /// rename it.
    pub(crate) name: &'a str,

    /// Prefix prepended to every public model name.
    pub(crate) model_prefix: &'a str,

    /// Account kind.
    pub(crate) provider_type: ProviderType,

    /// Upstream base URL; required for static-key providers, absent for OAuth logins.
    pub(crate) base_url: Option<&'a str>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: &'a Credentials,

    /// Unix timestamp in microseconds when the OAuth access token expires.
    pub(crate) expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,
}

/// A stored upstream provider with its parsed credentials.
#[derive(Debug, Clone)]
pub(crate) struct Provider {
    /// Row identifier.
    pub(crate) id: i64,

    /// Unique, non-empty, renameable label (README §3); the immutable `id` is the provider's
    /// public identifier.
    pub(crate) name: String,

    /// Prefix prepended to every public model name.
    pub(crate) model_prefix: String,

    /// Account kind.
    pub(crate) provider_type: ProviderType,

    /// Upstream base URL; required for static-key providers, absent for OAuth logins.
    pub(crate) base_url: Option<String>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: Credentials,

    /// Unix timestamp in microseconds when the OAuth access token expires.
    pub(crate) expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,
}

impl Provider {
    /// The upstream wire format the provider speaks, pinned by its type.
    pub(crate) fn upstream_format(&self) -> UpstreamFormat {
        self.provider_type.upstream_format()
    }
}

/// A stored model row.
#[derive(Debug, Clone)]
pub(crate) struct Model {
    /// Row identifier: an `AUTOINCREMENT` number that is never recycled, so a deleted row's
    /// identity can never be reattached to a later row (README §3).
    pub(crate) id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_id: String,

    /// Public routing ID, maintained at write time.
    pub(crate) public_id: String,

    /// Non-`None` explicitly sets the public routing ID directly, bypassing the provider's
    /// `model_prefix`.
    pub(crate) id_override: Option<String>,

    /// Whether the model is enabled to serve traffic under its provider.
    pub(crate) is_enabled: bool,

    /// Discovery lifecycle state of the row (README §3).
    pub(crate) discovery_state: DiscoveryState,
}

/// A provider row joined with the model row a public ID resolved to.
#[derive(Debug, Clone)]
pub(crate) struct ModelRouting {
    /// Owning provider.
    pub(crate) provider: Provider,

    /// Resolved model row.
    pub(crate) model: Model,

    /// Whether the model effectively serves traffic ([`Model::is_effectively_enabled`]).
    pub(crate) is_effectively_enabled: bool,
}

impl Model {
    /// The public routing ID the model carries: an explicit override directly replaces the ID
    /// without prefix, else `model_prefix + upstream_id`.
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

    /// The model's effective serving status — the one definition routing, the enabled-model
    /// listing, and the management rendering all share (README §3): true if and only if the owning
    /// provider is enabled, the model itself is enabled, and the row is not `vanished`. The
    /// provider's `enabled` flag serves as the master switch, and a vanished row has its service
    /// suspended regardless of its configured switch — without the switch ever being written
    /// (discovery only writes `discovery_state`).
    pub(crate) fn is_effectively_enabled(&self, is_provider_enabled: bool) -> bool {
        is_provider_enabled && self.is_enabled && self.discovery_state != DiscoveryState::Vanished
    }
}

/// Serializes one stored credentials form into the exact plaintext JSON text the `credentials`
/// column carries (README §3).
///
/// # Panics
///
/// Panics if serialization fails, which cannot occur: every stored form is a struct of strings, a
/// shape serde_json always renders.
fn serialize_stored_credentials<T>(credentials: &T) -> String
where
    T: Serialize,
{
    #[expect(
        clippy::expect_used,
        reason = "a stored credentials form is a struct of strings, which serde_json always renders"
    )]
    serde_json::to_string(credentials).expect("stored credentials serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that the serde wire names and the storage names of the closed type vocabularies are
    /// one vocabulary: every variant serializes to, and deserializes from, exactly its `as_str`
    /// name, so the management wire, the TypeScript bindings, and the database rows can never drift
    /// apart.
    ///
    /// # Panics
    ///
    /// Panics if a wire name diverges from the storage name.
    #[test]
    fn round_trips_type_names_with_their_storage_forms() {
        for provider_type in [
            ProviderType::OpenAiResponse,
            ProviderType::OpenAiChatCompletion,
            ProviderType::GeminiGenerateContent,
            ProviderType::CodexOauth,
            ProviderType::AntigravityOauth,
        ] {
            #[expect(clippy::expect_used, reason = "a unit enum always serializes")]
            let rendered =
                serde_json::to_string(&provider_type).expect("a provider type always serializes");

            assert_eq!(rendered, format!("\"{}\"", provider_type.as_str()));

            #[expect(
                clippy::expect_used,
                reason = "the name was just rendered from the same variant"
            )]
            let parsed: ProviderType =
                serde_json::from_str(&rendered).expect("a rendered name parses back");

            assert_eq!(parsed, provider_type);
        }

        for oauth_type in [
            OAuthProviderType::CodexOauth,
            OAuthProviderType::AntigravityOauth,
        ] {
            #[expect(clippy::expect_used, reason = "a unit enum always serializes")]
            let rendered =
                serde_json::to_string(&oauth_type).expect("an OAuth type always serializes");

            assert_eq!(rendered, format!("\"{}\"", oauth_type.as_str()));
        }

        for state in [
            DiscoveryState::Discovered,
            DiscoveryState::Vanished,
            DiscoveryState::Manual,
        ] {
            #[expect(clippy::expect_used, reason = "a unit enum always serializes")]
            let rendered =
                serde_json::to_string(&state).expect("a discovery state always serializes");

            assert_eq!(rendered, format!("\"{}\"", state.as_str()));

            #[expect(
                clippy::expect_used,
                reason = "the name was just rendered from the same variant"
            )]
            let parsed: DiscoveryState =
                serde_json::from_str(&rendered).expect("a rendered name parses back");

            assert_eq!(parsed, state);
        }
    }

    /// Verifies that `Model::public_id` uses `model_prefix + upstream_id` when unoverridden, and
    /// uses `id_override` directly without `model_prefix` when an override is present.
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

    /// Verifies that `Model::is_effectively_enabled` is the three-input definition — provider
    /// enabled, model enabled, row not vanished — and that each failing input alone suspends
    /// service.
    ///
    /// # Panics
    ///
    /// Panics if effective enabled assertions fail.
    #[test]
    fn resolves_effective_enabled_status() {
        let model = |is_enabled, discovery_state| Model {
            id: 1,
            upstream_id: "m1".to_string(),
            public_id: "p/m1".to_string(),
            id_override: None,
            is_enabled,
            discovery_state,
        };

        assert!(model(true, DiscoveryState::Discovered).is_effectively_enabled(true));
        assert!(model(true, DiscoveryState::Manual).is_effectively_enabled(true));

        // Each failing input alone suspends service.
        assert!(!model(true, DiscoveryState::Discovered).is_effectively_enabled(false));
        assert!(!model(false, DiscoveryState::Discovered).is_effectively_enabled(true));
        assert!(!model(true, DiscoveryState::Vanished).is_effectively_enabled(true));
        assert!(!model(true, DiscoveryState::Vanished).is_effectively_enabled(false));
    }

    /// Verifies that `OAuthProviderType` accurately maps to `ProviderType` without relying on an
    /// implicit `From` implementation.
    ///
    /// # Panics
    ///
    /// Panics if provider type mapping assertions fail.
    #[test]
    fn maps_oauth_provider_type_to_provider_type() {
        assert_eq!(
            OAuthProviderType::CodexOauth.as_provider_type(),
            ProviderType::CodexOauth
        );
        assert_eq!(
            OAuthProviderType::AntigravityOauth.as_provider_type(),
            ProviderType::AntigravityOauth
        );
    }

    /// Verifies the exact plaintext storage form of every provider kind (README §3): the stored key
    /// names are the contract the management input and every stored row share, and each form
    /// round-trips through [`Credentials::to_json`] and [`Credentials::parse`]..
    ///
    /// # Panics
    ///
    /// Panics if a stored form or its round-trip diverges from the contract.
    #[test]
    fn round_trips_stored_credentials() {
        let cases = [
            (
                ProviderType::CodexOauth,
                Credentials::CodexOauth {
                    access_token: "access".to_string(),
                    refresh_token: "refresh".to_string(),
                    account_id: "account".to_string(),
                },
                r#"{"accessToken":"access","refreshToken":"refresh","accountId":"account"}"#,
            ),
            (
                ProviderType::AntigravityOauth,
                Credentials::AntigravityOauth {
                    access_token: "access".to_string(),
                    refresh_token: "refresh".to_string(),
                    project_id: "project".to_string(),
                },
                r#"{"accessToken":"access","refreshToken":"refresh","projectId":"project"}"#,
            ),
            (
                ProviderType::OpenAiChatCompletion,
                Credentials::ApiKey("sk-secret".to_string()),
                r#"{"apiKey":"sk-secret"}"#,
            ),
            (
                ProviderType::OpenAiResponse,
                Credentials::ApiKey("sk-secret".to_string()),
                r#"{"apiKey":"sk-secret"}"#,
            ),
            (
                ProviderType::GeminiGenerateContent,
                Credentials::ApiKey("sk-secret".to_string()),
                r#"{"apiKey":"sk-secret"}"#,
            ),
        ];

        for (provider_type, credentials, stored) in cases {
            assert_eq!(credentials.to_json(), stored);

            #[expect(
                clippy::unwrap_used,
                reason = "each stored form round-trips into its credentials"
            )]
            let parsed = Credentials::parse(provider_type, stored).unwrap();

            assert_eq!(parsed, credentials);
        }
    }

    /// Verifies the credentials rejection taxonomy every management input and every stored row
    /// passes through: a key outside the provider kind's exact field set is rejected naming it and
    /// the allowed set, while a field that is absent or carries a non-string value is rejected as
    /// missing — including a payload that is not a JSON object at all, whose missing first field is
    /// named instead of being reported as malformed JSON. Non-object payloads and non-string values
    /// are therefore never silently accepted or silently dropped.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented variant or wording.
    #[test]
    fn rejects_credentials_outside_the_exact_field_set() {
        #[expect(
            clippy::unwrap_used,
            reason = "the unknown field is constructed to be rejected"
        )]
        let error = Credentials::parse(
            ProviderType::CodexOauth,
            r#"{"accessToken":"a","refreshToken":"r","accountId":"x","expiresAt":123}"#,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ParseError::UnknownField(ref field, ref allowed)
                if field == "expiresAt" && allowed == "accessToken`, `refreshToken`, `accountId"
        ));

        // A field another provider kind stores is as unknown to this kind's exact field set as a
        // field no kind stores, even when its value is no string.
        #[expect(
            clippy::unwrap_used,
            reason = "the foreign stored field is constructed to be rejected"
        )]
        let error = Credentials::parse(
            ProviderType::AntigravityOauth,
            r#"{"accessToken":"a","refreshToken":"r","projectId":"p","apiKey":null}"#,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ParseError::UnknownField(ref field, ref allowed)
                if field == "apiKey" && allowed == "accessToken`, `refreshToken`, `projectId"
        ));

        // Of several unknown keys the lexically first is named, because the collected extra keys
        // form an ordered map rather than a payload-ordered list.
        #[expect(
            clippy::unwrap_used,
            reason = "the unknown fields are constructed to be rejected"
        )]
        let error = Credentials::parse(
            ProviderType::OpenAiChatCompletion,
            r#"{"apiKey":"sk","expiresAt":1,"clientId":2}"#,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ParseError::UnknownField(ref field, ref allowed)
                if field == "clientId" && allowed == "apiKey"
        ));

        for payload in [
            r#"{"refreshToken":"r","accountId":"x"}"#,
            r#"{"accessToken":5,"refreshToken":"r","accountId":"x"}"#,
            r#"{"accessToken":null,"refreshToken":"r","accountId":"x"}"#,
            r#"{"accessToken":[],"refreshToken":"r","accountId":"x"}"#,
            r#"{"accessToken":{"nested":true},"refreshToken":"r","accountId":"x"}"#,
            r#""sk-secret""#,
            "[]",
            "null",
        ] {
            #[expect(
                clippy::unwrap_used,
                reason = "every payload is constructed to fail on the access token field"
            )]
            let error = Credentials::parse(ProviderType::CodexOauth, payload).unwrap_err();

            assert_eq!(
                error.to_string(),
                "Credentials must carry a string `accessToken` field",
                "payload: {payload}"
            );
        }

        #[expect(
            clippy::unwrap_used,
            reason = "the non-object payload is constructed to be rejected"
        )]
        let error = Credentials::parse(ProviderType::CodexOauth, "{").unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("Credentials are not valid JSON")
        );
    }
}
