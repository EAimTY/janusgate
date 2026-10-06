import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { readProviderPools } from "/src/features/inventory/read";
import { translation } from "/src/i18n";
import { poolMembersMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type { ProviderPoolResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { createScreenRead, readOne } from "/src/screen-state/read";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { ReadNotice } from "/src/ui/read-notice";
import { RowLink } from "/src/ui/row-link";
import { SkeletonRows } from "/src/ui/skeleton-rows";

/** Pools are authored account groups; their list never polls upstream quota. */
export function ProviderPoolsScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
    const read = createScreenRead({
        load: async (signal) => {
            const result = await readOne(signal, readProviderPools(props.api));

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
                                                      ($) =>
                                                          $.screens.routingListsScreen
                                                              .noModelPrefix,
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
