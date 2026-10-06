import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { isSameRowFields } from "../inventory/row-equality";
import { isSamePolicy } from "./access-control";

/**
 * Whether a fresh read answers the same serving-key row, content and all:
 * the shared row walk over every generated column, with the access policy
 * compared structurally (`isSamePolicy`) instead of by reference — a policy
 * read back as equal rules in a fresh object keeps the row's identity.
 */
export function isSameApiKeyRow(previous: ApiKeyResponse, fresh: ApiKeyResponse): boolean {
    return isSameRowFields(previous, fresh, {
        access_control: isSamePolicy,
    });
}
