import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import {
    credentialAttention,
    inventoryGapAttention,
    type ProviderAttention,
    type ProviderInventoryAttention,
} from "/src/features/overview/attention";
import { OUTWARD_SURFACES } from "/src/features/overview/outward-surfaces";
import {
    buildOverviewDailySeries,
    type OverviewDailySeries,
    type OverviewRecentTotals,
    overviewDailyWindow,
    overviewRecentFrom,
    overviewRecentTotals,
} from "/src/features/overview/overview-stats";
import { EMPTY_USAGE_FILTER_DRAFT } from "/src/features/usage/filter";
import {
    EMPTY_USAGE_ENTITIES,
    readUsageBuckets,
    readUsageEntities,
} from "/src/features/usage/read";
import { nowMicros } from "/src/format";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import {
    inventoryAttentionMessage,
    requestChartPointMessage,
    requestSummaryMessage,
    servingKeyMessage,
} from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import { readyInventoryCountsByProvider } from "/src/management-api/serving";
import { CREDENTIAL_KIND } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { indexRowsById } from "/src/row-index";
import { createScreenRead, readAll } from "/src/screen-state/read";
import { BarChart } from "/src/ui/bar-chart";
import { CopyButton } from "/src/ui/clipboard";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, Skeleton, StatLink } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ReadNotice } from "/src/ui/read-notice";

