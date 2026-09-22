import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { hrefs, navigate } from "/src/router";
import { createScreenRead, readAll } from "/src/screens/read";
import { ButtonLink, SelectField } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { PageHeader } from "/src/ui/layout";
import { ModelList } from "/src/ui/models-table";

/**
 * Every model on one page, filterable by provider and searchable by ID.
 * Rows state the two facts a model has — serving and discovery state — and
 * each row opens the model's detail page, behind which its edit page owns
 * every change.
 */
export function ModelsScreen(props: {
    readonly api: ManagementApi;
    readonly providerFilter: number | null;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [search, setSearch] = createSignal("");

    const read = createScreenRead({
        reReadOn: () => props.providerFilter,
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

    const providersById = createMemo(
        () =>
            new Map<number, ProviderResponse>(
                providers().map((row) => [
                    row.id,
                    row,
                ]),
            ),
    );

    const filterOptions = createMemo(() => [
        {
            value: "",
            label: "All providers",
        },
        ...providers().map((row) => ({
            value: String(row.id),
            label: row.name,
        })),
    ]);

    const visibleModels = createMemo(() => {
        const needle = search().trim().toLowerCase();

        if (needle === "") {
            return models();
        }

        return models().filter(
            (model) =>
                model.public_id.toLowerCase().includes(needle) ||
                model.upstream_id.toLowerCase().includes(needle),
        );
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

            <Show when={read.notice()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={read.isReading() && !read.hasRead() && read.notice() === null}>
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
                    when={models().length > 0 || providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title="No models yet"
                            description="Add a provider first, then sync its models — or add one by hand."
                        >
                            <ButtonLink
                                href={hrefs.modelNew(props.providerFilter)}
                                tone="primary"
                                glyph="plus"
                            >
                                Create model
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <div class="flex flex-wrap items-end gap-3">
                        <div class="min-w-0 flex-1 basis-56 sm:max-w-xs">
                            <label
                                for="models-search"
                                class="block text-[13px] font-medium text-ink"
                            >
                                Search
                            </label>
                            <div class="relative mt-1.5">
                                <Glyph
                                    name="search"
                                    class="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-faint"
                                />
                                <input
                                    id="models-search"
                                    type="search"
                                    placeholder="Public or upstream ID"
                                    value={search()}
                                    onInput={(event) => setSearch(event.currentTarget.value)}
                                    class="w-full rounded-lg border border-rule-strong bg-surface py-2 pr-3 pl-9 text-ink placeholder:text-ink-faint transition-colors duration-150 ease-out hover:border-ink-faint focus:border-accent focus:outline-none"
                                />
                            </div>
                        </div>
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
                                    search().trim() === ""
                                        ? "This provider holds no models."
                                        : `No models match "${search().trim()}".`
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
