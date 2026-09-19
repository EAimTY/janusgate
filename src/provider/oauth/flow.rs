//! The OAuth protocol: authorization begin, authorization code exchange, and the refresh grant,
//! over the OpenAI Codex and Google Antigravity clients.

use super::{Error, project, token};
use crate::{
    provider::{Credentials, OAuthProviderType, endpoints, http_body, http_body::ExchangeFailure},
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use log::debug;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};

/// OpenAI OAuth client identifier of the Codex CLI.
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI OAuth issuer root.
const OPENAI_ISSUER: &str = "https://auth.openai.com";

/// OpenAI OAuth loopback redirect.
const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// OpenAI OAuth scopes requested during authorization, mirroring the Codex CLI: the identity
/// scopes plus the connectors scopes its tools invoke with.
const OPENAI_SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";

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

/// Lifetime of a pending OAuth state row, in database-clock microseconds.
const OAUTH_STATE_TTL: i64 = 600 * unix_time::MICROS_PER_SEC;

/// A freshly begun OAuth authorization flow.
#[derive(Debug, Clone)]
pub(crate) struct OAuthFlow {
    /// Opaque state token identifying the flow.
    pub(crate) state: String,

    /// PKCE verifier; empty for flows without PKCE.
    pub(crate) verifier: String,

    /// URL the administrator opens to authorize.
    pub(crate) authorize_url: String,

    /// Unix timestamp in microseconds when the flow expires.
    pub(crate) expires_at: i64,
}

/// Result of an authorization code exchange.
#[derive(Debug, Clone)]
pub(crate) struct ExchangedTokens {
    /// Credentials built from the exchanged tokens.
    pub(crate) credentials: Credentials,

    /// Unix timestamp in microseconds when the access token expires.
    pub(crate) expires_at: i64,
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

/// The rejected token request's error payload: either the RFC 6749 §5.2 string form Google's
/// token endpoint sends (`{"error": "invalid_grant", ...}`) or the object form
/// `auth.openai.com` sends (`{"error": {"code": "token_expired", ...}}`). Both shapes are
/// read like the CLI's `parse_token_endpoint_error` in `server.rs` at rev `fcf0545`. The payload
/// is upstream-owned, so unknown fields are ignored and an unrecognized shape classifies
/// nothing.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    /// Machine-readable failure name, in either wire shape.
    error: Option<TokenErrorName>,
}

/// One token-endpoint failure name in either wire shape it arrives in.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TokenErrorName {
    /// RFC 6749 §5.2 string form, such as `invalid_grant`.
    Name(String),

    /// `auth.openai.com` object form, carrying the failure in its `code` field.
    Object(TokenErrorObject),
}

/// The `auth.openai.com` object-form failure: the machine-readable failure is its `code` field.
/// The payload is upstream-owned, so only the code is modeled and unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct TokenErrorObject {
    /// Machine-readable failure code, such as `token_expired`.
    code: Option<String>,
}

/// Which token grant a token-endpoint request redeems: the single-use authorization code of an
/// OAuth completion, or the stored refresh token of a lease-guarded refresh. The kind decides the
/// recovery a terminal rejection names: a dead code needs a fresh flow completed promptly, while
/// a dead refresh token needs the provider re-authorized.
#[derive(Debug, Clone, Copy)]
enum TokenGrant {
    /// The authorization code of `oauth:complete` and `oauth:reauthorize`.
    AuthorizationCode,

    /// The stored refresh token of the lease-guarded refresh loop.
    RefreshToken,
}

