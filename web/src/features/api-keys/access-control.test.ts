import { describe, expect, it } from "vitest";
import { translation } from "/src/i18n";
import type {
    AccessDecision,
    ApiKeyAccessControl,
    ModelRouteResponse,
} from "/src/management-api/generated/v0";
import { modelRow, providerRow } from "/src/test-support/management-api";
import {
    type AccessInventory,
    canonicalPolicy,
    isSamePolicy,
    resolveAccess,
    setOwnerChoice,
    setRouteChoice,
    structuralStatus,
} from "./access-control";

describe("public route access", () => {
    it("resolves the complete precedence matrix and keeps owner kinds distinct", () => {
        const decisions: ReadonlyArray<AccessDecision> = [
            "allow",
            "deny",
        ];
        for (const default_access of decisions) {
            for (const ownerDecision of [
                undefined,
                ...decisions,
            ]) {
                for (const routeDecision of [
                    undefined,
                    ...decisions,
                ]) {
                    for (const type of [
                        "provider",
                        "pool",
                    ] as const) {
                        const owner = {
                            type,
                            id: 7,
                        };
                        let policy: ApiKeyAccessControl = {
                            default_access,
                            owner_defaults: [],
                            route_overrides: [],
                        };
                        policy = setOwnerChoice(policy, owner, ownerDecision ?? "inherit");
                        policy = setRouteChoice(policy, 41, routeDecision ?? "inherit");
                        expect(resolveAccess(policy, 41, owner).access).toBe(
                            routeDecision ?? ownerDecision ?? default_access,
                        );
                        expect(
                            resolveAccess(policy, 42, {
                                type: type === "pool" ? "provider" : "pool",
                                id: 7,
                            }).access,
                        ).toBe(default_access);
                    }
                }
            }
        }
    });

    it("preserves equal-default intent and removes only the chosen override", () => {
        const original: ApiKeyAccessControl = {
            default_access: "allow",
            owner_defaults: [],
            route_overrides: [],
        };
        const pinned = setRouteChoice(setRouteChoice(original, 2, "deny"), 1, "allow");
        const changed = {
            ...pinned,
            default_access: "deny" as const,
        };
        expect(
            resolveAccess(changed, 1, {
                type: "provider",
                id: 7,
            }).access,
        ).toBe("allow");
        expect(setRouteChoice(changed, 1, "inherit").route_overrides).toEqual([
            {
                model_route_id: 2,
                access: "deny",
            },
        ]);
        expect(original.route_overrides).toEqual([]);
        expect(canonicalPolicy(pinned)).toEqual(
            canonicalPolicy({
                ...pinned,
                route_overrides: [
                    ...pinned.route_overrides,
                ].reverse(),
            }),
        );
    });

    it("compares policies structurally, indifferent to key order and rule order", () => {
        // The first policy's rules are spelled in a parser's key order, the
        // second built by the draft constructors — today's serialization
        // equality agreed only because the two orders happened to coincide.
        const parserShaped: ApiKeyAccessControl = {
            route_overrides: [
                {
                    access: "deny",
                    model_route_id: 2,
                },
            ],
            owner_defaults: [
                {
                    access: "allow",
                    type: "provider",
                    provider_id: 7,
                },
                {
                    access: "deny",
                    type: "pool",
                    pool_id: 3,
                },
            ],
            default_access: "deny",
        };
        const constructorShaped = setRouteChoice(
            setOwnerChoice(
                setOwnerChoice(
                    {
                        default_access: "deny",
                        owner_defaults: [],
                        route_overrides: [],
                    },
                    {
                        type: "pool",
                        id: 3,
                    },
                    "deny",
                ),
                {
                    type: "provider",
                    id: 7,
                },
                "allow",
            ),
            2,
            "deny",
        );
        expect(isSamePolicy(parserShaped, constructorShaped)).toBe(true);

        // Rule order alone is not a difference; any identity segment,
        // decision, or the default is.
        expect(
            isSamePolicy(parserShaped, {
                ...parserShaped,
                owner_defaults: [
                    {
                        type: "pool",
                        pool_id: 3,
                        access: "deny",
                    },
                    {
                        type: "provider",
                        provider_id: 7,
                        access: "allow",
                    },
                ],
            }),
        ).toBe(true);
        expect(
            isSamePolicy(parserShaped, {
                ...parserShaped,
                default_access: "allow",
            }),
        ).toBe(false);
        expect(
            isSamePolicy(parserShaped, {
                ...parserShaped,
                owner_defaults: [
                    {
                        type: "provider",
                        provider_id: 7,
                        access: "deny",
                    },
                ],
            }),
        ).toBe(false);
        expect(
            isSamePolicy(parserShaped, {
                ...parserShaped,
                route_overrides: [
                    {
                        model_route_id: 2,
                        access: "allow",
                    },
                ],
            }),
        ).toBe(false);
    });
});

