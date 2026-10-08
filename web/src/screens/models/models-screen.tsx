import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { readPublishedInventory } from "/src/features/inventory/read";
import { ModelList } from "/src/features/models/model-list";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import { noModelsMatchMessage, noProviderModelsMatchMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { hrefs, navigate } from "/src/router";
import { indexRowsById } from "/src/row-index";
import { createScreenRead, readOne } from "/src/screen-state/read";
import {
    ButtonLink,
    entitySelectOptions,
    listingNeedle,
    noLongerListedLabel,
    SearchInput,
    SelectField,
} from "/src/ui/controls";
import { EmptyState, Notice } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { ReadNotice } from "/src/ui/read-notice";
import { SkeletonRows } from "/src/ui/skeleton-rows";

/**
 * Every model on one page, filterable by provider and searchable by ID or
 * provider name. Rows state the two facts a model has — serving and
 * discovery state — and each row opens the model's detail page, behind which
 * its edit page owns every change.
 */
export function ModelsScreen(props: {
    readonly api: ManagementApi;
    readonly providerFilter: number | null;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [modelRoutes, setModelRoutes] = createSignal<ReadonlyArray<ModelRouteResponse>>([]);
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
    const [search, setSearch] = createSignal("");

    const read = createScreenRead({
        load: async (signal) => {
            const catalog = await readOne(
                signal,
                readPublishedInventory(props.api, props.providerFilter),
            );

            setProviders(catalog.providers);
            setModels(catalog.models);
            setModelRoutes(catalog.modelRoutes);
            setPools(catalog.pools);
        },
    });

    const providersById = createMemo(() => indexRowsById(providers()));

    // The option list tracks only the provider listing. A filter is the
    // route, so it can name a provider deleted since the link was written —
    // the listing the screen read no longer carries it — and the select
    // states that itself: its absent-value option, worded by
    // `noLongerListedLabel`, keeps the filter in force visible instead of
    // letting the select silently reset to "All providers".
    const filterOptions = createMemo(() =>
        entitySelectOptions(
            providers(),
            (row) => row.name,
            translation()(($) => $.remainingText.allProviders),
        ),
    );

    const visibleModels = createMemo(() => {
        const needle = listingNeedle(search());

        if (needle === "") {
            return models();
        }

        return models().filter((model) => {
            // The rows name their provider, so the provider's name is
            // searchable exactly like the two ids the row also displays.
            const providerName = providersById().get(model.provider_id)?.name ?? "";

            return (
                model.upstream_id.toLowerCase().includes(needle) ||
                providerName.toLowerCase().includes(needle)
            );
        });
    });

    // The count line is the plain count while the whole listing shows — an
    // "x of y" fraction over it would read as a narrowed table and send the
    // operator looking for the narrowing — and keeps the fraction only while
    // a search stands, the narrow it then reports.
    const countLine = createMemo(() => {
        const total = models().length;

        if (listingNeedle(search()).length === 0) {
            return translation()(($) => $.counts.models, {
                count: total,
                total: formatNumber(total),
            });
        }

        return translation()(($) => $.counts.filteredModels, {
            count: total,
            total: formatNumber(total),
            shown: formatNumber(visibleModels().length),
        });
    });

    // The provider listing is the authority on a filter's subject's
    // existence: a filtered model listing for an id the listing no longer
    // carries answers an empty list, so that emptiness proves nothing about
    // the provider's models.
    const isFilteredProviderListed = createMemo(
        () =>
            props.providerFilter !== null &&
            providers().some((row) => row.id === props.providerFilter),
    );

    // The empty answer names exactly the narrows in force: a search under a
    // filter names both, and a filter whose provider the listing no longer
    // carries states that listing fact — beside the select's "(no longer
    // listed)" option, which already does — instead of a models verdict the
    // empty filtered list cannot prove.
    const emptyNoticeTitle = createMemo(() => {
        // The title quotes the query as typed, so it reads the trimmed raw
        // value, not the case-folded `listingNeedle` the emptiness check uses.
        const query = search().trim();

        if (listingNeedle(search()).length !== 0) {
            return props.providerFilter === null
                ? noModelsMatchMessage(query)
                : noProviderModelsMatchMessage(query);
        }

        if (props.providerFilter === null) {
            return translation()(
                ($) => $.screens.modelsScreen.noModelsOnAnyProviderYetDiscoverOrMessage,
            );
        }

        return isFilteredProviderListed()
            ? translation()(($) => $.screens.modelsScreen.thisProviderHoldsNoModels)
            : translation()(
                  ($) =>
                      $.screens.modelsScreen
                          .thisProviderIsNoLongerListedPickAnotherInTheProviderFilter,
              );
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.modelsScreen.models)}
                description={translation()(
                    ($) =>
                        $.screens.modelsScreen
                            .modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage,
                )}
            >
                <ButtonLink href={hrefs.bulkPublication(props.providerFilter)}>
                    {translation()(($) => $.screens.modelsScreen.createAliases)}
                </ButtonLink>
                <ButtonLink
                    href={hrefs.modelNew(props.providerFilter)}
                    tone="primary"
                    glyph="plus"
                >
                    {translation()(($) => $.screens.modelsScreen.createModel)}
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
                    when={models().length > 0 || providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title={translation()(($) => $.screens.modelsScreen.noModelsYet)}
                            description={translation()(
                                ($) =>
                                    $.screens.modelsScreen
                                        .addAProviderFirstThenDiscoverItsModelsMessage,
                            )}
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                {translation()(($) => $.screens.modelsScreen.createProvider)}
                            </ButtonLink>
                            <ButtonLink href={hrefs.modelNew(props.providerFilter)}>
                                {translation()(($) => $.screens.modelsScreen.createModel)}
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <div class="flex flex-wrap items-end gap-3">
                        <SearchInput
                            label={translation()(($) => $.screens.modelsScreen.searchModels)}
                            placeholder={translation()(
                                ($) => $.screens.modelsScreen.searchByUpstreamIDOrProviderName,
                            )}
                            value={search()}
                            onInput={setSearch}
                            class="min-w-0 flex-1 basis-56 sm:max-w-xs"
                        />
                        <SelectField
                            label={translation()(($) => $.screens.modelsScreen.provider)}
                            value={
                                props.providerFilter === null ? "" : String(props.providerFilter)
                            }
                            options={filterOptions()}
                            absentLabel={noLongerListedLabel}
                            onChange={(value) =>
                                navigate(hrefs.models(value === "" ? null : Number(value)))
                            }
                            class="min-w-44"
                        />
                    </div>

                    <Show
                        when={visibleModels().length > 0}
                        fallback={
                            <Notice
                                tone="info"
                                title={emptyNoticeTitle()}
                            />
                        }
                    >
                        <p
                            class="text-[12.5px] text-ink-faint"
                            role="status"
                        >
                            {countLine()}
                        </p>
                        <ModelList
                            models={visibleModels()}
                            modelRoutes={modelRoutes()}
                            pools={pools()}
                            providersById={providersById()}
                            showProvider={props.providerFilter === null}
                        />
                    </Show>
                </Show>
            </Show>
        </div>
    );
}
