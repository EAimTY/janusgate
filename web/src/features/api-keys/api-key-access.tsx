import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { translation } from "/src/i18n";
import {
    directAccessTargetMessage,
    modelAccessMessage,
    ownerDefaultMessage,
    poolAccessTargetMessage,
} from "/src/i18n/rich";
import type {
    AccessDecision,
    ApiKeyAccessControl,
    ModelRouteResponse,
} from "/src/management-api/generated/v0";
import { Button, SelectField, Switch, TextField } from "/src/ui/controls";
import {
    type AccessChoice,
    type AccessDecisionWords,
    type AccessInventory,
    accessDecisionLabel,
    accessLabel,
    groupRoutesByOwner,
    missingReferences,
    type Owner,
    ownerChoice,
    ownerKey,
    resolveAccess,
    routeChoice,
    setOwnerChoice,
    setRouteChoice,
    structuralStatus,
} from "./access-control";

const choices: ReadonlyArray<{
    readonly value: AccessChoice;
    readonly label: string;
}> = [
    {
        value: "inherit",
        get label() {
            return translation()(($) => $.helpers.apiKeyAccess.inherit);
        },
    },
    {
        value: "allow",
        get label() {
            return translation()(($) => $.helpers.apiKeyAccess.allow);
        },
    },
    {
        value: "deny",
        get label() {
            return translation()(($) => $.helpers.apiKeyAccess.deny);
        },
    },
];
const defaults: ReadonlyArray<{
    readonly value: AccessDecision;
    readonly label: string;
}> = [
    {
        value: "allow",
        get label() {
            return translation()(($) => $.helpers.apiKeyAccess.allow);
        },
    },
    {
        value: "deny",
        get label() {
            return translation()(($) => $.helpers.apiKeyAccess.deny);
        },
    },
];
/** The choice words, read lazily so the catalog in force renders. */
const decisionWords: AccessDecisionWords = {
    get allow() {
        return translation()(($) => $.helpers.apiKeyAccess.allow);
    },
    get deny() {
        return translation()(($) => $.helpers.apiKeyAccess.deny);
    },
};

