import { OPENAI_EGRESS_RELAY_BASE_URL } from "/config";

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

export function openAiRelayUrl(relayBaseUrl: string, url: string): string {
    const parsed = new URL(url);
    return `${relayBaseUrl.replace(/\/+$/, "")}/${parsed.host}${parsed.pathname}${parsed.search}`;
}

/**
 * Worker-side OpenAI URLs (token exchange, refresh, codex responses) route through the egress
 * relay when one is configured: OpenAI geo-blocks by the egress IP's region, which follows the
 * executing colo, so deployments entered from an unsupported region cannot reach OpenAI
 * directly. The authorize URL is browser-side and never passes through here.
 */
export function openAiUpstreamUrl(url: string): string {
    return OPENAI_EGRESS_RELAY_BASE_URL === null
        ? url
        : openAiRelayUrl(OPENAI_EGRESS_RELAY_BASE_URL, url);
}

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
