/** Sparse public-entry permissions. Global availability is a separate inventory fact. */
import type {
    AccessDecision,
    ApiKeyAccessControl,
    ApiKeyOwnerDefault,
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";

export type Owner = {
    readonly type: "provider" | "pool";
    readonly id: number;
};
export type AccessChoice = AccessDecision | "inherit";
export type AccessInventory = {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly pools: ReadonlyArray<ProviderPoolResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly modelRoutes: ReadonlyArray<ModelRouteResponse>;
};
export type AccessSource = "key" | "provider" | "pool" | "route";
export type ResolvedAccess = {
    readonly access: AccessDecision;
    readonly source: AccessSource;
};

/** New keys deny by default; every API submission still states the complete policy. */
export function newAccessControl(): ApiKeyAccessControl {
    return {
        default_access: "deny",
        owner_defaults: [],
        route_overrides: [],
    };
}

export function ruleOwner(rule: ApiKeyOwnerDefault): Owner {
    return rule.type === "provider"
        ? {
              type: "provider",
              id: rule.provider_id,
          }
        : {
              type: "pool",
              id: rule.pool_id,
          };
}

export function isSameOwner(left: Owner, right: Owner): boolean {
    return left.type === right.type && left.id === right.id;
}

export function ownerChoice(policy: ApiKeyAccessControl, owner: Owner): AccessChoice {
    return (
        policy.owner_defaults.find((rule) => isSameOwner(ruleOwner(rule), owner))?.access ??
        "inherit"
    );
}

export function routeChoice(policy: ApiKeyAccessControl, id: number): AccessChoice {
    return policy.route_overrides.find((rule) => rule.model_route_id === id)?.access ?? "inherit";
}

/** Route → typed owner → key, independent of switches and pool-member decisions. */
export function resolveAccess(
    policy: ApiKeyAccessControl,
    routeId: number | null,
    owner: Owner,
): ResolvedAccess {
    const route =
        routeId === null
            ? undefined
            : policy.route_overrides.find((rule) => rule.model_route_id === routeId);
    if (route)
        return {
            access: route.access,
            source: "route",
        };
    const choice = ownerChoice(policy, owner);
    if (choice !== "inherit")
        return {
            access: choice,
            source: owner.type,
        };
    return {
        access: policy.default_access,
        source: "key",
    };
}

export function accessLabel(result: ResolvedAccess): string {
    const decision = result.access === "allow" ? "Allowed" : "Denied";
    return `${decision} · ${result.source === "route" ? "Model override" : `from ${result.source}`}`;
}

/** Stable order only; no deduplication or removal of explicit equal-default pins. */
export function canonicalPolicy(policy: ApiKeyAccessControl): ApiKeyAccessControl {
    return {
        default_access: policy.default_access,
        owner_defaults: [
            ...policy.owner_defaults,
        ].sort((a, b) => {
            const left = ruleOwner(a);
            const right = ruleOwner(b);
            return left.type === right.type
                ? left.id - right.id
                : left.type === "provider"
                  ? -1
                  : 1;
        }),
        route_overrides: [
            ...policy.route_overrides,
        ].sort((a, b) => a.model_route_id - b.model_route_id),
    };
}

export function isSamePolicy(left: ApiKeyAccessControl, right: ApiKeyAccessControl): boolean {
    return JSON.stringify(canonicalPolicy(left)) === JSON.stringify(canonicalPolicy(right));
}

export function setOwnerChoice(
    policy: ApiKeyAccessControl,
    owner: Owner,
    choice: AccessChoice,
): ApiKeyAccessControl {
    const owner_defaults = policy.owner_defaults.filter(
        (rule) => !isSameOwner(ruleOwner(rule), owner),
    );
    if (choice !== "inherit")
        owner_defaults.push(
            owner.type === "provider"
                ? {
                      type: "provider",
                      provider_id: owner.id,
                      access: choice,
                  }
                : {
                      type: "pool",
                      pool_id: owner.id,
                      access: choice,
                  },
        );
    return {
        ...policy,
        owner_defaults,
    };
}

export function setRouteChoice(
    policy: ApiKeyAccessControl,
    id: number,
    choice: AccessChoice,
): ApiKeyAccessControl {
    const route_overrides = policy.route_overrides.filter((rule) => rule.model_route_id !== id);
    if (choice !== "inherit")
        route_overrides.push({
            model_route_id: id,
            access: choice,
        });
    return {
        ...policy,
        route_overrides,
    };
}

export function routeOwner(route: ModelRouteResponse, inventory: AccessInventory): Owner | null {
    const target = route.target;
    if (target.type === "pool")
        return {
            type: "pool",
            id: target.pool_id,
        };
    const model = inventory.models.find((model) => model.id === target.model_id);
    return model
        ? {
              type: "provider",
              id: model.provider_id,
          }
        : null;
}

/** Known deleted references stay in the draft and must be explicitly removed. */
export function missingReferences(
    policy: ApiKeyAccessControl,
    inventory: AccessInventory,
): ReadonlyArray<{
    readonly label: string;
    readonly access: AccessDecision;
    readonly owner?: Owner;
    readonly routeId?: number;
}> {
    return [
        ...policy.owner_defaults.flatMap((rule) => {
            const owner = ruleOwner(rule);
            const exists =
                owner.type === "provider"
                    ? inventory.providers.some((row) => row.id === owner.id)
                    : inventory.pools.some((row) => row.id === owner.id);
            return exists
                ? []
                : [
                      {
                          label: `Deleted ${owner.type} #${owner.id}`,
                          access: rule.access,
                          owner,
                      },
                  ];
        }),
        ...policy.route_overrides
            .filter(
                (rule) => !inventory.modelRoutes.some((route) => route.id === rule.model_route_id),
            )
            .map((rule) => ({
                label: `Deleted route #${rule.model_route_id}`,
                access: rule.access,
                routeId: rule.model_route_id,
            })),
    ];
}

export function structuralStatus(route: ModelRouteResponse, inventory: AccessInventory): string {
    if (!route.is_enabled) return "Globally disabled";
    const target = route.target;
    if (target.type === "direct") {
        const model = inventory.models.find((model) => model.id === target.model_id);
        const provider = inventory.providers.find((provider) => provider.id === model?.provider_id);
        if (!model || !provider) return "Target unavailable";
        if (!provider.is_enabled || !model.is_enabled) return "Globally disabled";
        return model.discovery_state === "vanished" ? "Target vanished" : "Globally available";
    }
    const pool = inventory.pools.find((pool) => pool.id === target.pool_id);
    if (!pool) return "Target unavailable";
    if (!pool.is_enabled) return "Globally disabled";
    const hasCandidate = pool.members.some(
        (member) =>
            inventory.providers.some(
                (provider) => provider.id === member.provider_id && provider.is_enabled,
            ) &&
            inventory.models.some(
                (model) =>
                    model.provider_id === member.provider_id &&
                    model.upstream_id === target.upstream_id &&
                    model.is_enabled &&
                    model.discovery_state !== "vanished",
            ),
    );
    return hasCandidate ? "Globally available" : "No available pool members";
}
