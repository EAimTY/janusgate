import type { JSX } from "@solidjs/web";
import { createSignal, createTrackedEffect, For, Show } from "solid-js";
import { formatCreatedAt, formatTokenCount } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiLogOutcome, ApiLogResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { apiLogEndpointLabel, apiLogOutcomeLabel } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import {
    type UsageEntities,
    usageApiKeyLabel,
    usageModelLabel,
    usageProviderLabel,
} from "/src/screens/usage-labels";
import { Button, ButtonLink } from "/src/ui/controls";
import { Badge, type BadgeTone, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { PageHeader } from "/src/ui/layout";
import { nextUsagePage, USAGE_PAGE_SIZE, usageCursor } from "./usage-pagination";

/**
 * Every dispatched request, newest first. The listing carries no filters —
 * the route's only cursor is the row id — so this page is the raw stream
 * with its labels resolved through the entity lists, and the aggregation
 * page carries the filtered questions.
 */
export function UsageScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [entities, setEntities] = createSignal<UsageEntities>({
        providers: [],
        models: [],
        apiKeys: [],
    });
    const [rows, setRows] = createSignal<ReadonlyArray<ApiLogResponse>>([]);
    const [hasMore, setHasMore] = createSignal(true);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [isLoadingMore, setIsLoadingMore] = createSignal(false);
    const [failure, setFailure] = createSignal<{
        readonly title: string;
        readonly detail: string;
        readonly fix: string | null;
    } | null>(null);

    // The read that owns the stream and the loader that extends it. Each async
    // continuation checks that its controller is still the current one and
    // discards its result otherwise: a superseded call must write nothing.
    let reader: AbortController | null = null;
    let loader: AbortController | null = null;

    const readEntities = async (signal: AbortSignal): Promise<boolean> => {
        const [providersOutcome, modelsOutcome, apiKeysOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(signal)),
            performManagementCall(() => props.api.listModels({}, signal)),
            performManagementCall(() => props.api.listApiKeys(signal)),
        ]);

        if (
            providersOutcome.status === "aborted" ||
            modelsOutcome.status === "aborted" ||
            apiKeysOutcome.status === "aborted"
        ) {
            return false;
        }

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : apiKeysOutcome.status === "failed"
                    ? apiKeysOutcome
                    : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return false;
        }

        setEntities({
            providers: providersOutcome.status === "ok" ? providersOutcome.result.providers : [],
            models: modelsOutcome.status === "ok" ? modelsOutcome.result.models : [],
            apiKeys: apiKeysOutcome.status === "ok" ? apiKeysOutcome.result.api_keys : [],
        });
        return true;
    };

    const readPage = async (
        signal: AbortSignal,
        beforeId: number | null,
    ): Promise<ReadonlyArray<ApiLogResponse> | null> => {
        const outcome = await performManagementCall(() =>
            props.api.listUsage(
                beforeId === null
                    ? {
                          limit: USAGE_PAGE_SIZE,
                      }
                    : {
                          limit: USAGE_PAGE_SIZE,
                          before_id: beforeId,
                      },
                signal,
            ),
        );

        if (outcome.status === "aborted") {
            return null;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, "list-usage");

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return null;
        }

        return outcome.result.api_logs;
    };

    const read = async (): Promise<void> => {
        reader?.abort();
        loader?.abort();
        loader = null;

        const call = new AbortController();
        reader = call;
        setIsReading(true);
        // The aborted loader will never land; the refresh owns the stream now.
        setIsLoadingMore(false);

        const entitiesRead = await readEntities(call.signal);

        if (call !== reader) {
            return;
        }

        if (!entitiesRead) {
            setIsReading(false);
            return;
        }

        const page = await readPage(call.signal, null);

        if (call !== reader) {
            return;
        }

        if (page === null) {
            setIsReading(false);
            return;
        }

        const continuation = nextUsagePage([], page);
        setFailure(null);
        setRows(continuation.rows);
        setHasMore(continuation.hasMore);
        setHasRead(true);
        setIsReading(false);
        reportRefresh();
    };

    const loadMore = async (): Promise<void> => {
        // A refresh in flight is about to replace the stream wholesale;
        // extending it would resurrect rows the replacement invalidates.
        if (isLoadingMore() || isReading()) {
            return;
        }

        const cursor = usageCursor(rows());

        if (cursor === null) {
            return;
        }

        loader?.abort();

        const call = new AbortController();
        loader = call;
        setIsLoadingMore(true);

        const page = await readPage(call.signal, cursor);

        if (call !== loader) {
            return;
        }

        setIsLoadingMore(false);

        if (page === null) {
            // A failed load is not the end of the stream: the failure notice
            // above is the signal, and the continuation stays available.
            return;
        }

        const continuation = nextUsagePage(rows(), page);
        setFailure(null);
        setRows(continuation.rows);
        setHasMore(continuation.hasMore);
    };

    createTrackedEffect(() => {
        refreshRequest();
        void read();

        return () => {
            reader?.abort();
            loader?.abort();
        };
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Usage"
                description="Every dispatched request, newest first — numbers only, never content."
            >
                <ButtonLink
                    href={hrefs.usageQuery()}
                    glyph="activity"
                >
                    View aggregates
                </ButtonLink>
            </PageHeader>

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
                <div class="flex flex-col gap-2">
                    <For
                        each={[
                            0,
                            1,
                            2,
                        ]}
                    >
                        {() => <Skeleton class="h-24 w-full rounded-xl" />}
                    </For>
                </div>
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={rows().length > 0}
                    fallback={
                        <EmptyState
                            glyph="activity"
                            title="No requests recorded yet"
                            description="One row appears per dispatched serving request — the moment the gateway sends upstream. If traffic flows and nothing appears, check api_logs.is_enabled in the gateway's configuration file."
                        >
                            <ButtonLink
                                href={hrefs.usageQuery()}
                                glyph="activity"
                            >
                                View aggregates
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <ul class="flex flex-col gap-2">
                        <For each={rows()}>
                            {(row) => (
                                <UsageRow
                                    row={row}
                                    entities={entities()}
                                />
                            )}
                        </For>
                    </ul>

                    <Show
                        when={hasMore()}
                        fallback={
                            <p class="text-[12.5px] text-ink-faint">That is every recorded row.</p>
                        }
                    >
                        <div>
                            <Button
                                glyph="chev-right"
                                isBusy={isLoadingMore()}
                                onClick={() => void loadMore()}
                            >
                                {isLoadingMore() ? "Loading…" : "Load more"}
                            </Button>
                        </div>
                    </Show>
                </Show>
            </Show>
        </div>
    );
}

/** The badge behind one recorded outcome. */
function usageOutcomeBadge(outcome: ApiLogOutcome): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    switch (outcome) {
        case "completed":
            return {
                tone: "ok",
                label: apiLogOutcomeLabel(outcome),
            };

        case "client_aborted":
            return {
                tone: "warn",
                label: apiLogOutcomeLabel(outcome),
            };

        case "upstream_error":
        case "internal":
            return {
                tone: "bad",
                label: apiLogOutcomeLabel(outcome),
            };
    }
}

