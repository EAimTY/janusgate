// Public PKCE client and wire constants shipped by the Codex CLI. Keep this fragile boundary pinned.
export const OPENAI_OAUTH_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann";
export const OPENAI_AUTHORIZE_URL = "https://auth.openai.com/oauth/authorize";
export const OPENAI_TOKEN_URL = "https://auth.openai.com/oauth/token";
export const OPENAI_REDIRECT_URI = "http://localhost:1455/auth/callback";
export const OPENAI_OAUTH_SCOPE =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
export const OPENAI_CODEX_RESPONSES_URL = "https://chatgpt.com/backend-api/codex/responses";
export const OPENAI_CODEX_ORIGINATOR = "codex_cli_rs";
export const OPENAI_CODEX_USER_AGENT = "codex_cli_rs/0.147.0 (JanusGate; Cloudflare Workers)";

// Codex login-flow selectors the CLI appends to the authorize URL beyond the standard PKCE set.
// Without them auth.openai.com does not route the request to the CLI login page.
export const OPENAI_CODEX_LOGIN_PARAMETERS: Readonly<Record<string, string>> = {
    codex_cli_simplified_flow: "true",
    id_token_add_organizations: "true",
    originator: OPENAI_CODEX_ORIGINATOR,
};

// Discovery is deliberately a deployment-pinned snapshot, not a live model-list dependency.
export const OPENAI_CODEX_MODELS: readonly string[] = [
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex-spark",
];
