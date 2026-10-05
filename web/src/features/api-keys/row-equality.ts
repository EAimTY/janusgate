import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { isSamePolicy } from "./access-control";

/** Whether a fresh read answers the same serving-key row, content and all. */
export function isSameApiKeyRow(previous: ApiKeyResponse, fresh: ApiKeyResponse): boolean {
    return (
        previous.id === fresh.id &&
        previous.name === fresh.name &&
        previous.key === fresh.key &&
        previous.is_enabled === fresh.is_enabled &&
        previous.created_at === fresh.created_at &&
        isSamePolicy(previous.access_control, fresh.access_control)
    );
}
