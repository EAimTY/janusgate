import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { discoveryStateLabel } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { AppLink } from "/src/ui/app-link";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
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
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [failure, setFailure] = createSignal<{
        readonly title: string;
        readonly detail: string;
        readonly fix: string | null;
    } | null>(null);

    let controller: AbortController | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const [providersOutcome, modelsOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() => props.api.listModels({}, call.signal)),
        ]);

        if (providersOutcome.status === "aborted" || modelsOutcome.status === "aborted") {
            return;
        }

        setIsReading(false);

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setFailure(null);
        setProviders(providersOutcome.status === "ok" ? providersOutcome.result.providers : []);
        setModels(modelsOutcome.status === "ok" ? modelsOutcome.result.models : []);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        props.modelId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const model = createMemo(() => models().find((row) => row.id === props.modelId) ?? null);

    const provider = createMemo(() => {
        const current = model();
        return current === null
            ? null
            : (providers().find((row) => row.id === current.provider_id) ?? null);
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={isReading() && !hasRead() && failure() === null}>
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && model() === null}>
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

/**
 * The vanished row's recovery paths, each a real link: discovery runs on the
 * owning provider's page, and claiming back is the Create model page opened
 * with this model's provider fixed. The same warn notice `Notice` renders,
 * with the two recovery actions linked where they are named.
 */
function VanishedModelNotice(props: { readonly providerId: number }): JSX.Element {
    return (
        <div
            class="flex items-start gap-2.5 rounded-xl border border-warn/30 bg-warn-soft px-3.5 py-3 text-ink"
            role="status"
        >
            <Glyph
                name="alert"
                class="mt-0.5 shrink-0"
            />
            <div class="min-w-0 flex-1">
                <p class="text-[13.5px] font-medium">The upstream no longer reports this model.</p>
                <p class="mt-1 break-words text-ink-dim">
                    It serves nothing meanwhile.{" "}
                    <AppLink
                        href={hrefs.providerDetail(props.providerId)}
                        class="font-medium text-accent hover:text-accent-strong"
                    >
                        Discover again
                    </AppLink>
                    {" when the upstream lists it, or "}
                    <AppLink
                        href={hrefs.modelNew(props.providerId)}
                        class="font-medium text-accent hover:text-accent-strong"
                    >
                        claim it back from the Create model page
                    </AppLink>
                    {"."}
                </p>
            </div>
        </div>
    );
}

function ModelFacts(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly model: ModelResponse;
}): JSX.Element {
    const serving = (): {
        readonly tone: "ok" | "warn" | "bad" | "muted" | "info";
        readonly label: string;
    } => modelServingBadge(props.model, props.provider ?? undefined);

    const discovery = (): {
        readonly tone: "ok" | "warn" | "bad" | "muted" | "info";
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
                <VanishedModelNotice providerId={props.model.provider_id} />
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
