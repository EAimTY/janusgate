import { describe, expect, test } from "vitest";
import { ContractViolationError } from "./errors";
import {
    parseCreateModelRouteResponse,
    parseCreateProviderPoolResponse,
    parseGetProviderAvailabilityResponse,
    parseListModelRoutesResponse,
    parseListProviderPoolsResponse,
} from "./routing";

/**
 * The pools, public routes, and availability parsers: complete authored pool
 * membership retaining every member by identity (no repeated provider or
 * priority), route targets discriminated direct-versus-pool, created rows
 * answering their permanent identity alone, and the availability
 * observation's blocks carrying complete restrictions — no `monitoring_only`
 * applicability, no empty or repeated key, and no reset at or before the
 * observation instant read as already recovered.
 */

/** One pool row fixture. */
const POOL_ROW = {
    id: 6,
    name: "fast pool",
    model_prefix: "fast",
    is_enabled: true,
    members: [
        {
            provider_id: 1,
            priority: 0,
        },
        {
            provider_id: 2,
            priority: 1,
        },
    ],
};

/** One authored public-route row fixture. */
const ROUTE_ROW = {
    id: 14,
    public_id: "gpt-x",
    is_automatic: false,
    is_enabled: true,
    target: {
        type: "direct",
        model_id: 31,
    },
};

describe("pool response parsers", () => {
    test("parse the listing with complete membership intact", () => {
        const listing = parseListProviderPoolsResponse({
            provider_pools: [
                POOL_ROW,
            ],
        });

        const pool = listing.provider_pools[0];
        if (pool === undefined) throw new Error("the fixture listed no pool");
        expect(pool.members).toEqual([
            {
                provider_id: 1,
                priority: 0,
            },
            {
                provider_id: 2,
                priority: 1,
            },
        ]);
    });

    test("reject a repeated member provider or priority", () => {
        expect(() =>
            parseListProviderPoolsResponse({
                provider_pools: [
                    {
                        ...POOL_ROW,
                        members: [
                            {
                                provider_id: 1,
                                priority: 0,
                            },
                            {
                                provider_id: 1,
                                priority: 1,
                            },
                        ],
                    },
                ],
            }),
        ).toThrowError("must not repeat an earlier provider_id or priority");
        expect(() =>
            parseListProviderPoolsResponse({
                provider_pools: [
                    {
                        ...POOL_ROW,
                        members: [
                            {
                                provider_id: 1,
                                priority: 0,
                            },
                            {
                                provider_id: 2,
                                priority: 0,
                            },
                        ],
                    },
                ],
            }),
        ).toThrowError(ContractViolationError);
    });

    test("reject a negative priority", () => {
        expect(() =>
            parseListProviderPoolsResponse({
                provider_pools: [
                    {
                        ...POOL_ROW,
                        members: [
                            {
                                provider_id: 1,
                                priority: -1,
                            },
                        ],
                    },
                ],
            }),
        ).toThrowError("must be a safe integer of at least 0");
    });

    test("parse the created pool's permanent identity alone", () => {
        expect(
            parseCreateProviderPoolResponse({
                provider_pool_id: 6,
            }),
        ).toEqual({
            provider_pool_id: 6,
        });
    });
});