/** What the overview's reads answered: the rolling figures plus the daily series. */
type OverviewTraffic = OverviewRecentTotals & {
    readonly series: OverviewDailySeries;
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
    const [entities, setEntities] = createSignal(EMPTY_USAGE_ENTITIES);
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

    // One walk over the models per listings change: the per-provider ready-inventory
    // counts the attention lines read below. The "Inventory ready" figure is
    // exactly that map's sum, so it derives from the same accumulation
    // instead of filtering the models a second time.
    const readyInventoryCounts = createMemo(() =>
        readyInventoryCountsByProvider(entities().models, providersById()),
    );

    const readyInventoryModels = createMemo(() => {
        let total = 0;

        for (const count of readyInventoryCounts().values()) {
            total += count;
        }

        return total;
    });

    const enabledRoutes = createMemo(
        () => entities().modelRoutes.filter((route) => route.is_enabled).length,
    );
    const hasNoPublicRoutes = () => entities().modelRoutes.length === 0;

    // Where the no-public-routes row's action lands: the page where discovery
    // actually runs, never the models listing — that page offers only
    // creation, and the sibling no-inventory row already opens the provider's
    // page for the same reason. Exactly one enabled provider is that page;
    // otherwise the providers list is the way in.
    const discoveryTarget = createMemo(() => {
        const enabled = entities().providers.filter((provider) => provider.is_enabled);
        const [single] = enabled;

        return enabled.length === 1 && single !== undefined
            ? hrefs.providerDetail(single.id)
            : hrefs.providers();
    });

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

    // An enabled provider with no ready inventory (README §3) needs
    // a human: its rows are all vanished, disabled, or absent — the vanished
    // case is the quietest failure there is, and a raw row count would hide
    // it. A deliberately disabled provider is the operator's choice, not a
    // fault, so it stays off this list. Each line's action follows the cause
    // its rows name, because discovery — the fix the absent case needs —
    // cannot repair a row that is merely switched off.
    const providersWithoutReadyInventory = createMemo(
        (): ReadonlyArray<ProviderInventoryAttention> => {
            const readyCounts = readyInventoryCounts();

            return entities().providers.flatMap((provider): ProviderInventoryAttention[] => {
                if (!provider.is_enabled || (readyCounts.get(provider.id) ?? 0) !== 0) {
                    return [];
                }

                return [
                    {
                        provider,
                        attention: inventoryGapAttention(entities().models, provider.id),
                    },
                ];
            });
        },
    );

    // Exactly what the list below counts: every item the section can show —
    // credential problems, providers with no ready inventory, the missing first
    // key — so the all-clear notice and the list can never contradict each
    // other.
    const attentionCount = createMemo(
        () =>
            attentionProviders().length +
            providersWithoutReadyInventory().length +
            (entities().apiKeys.length === 0 ? 1 : 0) +
            (hasNoPublicRoutes() ? 1 : 0),
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.homeScreen.overview)}
                description={translation()(
                    ($) => $.screens.homeScreen.theHealthOfThisGatewayAtAGlanceMessage,
                )}
            >
                <ButtonLink
                    href={hrefs.providerNew()}
                    tone="primary"
                    glyph="plus"
                >
                    {translation()(($) => $.screens.homeScreen.createProvider)}
                </ButtonLink>
            </PageHeader>

            <ReadNotice
                read={read}
                retry={read.reload}
            />

            <Show when={read.isFirstRead()}>
                <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-6">
                    <For
                        each={[
                            0,
                            1,
                            2,
                            3,
                            4,
                            5,
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
                    fallback={<GettingStarted apiKeyCount={entities().apiKeys.length} />}
                >
                    <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
                        <StatLink
                            href={hrefs.providers()}
                            value={String(entities().providers.length)}
                            label={translation()(($) => $.screens.homeScreen.providers)}
                        />
                        {/* The figure counts only the ready-inventory
                            rows — the three-input status — while its sibling
                            figures are raw counts, so the hint names the
                            listed total the figure selects from and the
                            smaller number cannot read as a wrong count. */}
                        <StatLink
                            href={hrefs.modelRoutes()}
                            value={String(enabledRoutes())}
                            label={translation()(($) => $.screens.homeScreen.publicRoutes)}
                            hint={translation()(($) => $.dynamicMessages.enabledStored, {
                                total: formatNumber(entities().modelRoutes.length),
                            })}
                        />
                        <StatLink
                            href={hrefs.models(null)}
                            value={String(readyInventoryModels())}
                            label={translation()(($) => $.screens.homeScreen.inventoryReady)}
                            hint={translation()(($) => $.dynamicMessages.readyListed, {
                                total: formatNumber(entities().models.length),
                            })}
                        />
                        <StatLink
                            href={hrefs.apiKeys()}
                            value={String(entities().apiKeys.length)}
                            label={translation()(($) => $.screens.homeScreen.apiKeys)}
                        />
                        <StatLink
                            href={hrefs.usage({
                                ...EMPTY_USAGE_FILTER_DRAFT,
                                window: "24h",
                            })}
                            value={String(traffic().requests)}
                            label={translation()(($) => $.screens.homeScreen.recentRequests24h)}
                        />
                        <StatLink
                            href={hrefs.usage({
                                ...EMPTY_USAGE_FILTER_DRAFT,
                                window: "24h",
                                outcome: "failures",
                            })}
                            value={String(traffic().errors)}
                            label={translation()(($) => $.screens.homeScreen.recentErrors24h)}
                        />
                    </div>

                    <Card>
                        <CardHeader
                            title={translation()(($) => $.screens.homeScreen.requestsPerDay)}
                            description={translation()(
                                ($) => $.screens.homeScreen.theLast14DaysInUTCTheGatewaysMessage,
                            )}
                        >
                            <ButtonLink
                                href={hrefs.usage()}
                                glyph="activity"
                            >
                                {translation()(($) => $.screens.homeScreen.openUsage)}
                            </ButtonLink>
                        </CardHeader>

                        <div class="mt-4">
                            <BarChart
                                points={traffic().series.points.map((point) => ({
                                    label: point.day,
                                    value: point.requests,
                                }))}
                                describePoint={(point) =>
                                    requestChartPointMessage(
                                        point.label,
                                        point.value,
                                        formatNumber(point.value),
                                    )
                                }
                                summary={trafficSummary(traffic().series)}
                            />
                        </div>
                    </Card>

                    <Show
                        when={attentionCount() > 0}
                        fallback={
                            <Notice
                                tone="ok"
                                title={translation()(
                                    ($) => $.screens.homeScreen.storedSettingsReviewedMessage,
                                )}
                                detail={translation()(
                                    ($) =>
                                        $.screens.homeScreen
                                            .inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage,
                                )}
                                fix={null}
                            />
                        }
                    >
                        <Card>
                            <CardHeader
                                title={translation()(($) => $.screens.homeScreen.needsAttention)}
                            />
                            <ul class="mt-4 flex flex-col divide-y divide-rule">
                                <Show when={hasNoPublicRoutes()}>
                                    <AttentionRow
                                        lead={
                                            <Glyph
                                                name="stack"
                                                class="shrink-0 text-ink-faint"
                                            />
                                        }
                                        action={{
                                            href: discoveryTarget(),
                                            get label() {
                                                return translation()(
                                                    ($) => $.screens.homeScreen.discoverModels,
                                                );
                                            },
                                        }}
                                    >
                                        <p class="min-w-0 flex-1 basis-48 text-[13.5px]">
                                            {translation()(
                                                ($) =>
                                                    $.screens.homeScreen
                                                        .noModelsAreAvailableDiscoverModelsOnAnMessage,
                                            )}
                                        </p>
                                    </AttentionRow>
                                </Show>
                                <Show when={entities().apiKeys.length === 0}>
                                    <AttentionRow
                                        lead={
                                            <Glyph
                                                name="key"
                                                class="shrink-0 text-ink-faint"
                                            />
                                        }
                                        action={{
                                            href: hrefs.apiKeyNew(),
                                            get label() {
                                                return translation()(
                                                    ($) => $.screens.homeScreen.createAPIKey,
                                                );
                                            },
                                        }}
                                    >
                                        <p class="min-w-0 flex-1 text-[13.5px]">
                                            <span class="font-medium">
                                                {translation()(
                                                    ($) => $.screens.homeScreen.noAPIKeysYetMessage,
                                                )}
                                            </span>
                                            <span class="text-ink-dim">
                                                {translation()(
                                                    ($) => $.dynamicMessages.noKeysDetail,
                                                )}
                                            </span>
                                        </p>
                                    </AttentionRow>
                                </Show>
                                <For each={attentionProviders()}>
                                    {(item) => (
                                        <AttentionRow
                                            lead={
                                                <Badge
                                                    tone="warn"
                                                    label={item.attention.label}
                                                />
                                            }
                                            action={item.attention.action}
                                        >
                                            <p class="min-w-0 flex-1 basis-48 text-[13.5px]">
                                                <span class="font-medium">
                                                    <bdi dir="auto">{item.provider.name}</bdi>
                                                </span>
                                                <span class="text-ink-dim">
                                                    {` · ${CREDENTIAL_KIND.label(item.provider.type)} · ${item.attention.detail}`}
                                                </span>
                                            </p>
                                        </AttentionRow>
                                    )}
                                </For>
                                <For each={providersWithoutReadyInventory()}>
                                    {(item) => (
                                        <AttentionRow
                                            lead={
                                                <Glyph
                                                    name="stack"
                                                    class="shrink-0 text-ink-faint"
                                                />
                                            }
                                            action={item.attention.action}
                                        >
                                            <p class="min-w-0 flex-1 basis-48 text-[13.5px]">
                                                {inventoryAttentionMessage(
                                                    item.provider.name,
                                                    item.attention.reason,
                                                )}
                                            </p>
                                        </AttentionRow>
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
        return translation()(($) => $.screens.homeScreen.noRequestsRecordedMessage);
    }

    return requestSummaryMessage(series.total, series.peak);
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
                title={translation()(($) => $.screens.homeScreen.endpoints)}
                description={translation()(
                    ($) => $.screens.homeScreen.everySurfaceServesEveryModelPointAnyOpenAIMessage,
                )}
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
                                    subject={translation()(
                                        ($) => $.screens.homeScreen.theURLBesideIt,
                                    )}
                                    ariaLabel={translation()(
                                        ($) => $.dynamicMessages.copyEndpoint,
                                        {
                                            surface: surface.label,
                                        },
                                    )}
                                />
                            </li>
                        );
                    }}
                </For>
            </ul>
            <p class="mt-3 text-[12.5px] text-ink-faint">{servingKeyMessage(hrefs.apiKeys())}</p>
        </Card>
    );
}