/** One policy view shared by complete-state forms and the read-only key detail. */
export function ApiKeyAccess(props: {
    readonly policy: ApiKeyAccessControl;
    readonly inventory: AccessInventory;
    readonly onChange?: (policy: ApiKeyAccessControl) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    const [search, setSearch] = createSignal("");
    const [hasOverridesOnly, setHasOverridesOnly] = createSignal(false);
    const owners = createMemo(() => [
        ...props.inventory.providers.map((provider) => ({
            identity: {
                type: "provider",
                id: provider.id,
            } as const,
            name: provider.name,
        })),
        ...props.inventory.pools.map((pool) => ({
            identity: {
                type: "pool",
                id: pool.id,
            } as const,
            name: pool.name,
        })),
    ]);
    const matches = (value: string): boolean =>
        value.toLowerCase().includes(search().toLowerCase());
    // One grouping sweep per inventory change (`groupRoutesByOwner` indexes
    // the model rows once), so the per-owner reads below resolve from the
    // memo instead of walking routes × models per rendered cell — the
    // indexed-once standard row-index.ts charters.
    const routesByOwner = createMemo(() => groupRoutesByOwner(props.inventory));
    const routesFor = (owner: Owner): ReadonlyArray<ModelRouteResponse> =>
        routesByOwner().get(ownerKey(owner)) ?? [];
    const visibleRoutes = (owner: Owner, name: string): ReadonlyArray<ModelRouteResponse> =>
        routesFor(owner).filter(
            (route) =>
                (matches(name) || matches(route.public_id)) &&
                (!hasOverridesOnly() || routeChoice(props.policy, route.id) !== "inherit"),
        );
    const isOwnerVisible = (owner: Owner, name: string): boolean =>
        visibleRoutes(owner, name).length > 0 ||
        (matches(name) && (!hasOverridesOnly() || ownerChoice(props.policy, owner) !== "inherit"));
    // One computed sweep per reactive run: the missing-reference scan walks
    // every rule against all four listings, translating each missing rule —
    // the render and the submit-gate read it together.
    const missing = createMemo(() => missingReferences(props.policy, props.inventory));

    return (
        <section
            class="flex min-w-0 flex-col gap-4"
            aria-label={translation()(($) => $.controls.modelAccess)}
        >
            <div>
                <h3 class="text-[15px] font-semibold text-ink">
                    {translation()(($) => $.controls.modelAccess)}
                </h3>
                <p class="mt-1 text-[13px] text-ink-faint">
                    {translation()(
                        ($) => $.controls.defaultsAffectExistingInheritingEntriesPoolAccessIncludes,
                    )}
                </p>
            </div>
            <Show
                when={props.onChange}
                fallback={
                    <p class="text-[13px] text-ink">
                        {translation()(($) => $.controls.defaultModelAccessLabel)}{" "}
                        {accessDecisionLabel(props.policy.default_access, decisionWords)}
                    </p>
                }
            >
                <SelectField
                    label={translation()(($) => $.controls.defaultModelAccess)}
                    value={props.policy.default_access}
                    options={defaults}
                    onChange={(default_access) =>
                        props.onChange?.({
                            ...props.policy,
                            default_access,
                        })
                    }
                    isDisabled={props.isDisabled ?? false}
                    hint={translation()(($) => $.controls.appliesToEntriesWithoutAProviderPoolOr)}
                />
            </Show>
            <div class="flex flex-col gap-3 sm:flex-row sm:items-end">
                <TextField
                    label={translation()(($) => $.controls.searchModelAccess)}
                    value={search()}
                    onInput={setSearch}
                    placeholder={translation()(($) => $.controls.publicIDOrOwnerName)}
                    class="min-w-0 flex-1"
                />
                <Switch
                    label={translation()(($) => $.controls.overridesOnly)}
                    isOn={hasOverridesOnly()}
                    onToggle={setHasOverridesOnly}
                />
            </div>
            <For each={missing()}>
                {(missing) => (
                    <div class="flex flex-wrap items-center justify-between gap-2 text-[13px] text-bad">
                        <span>
                            {missing.label} · {accessDecisionLabel(missing.access, decisionWords)}
                        </span>
                        <Show when={props.onChange}>
                            <Button
                                isDisabled={props.isDisabled ?? false}
                                onClick={() => {
                                    if (missing.owner)
                                        props.onChange?.(
                                            setOwnerChoice(props.policy, missing.owner, "inherit"),
                                        );
                                    else if (missing.routeId !== undefined)
                                        props.onChange?.(
                                            setRouteChoice(
                                                props.policy,
                                                missing.routeId,
                                                "inherit",
                                            ),
                                        );
                                }}
                            >
                                {translation()(($) => $.controls.removeOverride)}
                            </Button>
                        </Show>
                    </div>
                )}
            </For>
            <Show when={missing().length > 0 && props.onChange}>
                <p
                    role="alert"
                    class="text-[13px] text-bad"
                >
                    {translation()(
                        ($) => $.controls.removeDeletedReferencesBeforeSavingOrRefreshTo,
                    )}
                </p>
            </Show>
            <For each={owners()}>
                {(owner) => (
                    <Show when={isOwnerVisible(owner.identity, owner.name)}>
                        <div class="border-t border-rule pt-4">
                            <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                                <div class="min-w-0">
                                    <h4 class="break-words text-[13px] font-semibold text-ink">
                                        <bdi dir="auto">{owner.name}</bdi> ·{" "}
                                        {owner.identity.type === "provider"
                                            ? translation()(($) => $.helpers.apiKeyAccess.provider)
                                            : translation()(($) => $.helpers.apiKeyAccess.pool)}
                                    </h4>
                                    <p class="mt-1 text-[13px] text-ink-faint">
                                        {accessLabel(
                                            resolveAccess(props.policy, null, owner.identity),
                                        )}
                                    </p>
                                    <Show when={routesFor(owner.identity).length === 0}>
                                        <p class="mt-1 text-[13px] text-ink-faint">
                                            {translation()(
                                                ($) => $.controls.thisDefaultAppliesToFutureEntries,
                                            )}
                                        </p>
                                    </Show>
                                </div>
                                <Show when={props.onChange}>
                                    <SelectField
                                        label={ownerDefaultMessage(owner.name, owner.identity.type)}
                                        value={ownerChoice(props.policy, owner.identity)}
                                        options={choices}
                                        onChange={(choice) =>
                                            props.onChange?.(
                                                setOwnerChoice(
                                                    props.policy,
                                                    owner.identity,
                                                    choice,
                                                ),
                                            )
                                        }
                                        isDisabled={props.isDisabled ?? false}
                                    />
                                </Show>
                            </div>
                            <ul class="mt-3 divide-y divide-rule">
                                <For each={visibleRoutes(owner.identity, owner.name)}>
                                    {(route) => (
                                        <li class="flex flex-col gap-3 py-3 sm:flex-row sm:items-start sm:justify-between">
                                            <div class="min-w-0">
                                                <p class="break-all font-mono text-[13px] text-ink">
                                                    <bdi dir="ltr">{route.public_id}</bdi>
                                                </p>
                                                <p class="mt-1 text-[12.5px] text-ink-faint">
                                                    {route.target.type === "direct"
                                                        ? directAccessTargetMessage(
                                                              route.target.model_id,
                                                          )
                                                        : poolAccessTargetMessage(
                                                              route.target.upstream_id,
                                                          )}{" "}
                                                    ·{" "}
                                                    {route.is_automatic
                                                        ? translation()(
                                                              ($) =>
                                                                  $.helpers.apiKeyAccess.automatic,
                                                          )
                                                        : translation()(
                                                              ($) => $.helpers.apiKeyAccess.alias,
                                                          )}
                                                </p>
                                                <p class="mt-1 text-[13px] text-ink">
                                                    {accessLabel(
                                                        resolveAccess(
                                                            props.policy,
                                                            route.id,
                                                            owner.identity,
                                                        ),
                                                    )}
                                                </p>
                                                <p class="mt-1 text-[12.5px] text-ink-faint">
                                                    {structuralStatus(route, props.inventory)}
                                                </p>
                                            </div>
                                            <Show when={props.onChange}>
                                                <SelectField
                                                    label={modelAccessMessage(route.public_id)}
                                                    value={routeChoice(props.policy, route.id)}
                                                    options={choices}
                                                    onChange={(choice) =>
                                                        props.onChange?.(
                                                            setRouteChoice(
                                                                props.policy,
                                                                route.id,
                                                                choice,
                                                            ),
                                                        )
                                                    }
                                                    isDisabled={props.isDisabled ?? false}
                                                />
                                            </Show>
                                        </li>
                                    )}
                                </For>
                            </ul>
                        </div>
                    </Show>
                )}
            </For>
            <Show when={owners().every((owner) => !isOwnerVisible(owner.identity, owner.name))}>
                <p class="text-[13px] text-ink-faint">
                    {translation()(($) => $.controls.noEntriesMatchThisViewHiddenOverridesAre)}
                </p>
            </Show>
        </section>
    );
}