describe("route response parsers", () => {
    test("parse the listing across both target kinds", () => {
        const listing = parseListModelRoutesResponse({
            model_routes: [
                ROUTE_ROW,
                {
                    id: 15,
                    public_id: "fast-gpt-x",
                    is_automatic: true,
                    is_enabled: false,
                    target: {
                        type: "pool",
                        pool_id: 6,
                        upstream_id: "gpt-x",
                    },
                },
            ],
        });

        expect(listing.model_routes).toHaveLength(2);
        const pooled = listing.model_routes[1];
        if (pooled === undefined || pooled.target.type !== "pool") {
            throw new Error("the fixture's second route must target a pool");
        }
        expect(pooled.target.upstream_id).toBe("gpt-x");
    });

    test("reject a target of neither kind and a public id that is not a routing token", () => {
        expect(() =>
            parseListModelRoutesResponse({
                model_routes: [
                    {
                        ...ROUTE_ROW,
                        target: {
                            type: "alias",
                        },
                    },
                ],
            }),
        ).toThrowError("is not a route target: alias");
        expect(() =>
            parseListModelRoutesResponse({
                model_routes: [
                    {
                        ...ROUTE_ROW,
                        public_id: "models/gpt-x",
                    },
                ],
            }),
        ).toThrowError("is not a routing token: models/gpt-x");
    });

    test("parse the created route's permanent identity alone", () => {
        expect(
            parseCreateModelRouteResponse({
                model_route_id: 14,
            }),
        ).toEqual({
            model_route_id: 14,
        });
    });
});

describe("provider availability parser", () => {
    test("answers blocks beside their refresh metadata", () => {
        const availability = parseGetProviderAvailabilityResponse({
            blocks: [
                {
                    key: [
                        "usage",
                        "tokens",
                    ],
                    applies_to: {
                        type: "models",
                        upstream_ids: [
                            "gpt-x",
                        ],
                    },
                    observed_at: 1_800_000_000_000_000,
                    blocked_until: 1_800_000_100_000_000,
                },
            ],
            last_refresh: {
                type: "succeeded",
                attempted_at: 1_800_000_000_000_000,
                observed_at: 1_800_000_000_500_000,
            },
        });

        expect(availability.blocks).toHaveLength(1);
        expect(availability.last_refresh.type).toBe("succeeded");
    });

    test("answers every refresh category, the null blocked_until included", () => {
        const availability = parseGetProviderAvailabilityResponse({
            blocks: [
                {
                    key: [
                        "k",
                    ],
                    applies_to: {
                        type: "all_models",
                    },
                    observed_at: 10,
                    blocked_until: null,
                },
            ],
            last_refresh: {
                type: "not_observed",
            },
        });

        const block = availability.blocks[0];
        if (block === undefined) throw new Error("the fixture listed no block");
        expect(block.blocked_until).toBeNull();
        expect(availability.last_refresh).toEqual({
            type: "not_observed",
        });
    });

    test("rejects a monitoring_only applicability, an empty or repeated key, and a spent reset", () => {
        const base = {
            key: [
                "k",
            ],
            applies_to: {
                type: "all_models",
            },
            observed_at: 10,
            blocked_until: null,
        };

        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [
                    {
                        ...base,
                        applies_to: {
                            type: "monitoring_only",
                        },
                    },
                ],
                last_refresh: {
                    type: "not_observed",
                },
            }),
        ).toThrowError("must not be monitoring_only");

        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [
                    {
                        ...base,
                        key: [],
                    },
                ],
                last_refresh: {
                    type: "not_observed",
                },
            }),
        ).toThrowError("must not be empty or repeat another block's key");

        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [
                    {
                        ...base,
                        key: [
                            "a",
                        ],
                    },
                    {
                        ...base,
                        key: [
                            "a",
                        ],
                    },
                ],
                last_refresh: {
                    type: "not_observed",
                },
            }),
        ).toThrowError(ContractViolationError);

        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [
                    {
                        ...base,
                        observed_at: 10,
                        blocked_until: 10,
                    },
                ],
                last_refresh: {
                    type: "not_observed",
                },
            }),
        ).toThrowError("blocked_until must be after observed_at");
    });

    test("rejects a failed refresh with a foreign reason and an undeclared member", () => {
        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [],
                last_refresh: {
                    type: "failed",
                    attempted_at: 5,
                    reason: "vibes",
                },
            }),
        ).toThrowError("is not a refresh failure: vibes");

        expect(() =>
            parseGetProviderAvailabilityResponse({
                blocks: [],
                last_refresh: {
                    type: "reset",
                    attempted_at: 5,
                    extra: 1,
                },
            }),
        ).toThrowError("response.last_refresh.extra is not a declared field");
    });
});
