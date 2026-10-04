import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { translation } from "/src/i18n";
import {
    directTargetMessage,
    missingDirectTargetMessage,
    poolMembersMessage,
    poolTargetMessage,
} from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
} from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { RowLink } from "/src/ui/row-link";
import { createScreenRead, readAll } from "./read";
import { ReadNotice } from "./read-notice";
import { SkeletonRows } from "./skeleton-rows";

/** Pools are authored account groups; their list never polls upstream quota. */
export function ProviderPoolsScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
    const read = createScreenRead({
        load: async (signal) => {
            const result = await props.api.listProviderPools(signal);
            setPools(result.provider_pools);
        },
    });
    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.routingListsScreen.pools)}
                description={translation()(
                    ($) =>
                        $.screens.routingListsScreen
                            .namedModelCatalogsBuiltFromInterchangeableAccountsForMessage,
                )}
            >
                <ButtonLink
                    href={hrefs.providerPoolNew()}
                    tone="primary"
                    glyph="plus"
                >
                    {translation()(($) => $.screens.routingListsScreen.createPool)}
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
                    when={pools().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title={translation()(($) => $.screens.routingListsScreen.noPoolsYet)}
                            description={translation()(
                                ($) =>
                                    $.screens.routingListsScreen
                                        .createANamedPoolWithAModelPrefixMessage,
                            )}
                        />
                    }
                >
                    <ul class="flex flex-col gap-2">
                        <For each={pools()}>
                            {(pool) => (
                                <li>
                                    <RowLink
                                        href={hrefs.providerPoolEdit(pool.id)}
                                        primary={pool.name}
                                        secondary={poolMembersMessage(
                                            pool.members.length,
                                            pool.model_prefix === ""
                                                ? translation()(
                                                      ($) => $.vocabulary.reasoningLevel.none,
                                                  )
                                                : pool.model_prefix,
                                        )}
                                        badges={
                                            <Badge
                                                tone={pool.is_enabled ? "ok" : "muted"}
                                                label={
                                                    pool.is_enabled
                                                        ? translation()(
                                                              ($) =>
                                                                  $.additional.routingListsScreen
                                                                      .on,
                                                          )
                                                        : translation()(
                                                              ($) =>
                                                                  $.additional.routingListsScreen
                                                                      .off,
                                                          )
                                                }
                                            />
                                        }
                                    />
                                </li>
                            )}
                        </For>
                    </ul>
                </Show>
            </Show>
        </div>
    );
}

/** Explicit public routes, separately from the upstream inventory they address. */
export function ModelRoutesScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [routes, setRoutes] = createSignal<ReadonlyArray<ModelRouteResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
    const read = createScreenRead({
        load: async (signal) => {
            const [publicRoutes, inventory, poolRows] = await readAll(
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
                            {(route) => (
                                <li>
                                    <RowLink
                                        href={
                                            route.is_automatic
                                                ? route.target.type === "direct"
                                                    ? hrefs.modelDetail(route.target.model_id)
                                                    : hrefs.providerPoolEdit(route.target.pool_id)
                                                : hrefs.modelRouteEdit(route.id)
                                        }
                                        primary={route.public_id}
                                        secondary={`${route.is_automatic ? "Automatic · " : ""}${targetLabel(route)}`}
                                        badges={
                                            <Badge
                                                tone={route.is_enabled ? "ok" : "muted"}
                                                label={
                                                    route.is_enabled
                                                        ? translation()(
                                                              ($) =>
                                                                  $.additional.routingListsScreen
                                                                      .on,
                                                          )
                                                        : translation()(
                                                              ($) =>
                                                                  $.additional.routingListsScreen
                                                                      .off,
                                                          )
                                                }
                                            />
                                        }
                                    />
                                </li>
                            )}
                        </For>
                    </ul>
                </Show>
            </Show>
        </div>
    );
}
