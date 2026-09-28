import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { credentialExpiryLine, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import {
    isModelEffectivelyServing,
    servingModelCountsByProvider,
} from "/src/management-api/serving";
import {
    credentialKindLabel,
    credentialStanding,
    isLapsedCredentialStanding,
    isOAuthProviderRow,
    type OauthProviderRow,
} from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { indexRowsById } from "/src/row-index";
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
import { OUTWARD_SURFACES } from "./outward-surfaces";

/** What the overview's reads answered: the rolling figures plus the daily series. */
type OverviewTraffic = OverviewRecentTotals & {
    readonly series: OverviewDailySeries;
};

/** One listed attention line: its provider, the human reason it needs one, and the action that answers it. */
type ProviderAttention = {
    readonly provider: ProviderResponse;
    readonly attention: {
        readonly label: string;
        readonly detail: string;
        readonly action: {
            readonly href: string;
            readonly label: string;
        };
    };
};

/**
 * One enabled provider with nothing effectively serving, together with the
 * attention its cause calls for.
 */
type ProviderServingAttention = {
    readonly provider: ProviderResponse;
    readonly attention: {
        readonly detail: string;
        readonly action: string;
        readonly href: string;
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

    // An enabled provider with nothing effectively serving (README §3) needs
    // a human: its rows are all vanished, disabled, or absent — the vanished
    // case is the quietest failure there is, and a raw row count would hide
    // it. A deliberately disabled provider is the operator's choice, not a
    // fault, so it stays off this list. Each line's action follows the cause
    // its rows name, because discovery — the fix the absent case needs —
    // cannot repair a row that is merely switched off.
    const providersWithoutServingModels = createMemo(
        (): ReadonlyArray<ProviderServingAttention> => {
            const servingCounts = servingModelCountsByProvider(entities().models, providersById());

            return entities().providers.flatMap((provider): ProviderServingAttention[] => {
                if (!provider.is_enabled || (servingCounts.get(provider.id) ?? 0) !== 0) {
                    return [];
                }

                return [
                    {
                        provider,
                        attention: servingGapAttention(entities().models, provider.id),
                    },
                ];
            });
        },
    );

    // Exactly what the list below counts: every item the section can show —
    // credential problems, providers with nothing serving, the missing first
    // key — so the all-clear notice and the list can never contradict each
    // other.
    const attentionCount = createMemo(
        () =>
            attentionProviders().length +
            providersWithoutServingModels().length +
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

            <ReadNotice
                read={read}
                retry={read.reload}
            />

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
                    fallback={<GettingStarted apiKeyCount={entities().apiKeys.length} />}
                >
                    <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
                        <StatLink
                            href={hrefs.providers()}
                            value={String(entities().providers.length)}
                            label="Providers"
                        />
                        {/* The figure counts only the effectively-serving
                            rows — the three-input status — while its sibling
                            figures are raw counts, so the hint names the
                            listed total the figure selects from and the
                            smaller number cannot read as a wrong count. */}
                        <StatLink
                            href={hrefs.models(null)}
                            value={String(servingModels())}
                            label="Models routing"
                            hint={`serving of ${entities().models.length} listed`}
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
                                    <AttentionRow
                                        lead={
                                            <Glyph
                                                name="key"
                                                class="shrink-0 text-ink-faint"
                                            />
                                        }
                                        action={{
                                            href: hrefs.apiKeyNew(),
                                            label: "Create API key",
                                        }}
                                    >
                                        <p class="min-w-0 flex-1 text-[13.5px]">
                                            <span class="font-medium">No API keys yet.</span>
                                            <span class="text-ink-dim">
                                                {
                                                    " Serving clients authenticate with one — without it, the gateway serves nothing."
                                                }
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
                                                    {item.provider.name}
                                                </span>
                                                <span class="text-ink-dim">
                                                    {` · ${credentialKindLabel(item.provider.type)} · ${item.attention.detail}`}
                                                </span>
                                            </p>
                                        </AttentionRow>
                                    )}
                                </For>
                                <For each={providersWithoutServingModels()}>
                                    {(item) => (
                                        <AttentionRow
                                            lead={
                                                <Glyph
                                                    name="stack"
                                                    class="shrink-0 text-ink-faint"
                                                />
                                            }
                                            action={{
                                                href: item.attention.href,
                                                label: item.attention.action,
                                            }}
                                        >
                                            <p class="min-w-0 flex-1 basis-48 text-[13.5px]">
                                                <span class="font-medium">
                                                    {item.provider.name}
                                                </span>
                                                <span class="text-ink-dim">
                                                    {item.attention.detail}
                                                </span>
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

/**
 * The attention an enabled provider with nothing effectively serving carries,
 * chosen by the cause its rows name: a provider holding no model rows at all
 * is missing its discovery — the provider's page runs it — while a provider
 * whose rows exist but none serve holds only switched-off or vanished rows,
 * which discovery cannot repair; its models page states each row's own cause.
 */
function servingGapAttention(
    models: ReadonlyArray<ModelResponse>,
    providerId: number,
): {
    readonly detail: string;
    readonly action: string;
    readonly href: string;
} {
    const hasNoModels = !models.some((model) => model.provider_id === providerId);

    return hasNoModels
        ? {
              detail: " has no models yet.",
              action: "Open and discover",
              href: hrefs.providerDetail(providerId),
          }
        : {
              detail: " has no models serving — every model is switched off or suspended.",
              action: "Review models",
              href: hrefs.models(providerId),
          };
}

/**
 * The lapsed-credential wording itself: expired, or never fresh — `null`
 * while the credential still holds. Takes the OAuth member of the row union,
 * since the expiry is an OAuth row's column alone.
 */
function lapsedCredentialAttention(provider: OauthProviderRow): {
    readonly label: string;
    readonly detail: string;
} | null {
    // The gate is the vocabulary's one lapsed verdict over the row's
    // standing. The missing-expiry column below is the never-fresh
    // standing's own definition in `credentialStanding`, so past that gate
    // the narrow is only the title's switch, no second verdict — and the
    // narrowed column feeds the expired line's absolute instant.
    if (!isLapsedCredentialStanding(credentialStanding(provider))) {
        return null;
    }

    if (provider.expires_at === null) {
        return {
            label: "Credential not fresh",
            detail: "No expiry is stored for this provider.",
        };
    }

    return {
        label: "Credential expired",
        detail: credentialExpiryLine(provider.expires_at, nowMicros()),
    };
}

/**
 * Why an OAuth provider needs a human: expired, or never fresh. The row's
 * action follows the provider's parked state: reauthorization lives on the
 * edit page's Credentials card, but a disabled provider is the operator's
 * own choice, so its row names the parked state and opens the provider's
 * detail page — read-only itself, its *Edit provider* door reaching the
 * edit page's Settings card where the Enabled switch lives — instead of
 * ordering a reconnect the operator deliberately deferred. The credential
 * counts either way: a parked provider re-enabled with lapsed credentials
 * fails every request.
 */
function credentialAttention(provider: ProviderResponse): ProviderAttention["attention"] | null {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    const lapsed = lapsedCredentialAttention(provider);

    if (lapsed === null) {
        return null;
    }

    return provider.is_enabled
        ? {
              ...lapsed,
              action: {
                  href: hrefs.providerEdit(provider.id),
                  label: "Reconnect",
              },
          }
        : {
              ...lapsed,
              detail: `${lapsed.detail} The provider is disabled — reconnect when you re-enable it.`,
              action: {
                  href: hrefs.providerDetail(provider.id),
                  label: "Open provider",
              },
          };
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
 * A fresh gateway: the five steps from here to a working endpoint. The key
 * count rides in because keys can exist with no provider standing — the
 * Endpoints card below links them — and a key already issued is step four's
 * done state, however early it was taken.
 */
function GettingStarted(props: { readonly apiKeyCount: number }): JSX.Element {
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
                Create your first provider
            </ButtonLink>
            <ol class="mt-2 flex w-full list-none flex-col gap-2 text-[13.5px]">
                <GettingStartedStep
                    index={1}
                    isDone
                    title="Connected to the gateway"
                />
                <GettingStartedStep
                    index={2}
                    title="Create a provider"
                />
                <GettingStartedStep
                    index={3}
                    title="Discover its models from upstream"
                />
                <GettingStartedStep
                    index={4}
                    isDone={props.apiKeyCount > 0}
                    title="Issue an API key for serving clients"
                />
                <GettingStartedStep
                    index={5}
                    title="Point a client at an endpoint below"
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
