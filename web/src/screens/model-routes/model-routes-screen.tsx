import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { readModelRoutes, readModels, readProviderPools } from "/src/features/inventory/read";
import { translation } from "/src/i18n";
import { directTargetMessage, missingDirectTargetMessage, poolTargetMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
} from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { indexRowsById } from "/src/row-index";
import { createScreenRead, readAll } from "/src/screen-state/read";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { ReadNotice } from "/src/ui/read-notice";
import { RowLink } from "/src/ui/row-link";
import { SkeletonRows } from "/src/ui/skeleton-rows";

/** Explicit public routes, separately from the upstream inventory they address. */
export function ModelRoutesScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [routes, setRoutes] = createSignal<ReadonlyArray<ModelRouteResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
    const read = createScreenRead({
        load: async (signal) => {
            const [publicRoutes, inventory, poolRows] = await readAll(
                signal,
                readModelRoutes(props.api),
                readModels(props.api, {}),
                readProviderPools(props.api),
            );
            setRoutes(publicRoutes.model_routes);
            setModels(inventory.models);
            setPools(poolRows.provider_pools);
        },
    });
    // O(1) target lookups: the shared id index over each listing, rebuilt
    // once per read, instead of a linear find per rendered route row.
    const modelsById = createMemo(() => indexRowsById(models()));
    const poolsById = createMemo(() => indexRowsById(pools()));
    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.routingListsScreen.publicRoutes)}
                description={translation()(
                    ($) =>
                        $.screens.routingListsScreen
                            .modelNamesClientsRequestStandaloneProvidersAndPoolsMessage,
                )}
            >
                <ButtonLink
                    href={hrefs.modelRouteNew()}
                    tone="primary"
                    glyph="plus"
                >
                    {translation()(($) => $.screens.routingListsScreen.createPublicRoute)}
                </ButtonLink>
            </PageHeader>
            <ReadNotice
                read={read}
                retry={read.reload}
            />
            <SkeletonRows
                read={read}
                rowCount={3}
                rowHeight="h-16"
            />
            <Show when={read.hasRead()}>
                <Show
                    when={routes().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title={translation()(
                                ($) => $.screens.routingListsScreen.noPublicRoutesYet,
                            )}
                            description={translation()(
                                ($) =>
                                    $.screens.routingListsScreen
                                        .publishAPhysicalModelDirectlyOrTargetAMessage,
                            )}
                        />
                    }
                >
                    <ul class="flex flex-col gap-2">
                        <For each={routes()}>
                            {(route) => (
                                <RouteRow
                                    route={route}
                                    modelsById={modelsById()}
                                    poolsById={poolsById()}
                                />
                            )}
                        </For>
                    </ul>
                </Show>
            </Show>
        </div>
    );
}

/**
 * One route's row: where the route leads and whether it serves. A component
 * of its own so its label reads — and keeps reading — through a tracking
 * scope, the way every per-row read beside a listing must.
 */
function RouteRow(props: {
    readonly route: ModelRouteResponse;
    readonly modelsById: ReadonlyMap<number, ModelResponse>;
    readonly poolsById: ReadonlyMap<number, ProviderPoolResponse>;
}): JSX.Element {
    const route = props.route;
    const target = route.target;
    const targetLabel = (): JSX.Element => {
        if (target.type === "direct") {
            const model = props.modelsById.get(target.model_id);
            return model === undefined
                ? missingDirectTargetMessage(target.model_id)
                : directTargetMessage(model.provider, model.upstream_id);
        }
        return poolTargetMessage(
            props.poolsById.get(target.pool_id)?.name ?? `#${target.pool_id}`,
            target.upstream_id,
        );
    };
    return (
        <li>
            <RowLink
                href={
                    route.is_automatic
                        ? target.type === "direct"
                            ? hrefs.modelDetail(target.model_id)
                            : hrefs.providerPoolEdit(target.pool_id)
                        : hrefs.modelRouteEdit(route.id)
                }
                primary={route.public_id}
                secondary={
                    <>
                        {route.is_automatic
                            ? translation()(($) => $.screens.routingListsScreen.automaticPrefix)
                            : ""}
                        {targetLabel()}
                    </>
                }
                badges={
                    <Badge
                        tone={route.is_enabled ? "ok" : "muted"}
                        label={
                            route.is_enabled
                                ? translation()(($) => $.screens.routingListsScreen.on)
                                : translation()(($) => $.screens.routingListsScreen.off)
                        }
                    />
                }
            />
        </li>
    );
}
