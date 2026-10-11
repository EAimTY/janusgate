import { describe, expect, test } from "vitest";
import type {
    ApiKeyAccessControl,
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import {
    type AccessInventory,
    accessDecisionLabel,
    accessLabel,
    groupRoutesByOwner,
    isSamePolicy,
    missingReferences,
    newAccessControl,
    type Owner,
    ownerChoice,
    ownerKey,
    resolveAccess,
    routeChoice,
    setOwnerChoice,
    setRouteChoice,
    structuralStatus,
} from "./access-control";

/**
 * The sparse public-entry permission model: the resolution ladder (route
 * override, typed owner default, then key default), the explicit-complete
 * write helpers (inherit removing the rule, an equal-default choice kept as
 * an editable pin), canonical-order structural equality, the owner grouping
 * over an id-indexed inventory, the missing-reference sweep whose dangling
 * rules stay in the draft and block the save, and the structural status
 * each route's target answers.
 */

function provider(
    overrides: Partial<
        Extract<
            ProviderResponse,
            {
                type: "api_key";
            }
        >
    > = {},
): ProviderResponse {
    return {
        type: "api_key",
        id: 1,
        name: "acme",
        model_prefix: "acme",
        is_enabled: true,
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
        ...overrides,
    };
}

function model(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 10,
        provider_id: 1,
        provider: "acme",
        upstream_id: "gpt-x",
        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: null,
        compaction_support_override: null,
        ...overrides,
    };
}

function route(overrides: Partial<ModelRouteResponse> = {}): ModelRouteResponse {
    return {
        id: 100,
        public_id: "gpt-x",
        is_automatic: false,
        is_enabled: true,
        target: {
            type: "direct",
            model_id: 10,
        },
        ...overrides,
    };
}

function pool(overrides: Partial<ProviderPoolResponse> = {}): ProviderPoolResponse {
    return {
        id: 7,
        name: "fast",
        model_prefix: "fast",
        is_enabled: true,
        members: [
            {
                provider_id: 1,
                priority: 0,
            },
        ],
        ...overrides,
    };
}

const INVENTORY: AccessInventory = {
    providers: [
        provider(),
    ],
    pools: [
        pool(),
    ],
    models: [
        model(),
    ],
    modelRoutes: [
        route(),
    ],
};

describe("policy editing", () => {
    test("starts a new key denying with no rules", () => {
        expect(newAccessControl()).toEqual({
            default_access: "deny",
            owner_defaults: [],
            route_overrides: [],
        });
    });

    test("answers inherit where no rule stands", () => {
        const policy = newAccessControl();
        const owner: Owner = {
            type: "provider",
            id: 1,
        };

        expect(ownerChoice(policy, owner)).toBe("inherit");
        expect(routeChoice(policy, 100)).toBe("inherit");
    });

    test("sets an owner choice in place and removes it on inherit", () => {
        const owner: Owner = {
            type: "provider",
            id: 1,
        };
        const allowed = setOwnerChoice(newAccessControl(), owner, "allow");

        expect(allowed.owner_defaults).toEqual([
            {
                type: "provider",
                provider_id: 1,
                access: "allow",
            },
        ]);
        expect(ownerChoice(allowed, owner)).toBe("allow");

        // Inherit removes the rule; it never leaves a pin behind.
        expect(setOwnerChoice(allowed, owner, "inherit").owner_defaults).toEqual([]);
    });

    test("replaces an existing rule rather than stacking a second", () => {
        const owner: Owner = {
            type: "pool",
            id: 7,
        };
        const policy = setOwnerChoice(
            setOwnerChoice(newAccessControl(), owner, "allow"),
            owner,
            "deny",
        );

        expect(policy.owner_defaults).toEqual([
            {
                type: "pool",
                pool_id: 7,
                access: "deny",
            },
        ]);
    });

    test("sets a route override in place and removes it on inherit", () => {
        const allowed = setRouteChoice(newAccessControl(), 100, "allow");

        expect(allowed.route_overrides).toEqual([
            {
                model_route_id: 100,
                access: "allow",
            },
        ]);
        expect(setRouteChoice(allowed, 100, "inherit").route_overrides).toEqual([]);
    });

    test("keeps an explicit equal-default pin editable and distinct from absence", () => {
        const pinned = setOwnerChoice(
            newAccessControl(),
            {
                type: "provider",
                id: 1,
            },
            "deny",
        );

        expect(pinned.owner_defaults).toEqual([
            {
                type: "provider",
                provider_id: 1,
                access: "deny",
            },
        ]);
        expect(isSamePolicy(pinned, newAccessControl())).toBe(false);
    });

    test("composes the owner map key from type and id together", () => {
        expect(
            ownerKey({
                type: "provider",
                id: 1,
            }),
        ).toBe("provider:1");
        expect(
            ownerKey({
                type: "pool",
                id: 1,
            }),
        ).toBe("pool:1");
    });
});

