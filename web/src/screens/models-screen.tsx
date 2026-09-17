import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { ModelList } from "/src/components/model-list";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse } from "/src/management-api/generated/v0";
import { hrefs, navigate, parseRowIdentifier } from "/src/router";
import { createAsyncResource } from "/src/screens/resource";
import { Banner } from "/src/ui/banner";
import { Button } from "/src/ui/button";
import { Card } from "/src/ui/card";
import { SelectField, type SelectOption } from "/src/ui/field";
import { LinkButton } from "/src/ui/link-button";
import { PageHeader } from "/src/ui/page-header";

export type ModelsScreenProps = {
    readonly api: ManagementApi;
    readonly providerFilter: number | null;
};

/**
 * The model listing (README §4.4 `models:list`), optionally filtered to one
 * provider, with inline override editing. Model rows exist only through
 * `models:discover`, so creation happens on the owning provider's screen.
 */
export function ModelsScreen(props: ModelsScreenProps): JSX.Element {
    const [actionFailure, setActionFailure] = createSignal<string | null>(null);
    const providers = createAsyncResource((signal) => props.api.listProviders(signal));
    const models = createAsyncResource((signal) => {
        const providerFilter = props.providerFilter;
        return props.api.listModels(
            providerFilter === null
                ? {}
                : {
                      provider_id: providerFilter,
                  },
            signal,
        );
    });

    // The filter rides the URL; a change reloads the listing even when the
    // screen itself is not remounted.
    createEffect(
        () => props.providerFilter,
        () => {
            void models.reload();
        },
        {
            defer: true,
        },
    );

    const rows = (): ModelResponse[] | null => models.value()?.models ?? null;

    const filterOptions = createMemo<SelectOption[]>(() => {
        const options: SelectOption[] = [
            {
                value: "",
                label: "All providers",
            },
        ];
        for (const provider of providers.value()?.providers ?? []) {
            options.push({
                value: String(provider.id),
                label: provider.name,
            });
        }
        const providerFilter = props.providerFilter;
        if (
            providerFilter !== null &&
            !options.some((option) => option.value === String(providerFilter))
        ) {
            options.push({
                value: String(providerFilter),
                label: `Provider #${providerFilter} (not in the listing)`,
            });
        }
        return options;
    });

    const selectProvider = (value: string): void => {
        navigate(hrefs.models(parseRowIdentifier(value)));
    };

    const replaceModel = (updated: ModelResponse): void => {
        const current = models.value();
        if (current === null) {
            return;
        }
        models.setValue({
            models: current.models.map((row) =>
                row.provider_id === updated.provider_id && row.upstream_id === updated.upstream_id
                    ? updated
                    : row,
            ),
        });
    };

    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title="Models"
                description="Every routed model and its overrides. A model's public ID is the name serving clients resolve; overrides are rewritten as read-modify-write."
            />

            <Show when={actionFailure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>

            <Show when={models.failure()}>
                {(message) => (
                    <Banner variant="error">
                        <p>{message()}</p>
                        <div class="mt-2">
                            <Button
                                variant="secondary"
                                onClick={() => void models.reload()}
                            >
                                Retry
                            </Button>
                        </div>
                    </Banner>
                )}
            </Show>

            <div class="max-w-sm">
                <SelectField
                    id="models-provider-filter"
                    label="Provider filter"
                    value={props.providerFilter === null ? "" : String(props.providerFilter)}
                    options={filterOptions()}
                    onChange={selectProvider}
                    hint="A filter narrows the listing; an unknown provider yields an empty list."
                />
            </div>

            <Show when={models.isLoading()}>
                <p
                    role="status"
                    class="text-sm text-slate-500"
                >
                    Loading models…
                </p>
            </Show>

            <Show when={rows()}>
                {(modelRows) => (
                    <Show
                        when={modelRows().length > 0}
                        fallback={
                            <Card title="No models here">
                                <Show
                                    when={props.providerFilter}
                                    fallback={
                                        <p class="text-sm text-slate-600">
                                            No provider has been discovered yet. Open a provider to
                                            refresh its model rows from its upstream.
                                        </p>
                                    }
                                >
                                    {(providerId) => (
                                        <div class="flex flex-col items-start gap-3">
                                            <p class="text-sm text-slate-600">
                                                Provider #{providerId()} has no model rows yet.
                                                Discovery replaces them from that provider's
                                                upstream.
                                            </p>
                                            <LinkButton
                                                href={hrefs.providerDetail(providerId())}
                                                variant="secondary"
                                            >
                                                Open provider #{providerId()}
                                            </LinkButton>
                                        </div>
                                    )}
                                </Show>
                            </Card>
                        }
                    >
                        <ModelList
                            api={props.api}
                            models={modelRows()}
                            shouldShowProvider
                            onModelUpdated={replaceModel}
                            onModelVanished={(message) => {
                                setActionFailure(message);
                                void models.reload();
                            }}
                        />
                    </Show>
                )}
            </Show>
        </div>
    );
}