impl TokenGrant {
    /// Human-readable grant name carried in failure messages.
    const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationCode => "authorization code",
            Self::RefreshToken => "refresh token",
        }
    }

    /// Whether the named failure code dooms this grant, so retrying it is futile: the RFC 6749
    /// `invalid_grant`, the refresh codes the CLI classifies as terminal
    /// (`classify_refresh_token_failure` in `auth/manager.rs` at rev `fcf0545`:
    /// `refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated`), plus the
    /// object-form codes observed live on `auth.openai.com`. Codes compare case-insensitively,
    /// mirroring the CLI's normalization. An authorization code is single-use and short-lived —
    /// an expired, consumed, or malformed code never revives — while a refresh token dies only
    /// by revocation or expiry.
    fn is_terminal_code(self, code: &str) -> bool {
        let normalized = code.to_ascii_lowercase();
        let code = normalized.as_str();

        match self {
            Self::AuthorizationCode => matches!(
                code,
                "invalid_grant" | "token_expired" | "invalid_token" | "token_exchange_user_error"
            ),

            Self::RefreshToken => matches!(
                code,
                "invalid_grant"
                    | "token_expired"
                    | "invalid_token"
                    | "refresh_token_expired"
                    | "refresh_token_reused"
                    | "refresh_token_invalidated"
            ),
        }
    }

    /// Recovery a terminal rejection of this grant names.
    const fn recovery(self) -> &'static str {
        match self {
            Self::AuthorizationCode => {
                "the code is expired, already used, or invalid — including a code issued for an \
                 earlier flow; start a fresh flow, authorize its URL, and complete it promptly"
            }

            Self::RefreshToken => {
                "the grant is revoked or expired and the provider needs re-authorization"
            }
        }
    }
}

/// The OpenAI refresh-grant request body (README §5.1): mirrors `RefreshRequest` in
/// `auth/manager.rs` at rev `fcf0545`, which the CLI posts as JSON.
#[derive(Debug, Serialize)]
struct OpenAiRefreshGrantRequest<'a> {
    /// OAuth client identifier of the Codex CLI.
    client_id: &'a str,

    /// Grant type; always `refresh_token`.
    grant_type: &'a str,

    /// Grant being refreshed.
    refresh_token: &'a str,
}

/// Begins a pending OAuth authorization flow for the requested OAuth provider kind, generating a
/// random state, optional PKCE verifier, and the provider's authorization URL.
pub(crate) fn begin_flow(oauth_type: OAuthProviderType) -> OAuthFlow {
    let state = random_url_safe(32);

    let (verifier, authorize_url) = match oauth_type {
        OAuthProviderType::OpenAiOauth => {
            let verifier = random_url_safe(64);

            let challenge = pkce_challenge(&verifier);

            // The `originator` parameter carries the same Codex CLI value as the inference-time
            // `Originator` header (README §5.1), so both source it from the endpoints module's
            // single home. The organizations and simplified-flow flags mirror the CLI's
            // `build_authorize_url` in `server.rs` at rev `fcf0545` in the same position, so the
            // issued code is redeemable by the same exchange.
            let authorize_url = format!(
                "{OPENAI_ISSUER}/oauth/authorize?response_type=code&client_id={OPENAI_CLIENT_ID}\
                 &redirect_uri={OPENAI_REDIRECT_URI}&scope={}&code_challenge={challenge}\
                 &code_challenge_method=S256&id_token_add_organizations=true\
                 &codex_cli_simplified_flow=true&state={state}&originator={}",
                encode_scopes(OPENAI_SCOPES),
                endpoints::CODEX_ORIGINATOR
            );

            (verifier, authorize_url)
        }

        OAuthProviderType::AntigravityOauth => {
            // The reference (`authorizeAntigravity` in `oauth.ts` at rev `16e0056`) drives the
            // same Google installed-app client with PKCE; the gateway authenticates the exchange
            // with the client secret instead — Google accepts the secret-only style for this
            // client, and the flow is verified live — so no verifier is minted or stored here.
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
        expires_at: unix_time::unix_now_micros() + OAUTH_STATE_TTL,
    }
}

/// Builds the Codex authorization-code exchange form in the CLI's field set and order
/// (`server.rs` `exchange_code_for_tokens` at rev `fcf0545`): `grant_type`, `code`,
/// `redirect_uri`, `client_id`, `code_verifier`. The caller form-encodes the pairs.
fn openai_exchange_form<'a>(code: &'a str, verifier: &'a str) -> [(&'static str, &'a str); 5] {
    [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", OPENAI_REDIRECT_URI),
        ("client_id", OPENAI_CLIENT_ID),
        ("code_verifier", verifier),
    ]
}

