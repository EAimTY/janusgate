/** Canonical gateway reasoning scale, ordered from least to most effort. */
export const CANONICAL_THINKING_LEVELS = [
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
] as const;

export type ThinkingLevel = (typeof CANONICAL_THINKING_LEVELS)[number];

export const PROVIDER_NAME_MAX_LENGTH = 64;
export const PROVIDER_NAME_PATTERN = new RegExp(`^[a-z0-9_-]{1,${PROVIDER_NAME_MAX_LENGTH}}$`);

export const MODEL_PREFIX_MAX_LENGTH = 32;
export const MODEL_PREFIX_PATTERN = new RegExp(`^[a-z0-9._:/-]{0,${MODEL_PREFIX_MAX_LENGTH}}$`);

/** All persisted and API timestamps are whole Unix seconds. */
export const MILLISECONDS_PER_SECOND = 1_000;
export const OAUTH_UPSTREAM_TIMEOUT_SECONDS = 30;
/** §7 token freshness margin and credential-mutation lease, shared by gateway and panel API. */
export const TOKEN_REFRESH_MARGIN_SECONDS = 300;
export const REFRESH_LEASE_SECONDS = 60;

/**
 * OpenAI egress relay base URL (§8.3), or `null` to call OpenAI directly. OpenAI geo-blocks by
 * the egress IP's region and Worker egress follows the executing colo, so a deployment whose
 * requests enter Cloudflare in an unsupported region must relay every Worker-side OpenAI call
 * through an HTTPS host in a supported region. The relay receives the original host as the
 * first path segment (e.g. `…/auth.openai.com/oauth/token`); the browser-side authorize URL is
 * never relayed.
 */
export const OPENAI_EGRESS_RELAY_BASE_URL: string | null = null;
