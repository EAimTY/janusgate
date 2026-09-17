//! Upstream endpoint constants of the built-in provider kinds.
//!
//! Endpoint knowledge belongs to the provider layer (the only place that dials a given upstream),
//! so gateway dispatch, model discovery, and OAuth project resolution all source their URLs and
//! fixed headers from here instead of from the format codecs.

/// Upstream URL of the Codex inference (Responses) endpoint.
pub(crate) const CODEX_INFERENCE_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Upstream URL of the Codex model discovery endpoint.
pub(crate) const CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";

/// `Originator` header value required by the Codex endpoint.
pub(crate) const CODEX_ORIGINATOR: &str = "codex_cli_rs";

/// `chatgpt-account-id` header name required by the Codex endpoint, carrying the ChatGPT account ID
/// of the OAuth credentials.
pub(crate) const CODEX_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";

/// CLI version the Codex upstream expects: it is required as the `client_version` query parameter
/// on model discovery and is embedded in the `User-Agent` value below.
pub(crate) const CODEX_CLIENT_VERSION: &str = "0.147.0";

/// Base URL of the Antigravity Code Assist API.
///
/// Personal/consumer accounts must use `daily-cloudcode-pa.googleapis.com`;
/// `cloudcode-pa.googleapis.com` returns `429 RESOURCE_EXHAUSTED`.
pub(crate) const ANTIGRAVITY_BASE_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal";

/// `loadCodeAssist` action suffix on the Antigravity base URL.
pub(crate) const ANTIGRAVITY_LOAD_CODE_ASSIST_ACTION: &str = ":loadCodeAssist";

/// `onboardUser` action suffix on the Antigravity base URL.
pub(crate) const ANTIGRAVITY_ONBOARD_USER_ACTION: &str = ":onboardUser";

/// `fetchAvailableModels` action suffix on the Antigravity base URL.
pub(crate) const ANTIGRAVITY_FETCH_AVAILABLE_MODELS_ACTION: &str = ":fetchAvailableModels";

/// `User-Agent` header value required by the Antigravity Code Assist endpoint.
pub(crate) const ANTIGRAVITY_USER_AGENT: &str = "antigravity";

/// Header name carrying the API key of a compatible Gemini upstream (README §5.3); the
/// OpenAI-family compatible upstreams authenticate through `Authorization: Bearer` instead.
pub(crate) const GEMINI_API_KEY_HEADER: &str = "x-goog-api-key";

/// `User-Agent` header value required by the Codex endpoint, derived from [`CODEX_CLIENT_VERSION`].
pub(crate) fn codex_user_agent() -> String {
    format!("codex_cli_rs/{CODEX_CLIENT_VERSION}")
}
