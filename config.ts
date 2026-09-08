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
/** Charset fragment shared by PROVIDER_NAME_PATTERN and the user-facing validation message. */
export const PROVIDER_NAME_CHARSET = "[a-z0-9_-]";
export const PROVIDER_NAME_PATTERN = new RegExp(
    `^${PROVIDER_NAME_CHARSET}{1,${PROVIDER_NAME_MAX_LENGTH}}$`,
);

export const MODEL_PREFIX_MAX_LENGTH = 32;
/** Charset fragment shared by MODEL_PREFIX_PATTERN and the user-facing validation message. */
export const MODEL_PREFIX_CHARSET = "[a-z0-9._:/-]";
export const MODEL_PREFIX_PATTERN = new RegExp(
    `^${MODEL_PREFIX_CHARSET}{0,${MODEL_PREFIX_MAX_LENGTH}}$`,
);

/** All persisted and API timestamps are whole Unix seconds. */
export const MILLISECONDS_PER_SECOND = 1_000;
export const OAUTH_UPSTREAM_TIMEOUT_SECONDS = 30;
/** §8.1 paste-back OAuth state rows expire after this TTL. */
export const OAUTH_STATE_TTL_SECONDS = 900;
/** §7 token freshness margin and credential-mutation lease, shared by gateway and panel API. */
export const TOKEN_REFRESH_MARGIN_SECONDS = 300;
export const REFRESH_LEASE_SECONDS = 60;
