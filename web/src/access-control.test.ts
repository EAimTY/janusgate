import { describe, expect, it } from "vitest";
import type { AccessDecision, ApiKeyAccessControl } from "/src/management-api/generated/v0";
import { canonicalPolicy, resolveAccess, setOwnerChoice, setRouteChoice } from "./access-control";

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
});