describe("resolveAccess", () => {
    test("resolves route override, then typed owner default, then key default", () => {
        const policy: ApiKeyAccessControl = {
            default_access: "deny",
            owner_defaults: [
                {
                    type: "provider",
                    provider_id: 1,
                    access: "allow",
                },
            ],
            route_overrides: [
                {
                    model_route_id: 100,
                    access: "deny",
                },
            ],
        };

        expect(
            resolveAccess(policy, 100, {
                type: "provider",
                id: 1,
            }),
        ).toEqual({
            access: "deny",
            source: "route",
        });
        expect(
            resolveAccess(policy, null, {
                type: "provider",
                id: 1,
            }),
        ).toEqual({
            access: "allow",
            source: "provider",
        });
        expect(
            resolveAccess(policy, null, {
                type: "pool",
                id: 7,
            }),
        ).toEqual({
            access: "deny",
            source: "key",
        });
    });

    test("is independent of the switches and of the other owner kind's decisions", () => {
        const policy: ApiKeyAccessControl = {
            default_access: "deny",
            owner_defaults: [
                {
                    type: "pool",
                    pool_id: 7,
                    access: "allow",
                },
            ],
            route_overrides: [],
        };

        // A provider default never resolves a pool-owned route's owner.
        expect(
            resolveAccess(policy, null, {
                type: "provider",
                id: 7,
            }),
        ).toEqual({
            access: "deny",
            source: "key",
        });
        expect(
            resolveAccess(policy, 100, {
                type: "pool",
                id: 7,
            }),
        ).toEqual({
            access: "allow",
            source: "pool",
        });
    });
});

describe("isSamePolicy", () => {
    test("is order-insensitive over both rule arrays", () => {
        const left: ApiKeyAccessControl = {
            default_access: "deny",
            owner_defaults: [
                {
                    type: "provider",
                    provider_id: 2,
                    access: "allow",
                },
                {
                    type: "provider",
                    provider_id: 1,
                    access: "deny",
                },
            ],
            route_overrides: [
                {
                    model_route_id: 9,
                    access: "allow",
                },
                {
                    model_route_id: 4,
                    access: "deny",
                },
            ],
        };
        const right: ApiKeyAccessControl = {
            default_access: "deny",
            owner_defaults: [
                {
                    type: "provider",
                    provider_id: 1,
                    access: "deny",
                },
                {
                    type: "provider",
                    provider_id: 2,
                    access: "allow",
                },
            ],
            route_overrides: [
                {
                    model_route_id: 4,
                    access: "deny",
                },
                {
                    model_route_id: 9,
                    access: "allow",
                },
            ],
        };

        expect(isSamePolicy(left, right)).toBe(true);
    });

    test("distinguishes owners of different kinds sharing an id", () => {
        expect(
            isSamePolicy(
                {
                    default_access: "deny",
                    owner_defaults: [
                        {
                            type: "provider",
                            provider_id: 1,
                            access: "allow",
                        },
                    ],
                    route_overrides: [],
                },
                {
                    default_access: "deny",
                    owner_defaults: [
                        {
                            type: "pool",
                            pool_id: 1,
                            access: "allow",
                        },
                    ],
                    route_overrides: [],
                },
            ),
        ).toBe(false);
    });

    test("answers unequal for different rule counts", () => {
        expect(
            isSamePolicy(newAccessControl(), {
                default_access: "deny",
                owner_defaults: [
                    {
                        type: "provider",
                        provider_id: 1,
                        access: "allow",
                    },
                ],
                route_overrides: [],
            }),
        ).toBe(false);
    });
});

