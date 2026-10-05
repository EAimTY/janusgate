import { describe, expect, it } from "vitest";
import { parseQuotaDescriptor } from "./definitions/quota";
import { parseGetProviderQuotaResponse } from "./responses/quota";

const entry = {
    key: [
        "pool",
        "weekly",
    ],
    label: "Weekly",
    routing: {
        type: "monitoring_only",
    },
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
    it("keeps model applicability independent of presentation labels and rejects malformed sets", () => {
        for (const routing of [
            {
                type: "monitoring_only",
            },
            {
                type: "all_models",
            },
            {
                type: "models",
                upstream_ids: [
                    "M",
                    "N",
                ],
            },
        ]) {
            const snapshot = {
                observed_at: 1,
                quotas: [
                    {
                        ...entry,
                        routing,
                    },
                ],
            };
            expect(parseGetProviderQuotaResponse(snapshot)).toEqual(snapshot);
        }
        for (const routing of [
            undefined,
            null,
            {
                type: "models",
                upstream_ids: [],
            },
            {
                type: "models",
                upstream_ids: [
                    "M",
                    "M",
                ],
            },
            {
                type: "models",
                upstream_ids: [
                    "bad id",
                ],
            },
            {
                type: "all_models",
                upstream_ids: [
                    "M",
                ],
            },
            {
                type: "model",
                upstream_ids: [
                    "M",
                ],
            },
        ]) {
            expect(() =>
                parseGetProviderQuotaResponse({
                    observed_at: 1,
                    quotas: [
                        {
                            ...entry,
                            routing,
                        },
                    ],
                }),
            ).toThrow();
        }
    });

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

describe("authored quota routing", () => {
    it("defaults to monitoring and preserves required per-entry selectors in their collection context", () => {
        const parse = (routing: unknown, type = "map") =>
            parseQuotaDescriptor(
                {
                    request: {
                        method: "GET",
                        url: [
                            {
                                type: "literal",
                                value: "https://example.test/quota",
                            },
                        ],
                    },
                    sources: [
                        {
                            type,
                            key: "models",
                            at: [],
                            is_required: true,
                            scope: {
                                type: "unknown",
                            },
                            entries: [
                                {
                                    key: "allowance",
                                    label: "Allowance",
                                    at: [],
                                    is_required: true,
                                    window: {
                                        type: "unknown",
                                    },
                                    measurement: {
                                        type: "unknown",
                                    },
                                    reset: {
                                        type: "unknown",
                                    },
                                    ...(routing === undefined
                                        ? {}
                                        : {
                                              routing,
                                          }),
                                },
                            ],
                        },
                    ],
                },
                "quota",
            );
        expect(parse(undefined).sources[0]?.entries[0]?.routing).toEqual({
            type: "monitoring_only",
        });
        const routing = {
            type: "models",
            upstream_ids: [
                {
                    type: "map_key",
                },
                {
                    type: "field",
                    field: {
                        base: "entry",
                        path: [
                            "model",
                        ],
                        is_required: true,
                    },
                    encoding: "string",
                },
                {
                    type: "literal",
                    value: "M",
                },
            ],
        };
        expect(parse(routing).sources[0]?.entries[0]?.routing).toEqual(routing);
        expect(() => parse(routing, "object")).toThrow();
        for (const upstream_ids of [
            [],
            [
                {
                    type: "literal",
                    value: "bad id",
                },
            ],
            [
                {
                    type: "field",
                    field: {
                        base: "entry",
                        path: [
                            "model",
                        ],
                        is_required: false,
                    },
                    encoding: "string",
                },
            ],
        ]) {
            expect(() =>
                parse({
                    type: "models",
                    upstream_ids,
                }),
            ).toThrow();
        }
    });
});
