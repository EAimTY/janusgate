import { ContractViolationError } from "../errors";
import type { GetProviderQuotaResponse, QuotaEntry } from "../generated/v0";
import { expectQuotaPath, measurement, parseQuotaRouting, reset, scope, window } from "../quota";
import {
    expectArray,
    expectDeclaredRecord,
    expectInteger,
    expectNonEmptyString,
} from "../validation-primitives";

/** Validates the entire observation before any row reaches the UI. */
export function parseGetProviderQuotaResponse(value: unknown): GetProviderQuotaResponse {
    const record = expectDeclaredRecord(
        value,
        [
            "observed_at",
            "quotas",
        ],
        "response",
    );
    const identities = new Set<string>();
    const quotas = expectArray(Reflect.get(record, "quotas"), "response.quotas").map(
        (value, index): QuotaEntry => {
            const context = `response.quotas[${index}]`;
            const record = expectDeclaredRecord(
                value,
                [
                    "key",
                    "label",
                    "scope",
                    "routing",
                    "window",
                    "measurement",
                    "reset",
                ],
                context,
            );
            const key = expectQuotaPath(Reflect.get(record, "key"), `${context}.key`);
            const identity = JSON.stringify(key);
            if (key.length === 0 || identities.has(identity)) {
                throw new ContractViolationError(
                    `${context}.key must not be empty or repeat another quota's key`,
                );
            }
            identities.add(identity);
            return {
                key,
                label: expectNonEmptyString(Reflect.get(record, "label"), `${context}.label`),
                scope: scope(Reflect.get(record, "scope"), `${context}.scope`),
                routing: parseQuotaRouting(Reflect.get(record, "routing"), `${context}.routing`),
                window: window(Reflect.get(record, "window"), `${context}.window`),
                measurement: measurement(
                    Reflect.get(record, "measurement"),
                    `${context}.measurement`,
                ),
                reset: reset(Reflect.get(record, "reset"), `${context}.reset`),
            };
        },
    );
    const observed_at = expectInteger(Reflect.get(record, "observed_at"), "response.observed_at");
    if (!Number.isSafeInteger(observed_at)) {
        throw new ContractViolationError("response.observed_at must be a safe integer");
    }
    return {
        observed_at,
        quotas,
    };
}
