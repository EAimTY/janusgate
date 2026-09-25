import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { credentialExpiryLine, isCredentialExpired, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { OUTWARD_SURFACES } from "/src/management-api/outward-surfaces";
import {
    indexRowsById,
    isModelEffectivelyServing,
    modelCountsByProvider,
} from "/src/management-api/serving";
import { credentialKindLabel, isOAuthProviderRow } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import {
    buildOverviewDailySeries,
    type OverviewDailySeries,
    type OverviewRecentTotals,
    overviewDailyWindow,
    overviewRecentFrom,
    overviewRecentTotals,
} from "/src/screens/overview-stats";
import { createScreenRead, readAll } from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { readUsageBuckets, readUsageEntities, type UsageEntities } from "/src/screens/usage-read";
import { AppLink } from "/src/ui/app-link";
import { BarChart } from "/src/ui/bar-chart";
import { CopyButton } from "/src/ui/clipboard";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, Skeleton, StatLink } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { EMPTY_USAGE_FILTER_DRAFT } from "/src/usage-filter";

/** What the overview's reads answered: the rolling figures plus the daily series. */
type OverviewTraffic = OverviewRecentTotals & {
    readonly series: OverviewDailySeries;
};

/** One listed attention line: its provider and the human reason it needs one. */
type ProviderAttention = {
    readonly provider: ProviderResponse;
    readonly attention: {
        readonly label: string;
        readonly detail: string;
    };
};

/**
 * The overview: the gateway's health and its recent traffic at a glance.
 * Every figure is a door to the page that explains it, the trend below the
 * figures is the last fourteen days of requests, and the endpoints card
 * states where a client points — the one thing an operator cannot learn from
 * the gateway's own rows.
 */
