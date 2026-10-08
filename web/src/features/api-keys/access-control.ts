import { translation } from "/src/i18n";
/** Sparse public-entry permissions. Global availability is a separate inventory fact. */
import type {
    AccessDecision,
    ApiKeyAccessControl,
    ApiKeyOwnerDefault,
    ApiKeyRouteOverride,
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { indexRowsById } from "/src/row-index";
import {
    classifyStructuralAvailability,
    type StructuralAvailability,
} from "../inventory/availability";

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
type AccessSource = "key" | "provider" | "pool" | "route";
type ResolvedAccess = {
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

function ruleOwner(rule: ApiKeyOwnerDefault): Owner {
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

function isSameOwner(left: Owner, right: Owner): boolean {
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

/** One decision's two words, resolved lazily so the catalog in force renders. */
export type AccessDecisionWords = {
    readonly allow: string;
    readonly deny: string;
};

/**
 * The `AccessDecision` → word mapping, the conditional's single home: the
 * allow decision reads the pair's allow word, deny its deny word. The two
 * label pairs in force are deliberate — the resolved state reads the past
 * participle ("Allowed"), the choice controls the imperative ("Allow") — so
 * the pair rides the call.
 */
export function accessDecisionLabel(decision: AccessDecision, words: AccessDecisionWords): string {
    return decision === "allow" ? words.allow : words.deny;
}

export function accessLabel(result: ResolvedAccess): string {
    const decision = accessDecisionLabel(result.access, {
        get allow() {
            return translation()(($) => $.helpers.accessControl.allowed);
        },
        get deny() {
            return translation()(($) => $.helpers.accessControl.denied);
        },
    });
    return `${decision} · ${result.source === "route" ? translation()(($) => $.helpers.accessControl.modelOverride) : translation()(($) => $.domainMessages.source[result.source])}`;
}

/** Stable order only; no deduplication or removal of explicit equal-default pins. */
function canonicalPolicy(policy: ApiKeyAccessControl): ApiKeyAccessControl {
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

/**
 * Structural equality over the canonical order: the default decision and the
 * two rule arrays compared element-wise by their identity segments and
 * decision — never by serialization, since an object's key order is not
 * data. Canonical sorting first makes it order-insensitive.
 */
export function isSamePolicy(left: ApiKeyAccessControl, right: ApiKeyAccessControl): boolean {
    const leftCanonical = canonicalPolicy(left);
    const rightCanonical = canonicalPolicy(right);
    return (
        leftCanonical.default_access === rightCanonical.default_access &&
        isSameOrderedRules(
            leftCanonical.owner_defaults,
            rightCanonical.owner_defaults,
            isSameOwnerRule,
        ) &&
        isSameOrderedRules(
            leftCanonical.route_overrides,
            rightCanonical.route_overrides,
            isSameRouteRule,
        )
    );
}

/** Element-wise equality over two canonically ordered rule arrays. */
function isSameOrderedRules<TRule>(
    left: ReadonlyArray<TRule>,
    right: ReadonlyArray<TRule>,
    isSameRule: (leftRule: TRule, rightRule: TRule) => boolean,
): boolean {
    return (
        left.length === right.length &&
        left.every((leftRule, index) => {
            const rightRule = right[index];
            return rightRule !== undefined && isSameRule(leftRule, rightRule);
        })
    );
}

/** One owner-default rule's identity tuple and decision. */
function isSameOwnerRule(left: ApiKeyOwnerDefault, right: ApiKeyOwnerDefault): boolean {
    return isSameOwner(ruleOwner(left), ruleOwner(right)) && left.access === right.access;
}

/** One route-override rule's identity tuple and decision. */
function isSameRouteRule(left: ApiKeyRouteOverride, right: ApiKeyRouteOverride): boolean {
    return left.model_route_id === right.model_route_id && left.access === right.access;
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

/** The owner's composite map key: type and id are unique together by contract. */
export function ownerKey(owner: Owner): string {
    return `${owner.type}:${owner.id}`;
}

/**
 * The route's owner against an id-indexed model map: a pool target owns
 * itself, a direct target rides its model's provider. A model answering no
 * id leaves the route unowned — the missing-reference sweep owns it there.
 */
function indexedRouteOwner(
    route: ModelRouteResponse,
    models: ReadonlyMap<number, ModelResponse>,
): Owner | null {
    const target = route.target;
    if (target.type === "pool")
        return {
            type: "pool",
            id: target.pool_id,
        };
    const model = models.get(target.model_id);
    return model
        ? {
              type: "provider",
              id: model.provider_id,
          }
        : null;
}

/**
 * Routes grouped by resolved owner, built in one sweep against an
 * id-indexed model map: the per-owner walk this replaces resolved every
 * route against a linear model lookup once per rendered cell per reactive
 * run — O(routes × models) each time.
 */
export function groupRoutesByOwner(
    inventory: AccessInventory,
): ReadonlyMap<string, ReadonlyArray<ModelRouteResponse>> {
    const models = indexRowsById(inventory.models);
    const grouped = new Map<string, Array<ModelRouteResponse>>();
    for (const route of inventory.modelRoutes) {
        const identity = indexedRouteOwner(route, models);
        if (identity === null) {
            continue;
        }
        const key = ownerKey(identity);
        const owned = grouped.get(key);
        if (owned === undefined) {
            grouped.set(key, [
                route,
            ]);
        } else {
            owned.push(route);
        }
    }
    return grouped;
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
                          label: translation()(($) => $.domainMessages.deleted[owner.type], {
                              id: owner.id,
                          }),
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
                label: translation()(($) => $.domainMessages.deleted.route, {
                    id: rule.model_route_id,
                }),
                access: rule.access,
                routeId: rule.model_route_id,
            })),
    ];
}

/**
 * The direct target's label per shared structural rung, resolved lazily so
 * the catalog in force renders; the record type is exhaustive, so a rung
 * added to `StructuralAvailability` fails to compile until it is labeled. A
 * missing row is unavailable, a thrown switch disabled, a vanished row
 * vanished, and only the ready rung serves.
 */
const DIRECT_TARGET_STATUS: Record<
    StructuralAvailability,
    {
        readonly status: string;
    }
> = {
    account_missing: {
        get status() {
            return translation()(($) => $.helpers.accessControl.targetUnavailable);
        },
    },
    model_missing: {
        get status() {
            return translation()(($) => $.helpers.accessControl.targetUnavailable);
        },
    },
    account_disabled: {
        get status() {
            return translation()(($) => $.helpers.accessControl.globallyDisabled);
        },
    },
    model_disabled: {
        get status() {
            return translation()(($) => $.helpers.accessControl.globallyDisabled);
        },
    },
    model_vanished: {
        get status() {
            return translation()(($) => $.helpers.accessControl.targetVanished);
        },
    },
    ready: {
        get status() {
            return translation()(($) => $.helpers.accessControl.globallyAvailable);
        },
    },
};

export function structuralStatus(route: ModelRouteResponse, inventory: AccessInventory): string {
    if (!route.is_enabled) return translation()(($) => $.helpers.accessControl.globallyDisabled);
    const target = route.target;
    if (target.type === "direct") {
        const model = inventory.models.find((model) => model.id === target.model_id);
        const provider = inventory.providers.find((provider) => provider.id === model?.provider_id);
        return DIRECT_TARGET_STATUS[classifyStructuralAvailability(provider, model)].status;
    }
    const pool = inventory.pools.find((pool) => pool.id === target.pool_id);
    if (!pool) return translation()(($) => $.helpers.accessControl.targetUnavailable);
    if (!pool.is_enabled) return translation()(($) => $.helpers.accessControl.globallyDisabled);
    // One ready member candidate is the pool's serving verdict: the shared
    // ladder's ready rung per member, over the pool's exact upstream ID.
    const hasReadyMember = pool.members.some((member) => {
        const provider = inventory.providers.find(
            (provider) => provider.id === member.provider_id && provider.is_enabled,
        );
        const model =
            provider === undefined
                ? undefined
                : inventory.models.find(
                      (model) =>
                          model.provider_id === provider.id &&
                          model.upstream_id === target.upstream_id,
                  );
        return classifyStructuralAvailability(provider, model) === "ready";
    });
    return hasReadyMember
        ? translation()(($) => $.helpers.accessControl.globallyAvailable)
        : translation()(($) => $.helpers.accessControl.noAvailablePoolMembers);
}