describe("structural status", () => {
    const provider = providerRow();
    const model = modelRow({
        upstream_id: "M",
    });
    const disabledModel = modelRow({
        id: 12,
        upstream_id: "off",
        is_enabled: false,
    });
    const vanishedModel = modelRow({
        id: 13,
        upstream_id: "gone",
        discovery_state: "vanished",
    });
    const pool = {
        id: 5,
        name: "east",
        model_prefix: "east.",
        is_enabled: true,
        members: [
            {
                provider_id: provider.id,
                priority: 1,
            },
        ],
    };
    const inventory: AccessInventory = {
        providers: [
            provider,
        ],
        pools: [
            pool,
        ],
        models: [
            model,
            disabledModel,
            vanishedModel,
        ],
        modelRoutes: [],
    };

    const route = (target: ModelRouteResponse["target"], isEnabled = true): ModelRouteResponse => ({
        id: 30,
        public_id: "gpt-x",
        is_enabled: isEnabled,
        is_automatic: true,
        target,
    });
    const status = (target: ModelRouteResponse["target"], isEnabled = true): string =>
        structuralStatus(route(target, isEnabled), inventory);

    it("renders the direct target's rungs over the shared structural ladder", () => {
        const direct = (model_id: number): ModelRouteResponse["target"] => ({
            type: "direct",
            model_id,
        });
        expect(status(direct(model.id))).toBe(
            translation()(($) => $.helpers.accessControl.globallyAvailable),
        );
        expect(status(direct(999))).toBe(
            translation()(($) => $.helpers.accessControl.targetUnavailable),
        );
        expect(status(direct(model.id), false)).toBe(
            translation()(($) => $.helpers.accessControl.globallyDisabled),
        );
        expect(status(direct(disabledModel.id))).toBe(
            translation()(($) => $.helpers.accessControl.globallyDisabled),
        );
        expect(status(direct(vanishedModel.id))).toBe(
            translation()(($) => $.helpers.accessControl.targetVanished),
        );
        // The account rung renders for a present model under a disabled
        // account; a missing model leaves the account unresolvable (the join
        // runs through the model), so the absence verdict stands.
        const disabledAccountInventory: AccessInventory = {
            ...inventory,
            providers: [
                {
                    ...provider,
                    is_enabled: false,
                },
            ],
        };
        expect(structuralStatus(route(direct(model.id)), disabledAccountInventory)).toBe(
            translation()(($) => $.helpers.accessControl.globallyDisabled),
        );
        expect(
            structuralStatus(route(direct(999)), {
                ...disabledAccountInventory,
                models: [],
            }),
        ).toBe(translation()(($) => $.helpers.accessControl.targetUnavailable));
    });

    it("renders the pooled target from the pool's switch and its members' readiness", () => {
        const pooled: ModelRouteResponse["target"] = {
            type: "pool",
            pool_id: pool.id,
            upstream_id: "M",
        };
        expect(status(pooled)).toBe(
            translation()(($) => $.helpers.accessControl.globallyAvailable),
        );
        expect(status(pooled, false)).toBe(
            translation()(($) => $.helpers.accessControl.globallyDisabled),
        );
        expect(
            status({
                type: "pool",
                pool_id: 999,
                upstream_id: "M",
            }),
        ).toBe(translation()(($) => $.helpers.accessControl.targetUnavailable));
        // A pool whose switch stands but whose members serve nothing: a
        // disabled member account and a missing upstream ID both fail ready.
        expect(
            structuralStatus(route(pooled), {
                ...inventory,
                providers: [
                    {
                        ...provider,
                        is_enabled: false,
                    },
                ],
            }),
        ).toBe(translation()(($) => $.helpers.accessControl.noAvailablePoolMembers));
        expect(
            structuralStatus(
                route({
                    type: "pool",
                    pool_id: pool.id,
                    upstream_id: "absent",
                }),
                {
                    ...inventory,
                },
            ),
        ).toBe(translation()(($) => $.helpers.accessControl.noAvailablePoolMembers));
    });
});
