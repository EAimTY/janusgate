export const PANEL_API_BASE_PATH = "/panel-api";
export const OAUTH_RANDOM_VALUE_BYTES = 32;
export const ANTIGRAVITY_ONBOARDING_POLL_INTERVAL_MS = 5_000;
export const ANTIGRAVITY_ONBOARDING_MAX_POLLS = 24;

import type { OAuthProviderType } from "/common/domain";

// Keyed by the domain union so a new OAuth provider kind fails to compile until it gets a prefix.
export const AUTO_PROVIDER_NAME_PREFIX: Record<OAuthProviderType, string> = {
    antigravity_oauth: "antigravity-",
    openai_oauth: "openai-",
};
