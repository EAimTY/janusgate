import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteTarget,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { routingTokenError } from "/src/management-api/identifiers";
import { hrefs, navigate } from "/src/router";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Button, ButtonLink, SelectField, Switch, TextField } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { createDeleteAction, createEditForm } from "./edit-form";
import { PoolAvailabilityCard } from "./pool-availability-card";
import { createEditRow, createScreenRead, readAll, type ScreenRead } from "./read";
import { RowNotFound } from "./row-not-found";
import { ScreenFrame } from "./screen-frame";

type RouteSeed = {
    readonly id: number | null;
    readonly is_automatic: boolean;
    readonly public_id: string;
    readonly is_enabled: boolean;
    readonly target: ModelRouteTarget | null;
};

/** Public identity and target configuration are independent authored fields. */
function isSameRoute(left: RouteSeed, right: RouteSeed): boolean {
    const a = left.target;
    const b = right.target;
    const isTargetSame =
        a === null || b === null
            ? a === b
            : a.type === "direct" && b.type === "direct"
              ? a.model_id === b.model_id
              : a.type === "pool" &&
                b.type === "pool" &&
                a.pool_id === b.pool_id &&
                a.upstream_id === b.upstream_id;
    return (
        left.id === right.id &&
        left.public_id === right.public_id &&
        left.is_enabled === right.is_enabled &&
        isTargetSame
    );
}

/** Explicit public route creation and editing; inventory reads never publish implicitly. */
export function ModelRouteEditorScreen(props: {
    readonly api: ManagementApi;
    readonly routeId: number | null;
    readonly initialModelId: number | null;
}): JSX.Element {
    const [seed, setSeed] = createSignal<RouteSeed | null>(null);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const edit = createEditRow(seed, isSameRoute);
    const read = createScreenRead({
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const [routes, inventory, poolRows, accounts] = await readAll(
                signal,
                {
                    run: (signal) => props.api.listModelRoutes(signal),
                },
                {
                    run: (signal) => props.api.listModels({}, signal),
                },
                {
                    run: (signal) => props.api.listProviderPools(signal),
                },
                {
                    run: (signal) => props.api.listProviders(signal),
                },
            );
            setModels(inventory.models);
            setPools(poolRows.provider_pools);
            setProviders(accounts.providers);
            const initial = inventory.models.find((model) => model.id === props.initialModelId);
            setSeed(
                props.routeId === null
                    ? {
                          id: null,
                          is_automatic: false,
                          public_id: initial?.upstream_id ?? "",
                          is_enabled: true,
                          target:
                              initial === undefined
                                  ? null
                                  : {
                                        type: "direct",
                                        model_id: initial.id,
                                    },
                      }
                    : (routes.model_routes.find((route) => route.id === props.routeId) ?? null),
            );
            edit.reportReloaded();
        },
    });
    return (
        <ScreenFrame
            read={read}
            row={edit.row}
            firstReadHeader={
                <PageHeader
                    title={props.routeId === null ? "Create public route" : "Edit public route"}
                    backHref={hrefs.modelRoutes()}
                    backLabel="Public routes"
                />
            }
            notFound={
                <RowNotFound
                    kind="model_route"
                    headerTitle="Edit public route"
                />
            }
        >
            {(route) => (
                <>
                    <PageHeader
                        title={
                            route.is_automatic
                                ? route.public_id
                                : route.id === null
                                  ? "Create public route"
                                  : `Edit ${route.public_id}`
                        }
                        description={
                            route.is_automatic
                                ? "A model name maintained automatically by its provider or pool."
                                : "Clients request this public ID. Changing its target leaves that name intact."
                        }
                        backHref={hrefs.modelRoutes()}
                        backLabel="Public routes"
                    />
                    <HoldNotice
                        isHeld={edit.isRowHeld()}
                        subject="public route"
                    />
                    <Show
                        when={!route.is_automatic}
                        fallback={
                            <Card>
                                <p>
                                    This model name is managed automatically. Edit the owning
                                    provider or pool prefix to change it.
                                </p>
                                <Show when={route.target !== null}>
                                    <ButtonLink
                                        href={
                                            route.target?.type === "direct"
                                                ? hrefs.modelDetail(route.target.model_id)
                                                : route.target?.type === "pool"
                                                  ? hrefs.providerPoolEdit(route.target.pool_id)
                                                  : hrefs.models(null)
                                        }
                                    >
                                        {route.target?.type === "pool"
                                            ? "Open pool"
                                            : "Open physical model"}
                                    </ButtonLink>
                                </Show>
                            </Card>
                        }
                    >
                        <RouteForm
                            api={props.api}
                            seed={route}
                            models={models()}
                            pools={pools()}
                            providers={providers()}
                            read={read}
                            onDirtyChange={edit.reportFormDirty}
                            onSaved={edit.reportSaved}
                        />
                    </Show>
                    {route.id !== null && !route.is_automatic && (
                        <RouteDelete
                            api={props.api}
                            routeId={route.id}
                            read={read}
                        />
                    )}
                </>
            )}
        </ScreenFrame>
    );
}

