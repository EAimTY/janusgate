import type { GetProviderQuotaResponse, QuotaEntry } from "../generated/v0";
import { measurement, parseQuotaRouting, reset, scope, window } from "../quota";
import { integer, invalid, shape, text } from "../quota-validation";
import { expectArray } from "../validation-primitives";

/** Validates the entire observation before any row reaches the UI. */
export function parseGetProviderQuotaResponse(value: unknown): GetProviderQuotaResponse {
    const record = shape(
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
            const record = shape(
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
            const key = expectArray(Reflect.get(record, "key"), `${context}.key`).map(
                (part, index) => text(part, `${context}.key[${index}]`),
            );
            if (key.length === 0 || identities.has(JSON.stringify(key))) invalid(`${context}.key`);
            identities.add(JSON.stringify(key));
            return {
                key,
                label: text(Reflect.get(record, "label"), `${context}.label`),
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
    return {
        observed_at: integer(Reflect.get(record, "observed_at"), "response.observed_at"),
        quotas,
    };
}
