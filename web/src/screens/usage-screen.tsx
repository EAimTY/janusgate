import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { formatTimestamp, formatTokenCount, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiLogOutcome,
    ApiLogResponse,
    ListUsageRequest,
    UsageFilter,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { apiLogEndpointLabel, apiLogOutcomeLabel } from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { ReadNotice } from "/src/screens/read-notice";
import { Button, ButtonLink, SelectField } from "/src/ui/controls";
import { Badge, type BadgeTone, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { UsageFilterControls } from "/src/ui/usage-filter-controls";
import { createScreenRead, readAll, screenFailureNotice } from "./read";
import {
    draftUsageFilter,
    EMPTY_USAGE_FILTER_DRAFT,
    hasUsageFilter,
    USAGE_WINDOW_OPTIONS,
    type UsageFilterDraft,
} from "./usage-filter";
import {
    indexUsageEntities,
    readUsageEntities,
    type UsageEntities,
    type UsageEntityIndex,
    usageApiKeyLabel,
    usageModelLabel,
    usageProviderLabel,
} from "./usage-labels";
import { nextUsagePage, USAGE_PAGE_SIZE, usageCursor } from "./usage-pagination";

/**
 * Every dispatched request, newest first, under the shared filter its route
 * carries — the same filter the aggregates page narrows, so the two reads of
 * one gateway's accounting always describe the same question. The filter is
 * the address: applying it navigates, which makes a filtered stream linkable,
 * shareable, and openable by the browser's back button, and leaves the
 * unfiltered stream at exactly `/usage`.
 */
export function UsageScreen(props: {
    readonly api: ManagementApi;
    readonly filter: UsageFilterDraft;
}): JSX.Element {
    const [entities, setEntities] = createSignal<UsageEntities>({
        providers: [],
        models: [],
        apiKeys: [],
    });
    const [rows, setRows] = createSignal<ReadonlyArray<ApiLogResponse>>([]);
    const [hasMore, setHasMore] = createSignal(true);
    const [isLoadingMore, setIsLoadingMore] = createSignal(false);
    const [draft, setDraft] = createSignal<UsageFilterDraft>(props.filter);
    // The wire filter the rows in hand were listed under. A rolling window is
    // resolved once per cycle, never per page: a stream whose pages carried
    // different bounds could skip or repeat rows at the seam.
    const [activeFilter, setActiveFilter] = createSignal<UsageFilter>({});

    // The route owns the filter, so a navigation — the browser's back button
    // included — rewrites the form to the address it landed on.
    createEffect(
        () => props.filter,
        (filter) => {
            setDraft(filter);
        },
    );

    // The loader that extends the stream, beside the harness's read cycle. Its
    // async continuation checks that its controller is still the current one
    // and discards its result otherwise: a superseded call writes nothing.
    let loader: AbortController | null = null;

    onCleanup(() => {
        loader?.abort();
    });

    const read = createScreenRead({
        reReadOn: () => props.filter,
        load: async (signal) => {
            // A page the refresh is about to replace must never land; the
            // refresh owns the stream now.
            loader?.abort();
            loader = null;
            setIsLoadingMore(false);

            const filter = draftUsageFilter(props.filter, nowMicros());
            setActiveFilter(filter);

            const [entitiesResult, pageResult] = await readAll(
                signal,
                readUsageEntities(props.api),
                {
                    action: "list-usage",
                    run: (cycle) => props.api.listUsage(usageListRequest(filter, null), cycle),
                },
            );

            setEntities(entitiesResult);

            const continuation = nextUsagePage([], pageResult.api_logs);
            setRows(continuation.rows);
            setHasMore(continuation.hasMore);
        },
    });

    // Indexed once per entities change, so every row's label lookups are O(1).
    const index = createMemo(() => indexUsageEntities(entities()));

    /** Reads one `usage:list` page, stating a failure and answering nothing. */
    const readPage = async (
        signal: AbortSignal,
        beforeId: number,
    ): Promise<ReadonlyArray<ApiLogResponse> | null> => {
        const outcome = await performManagementCall(() =>
            props.api.listUsage(usageListRequest(activeFilter(), beforeId), signal),
        );

        if (outcome.status === "aborted") {
            return null;
        }

        if (outcome.status === "failed") {
            read.showNotice(
                screenFailureNotice(outcome.cause, {
                    action: "list-usage",
                }),
            );
            return null;
        }

        return outcome.result.api_logs;
    };

    const loadMore = async (): Promise<void> => {
        // A refresh in flight is about to replace the stream wholesale;
        // extending it would resurrect rows the replacement invalidates.
        if (isLoadingMore() || read.isReading()) {
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
        read.showNotice(null);
        setRows(continuation.rows);
        setHasMore(continuation.hasMore);
    };

    /**
     * Applies the form to the stream by navigating to the address it names.
     * The same address re-reads instead of navigating — applying an unchanged
     * filter is a request for the newest rows, not a no-op.
     */
    const applyFilter = (): void => {
        const target = hrefs.usage(draft());

        if (target === `${window.location.pathname}${window.location.search}`) {
            read.reload();
            return;
        }

        navigate(target);
    };

    const clearFilter = (): void => {
        setDraft(EMPTY_USAGE_FILTER_DRAFT);
        navigate(hrefs.usage());
    };

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

            <ReadNotice read={read} />

            <Card>
                <CardHeader
                    title="Filter"
                    description="The shared accounting filter — the same one the aggregates page narrows."
                />
                <form
                    novalidate={true}
                    class="mt-4 flex flex-col gap-4"
                    onSubmit={(event) => {
                        event.preventDefault();
                        applyFilter();
                    }}
                >
                    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                        <SelectField
                            label="Window"
                            value={draft().window}
                            options={USAGE_WINDOW_OPTIONS}
                            onChange={(window) =>
                                setDraft({
                                    ...draft(),
                                    window,
                                })
                            }
                            isDisabled={read.isReading()}
                            hint="Rolling, resolved when the stream is read."
                        />
                        <UsageFilterControls
                            fields={draft()}
                            providers={entities().providers}
                            models={entities().models}
                            apiKeys={entities().apiKeys}
                            isDisabled={read.isReading()}
                            onEdit={(edits) =>
                                setDraft((current) => ({
                                    ...current,
                                    ...edits,
                                }))
                            }
                        />
                    </div>
                    <div class="flex flex-wrap items-center gap-2">
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={read.isReading()}
                        >
                            {read.isReading() ? "Reading…" : "Apply filters"}
                        </Button>
                        <Show when={hasUsageFilter(draft())}>
                            <Button
                                tone="ghost"
                                isDisabled={read.isReading()}
                                onClick={clearFilter}
                            >
                                Clear filters
                            </Button>
                        </Show>
                    </div>
                </form>
            </Card>

            <Show when={read.isFirstRead()}>
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

            <Show when={read.hasRead()}>
                <Show
                    when={rows().length > 0}
                    fallback={
                        <Show
                            when={hasUsageFilter(props.filter)}
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
                            <Notice
                                tone="info"
                                title="No recorded requests match this filter."
                                detail="Widen the window, drop a filter, or read the whole stream — Clear filters sits in the form above."
                                fix="The gateway's filter locates rows; it never validates a value against them, so an unknown id or a status never recorded simply matches nothing."
                            />
                        </Show>
                    }
                >
                    <ul class="flex flex-col gap-2">
                        <For each={rows()}>
                            {(row) => (
                                <UsageRow
                                    row={row}
                                    index={index()}
                                />
                            )}
                        </For>
                    </ul>

                    <Show
                        when={hasMore()}
                        fallback={
                            <p class="text-[12.5px] text-ink-faint">
                                {hasUsageFilter(props.filter)
                                    ? "That is every row this filter matches."
                                    : "That is every recorded row."}
                            </p>
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

/**
 * One `usage:list` request: the page size, the cursor when the stream
 * continues, and the filter only when something is chosen — an absent filter
 * is the wire's own spelling of "every row", so the unfiltered stream sends
 * exactly what it always did.
 */
function usageListRequest(filter: UsageFilter, beforeId: number | null): ListUsageRequest {
    return {
        limit: USAGE_PAGE_SIZE,
        ...(beforeId === null
            ? {}
            : {
                  before_id: beforeId,
              }),
        ...(Object.keys(filter).length === 0
            ? {}
            : {
                  filter,
              }),
    };
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
 * its numbers. Labels resolve through the indexed entity lists; an id with
 * no listing reads as its deleted kind.
 */
function UsageRow(props: {
    readonly row: ApiLogResponse;
    readonly index: UsageEntityIndex;
}): JSX.Element {
    const outcome = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => usageOutcomeBadge(props.row.outcome);

    const model = () => usageModelLabel(props.index.modelsById, props.row.model_id);
    const provider = () => usageProviderLabel(props.index.providersById, props.row.provider_id);
    const apiKey = () => usageApiKeyLabel(props.index.apiKeysById, props.row.api_key_id);

    const missingTitle = "This row is no longer listed.";

    return (
        <li class="rounded-xl border border-rule bg-surface px-4 py-3">
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                <span class="tnum text-[13px] font-medium">
                    {formatTimestamp(props.row.started_at)}
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
