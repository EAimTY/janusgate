import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { providerTypeLabel } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { createScreenRead, readAll } from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { ButtonLink, SearchInput } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { ProviderList } from "/src/ui/providers-table";

/**
 * The provider list: one row per upstream account. Rows carry identity,
 * type, model count, and the two states — nothing else, and no model's
 * details. Each row opens the provider's detail page; the edit page behind
 * it owns every change.
 */
export function ProvidersScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [search, setSearch] = createSignal("");

    const read = createScreenRead({
        load: async (signal) => {
            const [providersResult, modelsResult] = await readAll(
                signal,
                {
                    run: (cycle) => props.api.listProviders(cycle),
                },
                {
                    run: (cycle) => props.api.listModels({}, cycle),
                },
            );

            setProviders(providersResult.providers);
            setModels(modelsResult.models);
        },
    });

    const modelCounts = createMemo(() => {
        const counts = new Map<number, number>();

        for (const model of models()) {
            counts.set(model.provider_id, (counts.get(model.provider_id) ?? 0) + 1);
        }

        return counts;
    });

    const visibleProviders = createMemo(() => {
        const needle = search().trim().toLowerCase();

        if (needle === "") {
            return providers();
        }

        return providers().filter(
            (provider) =>
                provider.name.toLowerCase().includes(needle) ||
                providerTypeLabel(provider.type).toLowerCase().includes(needle),
        );
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Providers"
                description="Upstream accounts. Open one for its models and usage — editing lives one step further in."
            >
                <ButtonLink
                    href={hrefs.providerNew()}
                    tone="primary"
                    glyph="plus"
                >
                    Create provider
                </ButtonLink>
            </PageHeader>

            <ReadNotice read={read} />

            <Show when={read.isFirstRead()}>
                <div class="flex flex-col gap-2">
                    <For
                        each={[
                            0,
                            1,
                            2,
                        ]}
                    >
                        {() => <Skeleton class="h-16 w-full rounded-xl" />}
                    </For>
                </div>
            </Show>

            <Show when={read.hasRead()}>
                <Show
                    when={providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="plug"
                            title="No providers yet"
                            description="A provider is one upstream account."
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                Create provider
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <SearchInput
                        label="Search providers"
                        placeholder="Search by name or type"
                        value={search()}
                        onInput={setSearch}
                        class="sm:max-w-xs"
                    />

                    <Show
                        when={visibleProviders().length > 0}
                        fallback={
                            <Notice
                                tone="info"
                                title={`No providers match "${search().trim()}".`}
                            />
                        }
                    >
                        <ProviderList
                            providers={visibleProviders()}
                            modelCounts={modelCounts()}
                        />
                    </Show>
                </Show>
            </Show>
        </div>
    );
}
