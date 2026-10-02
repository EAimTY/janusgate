import { describe, expect, it } from "vitest";
import { parseGetProviderQuotaResponse } from "./quota";

const entry = {
    key: [
        "pool",
        "weekly",
    ],
    label: "Weekly",
    scope: {
        type: "unknown",
    },
    window: {
        type: "duration",
        seconds: 604800,
    },
    measurement: {
        type: "ratio",
        used_fraction: 1.25,
    },
    reset: {
        type: "date",
        date: "2030-02-28",
    },
};

describe("quota response validation", () => {
    it("retains over-limit readings and independent tuple segments, including empty success", () => {
        const response = {
            observed_at: 1900000000000000,
            quotas: [
                entry,
                {
                    ...entry,
                    key: [
                        "pool/weekly",
                    ],
                    measurement: {
                        type: "unlimited",
                    },
                },
            ],
        };
        expect(parseGetProviderQuotaResponse(response)).toEqual(response);
        expect(
            parseGetProviderQuotaResponse({
                observed_at: 1,
                quotas: [],
            }).quotas,
        ).toEqual([]);
    });
    it("rejects the whole snapshot for malformed nested facts or duplicate tuple identities", () => {
        for (const bad of [
            {
                ...entry,
                measurement: {
                    type: "unknown",
                    used_fraction: 0,
                },
            },
            {
                ...entry,
                scope: {
                    type: "unknown",
                    name: "Model",
                },
            },
            {
                ...entry,
                window: {
                    type: "duration",
                    seconds: 1,
                    unit: "month",
                },
            },
            {
                ...entry,
                reset: {
                    type: "date",
                    date: "2030-02-28",
                    at: 1,
                },
            },
            {
                ...entry,
                measurement: {
                    type: "ratio",
                    used_fraction: -1,
                },
            },
            {
                ...entry,
                measurement: {
                    type: "ratio",
                    used_fraction: Infinity,
                },
            },
            {
                ...entry,
                measurement: {
                    type: "counter",
                    used: 1,
                    limit: 0,
                    unit: "requests",
                },
            },
            {
                ...entry,
                measurement: {
                    type: "remaining",
                    remaining: NaN,
                    unit: "tokens",
                },
            },
            {
                ...entry,
                window: {
                    type: "duration",
                    seconds: 1.5,
                },
            },
            {
                ...entry,
                reset: {
                    type: "at",
                    at: Number.MAX_SAFE_INTEGER + 1,
                },
            },
            {
                ...entry,
                reset: {
                    type: "date",
                    date: "2030-02-29",
                },
            },
            {
                ...entry,
                key: [],
            },
            {
                ...entry,
                scope: null,
            },
        ]) {
            expect(() =>
                parseGetProviderQuotaResponse({
                    observed_at: 1,
                    quotas: [
                        bad,
                    ],
                }),
            ).toThrow();
        }
        expect(() =>
            parseGetProviderQuotaResponse({
                observed_at: 1,
                quotas: [
                    entry,
                    entry,
                ],
            }),
        ).toThrow();
    });
});
