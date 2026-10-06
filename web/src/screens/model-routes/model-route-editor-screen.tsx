import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import {
    readModelRoutes,
    readModels,
    readProviderPools,
    readProviders,
} from "/src/features/inventory/read";
import { PoolAvailabilityCard } from "/src/features/provider-pools/pool-availability-card";
import { translation } from "/src/i18n";
import { editIdentifierMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteTarget,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { routingTokenError } from "/src/management-api/identifiers";
import { hrefs, navigate } from "/src/router";
import { createDeleteAction, createEditForm } from "/src/screen-state/edit-form";
import { createScreenRead, readAll, type ScreenRead } from "/src/screen-state/read";
import { createEditRow } from "/src/screen-state/row-state";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import {
    Button,
    ButtonLink,
    entitySelectOptions,
    SelectField,
    Switch,
    TextField,
} from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "/src/ui/row-not-found";
import { ScreenFrame } from "/src/ui/screen-frame";

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
                readModelRoutes(props.api),
                readModels(props.api, {}),
                readProviderPools(props.api),
                readProviders(props.api),
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
                    title={
                        props.routeId === null
                            ? translation()(
                                  ($) => $.screens.modelRouteEditorScreen.createPublicRoute,
                              )
                            : translation()(($) => $.screens.modelRouteEditorScreen.editPublicRoute)
                    }
                    backHref={hrefs.modelRoutes()}
                    backLabel={translation()(($) => $.screens.modelRouteEditorScreen.publicRoutes)}
                />
            }
            notFound={
                <RowNotFound
                    kind="model_route"
                    headerTitle={translation()(
                        ($) => $.screens.modelRouteEditorScreen.editPublicRoute,
                    )}
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
                                  ? translation()(
                                        ($) => $.screens.modelRouteEditorScreen.createPublicRoute,
                                    )
                                  : editIdentifierMessage(route.public_id)
                        }
                        description={
                            route.is_automatic
                                ? translation()(
                                      ($) =>
                                          $.screens.modelRouteEditorScreen
                                              .aModelNameMaintainedAutomaticallyByItsProviderMessage,
                                  )
                                : translation()(
                                      ($) =>
                                          $.screens.modelRouteEditorScreen
                                              .clientsRequestThisPublicIDChangingItsTargetMessage,
                                  )
                        }
                        backHref={hrefs.modelRoutes()}
                        backLabel={translation()(
                            ($) => $.screens.modelRouteEditorScreen.publicRoutes,
                        )}
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
                                    {translation()(
                                        ($) =>
                                            $.screens.modelRouteEditorScreen
                                                .thisModelNameIsManagedAutomaticallyEditTheMessage,
                                    )}
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
                                            ? translation()(
                                                  ($) => $.screens.modelRouteEditorScreen.openPool,
                                              )
                                            : translation()(
                                                  ($) =>
                                                      $.screens.modelRouteEditorScreen
                                                          .openPhysicalModel,
                                              )}
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
            title={translation()(($) => $.screens.modelRouteEditorScreen.deleteThisPublicRoute)}
            description={translation()(
                ($) =>
                    $.screens.modelRouteEditorScreen
                        .clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage,
            )}
            question={translation()(
                ($) => $.screens.modelRouteEditorScreen.deleteThisPublicRouteQuestion,
            )}
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
    // The identity fields' emptiness errors open only once the field has
    // received input — a pristine form is not an error (the app's
    // lazy-error convention), and switching the target type to pool must not
    // convict the untouched Upstream ID from its first paint. A typed-invalid
    // value names itself wherever it was typed; the save gate holds the
    // action until the identity stands.
    const [hasPublicIdInput, setHasPublicIdInput] = createSignal(false);
    const [hasUpstreamInput, setHasUpstreamInput] = createSignal(false);
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
        routingTokenError(publicId(), "publicId") === null &&
        target() !== null &&
        (targetType() !== "pool" || routingTokenError(upstreamId(), "upstreamId") === null) &&
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
                get title() {
                    return translation()(($) => $.notices.routeSaved);
                },
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
                <CardHeader
                    title={translation()(
                        ($) => $.screens.modelRouteEditorScreen.publicRouteSettings,
                    )}
                />
                <form
                    novalidate
                    class="mt-4 flex max-w-2xl flex-col gap-5"
                    onSubmit={form.submit}
                >
                    <Switch
                        label={translation()(($) => $.screens.modelRouteEditorScreen.enabled)}
                        isOn={isEnabled()}
                        onToggle={setIsEnabled}
                        isDisabled={form.isSaving()}
                    />
                    <TextField
                        label={translation()(($) => $.screens.modelRouteEditorScreen.publicID)}
                        value={publicId()}
                        onInput={(value) => {
                            setHasPublicIdInput(true);
                            setPublicId(value);
                        }}
                        error={
                            hasPublicIdInput()
                                ? publicId() === ""
                                    ? translation()(
                                          ($) =>
                                              $.screens.modelRouteEditorScreen
                                                  .publicIDIsRequiredMessage,
                                      )
                                    : routingTokenError(publicId(), "publicId")
                                : null
                        }
                        hint={translation()(
                            ($) =>
                                $.screens.modelRouteEditorScreen
                                    .theExactModelNameClientsRequestRenamingRequiresMessage,
                        )}
                        isDisabled={form.isSaving()}
                    />
                    <SelectField
                        label={translation()(($) => $.screens.modelRouteEditorScreen.targetType)}
                        value={targetType()}
                        options={[
                            {
                                value: "direct",
                                label: translation()(
                                    ($) => $.screens.modelRouteEditorScreen.directPhysicalModel,
                                ),
                            },
                            {
                                value: "pool",
                                label: translation()(($) => $.screens.modelRouteEditorScreen.pool),
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
                                    label={translation()(
                                        ($) => $.screens.modelRouteEditorScreen.pool,
                                    )}
                                    value={poolId()}
                                    options={entitySelectOptions(
                                        props.pools,
                                        (pool) => pool.name,
                                        translation()(
                                            ($) => $.screens.modelRouteEditorScreen.chooseAPool,
                                        ),
                                    )}
                                    onChange={setPoolId}
                                    isDisabled={form.isSaving()}
                                />
                                <TextField
                                    label={translation()(
                                        ($) => $.screens.modelRouteEditorScreen.upstreamID,
                                    )}
                                    value={upstreamId()}
                                    onInput={(value) => {
                                        setHasUpstreamInput(true);
                                        setUpstreamId(value);
                                    }}
                                    error={
                                        hasUpstreamInput()
                                            ? upstreamId() === ""
                                                ? translation()(
                                                      ($) =>
                                                          $.screens.modelRouteEditorScreen
                                                              .upstreamIDIsRequiredMessage,
                                                  )
                                                : routingTokenError(upstreamId(), "upstreamId")
                                            : null
                                    }
                                    hint={translation()(
                                        ($) =>
                                            $.screens.modelRouteEditorScreen
                                                .exactIdentifierRequestedOnEveryMemberMissingModelsMessage,
                                    )}
                                    isDisabled={form.isSaving()}
                                />
                            </>
                        }
                    >
                        <SelectField
                            label={translation()(
                                ($) => $.screens.modelRouteEditorScreen.physicalModel,
                            )}
                            value={modelId()}
                            options={entitySelectOptions(
                                props.models,
                                (model) => `${model.provider} · ${model.upstream_id}`,
                                translation()(
                                    ($) => $.screens.modelRouteEditorScreen.chooseAPhysicalModel,
                                ),
                            )}
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
                                {translation()(
                                    ($) => $.screens.modelRouteEditorScreen.createPublicRoute,
                                )}
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
