import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { pluralNoun } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { hrefs, navigate } from "/src/router";
import { indexRowsById } from "/src/row-index";
import { createScreenRead, readOne, readProviderCatalog } from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
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
import { ModelList } from "/src/ui/models-table";
import { SkeletonRows } from "./skeleton-rows";

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
    const [search, setSearch] = createSignal("");

    const read = createScreenRead({
        load: async (signal) => {
            const catalog = await readOne(
                signal,
                readProviderCatalog(props.api, props.providerFilter),
            );

            setProviders(catalog.providers);
            setModels(catalog.models);
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
        entitySelectOptions(providers(), (row) => row.name, "All providers"),
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
                model.public_id.toLowerCase().includes(needle) ||
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
        const totalNoun = pluralNoun(total, "model");

        if (listingNeedle(search()).length === 0) {
            return `${total} ${totalNoun}`;
        }

        return `${visibleModels().length} of ${total} ${totalNoun}`;
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
        // value, not the case-folded `listingNeedle` the emptiness test uses.
        const query = search().trim();

        if (listingNeedle(search()).length !== 0) {
            return props.providerFilter === null
                ? `No models match "${query}".`
                : `No models of this provider match "${query}".`;
        }

        if (props.providerFilter === null) {
            return "No models on any provider yet — discover or add one.";
        }

        return isFilteredProviderListed()
            ? "This provider holds no models."
            : "This provider is no longer listed — pick another in the Provider filter.";
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Models"
                description="Every model, everywhere. Open one for its details, settings, and usage."
            >
                <ButtonLink
                    href={hrefs.modelNew(props.providerFilter)}
                    tone="primary"
                    glyph="plus"
                >
                    Create model
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
                            title="No models yet"
                            description="Add a provider first, then discover its models — or add one by hand."
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                Create provider
                            </ButtonLink>
                            <ButtonLink href={hrefs.modelNew(props.providerFilter)}>
                                Create model
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <div class="flex flex-wrap items-end gap-3">
                        <SearchInput
                            label="Search models"
                            placeholder="Search by public or upstream ID, or provider name"
                            value={search()}
                            onInput={setSearch}
                            class="min-w-0 flex-1 basis-56 sm:max-w-xs"
                        />
                        <SelectField
                            label="Provider"
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
                            providersById={providersById()}
                            showProvider={props.providerFilter === null}
                        />
                    </Show>
                </Show>
            </Show>
        </div>
    );
}
