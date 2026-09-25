import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { indexRowsById } from "/src/management-api/serving";
import { hrefs, navigate } from "/src/router";
import { createScreenRead, readAll } from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { ButtonLink, SearchInput, SelectField } from "/src/ui/controls";
import { EmptyState, Notice } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { listedOptions } from "/src/ui/listed-options";
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
            const [providersResult, modelsResult] = await readAll(
                signal,
                {
                    run: (cycle) => props.api.listProviders(cycle),
                },
                {
                    run: (cycle) =>
                        props.api.listModels(
                            props.providerFilter === null
                                ? {}
                                : {
                                      provider_id: props.providerFilter,
                                  },
                            cycle,
                        ),
                },
            );

            setProviders(providersResult.providers);
            setModels(modelsResult.models);
        },
    });

    const providersById = createMemo(() => indexRowsById(providers()));

    // A filter is the route, so it can name a provider deleted since the link
    // was written — the listing the screen read no longer carries it. The
    // fallback option states the filter in force instead of letting the
    // select silently reset to "All providers" while the route still filters.
    const filterOptions = createMemo(() =>
        listedOptions(
            [
                {
                    value: "",
                    label: "All providers",
                },
                ...providers().map((row) => ({
                    value: String(row.id),
                    label: row.name,
                })),
            ],
            props.providerFilter === null ? "" : String(props.providerFilter),
        ),
    );

    const visibleModels = createMemo(() => {
        const needle = search().trim().toLowerCase();

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

            <ReadNotice read={read} />

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
                            label="Search"
                            isLabelVisible={true}
                            id="models-search"
                            placeholder="Public or upstream ID, or provider name"
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
                                title={
                                    search().trim() !== ""
                                        ? `No models match "${search().trim()}".`
                                        : props.providerFilter === null
                                          ? "No models on any provider yet — discover or add one."
                                          : "This provider holds no models."
                                }
                            />
                        }
                    >
                        <p
                            class="text-[12.5px] text-ink-faint"
                            role="status"
                        >
                            {`${visibleModels().length} of ${models().length} ${models().length === 1 ? "model" : "models"}`}
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
