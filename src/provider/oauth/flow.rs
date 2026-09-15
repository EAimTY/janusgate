//! The OAuth protocol: authorization begin, authorization code exchange, and
//! the refresh grant, over the OpenAI Codex and Google Antigravity clients.

use super::{Error, project, token};
use crate::{
    provider::{Credentials, OAuthProviderType, endpoints, http_body, http_body::ExchangeFailure},
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// OpenAI OAuth client identifier of the Codex CLI.
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI OAuth issuer root.
const OPENAI_ISSUER: &str = "https://auth.openai.com";

/// OpenAI OAuth loopback redirect.
const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// OpenAI OAuth scopes requested during authorization.
const OPENAI_SCOPES: &str = "openid profile email offline_access";

/// Google OAuth client identifier of the Antigravity IDE.
const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// Google OAuth client secret of the Antigravity IDE.
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";

/// Google OAuth authorization endpoint.
const ANTIGRAVITY_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google OAuth token endpoint.
const ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google OAuth loopback redirect.
const ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";

/// Google OAuth scopes requested during authorization.
const ANTIGRAVITY_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
     https://www.googleapis.com/auth/userinfo.email \
     https://www.googleapis.com/auth/userinfo.profile \
     https://www.googleapis.com/auth/cclog \
     https://www.googleapis.com/auth/experimentsandconfigs";

/// Lifetime of a pending OAuth state row.
const OAUTH_STATE_TTL: i64 = 600;

/// A freshly begun OAuth authorization flow.
#[derive(Debug, Clone)]
pub(crate) struct OAuthFlow {
    /// Opaque state token identifying the flow.
    pub(crate) state: String,

    /// PKCE verifier; empty for flows without PKCE.
    pub(crate) verifier: String,

    /// URL the administrator opens to authorize.
    pub(crate) authorize_url: String,

    /// Unix timestamp when the flow expires.
    pub(crate) expires_at: i64,
}

/// Begins a pending OAuth authorization flow for the requested OAuth provider
/// kind, generating a random state, optional PKCE verifier, and the provider's
/// authorization URL.
pub(crate) fn begin_flow(oauth_type: OAuthProviderType) -> OAuthFlow {
    let state = random_url_safe(32);

    let (verifier, authorize_url) = match oauth_type {
        OAuthProviderType::OpenAiOauth => {
            let verifier = random_url_safe(64);

            let challenge =
                general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

            // The `originator` parameter carries the same Codex CLI value as
            // the inference-time `Originator` header (README §5.1), so both
            // source it from the endpoints module's single home.
            let authorize_url = format!(
                "{OPENAI_ISSUER}/oauth/authorize?response_type=code&client_id={OPENAI_CLIENT_ID}\
                 &redirect_uri={OPENAI_REDIRECT_URI}&scope={}&code_challenge={challenge}\
                 &code_challenge_method=S256&state={state}&originator={}",
                encode_scopes(OPENAI_SCOPES),
                endpoints::CODEX_ORIGINATOR
            );

            (verifier, authorize_url)
        }

        OAuthProviderType::AntigravityOauth => {
            let authorize_url = format!(
                "{ANTIGRAVITY_AUTH_URL}?access_type=offline&client_id={ANTIGRAVITY_CLIENT_ID}\
                 &prompt=consent&redirect_uri={ANTIGRAVITY_REDIRECT_URI}&response_type=code\
                 &scope={}&state={state}",
                encode_scopes(ANTIGRAVITY_SCOPES)
            );

            (String::new(), authorize_url)
        }
    };

    OAuthFlow {
        state,
        verifier,
        authorize_url,
        expires_at: unix_time::unix_now() + OAUTH_STATE_TTL,
    }
}

/// Tokens returned by an OAuth token endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
struct TokenResponse {
    /// New access token.
    access_token: Option<String>,

    /// Replacement refresh token.
    refresh_token: Option<String>,

    /// OpenAI identity token.
    id_token: Option<String>,

    /// Lifetime of the access token in seconds.
    expires_in: Option<i64>,
}

/// Result of an authorization code exchange.
#[derive(Debug, Clone)]
pub(crate) struct ExchangedTokens {
    /// Credentials built from the exchanged tokens.
    pub(crate) credentials: Credentials,

    /// Unix timestamp when the access token expires.
    pub(crate) expires_at: i64,
}

/// Exchanges an OAuth authorization code for stored credentials.
///
/// # Errors
///
/// Returns [`Error`] when the token endpoint fails or the response omits
/// required fields.
pub(crate) async fn exchange_code(
    http: &Client,
    oauth_type: OAuthProviderType,
    code: &str,
    verifier: &str,
) -> Result<ExchangedTokens, Error> {
    let request = match oauth_type {
        OAuthProviderType::OpenAiOauth => {
            http.post(format!("{OPENAI_ISSUER}/oauth/token")).form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", OPENAI_REDIRECT_URI),
                ("client_id", OPENAI_CLIENT_ID),
                ("code_verifier", verifier),
            ])
        }

        OAuthProviderType::AntigravityOauth => http.post(ANTIGRAVITY_TOKEN_URL).form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", ANTIGRAVITY_CLIENT_ID),
            ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
            ("redirect_uri", ANTIGRAVITY_REDIRECT_URI),
        ]),
    };

    let response: TokenResponse = post_token_request(request, "authorization code").await?;

    // Cloned rather than moved because the raw response still feeds the
    // `token_expiry_from` fallback below.
    let access_token = response
        .access_token
        .clone()
        .ok_or_else(|| Error::Unavailable {
            detail: "token exchange response lacks an access token".to_string(),
        })?;

    let refresh_token = response.refresh_token.ok_or_else(|| Error::Unavailable {
        detail: "token exchange response lacks a refresh token".to_string(),
    })?;

    let credentials = match oauth_type {
        OAuthProviderType::OpenAiOauth => {
            let id_token = response
                .id_token
                .clone()
                .ok_or_else(|| Error::Unavailable {
                    detail: "token exchange response lacks an id token".to_string(),
                })?;

            let account_id =
                token::openai_account_claim(&id_token).ok_or_else(|| Error::Unavailable {
                    detail: "id token lacks a ChatGPT account claim".to_string(),
                })?;

            Credentials::OpenAiOauth {
                access_token,
                refresh_token,
                account_id,
            }
        }

        OAuthProviderType::AntigravityOauth => {
            let project_id = project::resolve_project_id(http, &access_token).await?;

            Credentials::AntigravityOauth {
                access_token,
                refresh_token,
                project_id,
            }
        }
    };

    let expires_at =
        token::token_expiry_from(response.expires_in, response.access_token.as_deref())?;

    Ok(ExchangedTokens {
        credentials,
        expires_at,
    })
}