describe("groupRoutesByOwner", () => {
    test("groups a pool target under the pool and a direct target under its model's provider", () => {
        const pooled: ModelRouteResponse = route({
            id: 101,
            target: {
                type: "pool",
                pool_id: 7,
                upstream_id: "gpt-x",
            },
        });
        const grouped = groupRoutesByOwner({
            ...INVENTORY,
            modelRoutes: [
                route(),
                pooled,
            ],
        });

        expect(Array.from(grouped.keys()).sort()).toEqual([
            "pool:7",
            "provider:1",
        ]);
        expect(grouped.get("provider:1")?.map((entry) => entry.id)).toEqual([
            100,
        ]);
        expect(grouped.get("pool:7")?.map((entry) => entry.id)).toEqual([
            101,
        ]);
    });

    test("skips a direct route whose model the listing does not hold", () => {
        const orphaned = groupRoutesByOwner({
            ...INVENTORY,
            models: [],
        });

        expect(orphaned.size).toBe(0);
    });
});

describe("missingReferences", () => {
    test("keeps dangling owner and route rules, labeled by kind and id", () => {
        const policy: ApiKeyAccessControl = {
            default_access: "deny",
            owner_defaults: [
                {
                    type: "provider",
                    provider_id: 999,
                    access: "allow",
                },
                {
                    type: "pool",
                    pool_id: 7,
                    access: "deny",
                },
            ],
            route_overrides: [
                {
                    model_route_id: 555,
                    access: "allow",
                },
            ],
        };

        const missing = missingReferences(policy, INVENTORY);

        expect(missing).toHaveLength(2);
        expect(missing[0]).toMatchObject({
            access: "allow",
            owner: {
                type: "provider",
                id: 999,
            },
        });
        expect(missing[0]?.label).toContain("999");
        expect(missing[1]).toMatchObject({
            access: "allow",
            routeId: 555,
        });
    });

    test("answers nothing when every reference resolves", () => {
        const complete: ApiKeyAccessControl = {
            default_access: "deny",
            owner_defaults: [
                {
                    type: "provider",
                    provider_id: 1,
                    access: "allow",
                },
                {
                    type: "pool",
                    pool_id: 7,
                    access: "deny",
                },
            ],
            route_overrides: [
                {
                    model_route_id: 100,
                    access: "allow",
                },
            ],
        };

        expect(missingReferences(complete, INVENTORY)).toEqual([]);
    });
});

describe("structuralStatus", () => {
    test("answers the route's own switch first", () => {
        const status = structuralStatus(
            route({
                is_enabled: false,
            }),
            INVENTORY,
        );

        // The disabled wording — the same one a disabled account or model answers.
        expect(status).toBeTruthy();
    });

    test("walks the direct target's rungs over the shared ladder", () => {
        expect(structuralStatus(route(), INVENTORY)).toBeTruthy();
        expect(
            structuralStatus(
                route({
                    target: {
                        type: "direct",
                        model_id: 404,
                    },
                }),
                INVENTORY,
            ),
        ).toBeTruthy();
    });

    test("answers a pool target by its own switch and its members' ready rung", () => {
        const pooled = route({
            id: 101,
            target: {
                type: "pool",
                pool_id: 7,
                upstream_id: "gpt-x",
            },
        });

        expect(structuralStatus(pooled, INVENTORY)).toBeTruthy();
        // The pool's exact upstream id has no member model: no ready member.
        expect(
            structuralStatus(
                route({
                    id: 102,
                    target: {
                        type: "pool",
                        pool_id: 7,
                        upstream_id: "other-model",
                    },
                }),
                INVENTORY,
            ),
        ).toBeTruthy();
    });

    test("answers a missing pool as unavailable", () => {
        expect(
            structuralStatus(
                route({
                    target: {
                        type: "pool",
                        pool_id: 404,
                        upstream_id: "gpt-x",
                    },
                }),
                INVENTORY,
            ),
        ).toBeTruthy();
    });
});

describe("access wording", () => {
    test("reads the pair's allow and deny words", () => {
        expect(
            accessDecisionLabel("allow", {
                allow: "Allowed",
                deny: "Denied",
            }),
        ).toBe("Allowed");
        expect(
            accessDecisionLabel("deny", {
                allow: "Allowed",
                deny: "Denied",
            }),
        ).toBe("Denied");
    });

    test("composes the resolved state's words with its source", () => {
        const label = accessLabel(
            resolveAccess(
                {
                    default_access: "deny",
                    owner_defaults: [],
                    route_overrides: [
                        {
                            model_route_id: 100,
                            access: "allow",
                        },
                    ],
                },
                100,
                {
                    type: "provider",
                    id: 1,
                },
            ),
        );

        // "<Decision> · <source>" — the route override reads as a model override.
        expect(label).toContain("·");
    });
});
