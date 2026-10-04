import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import {
    type AccessChoice,
    type AccessInventory,
    accessLabel,
    isSameOwner,
    missingReferences,
    type Owner,
    ownerChoice,
    resolveAccess,
    routeChoice,
    routeOwner,
    setOwnerChoice,
    setRouteChoice,
    structuralStatus,
} from "/src/access-control";
import type {
    AccessDecision,
    ApiKeyAccessControl,
    ModelRouteResponse,
} from "/src/management-api/generated/v0";
import { Button, SelectField, Switch, TextField } from "./controls";

const choices: ReadonlyArray<{
    readonly value: AccessChoice;
    readonly label: string;
}> = [
    {
        value: "inherit",
        label: "Inherit",
    },
    {
        value: "allow",
        label: "Allow",
    },
    {
        value: "deny",
        label: "Deny",
    },
];
const defaults: ReadonlyArray<{
    readonly value: AccessDecision;
    readonly label: string;
}> = [
    {
        value: "allow",
        label: "Allow",
    },
    {
        value: "deny",
        label: "Deny",
    },
];

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
    const routesFor = (owner: Owner): ReadonlyArray<ModelRouteResponse> =>
        props.inventory.modelRoutes.filter((route) => {
            const identity = routeOwner(route, props.inventory);
            return identity !== null && isSameOwner(identity, owner);
        });
    const visibleRoutes = (owner: Owner, name: string): ReadonlyArray<ModelRouteResponse> =>
        routesFor(owner).filter(
            (route) =>
                (matches(name) || matches(route.public_id)) &&
                (!hasOverridesOnly() || routeChoice(props.policy, route.id) !== "inherit"),
        );
    const isOwnerVisible = (owner: Owner, name: string): boolean =>
        visibleRoutes(owner, name).length > 0 ||
        (matches(name) && (!hasOverridesOnly() || ownerChoice(props.policy, owner) !== "inherit"));

    return (
        <section
            class="flex min-w-0 flex-col gap-4"
            aria-label="Model access"
        >
            <div>
                <h3 class="text-[15px] font-semibold text-ink">Model access</h3>
                <p class="mt-1 text-[13px] text-ink-faint">
                    Defaults affect existing inheriting entries. Pool access includes future
                    eligible members. Global availability is separate from permission.
                </p>
            </div>
            <Show
                when={props.onChange}
                fallback={
                    <p class="text-[13px] text-ink">
                        Default model access:{" "}
                        {props.policy.default_access === "allow" ? "Allow" : "Deny"}
                    </p>
                }
            >
                <SelectField
                    label="Default model access"
                    value={props.policy.default_access}
                    options={defaults}
                    onChange={(default_access) =>
                        props.onChange?.({
                            ...props.policy,
                            default_access,
                        })
                    }
                    isDisabled={props.isDisabled ?? false}
                    hint="Applies to entries without a provider, pool, or model override, including future entries."
                />
            </Show>
            <div class="flex flex-col gap-3 sm:flex-row sm:items-end">
                <TextField
                    label="Search model access"
                    value={search()}
                    onInput={setSearch}
                    placeholder="Public ID or owner name"
                    class="min-w-0 flex-1"
                />
                <Switch
                    label="Overrides only"
                    isOn={hasOverridesOnly()}
                    onToggle={setHasOverridesOnly}
                />
            </div>
            <For each={missingReferences(props.policy, props.inventory)}>
                {(missing) => (
                    <div class="flex flex-wrap items-center justify-between gap-2 text-[13px] text-bad">
                        <span>
                            {missing.label} · {missing.access === "allow" ? "Allow" : "Deny"}
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
                                Remove override
                            </Button>
                        </Show>
                    </div>
                )}
            </For>
            <Show
                when={missingReferences(props.policy, props.inventory).length > 0 && props.onChange}
            >
                <p
                    role="alert"
                    class="text-[13px] text-bad"
                >
                    Remove deleted references before saving, or refresh to resolve the inventory
                    mismatch.
                </p>
            </Show>
            <For each={owners()}>
                {(owner) => (
                    <Show when={isOwnerVisible(owner.identity, owner.name)}>
                        <div class="border-t border-rule pt-4">
                            <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                                <div class="min-w-0">
                                    <h4 class="break-words text-[13px] font-semibold text-ink">
                                        {owner.name} ·{" "}
                                        {owner.identity.type === "provider" ? "Provider" : "Pool"}
                                    </h4>
                                    <p class="mt-1 text-[13px] text-ink-faint">
                                        {accessLabel(
                                            resolveAccess(props.policy, null, owner.identity),
                                        )}
                                    </p>
                                    <Show when={routesFor(owner.identity).length === 0}>
                                        <p class="mt-1 text-[13px] text-ink-faint">
                                            This default applies to future entries.
                                        </p>
                                    </Show>
                                </div>
                                <Show when={props.onChange}>
                                    <SelectField
                                        label={`${owner.name} ${owner.identity.type} default`}
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
                                                    {route.public_id}
                                                </p>
                                                <p class="mt-1 text-[12.5px] text-ink-faint">
                                                    {route.target.type === "direct"
                                                        ? `Direct · model #${route.target.model_id}`
                                                        : `Pool · ${route.target.upstream_id}`}{" "}
                                                    · {route.is_automatic ? "Automatic" : "Alias"}
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
                                                    label={`${route.public_id} model access`}
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
                    No entries match this view. Hidden overrides are preserved.
                </p>
            </Show>
        </section>
    );
}
