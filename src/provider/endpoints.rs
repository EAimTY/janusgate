//! Upstream endpoint constants of the built-in provider kinds.
//!
//! Endpoint knowledge belongs to the provider layer (the only place that
//! dials a given upstream), so gateway dispatch, model discovery, and OAuth
//! project resolution all source their URLs and fixed headers from here
//! instead of from the format codecs.

/// Upstream URL of the Codex inference (Responses) endpoint.
pub(crate) const CODEX_INFERENCE_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Upstream URL of the Codex model discovery endpoint.
pub(crate) const CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";

/// `Originator` header value required by the Codex endpoint.
pub(crate) const CODEX_ORIGINATOR: &str = "codex_cli_rs";

/// `User-Agent` header value required by the Codex endpoint.
pub(crate) const CODEX_USER_AGENT: &str = "codex_cli_rs/0.147.0";

/// Base URL of the Antigravity Code Assist API.
///
/// Personal/consumer accounts must use `daily-cloudcode-pa.googleapis.com`;
/// `cloudcode-pa.googleapis.com` returns `429 RESOURCE_EXHAUSTED`.
pub(crate) const ANTIGRAVITY_BASE_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal";
