import { describe, expect, it } from "vitest";
import {
    parseGetProviderAvailabilityResponse,
    parseListModelRoutesResponse,
    parseListProviderPoolsResponse,
} from "./routing";

describe("pool and route management decoding", () => {
    it("preserves the complete authored member order and direct/pool target distinction", () => {
        const pool = {
            model_prefix: "",
            id: 3,
            name: "Coding",
            is_enabled: true,
            members: [
                {
                    provider_id: 8,
                    priority: 0,
                },
                {
                    provider_id: 4,
                    priority: 7,
                },
            ],
        };
        expect(
            parseListProviderPoolsResponse({
                provider_pools: [
                    pool,
                ],
            }),
        ).toEqual({
            provider_pools: [
                pool,
            ],
        });
        for (const members of [
            [
                {
                    provider_id: 8,
                    priority: 0,
                },
                {
                    provider_id: 8,
                    priority: 1,
                },
            ],
            [
                {
                    provider_id: 8,
                    priority: 0,
                },
                {
                    provider_id: 4,
                    priority: 0,
                },
            ],
            [
                {
                    provider_id: 8,
                    priority: -1,
                },
            ],
        ]) {
            expect(() =>
                parseListProviderPoolsResponse({
                    provider_pools: [
                        {
                            ...pool,
                            members,
                        },
                    ],
                }),
            ).toThrow();
        }
        const direct = {
            is_automatic: false,
            id: 5,
            public_id: "private-M",
            is_enabled: true,
            target: {
                type: "direct",
                model_id: 10,
            },
        };
        const pooled = {
            ...direct,
            id: 6,
            public_id: "coding-M",
            target: {
                type: "pool",
                pool_id: 3,
                upstream_id: "M",
            },
        };
        expect(
            parseListModelRoutesResponse({
                model_routes: [
                    direct,
                    pooled,
                ],
            }),
        ).toEqual({
            model_routes: [
                direct,
                pooled,
            ],
        });
        for (const target of [
            {
                type: "direct",
                model_id: 10,
                pool_id: 3,
            },
            {
                type: "pool",
                pool_id: 3,
            },
            {
                type: "pool",
                pool_id: 3,
                upstream_id: "bad id",
            },
            {
                type: "other",
            },
        ]) {
            expect(() =>
                parseListModelRoutesResponse({
                    model_routes: [
                        {
                            ...direct,
                            target,
                        },
                    ],
                }),
            ).toThrow();
        }
    });
});

describe("availability management decoding", () => {
    it("retains unknown resets and safe refresh outcomes without inventing available measurements", () => {
        const block = {
            key: [
                "account",
                "weekly",
            ],
            applies_to: {
                type: "all_models",
            },
            observed_at: 100,
            blocked_until: null,
        };
        for (const last_refresh of [
            {
                type: "not_observed",
            },
            {
                type: "unsupported",
                attempted_at: 101,
            },
            {
                type: "reset",
                attempted_at: 102,
            },
            {
                type: "failed",
                attempted_at: 103,
                reason: "timeout",
            },
            {
                type: "succeeded",
                attempted_at: 104,
                observed_at: 105,
            },
        ]) {
            const snapshot = {
                blocks: [
                    block,
                ],
                last_refresh,
            };
            expect(parseGetProviderAvailabilityResponse(snapshot)).toEqual(snapshot);
        }
        for (const bad of [
            {
                ...block,
                blocked_until: 100,
            },
            {
                ...block,
                applies_to: {
                    type: "monitoring_only",
                },
            },
            {
                ...block,
                applies_to: {
                    type: "models",
                    upstream_ids: [],
                },
            },
            {
                ...block,
                key: [],
            },
        ]) {
            expect(() =>
                parseGetProviderAvailabilityResponse({
                    blocks: [
                        bad,
                    ],
                    last_refresh: {
                        type: "not_observed",
                    },
                }),
            ).toThrow();
        }
        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [
                    block,
                    block,
                ],
                last_refresh: {
                    type: "not_observed",
                },
            }),
        ).toThrow();
        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [],
                last_refresh: {
                    type: "failed",
                    attempted_at: 1,
                    reason: "secret foreign error",
                },
            }),
        ).toThrow();
    });
});
