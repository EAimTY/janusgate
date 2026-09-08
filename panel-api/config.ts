export const PANEL_API_BASE_PATH = "/panel-api";

// Full endpoint URLs for the panel API routes (README §12.1). The SPA derives its client
// URLs from these constants so its requests cannot drift from the panel-api base path.
export const PANEL_API_ENDPOINTS = {
    listProviders: `${PANEL_API_BASE_PATH}/list-providers`,
    saveProvider: `${PANEL_API_BASE_PATH}/save-provider`,
    deleteProvider: `${PANEL_API_BASE_PATH}/delete-provider`,
    startProviderOAuth: `${PANEL_API_BASE_PATH}/start-provider-oauth`,
    completeProviderOAuth: `${PANEL_API_BASE_PATH}/complete-provider-oauth`,
    listModels: `${PANEL_API_BASE_PATH}/list-models`,
    saveModel: `${PANEL_API_BASE_PATH}/save-model`,
    deleteModel: `${PANEL_API_BASE_PATH}/delete-model`,
    discoverModels: `${PANEL_API_BASE_PATH}/discover-models`,
} as const;

export const OAUTH_RANDOM_VALUE_BYTES = 32;
export const ANTIGRAVITY_ONBOARDING_POLL_INTERVAL_MS = 5_000;
export const ANTIGRAVITY_ONBOARDING_MAX_POLLS = 24;

import type { OAuthProviderType } from "/common/domain";

// Keyed by the domain union so a new OAuth provider kind fails to compile until it gets a prefix.
export const AUTO_PROVIDER_NAME_PREFIX: Record<OAuthProviderType, string> = {
    antigravity_oauth: "antigravity-",
    openai_oauth: "openai-",
};