/**
 * One needs-attention line: its lead mark, its statement, and the action that
 * answers it. Every attention kind the overview lists renders through here, so
 * the rows share one anatomy instead of drifting apart.
 */
function AttentionRow(props: {
    readonly lead: JSX.Element;
    readonly action: {
        readonly href: string;
        readonly label: string;
    };
    readonly children: JSX.Element;
}): JSX.Element {
    return (
        <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
            {props.lead}
            {props.children}
            <ButtonLink href={props.action.href}>{props.action.label}</ButtonLink>
        </li>
    );
}

/**
 * A fresh gateway: the six steps from here to a working endpoint. The key
 * count rides in because keys can exist with no provider standing — the
 * Endpoints card below links them — and a key already issued is step five's
 * done state, however early it was taken.
 */
function GettingStarted(props: { readonly apiKeyCount: number }): JSX.Element {
    return (
        <EmptyState
            glyph="plug"
            title={translation()(($) => $.screens.homeScreen.noProvidersYet)}
            description={translation()(
                ($) => $.screens.homeScreen.aProviderIsOneUpstreamAccountItsModelsMessage,
            )}
        >
            <ButtonLink
                href={hrefs.providerNew()}
                tone="primary"
                glyph="plus"
            >
                {translation()(($) => $.screens.homeScreen.createYourFirstProvider)}
            </ButtonLink>
            <ol class="mt-2 flex w-full list-none flex-col gap-2 text-[13.5px]">
                <GettingStartedStep
                    index={1}
                    isDone
                    title={translation()(($) => $.screens.homeScreen.connectedToTheGateway)}
                />
                <GettingStartedStep
                    index={2}
                    title={translation()(($) => $.screens.homeScreen.createAProvider)}
                />
                <GettingStartedStep
                    index={3}
                    title={translation()(($) => $.screens.homeScreen.discoverItsModelsFromUpstream)}
                />
                <GettingStartedStep
                    index={4}
                    isDone={props.apiKeyCount > 0}
                    title={translation()(
                        ($) => $.screens.homeScreen.issueAnAPIKeyForServingClients,
                    )}
                />
                <GettingStartedStep
                    index={5}
                    title={translation()(($) => $.screens.homeScreen.pointAClientAtAnEndpointBelow)}
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