/// Exchanges an OAuth authorization code for stored credentials.
///
/// # Errors
///
/// Returns [`Error`] when the token endpoint fails or the response omits required fields.
pub(crate) async fn exchange_code(
    http: &Client,
    oauth_type: OAuthProviderType,
    code: &str,
    verifier: &str,
) -> Result<ExchangedTokens, Error> {
    let request = match oauth_type {
        OAuthProviderType::OpenAiOauth => {
            // Lengths only: the values are secrets, but their shapes diagnose paste mistakes
            // (a truncated or URL-pasted code) without leaking them.
            debug!(
                "exchanging authorization code (code_len={}, verifier_len={})",
                code.len(),
                verifier.len()
            );

            http.post(format!("{OPENAI_ISSUER}/oauth/token"))
                .form(&openai_exchange_form(code, verifier))
        }

        // The reference (`exchangeAntigravity` in `oauth.ts` at rev `16e0056`) sends the client
        // secret alongside its PKCE verifier; the gateway sends the secret alone — Google accepts
        // it for this client, and the exchange is verified live — so the empty verifier minted by
        // `begin_flow` stays out of the form.
        OAuthProviderType::AntigravityOauth => http.post(ANTIGRAVITY_TOKEN_URL).form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", ANTIGRAVITY_CLIENT_ID),
            ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
            ("redirect_uri", ANTIGRAVITY_REDIRECT_URI),
        ]),
    };

    let response: TokenResponse =
        post_token_request(request, TokenGrant::AuthorizationCode).await?;

    // Cloned rather than moved because the raw response still feeds the `token_expiry_from`
    // fallback below.
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

/// Refreshes an OAuth access token with the provider's refresh grant.
///
/// # Errors
///
/// Returns [`Error`] when the token endpoint fails or omits an access token.
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

    let response: TokenResponse = post_token_request(request, TokenGrant::RefreshToken).await?;

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

/// Classifies a token-endpoint rejection body: a terminal failure code — a grant whose retry is
/// futile — is diagnosed with the grant's recovery instead of the generic upstream rendering,
/// while any other rejection keeps the generic rendering with the status embedded. The code is
/// read in either wire shape: the RFC 6749 §5.2 string form Google sends, or the object form
/// `auth.openai.com` sends.
fn token_grant_rejection(status: u16, body: &str, grant: TokenGrant) -> Error {
    let error_code = serde_json::from_str::<TokenErrorResponse>(body)
        .ok()
        .and_then(|response| response.error)
        .and_then(|name| match name {
            TokenErrorName::Name(code) => Some(code),
            TokenErrorName::Object(object) => object.code,
        });

    if let Some(code) = error_code
        .as_deref()
        .filter(|code| grant.is_terminal_code(code))
    {
        let grant_name = grant.as_str();
        let recovery = grant.recovery();

        return Error::Unavailable {
            detail: format!(
                "the token endpoint rejected the {grant_name} with `{code}`: {recovery}"
            ),
        };
    }

    Error::Rejected {
        status,
        detail: http_body::truncate_excerpt(body),
    }
}

/// POSTs the prepared token-endpoint request and deserializes the JSON response, classifying
/// non-success statuses through [`token_grant_rejection`] for the given grant. The body
/// classification needs the full bounded error body, so the token grants map the shared exchange
/// failure here instead of through the generic conversion on [`Error`].
///
/// # Errors
///
/// Returns [`Error`] for transport failures, non-success statuses, and malformed response bodies.
async fn post_token_request<T>(request: RequestBuilder, grant: TokenGrant) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    match http_body::send_and_parse_json(request, "token endpoint").await {
        Ok(parsed) => Ok(parsed),

        Err(ExchangeFailure::Rejected { status, body }) => {
            Err(token_grant_rejection(status, &body, grant))
        }

        Err(ExchangeFailure::Unavailable { detail }) => Err(Error::Unavailable { detail }),
    }
}