/// The `TokenResponse` counterpart of a rejected token request: the RFC 6749
/// §5.2 error payload, from which the terminal `invalid_grant` is diagnosed.
/// The payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    /// OAuth error name, such as `invalid_grant`.
    error: Option<String>,
}

/// The OpenAI refresh-grant request body (README §5.1).
#[derive(Debug, Serialize)]
struct OpenAiRefreshGrantRequest<'a> {
    /// OAuth client identifier of the Codex CLI.
    client_id: &'a str,

    /// Grant type; always `refresh_token`.
    grant_type: &'a str,

    /// Grant being refreshed.
    refresh_token: &'a str,
}

/// Result of a refresh grant.
#[derive(Debug, Clone)]
pub(super) struct RefreshedTokens {
    /// New access token.
    pub(super) access_token: String,

    /// Replacement refresh token, when the endpoint rotated it.
    pub(super) refresh_token: Option<String>,

    /// Replacement identity token, when issued.
    pub(super) id_token: Option<String>,

    /// Lifetime of the access token in seconds, when the endpoint reports it.
    pub(super) expires_in: Option<i64>,
}

/// Refreshes an OAuth access token with the provider's refresh grant.
///
/// # Errors
///
/// Returns [`Error`] when the token endpoint fails or omits an access
/// token.
pub(super) async fn refresh_tokens(
    http: &Client,
    oauth_type: OAuthProviderType,
    refresh_token: &str,
) -> Result<RefreshedTokens, Error> {
    let request = match oauth_type {
        OAuthProviderType::OpenAiOauth => {
            http.post(format!("{OPENAI_ISSUER}/oauth/token"))
                .json(&OpenAiRefreshGrantRequest {
                    client_id: OPENAI_CLIENT_ID,
                    grant_type: "refresh_token",
                    refresh_token,
                })
        }

        OAuthProviderType::AntigravityOauth => http.post(ANTIGRAVITY_TOKEN_URL).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", ANTIGRAVITY_CLIENT_ID),
            ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
        ]),
    };

    let response: TokenResponse = post_token_request(request, "refresh token").await?;

    let access_token = response.access_token.ok_or_else(|| Error::Unavailable {
        detail: "token refresh response lacks an access token".to_string(),
    })?;

    Ok(RefreshedTokens {
        access_token,
        refresh_token: response.refresh_token,
        id_token: response.id_token,
        expires_in: response.expires_in,
    })
}

