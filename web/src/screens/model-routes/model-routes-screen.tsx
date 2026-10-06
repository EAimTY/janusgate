import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
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
    const targetLabel = (route: ModelRouteResponse): JSX.Element => {
        const target = route.target;
        if (target.type === "direct") {
            const model = models().find((row) => row.id === target.model_id);
            return model === undefined
                ? missingDirectTargetMessage(target.model_id)
                : directTargetMessage(model.provider, model.upstream_id);
        }
        return poolTargetMessage(
            pools().find((pool) => pool.id === target.pool_id)?.name ?? `#${target.pool_id}`,
            target.upstream_id,
        );
    };
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
                            {(route) => {
                                const automatic = route.is_automatic
                                    ? translation()(
                                          ($) => $.screens.routingListsScreen.automaticPrefix,
                                      )
                                    : "";
                                return (
                                    <li>
                                        <RowLink
                                            href={
                                                route.is_automatic
                                                    ? route.target.type === "direct"
                                                        ? hrefs.modelDetail(route.target.model_id)
                                                        : hrefs.providerPoolEdit(
                                                              route.target.pool_id,
                                                          )
                                                    : hrefs.modelRouteEdit(route.id)
                                            }
                                            primary={route.public_id}
                                            secondary={`${automatic}${targetLabel(route)}`}
                                            badges={
                                                <Badge
                                                    tone={route.is_enabled ? "ok" : "muted"}
                                                    label={
                                                        route.is_enabled
                                                            ? translation()(
                                                                  ($) =>
                                                                      $.screens.routingListsScreen
                                                                          .on,
                                                              )
                                                            : translation()(
                                                                  ($) =>
                                                                      $.screens.routingListsScreen
                                                                          .off,
                                                              )
                                                    }
                                                />
                                            }
                                        />
                                    </li>
                                );
                            }}
                        </For>
                    </ul>
                </Show>
            </Show>
        </div>
    );
}
