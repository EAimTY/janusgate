import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { discoveryStateLabel } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { createScreenRead, isSameModelRow, isSameProviderRow, readAll } from "/src/screens/read";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { AppLink } from "/src/ui/app-link";
import { ButtonLink } from "/src/ui/controls";
import { Badge, type BadgeTone, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { modelDiscoveryBadge, modelServingBadge } from "/src/ui/models-table";

/**
 * One model's page: what it is, where it stands, and what serving clients
 * did with it. Nothing is changed here — the edit page owns every change.
 */
export function ModelDetailScreen(props: {
    readonly api: ManagementApi;
    readonly modelId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);

    const read = createScreenRead({
        reReadOn: () => props.modelId,
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

    // Each row memo keeps its previous answer when a re-read returns the same
    // content, so a refresh never flips object identity and never remounts
    // the cards (or restarts the usage read) over unchanged data.
    const model = createMemo((previous: ModelResponse | null | undefined) => {
        const fresh = models().find((row) => row.id === props.modelId) ?? null;

        if (previous === null || previous === undefined || fresh === null) {
            return fresh;
        }

        return isSameModelRow(previous, fresh) ? previous : fresh;
    });

    const provider = createMemo((previous: ProviderResponse | null | undefined) => {
        const current = model();
        const fresh =
            current === null
                ? null
                : (providers().find((row) => row.id === current.provider_id) ?? null);

        if (previous === null || previous === undefined || fresh === null) {
            return fresh;
        }

        return isSameProviderRow(previous, fresh) ? previous : fresh;
    });

    return (
        <div class="rise-in flex flex-col gap-5">
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
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead() && model() === null}>
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <EmptyState
                    title="Model not found"
                    description="The gateway holds no model here. It was deleted, or the address was typed wrong — a deleted discovered model returns as new on the next discovery."
                >
                    <ButtonLink
                        href={hrefs.models(null)}
                        tone="primary"
                    >
                        Back to models
                    </ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={(() => {
                    const current = model();
                    return current === null
                        ? null
                        : {
                              model: current,
                              provider: provider(),
                          };
                })()}
                keyed
            >
                {(both) => (
                    <ModelFacts
                        api={props.api}
                        provider={both.provider}
                        model={both.model}
                    />
                )}
            </Show>
        </div>
    );
}

function ModelFacts(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly model: ModelResponse;
}): JSX.Element {
    const serving = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelServingBadge(props.model, props.provider ?? undefined);

    const discovery = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelDiscoveryBadge(props.model);

    return (
        <>
            <PageHeader
                title={props.model.public_id}
                description={`${props.model.upstream_id} on ${props.model.provider}.`}
                backHref={hrefs.models(props.model.provider_id)}
                backLabel="Models"
            >
                <Badge
                    tone={serving().tone}
                    label={serving().label}
                />
                <Badge
                    tone={discovery().tone}
                    label={discovery().label}
                />
                <ButtonLink href={hrefs.modelEdit(props.model.id)}>Edit model</ButtonLink>
            </PageHeader>

            <Show when={props.model.discovery_state === "vanished"}>
                {/* The same warn notice every screen states, with the two
                recovery paths linked where they are named. */}
                <Notice
                    tone="warn"
                    title="The upstream no longer reports this model."
                    detail={
                        <>
                            It serves nothing meanwhile.{" "}
                            <AppLink
                                href={hrefs.providerDetail(props.model.provider_id)}
                                class="font-medium text-accent hover:text-accent-strong"
                            >
                                Discover again on the provider's page
                            </AppLink>
                            {" when the upstream lists it, or "}
                            <AppLink
                                href={hrefs.modelNew(props.model.provider_id)}
                                class="font-medium text-accent hover:text-accent-strong"
                            >
                                claim it back from the Create model page
                            </AppLink>
                            {"."}
                        </>
                    }
                />
            </Show>

            <Card>
                <CardHeader title="Details" />
                <dl class="mt-4 grid gap-x-8 gap-y-3 sm:grid-cols-2">
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Public ID</dt>
                        <dd class="tnum mt-1 break-all">{props.model.public_id}</dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Upstream ID</dt>
                        <dd class="tnum mt-1 break-all">{props.model.upstream_id}</dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Provider</dt>
                        <dd class="mt-1">
                            {props.provider === null ? (
                                <span class="text-ink-dim">{props.model.provider}</span>
                            ) : (
                                <AppLink
                                    href={hrefs.providerDetail(props.provider.id)}
                                    class="font-medium text-accent hover:text-accent-strong"
                                >
                                    {props.provider.name}
                                </AppLink>
                            )}
                        </dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Discovery</dt>
                        <dd class="mt-1 text-ink-dim">
                            {discoveryStateLabel(props.model.discovery_state)}
                        </dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Enabled</dt>
                        <dd class="mt-1">{props.model.is_enabled ? "On" : "Off"}</dd>
                    </div>
                </dl>
            </Card>

            <EntityUsageCard
                api={props.api}
                filter={{
                    model_id: props.model.id,
                }}
                subjectKind="model"
            />
        </>
    );
}