function RouteDelete(props: {
    readonly api: ManagementApi;
    readonly routeId: number;
    readonly read: ScreenRead;
}): JSX.Element {
    const remove = createDeleteAction({
        read: props.read,
        action: "delete-route",
        perform: () =>
            props.api.deleteModelRoute({
                model_route_id: props.routeId,
            }),
        onDeleted: () => navigate(hrefs.modelRoutes()),
    });
    return (
        <ConfirmDeleteCard
            title="Delete this public route"
            description="Clients requesting its public ID will receive model-not-found. Physical inventory and pools remain stored."
            question="Delete this public route?"
            onConfirm={remove}
        />
    );
}

function RouteForm(props: {
    readonly api: ManagementApi;
    readonly seed: RouteSeed;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly pools: ReadonlyArray<ProviderPoolResponse>;
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly read: ScreenRead;
    readonly onDirtyChange: (isDirty: boolean) => void;
    readonly onSaved: () => void;
}): JSX.Element {
    const initial = props.seed.target;
    const [baseline, setBaseline] = createSignal(props.seed);
    const [publicId, setPublicId] = createSignal(props.seed.public_id);
    const [isEnabled, setIsEnabled] = createSignal(props.seed.is_enabled);
    const [targetType, setTargetType] = createSignal<"direct" | "pool">(initial?.type ?? "direct");
    const [modelId, setModelId] = createSignal(
        initial?.type === "direct" ? String(initial.model_id) : "",
    );
    const [poolId, setPoolId] = createSignal(
        initial?.type === "pool" ? String(initial.pool_id) : "",
    );
    const [upstreamId, setUpstreamId] = createSignal(
        initial?.type === "pool" ? initial.upstream_id : "",
    );
    const target = (): ModelRouteTarget | null =>
        targetType() === "direct"
            ? modelId() === ""
                ? null
                : {
                      type: "direct",
                      model_id: Number(modelId()),
                  }
            : poolId() === "" || upstreamId() === ""
              ? null
              : {
                    type: "pool",
                    pool_id: Number(poolId()),
                    upstream_id: upstreamId(),
                };
    const desired = (): RouteSeed => ({
        id: baseline().id,
        is_automatic: baseline().is_automatic,
        public_id: publicId(),
        is_enabled: isEnabled(),
        target: target(),
    });
    // An incomplete new target is still an edit, even before it can be serialized.
    const isDirty = (): boolean =>
        !isSameRoute(baseline(), desired()) ||
        (baseline().target === null &&
            (targetType() !== "direct" ||
                modelId() !== "" ||
                poolId() !== "" ||
                upstreamId() !== ""));
    const canSave = (): boolean =>
        publicId() !== "" &&
        routingTokenError(publicId(), "Public ID") === null &&
        target() !== null &&
        (targetType() !== "pool" || routingTokenError(upstreamId(), "Upstream ID") === null) &&
        (baseline().id === null || isDirty());
    const form = createEditForm({
        read: props.read,
        action: props.seed.id === null ? "create-route" : "update-route",
        isDirty,
        canSave,
        perform: async () => {
            const state = desired();
            const selected = state.target;
            if (selected === null) throw new Error("Route save passed without a target.");
            if (state.id === null) {
                const result = await props.api.createModelRoute({
                    public_id: state.public_id,
                    is_enabled: state.is_enabled,
                    target: selected,
                });
                return result.model_route_id;
            }
            await props.api.updateModelRoute({
                model_route_id: state.id,
                public_id: state.public_id,
                is_enabled: state.is_enabled,
                target: selected,
            });
            return state.id;
        },
        onSucceeded: (id) => {
            setBaseline({
                ...desired(),
                id,
            });
            if (props.seed.id === null) navigate(hrefs.modelRouteEdit(id));
            return {
                tone: "ok",
                title: "Public route saved.",
                detail: null,
            };
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });
    const selectedPool = (): ProviderPoolResponse | undefined =>
        props.pools.find((pool) => pool.id === Number(poolId()));
    const revert = (): void => {
        const state = baseline();
        setPublicId(state.public_id);
        setIsEnabled(state.is_enabled);
        setTargetType(state.target?.type ?? "direct");
        setModelId(state.target?.type === "direct" ? String(state.target.model_id) : "");
        setPoolId(state.target?.type === "pool" ? String(state.target.pool_id) : "");
        setUpstreamId(state.target?.type === "pool" ? state.target.upstream_id : "");
        props.read.reload();
    };
    return (
        <>
            <Card>
                <CardHeader title="Public route settings" />
                <form
                    novalidate
                    class="mt-4 flex max-w-2xl flex-col gap-5"
                    onSubmit={form.submit}
                >
                    <Switch
                        label="Enabled"
                        isOn={isEnabled()}
                        onToggle={setIsEnabled}
                        isDisabled={form.isSaving()}
                    />
                    <TextField
                        label="Public ID"
                        value={publicId()}
                        onInput={setPublicId}
                        error={
                            publicId() === ""
                                ? "Public ID is required."
                                : routingTokenError(publicId(), "Public ID")
                        }
                        hint="The exact model name clients request. Renaming requires clients to update."
                        isDisabled={form.isSaving()}
                    />
                    <SelectField
                        label="Target type"
                        value={targetType()}
                        options={[
                            {
                                value: "direct",
                                label: "Direct physical model",
                            },
                            {
                                value: "pool",
                                label: "Pool",
                            },
                        ]}
                        onChange={setTargetType}
                        isDisabled={form.isSaving()}
                    />
                    <Show
                        when={targetType() === "direct"}
                        fallback={
                            <>
                                <SelectField
                                    label="Pool"
                                    value={poolId()}
                                    options={[
                                        {
                                            value: "",
                                            label: "Choose a pool",
                                        },
                                        ...props.pools.map((pool) => ({
                                            value: String(pool.id),
                                            label: pool.name,
                                        })),
                                    ]}
                                    onChange={setPoolId}
                                    isDisabled={form.isSaving()}
                                />
                                <TextField
                                    label="Upstream ID"
                                    value={upstreamId()}
                                    onInput={setUpstreamId}
                                    error={
                                        upstreamId() === ""
                                            ? "Upstream ID is required."
                                            : routingTokenError(upstreamId(), "Upstream ID")
                                    }
                                    hint="Exact identifier requested on every member. Missing models do not prevent publication."
                                    isDisabled={form.isSaving()}
                                />
                            </>
                        }
                    >
                        <SelectField
                            label="Physical model"
                            value={modelId()}
                            options={[
                                {
                                    value: "",
                                    label: "Choose a physical model",
                                },
                                ...props.models.map((model) => ({
                                    value: String(model.id),
                                    label: `${model.provider} · ${model.upstream_id}`,
                                })),
                            ]}
                            onChange={setModelId}
                            isDisabled={form.isSaving()}
                        />
                    </Show>
                    {props.seed.id === null ? (
                        <div>
                            <Button
                                type="submit"
                                tone="primary"
                                isDisabled={!canSave()}
                                isBusy={form.isSaving()}
                            >
                                Create public route
                            </Button>
                        </div>
                    ) : (
                        <DirtyFormFooter
                            isDirty={isDirty()}
                            isSaving={form.isSaving()}
                            canSave={canSave()}
                            onRevert={revert}
                        />
                    )}
                </form>
            </Card>
            <Show
                when={targetType() === "pool" ? selectedPool() : undefined}
                keyed
            >
                {(pool) => (
                    <PoolAvailabilityCard
                        api={props.api}
                        pool={pool}
                        providers={props.providers}
                        models={props.models}
                        upstreamId={upstreamId() === "" ? null : upstreamId()}
                    />
                )}
            </Show>
        </>
    );
}