export function HomeScreen(props: {
    readonly api: ManagementApi;
    readonly baseUrl: string;
}): JSX.Element {
    const [entities, setEntities] = createSignal<UsageEntities>({
        providers: [],
        models: [],
        apiKeys: [],
    });
    const [traffic, setTraffic] = createSignal<OverviewTraffic>({
        requests: 0,
        errors: 0,
        series: buildOverviewDailySeries([], nowMicros()),
    });

    const read = createScreenRead({
        load: async (signal) => {
            // One moment for the whole cycle: the rolling window, the 14-day
            // axis, and the figures all describe the same instant.
            const readAt = nowMicros();
            const dailyWindow = overviewDailyWindow(readAt);

            const [entitiesResult, recentResult, dailyResult] = await readAll(
                signal,
                readUsageEntities(props.api),
                readUsageBuckets(props.api, {
                    group_by: [
                        "outcome",
                    ],
                    filter: {
                        from: overviewRecentFrom(readAt),
                    },
                }),
                readUsageBuckets(props.api, {
                    group_by: [
                        "day",
                    ],
                    filter: {
                        from: dailyWindow.from,
                        to: dailyWindow.to,
                    },
                }),
            );

            setEntities(entitiesResult);
            setTraffic({
                ...overviewRecentTotals(recentResult.buckets),
                series: buildOverviewDailySeries(dailyResult.buckets, readAt),
            });
        },
    });

    const providersById = createMemo(() => indexRowsById(entities().providers));

    const servingModels = createMemo(
        () =>
            entities().models.filter((model) =>
                isModelEffectivelyServing(model, providersById().get(model.provider_id)),
            ).length,
    );

    const attentionProviders = createMemo(() =>
        entities().providers.flatMap((provider): ProviderAttention[] => {
            const attention = credentialAttention(provider);

            return attention === null
                ? []
                : [
                      {
                          provider,
                          attention,
                      },
                  ];
        }),
    );

    const emptyProviders = createMemo(() => {
        const modelCounts = modelCountsByProvider(entities().models);

        return entities().providers.filter((provider) => (modelCounts.get(provider.id) ?? 0) === 0);
    });

    // Exactly what the list below counts: every item the section can show —
    // credential problems, providers without models, the missing first key —
    // so the all-clear notice and the list can never contradict each other.
    const attentionCount = createMemo(
        () =>
            attentionProviders().length +
            emptyProviders().length +
            (entities().apiKeys.length === 0 ? 1 : 0),
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Overview"
                description="The health of this gateway at a glance."
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
                <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
                    <For
                        each={[
                            0,
                            1,
                            2,
                            3,
                            4,
                        ]}
                    >
                        {() => <Skeleton class="h-24 w-full rounded-2xl" />}
                    </For>
                </div>
                <Skeleton class="h-56 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead()}>
                <Show
                    when={entities().providers.length > 0}
                    fallback={<GettingStarted />}
                >
                    <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
                        <StatLink
                            href={hrefs.providers()}
                            value={String(entities().providers.length)}
                            label="Providers"
                        />
                        <StatLink
                            href={hrefs.models(null)}
                            value={String(servingModels())}
                            label="Models routing"
                        />
                        <StatLink
                            href={hrefs.apiKeys()}
                            value={String(entities().apiKeys.length)}
                            label="API keys"
                        />
                        <StatLink
                            href={hrefs.usage({
                                ...EMPTY_USAGE_FILTER_DRAFT,
                                window: "24h",
                            })}
                            value={String(traffic().requests)}
                            label="Recent requests (24h)"
                        />
                        <StatLink
                            href={hrefs.usage({
                                ...EMPTY_USAGE_FILTER_DRAFT,
                                window: "24h",
                                outcome: "failures",
                            })}
                            value={String(traffic().errors)}
                            label="Recent errors (24h)"
                        />
                    </div>

                    <Card>
                        <CardHeader
                            title="Requests per day"
                            description="The last 14 days, in UTC — the gateway's own day boundary."
                        >
                            <ButtonLink
                                href={hrefs.usage()}
                                glyph="activity"
                            >
                                Open usage
                            </ButtonLink>
                        </CardHeader>

                        <div class="mt-4">
                            <BarChart
                                points={traffic().series.points.map((point) => ({
                                    label: point.day,
                                    value: point.requests,
                                }))}
                                unit="requests"
                                summary={trafficSummary(traffic().series)}
                            />
                        </div>
                    </Card>

                    <Show
                        when={attentionCount() > 0}
                        fallback={
                            <Notice
                                tone="ok"
                                title="Everything looks healthy."
                                detail={null}
                                fix={null}
                            />
                        }
                    >
                        <Card>
                            <CardHeader title="Needs attention" />
                            <ul class="mt-4 flex flex-col divide-y divide-rule">
                                <Show when={entities().apiKeys.length === 0}>
                                    <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
                                        <Glyph
                                            name="key"
                                            class="shrink-0 text-ink-faint"
                                        />
                                        <p class="min-w-0 flex-1 text-[13.5px]">
                                            <span class="font-medium">No API keys yet.</span>
                                            <span class="text-ink-dim">
                                                {
                                                    " Serving clients authenticate with one — without it, the gateway serves nothing."
                                                }
                                            </span>
                                        </p>
                                        <ButtonLink href={hrefs.apiKeyNew()}>
                                            Create API key
                                        </ButtonLink>
                                    </li>
                                </Show>
                                <For each={attentionProviders()}>
                                    {(item) => (
                                        <AttentionRow
                                            provider={item.provider}
                                            attention={item.attention}
                                        />
                                    )}
                                </For>
                                <For each={emptyProviders()}>
                                    {(provider) => (
                                        <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
                                            <Glyph
                                                name="stack"
                                                class="shrink-0 text-ink-faint"
                                            />
                                            <p class="min-w-0 flex-1 basis-48 text-[13.5px]">
                                                <span class="font-medium">{provider.name}</span>
                                                <span class="text-ink-dim">
                                                    {" has no models yet."}
                                                </span>
                                            </p>
                                            <ButtonLink href={hrefs.providerDetail(provider.id)}>
                                                Open and discover
                                            </ButtonLink>
                                        </li>
                                    )}
                                </For>
                            </ul>
                        </Card>
                    </Show>
                </Show>

                <EndpointsCard baseUrl={props.baseUrl} />
            </Show>
        </div>
    );
}

/**
 * One line stating the window's total and its busiest day, or that nothing was
 * recorded. It stays one line at the narrowest width the console ships: the
 * card's own description already names the window, so the total does not
 * repeat it, and the day it states is unbreakable.
 */
function trafficSummary(series: OverviewDailySeries): JSX.Element {
    if (series.total === 0) {
        return "No requests recorded.";
    }

    const total = `${series.total.toLocaleString()} ${series.total === 1 ? "request" : "requests"}`;

    if (series.peak === null) {
        return total;
    }

    return (
        <>
            {`${total} · peak ${series.peak.requests.toLocaleString()} on `}
            <span class="whitespace-nowrap">{series.peak.day}</span>
        </>
    );
}
/**
 * Where a client points: the instance's own address plus one path per outward
 * surface, each copyable. The gateway's own documentation stays normative —
 * this card exists so the operator does not have to leave the console to
 * learn the one fact the management API never states.
 */