/// Classifies a token-endpoint rejection body (RFC 6749 §5.2): a terminal
/// `invalid_grant` — a revoked or expired grant that only user
/// re-authorization recovers — is diagnosed in the message so the doomed
/// grant is not re-attempted as if it were transient, while any other
/// rejection keeps the generic upstream rendering with the status embedded.
fn token_grant_rejection(status: u16, body: &str, grant: &str) -> Error {
    let error_name = serde_json::from_str::<TokenErrorResponse>(body)
        .ok()
        .and_then(|response| response.error);

    if error_name.as_deref() == Some("invalid_grant") {
        return Error::Unavailable {
            detail: format!(
                "the token endpoint rejected the {grant} with `invalid_grant` (RFC 6749 §5.2): \
             the grant is revoked or expired and the provider needs re-authorization"
            ),
        };
    }

    Error::Rejected {
        status,
        detail: http_body::truncate_excerpt(body),
    }
}

/// POSTs the prepared token-endpoint request and deserializes the JSON
/// response, classifying non-success statuses through
/// [`token_grant_rejection`] with the given grant description. The body
/// classification needs the full bounded error body, so the token grants map
/// the shared exchange failure here instead of through the generic
/// conversion on [`Error`].
///
/// # Errors
///
/// Returns [`Error`] for transport failures, non-success statuses, and
/// malformed response bodies.
async fn post_token_request<T>(request: reqwest::RequestBuilder, grant: &str) -> Result<T, Error>
where
    T: serde::de::DeserializeOwned,
{
    match http_body::send_and_parse_json(request, "token endpoint").await {
        Ok(parsed) => Ok(parsed),

        Err(ExchangeFailure::Rejected { status, body }) => {
            Err(token_grant_rejection(status, &body, grant))
        }

        Err(ExchangeFailure::Unavailable { detail }) => Err(Error::Unavailable { detail }),
    }
}

/// Percent-encodes a space-joined OAuth scope list for placement in an
/// authorize URL query: RFC 3986 forbids raw spaces and README §5.1/§5.2
/// document the `%20` form. Scope values are HTTPS URLs whose colons and
/// slashes are legal query characters, so spaces are the only bytes that need
/// escaping.
fn encode_scopes(scopes: &str) -> String {
    scopes.replace(' ', "%20")
}

/// Generates `byte_count` random bytes encoded as unpadded URL-safe Base64.
fn random_url_safe(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];

    rand::fill(&mut bytes[..]);

    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
