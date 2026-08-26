import { isJsonObject } from "/common/validation";
import { PanelApiError } from "./errors";

/**
 * Structural validators for upstream JSON responses on panel-API flows (§13: HTTP 2xx plus
 * the implied JSON schema; field content is never validated).
 */

export async function readJsonObjectResponse(
    response: Response,
    context: string,
): Promise<Record<string, unknown>> {
    if (!response.ok) {
        throw new PanelApiError(502, `${context} failed with HTTP ${response.status}`);
    }
    let value: unknown;
    try {
        value = await response.json();
    } catch {
        throw new PanelApiError(502, `${context} returned invalid JSON`);
    }
    if (!isJsonObject(value)) {
        throw new PanelApiError(502, `${context} returned an invalid response`);
    }
    return value;
}

export function requireResponseString(
    object: Record<string, unknown>,
    fieldName: string,
    context: string,
): string {
    const value = object[fieldName];
    if (typeof value !== "string") {
        throw new PanelApiError(502, `${context} returned an invalid response`);
    }
    return value;
}

export function requireResponseNumber(
    object: Record<string, unknown>,
    fieldName: string,
    context: string,
): number {
    const value = object[fieldName];
    if (typeof value !== "number") {
        throw new PanelApiError(502, `${context} returned an invalid response`);
    }
    return value;
}