function EndpointsCard(props: { readonly baseUrl: string }): JSX.Element {
    return (
        <Card>
            <CardHeader
                title="Endpoints"
                description="Every surface serves every model. Point any OpenAI-, Gemini-, or Anthropic-shaped client at this instance."
            />
            <ul class="mt-4 flex flex-col divide-y divide-rule">
                <For each={OUTWARD_SURFACES}>
                    {(surface) => {
                        const url = (): string => `${props.baseUrl}${surface.path}`;

                        return (
                            <li class="flex flex-wrap items-center gap-x-4 gap-y-2 py-3">
                                <span class="min-w-0 flex-1 basis-48">
                                    <span class="block text-[13.5px] font-medium">
                                        {surface.label}
                                    </span>
                                    <span class="block text-[12.5px] text-ink-faint">
                                        {surface.speaks}
                                    </span>
                                </span>
                                <span
                                    class="tnum min-w-0 flex-1 basis-64 truncate text-[13px]"
                                    title={url()}
                                >
                                    {url()}
                                </span>
                                <CopyButton
                                    value={url()}
                                    subject="the URL beside it"
                                    ariaLabel={`Copy the ${surface.label} endpoint URL`}
                                />
                            </li>
                        );
                    }}
                </For>
            </ul>
            <p class="mt-3 text-[12.5px] text-ink-faint">
                A serving client authenticates with a serving API key — create one under{" "}
                <AppLink
                    href={hrefs.apiKeys()}
                    class="font-medium text-accent hover:underline dark:text-accent-strong"
                >
                    API keys
                </AppLink>{" "}
                before pointing anything here.
            </p>
        </Card>
    );
}

/** Why an OAuth provider needs a human: expired, or never fresh. */
function credentialAttention(provider: ProviderResponse): {
    readonly label: string;
    readonly detail: string;
} | null {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    if (provider.expires_at === null) {
        return {
            label: "Credential not fresh",
            detail: "No expiry is stored for this provider.",
        };
    }

    if (!isCredentialExpired(provider.expires_at, nowMicros())) {
        return null;
    }

    return {
        label: "Credential expired",
        detail: credentialExpiryLine(provider.expires_at, nowMicros()),
    };
}

function AttentionRow(props: ProviderAttention): JSX.Element {
    return (
        <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
            <Badge
                tone="warn"
                label={props.attention.label}
            />
            <p class="min-w-0 flex-1 basis-48 text-[13.5px]">
                <span class="font-medium">{props.provider.name}</span>
                <span class="text-ink-dim">{` · ${credentialKindLabel(props.provider.type)} · ${props.attention.detail}`}</span>
            </p>
            {/* Reauthorization lives on the edit page's Credentials card. */}
            <ButtonLink href={hrefs.providerEdit(props.provider.id)}>Reconnect</ButtonLink>
        </li>
    );
}

/** A fresh gateway: the five steps from here to a working endpoint. */
function GettingStarted(): JSX.Element {
    return (
        <EmptyState
            glyph="plug"
            title="No providers yet"
            description="A provider is one upstream account. Its models arrive when you discover them."
        >
            <ButtonLink
                href={hrefs.providerNew()}
                tone="primary"
                glyph="plus"
            >
                Add your first provider
            </ButtonLink>
            <ol class="mt-2 flex w-full list-none flex-col gap-2 text-[13.5px]">
                <GettingStartedStep
                    index={1}
                    isDone
                    title="Connected to the gateway"
                />
                <GettingStartedStep
                    index={2}
                    title="Add a provider"
                />
                <GettingStartedStep
                    index={3}
                    title="Discover its models from upstream"
                />
                <GettingStartedStep
                    index={4}
                    title="Point a client at an endpoint below"
                />
                <GettingStartedStep
                    index={5}
                    title="Issue an API key for serving clients"
                />
            </ol>
        </EmptyState>
    );
}

function GettingStartedStep(props: {
    readonly index: number;
    readonly title: string;
    readonly isDone?: boolean | undefined;
}): JSX.Element {
    return (
        <li class="flex items-center gap-3">
            <span
                class={
                    props.isDone === true
                        ? "grid size-5 shrink-0 place-items-center rounded-full bg-ok-soft text-ok"
                        : "grid size-5 shrink-0 place-items-center rounded-full bg-muted text-[12px] font-semibold text-ink-dim"
                }
            >
                <Show
                    when={props.isDone === true}
                    fallback={props.index}
                >
                    <Glyph name="check" />
                </Show>
            </span>
            <span class={props.isDone === true ? "text-ink-dim" : ""}>{props.title}</span>
        </li>
    );
}