/**
 * One recorded request: when it started, how it ended, what it was, and
 * its numbers. Labels resolve through the current entity lists; an id with
 * no listing reads as its deleted kind.
 */
function UsageRow(props: {
    readonly row: ApiLogResponse;
    readonly entities: UsageEntities;
}): JSX.Element {
    const outcome = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => usageOutcomeBadge(props.row.outcome);

    const model = () => usageModelLabel(props.entities, props.row.model_id);
    const provider = () => usageProviderLabel(props.entities, props.row.provider_id);
    const apiKey = () => usageApiKeyLabel(props.entities, props.row.api_key_id);

    const missingTitle = "This row is no longer listed.";

    return (
        <li class="rounded-xl border border-rule bg-surface px-4 py-3">
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                <span class="tnum text-[13px] font-medium">
                    {formatCreatedAt(props.row.started_at)}
                </span>
                <Badge
                    tone={outcome().tone}
                    label={outcome().label}
                />
                <span class="text-[12.5px] text-ink-faint">
                    {apiLogEndpointLabel(props.row.endpoint_type)}
                    {props.row.is_stream ? " · Stream" : ""}
                </span>
            </div>
            <p class="tnum mt-1.5 flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-[13.5px]">
                <span title={model().isListed ? undefined : missingTitle}>{model().text}</span>
                <span
                    aria-hidden="true"
                    class="text-ink-faint"
                >
                    ·
                </span>
                <span title={provider().isListed ? undefined : missingTitle}>
                    {provider().text}
                </span>
                <span
                    aria-hidden="true"
                    class="text-ink-faint"
                >
                    ·
                </span>
                <span class="inline-flex items-center gap-1">
                    <Glyph
                        name="key"
                        class="text-ink-faint"
                    />
                    <span title={apiKey().isListed ? undefined : missingTitle}>
                        {apiKey().text}
                    </span>
                </span>
            </p>
            <p class="tnum mt-1 text-[12.5px] text-ink-faint">
                {props.row.http_status === null ? "—" : props.row.http_status}
                {` · ${props.row.duration} ms · in ${formatTokenCount(props.row.input_tokens)} · out ${formatTokenCount(props.row.output_tokens)} · cached ${formatTokenCount(props.row.cached_tokens)} · write ${formatTokenCount(props.row.cache_write_tokens)} · reasoning ${formatTokenCount(props.row.reasoning_tokens)}`}
            </p>
        </li>
    );
}
