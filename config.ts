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
