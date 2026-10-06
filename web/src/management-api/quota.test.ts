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
        for (const [routing, expected] of [
            [
                undefined,
                /response\.quotas\[0\]\.routing must be an object/,
            ],
            [
                null,
                /response\.quotas\[0\]\.routing must be an object/,
            ],
            [
                {
                    type: "models",
                    upstream_ids: [],
                },
                /routing\.upstream_ids must be a nonempty set of unique model identifiers/,
            ],
            [
                {
                    type: "models",
                    upstream_ids: [
                        "M",
                        "M",
                    ],
                },
                /routing\.upstream_ids must be a nonempty set of unique model identifiers/,
            ],
            [
                {
                    type: "models",
                    upstream_ids: [
                        "bad id",
                    ],
                },
                /routing\.upstream_ids\[0\] is not a routing token: bad id/,
            ],
            [
                {
                    type: "all_models",
                    upstream_ids: [
                        "M",
                    ],
                },
                /routing\.upstream_ids is not a declared field/,
            ],
            [
                {
                    type: "model",
                    upstream_ids: [
                        "M",
                    ],
                },
                /routing\.type is not a routing applicability: model/,
            ],
        ] as Array<
            [
                unknown,
                RegExp,
            ]
        >) {
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
            ).toThrow(expected);
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
        for (const [bad, expected] of [
            [
                {
                    ...entry,
                    measurement: {
                        type: "unknown",
                        used_fraction: 0,
                    },
                },
                /measurement\.used_fraction is not a declared field/,
            ],
            [
                {
                    ...entry,
                    scope: {
                        type: "unknown",
                        name: "Model",
                    },
                },
                /scope\.name is not a declared field/,
            ],
            [
                {
                    ...entry,
                    window: {
                        type: "duration",
                        seconds: 1,
                        unit: "month",
                    },
                },
                /window\.unit is not a declared field/,
            ],
            [
                {
                    ...entry,
                    reset: {
                        type: "date",
                        date: "2030-02-28",
                        at: 1,
                    },
                },
                /reset\.at is not a declared field/,
            ],
            [
                {
                    ...entry,
                    measurement: {
                        type: "ratio",
                        used_fraction: -1,
                    },
                },
                /measurement\.used_fraction must be a finite non-negative amount/,
            ],
            [
                {
                    ...entry,
                    measurement: {
                        type: "ratio",
                        used_fraction: Infinity,
                    },
                },
                /measurement\.used_fraction must be a finite non-negative amount/,
            ],
            [
                {
                    ...entry,
                    measurement: {
                        type: "counter",
                        used: 1,
                        limit: 0,
                        unit: "requests",
                    },
                },
                /measurement\.limit must be a finite positive amount/,
            ],
            [
                {
                    ...entry,
                    measurement: {
                        type: "remaining",
                        remaining: NaN,
                        unit: "tokens",
                    },
                },
                /measurement\.remaining must be a finite non-negative amount/,
            ],
            [
                {
                    ...entry,
                    window: {
                        type: "duration",
                        seconds: 1.5,
                    },
                },
                /window\.seconds must be an integer/,
            ],
            [
                {
                    ...entry,
                    reset: {
                        type: "at",
                        at: Number.MAX_SAFE_INTEGER + 1,
                    },
                },
                /reset\.at must be a safe integer/,
            ],
            [
                {
                    ...entry,
                    window: {
                        type: "duration",
                        seconds: 0,
                    },
                },
                /window\.seconds must be a safe integer of at least 1/,
            ],
            [
                {
                    ...entry,
                    reset: {
                        type: "date",
                        date: "2030-02-29",
                    },
                },
                /reset\.date is not a calendar date: 2030-02-29/,
            ],
            [
                {
                    ...entry,
                    key: [],
                },
                /\.key must not be empty or repeat another quota's key/,
            ],
            [
                {
                    ...entry,
                    scope: null,
                },
                /\.scope must be an object/,
            ],
        ] as Array<
            [
                Record<string, unknown>,
                RegExp,
            ]
        >) {
            expect(() =>
                parseGetProviderQuotaResponse({
                    observed_at: 1,
                    quotas: [
                        bad,
                    ],
                }),
            ).toThrow(expected);
        }
        expect(() =>
            parseGetProviderQuotaResponse({
                observed_at: 1,
                quotas: [
                    entry,
                    entry,
                ],
            }),
        ).toThrow(/quotas\[1\]\.key must not be empty or repeat another quota's key/);
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
        expect(() => parse(routing, "object")).toThrow(
            /upstream_ids\[0\]\.type must not be `map_key` outside a map/,
        );
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