/// Percent-encodes a space-joined OAuth scope list for placement in an authorize URL query: RFC
/// 3986 forbids raw spaces and README §5.1/§5.2 document the `%20` form. Scope values are HTTPS
/// URLs whose colons and slashes are legal query characters, so spaces are the only bytes that need
/// escaping.
fn encode_scopes(scopes: &str) -> String {
    scopes.replace(' ', "%20")
}

/// Derives the PKCE S256 challenge of a verifier: Base64URL (unpadded) of SHA-256 over the
/// ASCII verifier bytes (RFC 7636 §4.2).
fn pkce_challenge(verifier: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Generates `byte_count` random bytes encoded as unpadded URL-safe Base64.
fn random_url_safe(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];

    rand::fill(&mut bytes[..]);

    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 appendix B pins the S256 challenge of a known verifier, so the PKCE transform
    /// the gateway advertises is verified against the standard itself rather than the
    /// implementation: a verifier the gateway mints always matches the challenge its authorize
    /// URL carries, and a challenge mismatch — which the token endpoint rejects opaquely — cannot
    /// slip in.
    ///
    /// # Panics
    ///
    /// Panics if the derived challenge differs from the RFC vector.
    #[test]
    fn derives_the_pkce_challenge_per_rfc7636() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    /// `auth.openai.com` rejects a dead refresh token with the object-form error observed live:
    /// a 401 carrying `{"error": {"code": "token_expired", ...}}`. The rejection must
    /// diagnose re-authorization instead of rendering the generic upstream excerpt.
    ///
    /// # Panics
    ///
    /// Panics if the rejection is not the diagnosed re-authorization failure.
    #[test]
    fn diagnoses_a_dead_refresh_token_from_the_object_form() {
        let body = concat!(
            r#"{"error": {"message": "Could not validate your token. "#,
            r#"Please try signing in again.", "type": "invalid_request_error", "#,
            r#""param": null, "code": "token_expired"}}"#
        );

        #[expect(
            clippy::panic,
            reason = "a dead refresh token must diagnose, which is what the test asserts"
        )]
        let Error::Unavailable { detail } =
            token_grant_rejection(401, body, TokenGrant::RefreshToken)
        else {
            panic!("a dead refresh token must diagnose re-authorization");
        };

        assert!(
            detail.contains("`token_expired`") && detail.contains("needs re-authorization"),
            "unexpected diagnosis: {detail}"
        );
    }

    /// The same object form on the authorization-code grant means the code is expired, already
    /// used, or invalid, so the rejection must direct a fresh flow instead of a retry of the dead
    /// code.
    ///
    /// # Panics
    ///
    /// Panics if the rejection is not the diagnosed fresh-flow failure.
    #[test]
    fn diagnoses_a_dead_authorization_code_from_the_object_form() {
        let body = concat!(
            r#"{"error": {"message": "Could not validate your token. "#,
            r#"Please try signing in again.", "type": "invalid_request_error", "#,
            r#""param": null, "code": "token_expired"}}"#
        );

        #[expect(
            clippy::panic,
            reason = "a dead authorization code must diagnose, which is what the test asserts"
        )]
        let Error::Unavailable { detail } =
            token_grant_rejection(401, body, TokenGrant::AuthorizationCode)
        else {
            panic!("a dead authorization code must direct a fresh flow");
        };

        assert!(
            detail.contains("`token_expired`") && detail.contains("fresh flow"),
            "unexpected diagnosis: {detail}"
        );
    }

    /// The RFC 6749 string form Google's token endpoint sends keeps its re-authorization
    /// diagnosis.
    ///
    /// # Panics
    ///
    /// Panics if the rejection is not the diagnosed re-authorization failure.
    #[test]
    fn keeps_the_rfc6749_invalid_grant_diagnosis() {
        let body = concat!(
            r#"{"error": "invalid_grant", "#,
            r#""error_description": "Token has been expired or revoked."}"#
        );

        #[expect(
            clippy::panic,
            reason = "an invalid_grant must diagnose, which is what the test asserts"
        )]
        let Error::Unavailable { detail } =
            token_grant_rejection(400, body, TokenGrant::RefreshToken)
        else {
            panic!("an invalid_grant must diagnose re-authorization");
        };

        assert!(
            detail.contains("`invalid_grant`") && detail.contains("needs re-authorization"),
            "unexpected diagnosis: {detail}"
        );
    }

    /// Failure codes outside the terminal set — and bodies carrying no machine-readable code at
    /// all — keep the generic upstream rendering with the foreign status embedded.
    ///
    /// # Panics
    ///
    /// Panics if a non-terminal rejection leaves the generic rendering.
    #[test]
    fn keeps_generic_rendering_for_non_terminal_rejections() {
        let unrecognized = r#"{"error": {"message": "Rate limited.", "code": "rate_limited"}}"#;

        for (status, body) in [(429, unrecognized), (500, "not json at all")] {
            #[expect(
                clippy::panic,
                reason = "a non-terminal rejection stays generic, which is what the test asserts"
            )]
            let Error::Rejected {
                status: rendered,
                detail,
            } = token_grant_rejection(status, body, TokenGrant::RefreshToken)
            else {
                panic!("a non-terminal rejection must stay generic");
            };

            assert_eq!(rendered, status);
            assert!(detail.contains(body), "unexpected excerpt: {detail}");
        }
    }

    /// The Codex token exchange carries exactly the CLI's form fields in their order
    /// (`server.rs` `exchange_code_for_tokens`): a drift in set or order would redeem against a
    /// different binding and fail opaquely at the token endpoint.
    ///
    /// # Panics
    ///
    /// Panics if the exchange form diverges from the CLI's fields.
    #[test]
    fn builds_the_codex_exchange_form_like_the_cli() {
        assert_eq!(
            openai_exchange_form("code-value", "verifier-value"),
            [
                ("grant_type", "authorization_code"),
                ("code", "code-value"),
                ("redirect_uri", OPENAI_REDIRECT_URI),
                ("client_id", OPENAI_CLIENT_ID),
                ("code_verifier", "verifier-value"),
            ]
        );
    }

    /// The Codex CLI authorizes the identity scopes plus the connectors scopes its tools invoke
    /// with, and marks the request with the organizations and simplified-flow flags: the gateway
    /// mints the same authorize URL, so the issued code is redeemable by the same exchange.
    ///
    /// # Panics
    ///
    /// Panics if the authorize URL diverges from the CLI's parameters.
    #[test]
    fn builds_the_codex_authorize_url_like_the_cli() {
        let flow = begin_flow(OAuthProviderType::OpenAiOauth);

        assert!(
            flow.authorize_url.contains(concat!(
                "scope=openid%20profile%20email%20offline_access",
                "%20api.connectors.read%20api.connectors.invoke"
            )),
            "unexpected scopes: {}",
            flow.authorize_url
        );
        assert!(
            flow.authorize_url
                .contains("id_token_add_organizations=true"),
            "unexpected authorize URL: {}",
            flow.authorize_url
        );
        assert!(
            flow.authorize_url
                .contains("codex_cli_simplified_flow=true"),
            "unexpected authorize URL: {}",
            flow.authorize_url
        );
        assert!(
            flow.authorize_url.contains(&pkce_challenge(&flow.verifier)),
            "the advertised challenge must match the stored verifier"
        );
    }

    /// Terminal failure codes compare case-insensitively and include the CLI-classified refresh
    /// code: a backend `refresh_token_expired` in any letter case diagnoses re-authorization.
    ///
    /// # Panics
    ///
    /// Panics if the rejection is not the diagnosed re-authorization failure.
    #[test]
    fn diagnoses_cli_classified_refresh_codes_in_any_case() {
        for code in ["refresh_token_expired", "REFRESH_TOKEN_EXPIRED"] {
            let body = format!(r#"{{"error": {{"code": "{code}"}}}}"#);

            #[expect(
                clippy::panic,
                reason = "a dead refresh token must diagnose, which is what the test asserts"
            )]
            let Error::Unavailable { detail } =
                token_grant_rejection(401, &body, TokenGrant::RefreshToken)
            else {
                panic!("a dead refresh token must diagnose re-authorization");
            };

            assert!(
                detail.contains("needs re-authorization"),
                "unexpected diagnosis: {detail}"
            );
        }
    }
}
