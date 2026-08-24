import type { ProviderType } from "/common/domain";

export const API_ROOT_PATH = "/api";
export const API_BASE_PATH = `${API_ROOT_PATH}/openai-v1`;
export const API_VERSION_PATH = API_BASE_PATH.slice(API_ROOT_PATH.length);
export const MAX_REQUEST_BYTES = 10 * 1024 * 1024;
export const MAX_RESPONSE_BYTES = 25 * 1024 * 1024;
export const UPSTREAM_IDLE_TIMEOUT_SECONDS = 300;
export const TOKEN_REFRESH_MARGIN_SECONDS = 300;
export const REFRESH_LEASE_SECONDS = 60;
export const FORCED_REFRESH_RETRY_COUNT = 1;
export const SAFE_UPSTREAM_RESPONSE_HEADERS = [
    "cache-control",
    "content-encoding",
    "content-language",
    "content-type",
    "retry-after",
] as const;

export const MODEL_OWNER_BY_PROVIDER_TYPE: Readonly<Record<ProviderType, string>> = {
    antigravity_oauth: "antigravity",
    openai_compatible: "custom",
    openai_oauth: "openai",
};
