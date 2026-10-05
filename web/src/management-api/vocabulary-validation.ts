import { ContractViolationError } from "./errors";
import type { ProviderResponse } from "./generated/v0";
import { UPSTREAM_FORMAT } from "./vocabulary";

/** Narrows a wire value that must be one of the five upstream wire formats. */
export function parseUpstreamFormat(value: unknown, context: string): ProviderResponse["format"] {
    const format = UPSTREAM_FORMAT.find(value);

    if (format === null) {
        throw new ContractViolationError(`${context} is not an upstream format: ${String(value)}`);
    }

    return format;
}
