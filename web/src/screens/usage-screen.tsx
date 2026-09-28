import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { formatDuration, formatTimestamp, formatTokenCount, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiLogOutcome, ApiLogResponse, UsageFilter } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { apiLogEndpointLabel, apiLogOutcomeLabel } from "/src/management-api/vocabulary";
import { hrefs, isCurrentAddress, navigate } from "/src/router";
import { ReadNotice } from "/src/screens/read-notice";
import { AppLink } from "/src/ui/app-link";
import { Button, ButtonLink, SelectField } from "/src/ui/controls";
import { Badge, type BadgeTone, EmptyState, Notice } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { UsageFilterControls } from "/src/ui/usage-filter-controls";
import {
    buildUsageFilter,
    draftUsageFilter,
    EMPTY_USAGE_FILTER_DRAFT,
    hasUsageFilter,
    namedSubjects,
    soleSubject,
    statusError,
    USAGE_OUTCOME_FAILURES,
    USAGE_QUERY_SUBJECT_DIMENSIONS,
    USAGE_QUERY_SUBJECT_FIELDS,
    USAGE_WINDOW_OPTIONS,
    type UsageFilterDraft,
    type UsageQuerySubject,
} from "/src/usage-filter";
import { createScreenRead, readAll, type ScreenNotice, screenFailureNotice } from "./read";
import { SkeletonRows } from "./skeleton-rows";
import {
    indexUsageEntities,
    type UsageEntityIndex,
    usageApiKeyLabel,
    usageModelLabel,
    usageProviderLabel,
} from "./usage-labels";
import { nextUsagePage, usageCursor } from "./usage-pagination";
import {
    readUsageEntities,
    readUsagePage,
    type UsageEntities,
    usageListRequest,
} from "./usage-read";

/**
 * Every dispatched request, newest first, under the shared filter its route
 * carries — the same filter the aggregates page narrows, so the two reads of
 * one gateway's accounting always describe the same question. The header's
 * aggregates door carries the standing draft's single-entity narrow as the
 * aggregates page's subject — the same source the aggregates page's own door
 * carries back — so a narrowed stream lands asking about that entity rather
 * than silently totaling the gateway, while the window and every other filter
 * stay the aggregates page's own; the door states that trade visibly beside
 * its label exactly while the draft holds one. The filter is
 * the address: applying it navigates, which makes a filtered stream linkable,
 * shareable, and openable by the browser's back button, and leaves the
 * unfiltered stream at exactly `/usage`. The route Match keys this screen, so
 * the arrival filter seeds the form once — a changed filter is a fresh mount,
 * never a prop the form must chase.
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
    // The read's first success flips this, permanently for the mount: the
    // read lands the listings and the page together, and a failure installs
    // nothing, so only a landed read teaches the filter fields which ids the
    // listings no longer carry. Until then a route-seeded id renders as its
    // plain value — honest about the nothing read yet.
    const [hasEntityListings, setHasEntityListings] = createSignal(false);
    const [hasMore, setHasMore] = createSignal(true);
    const [isLoadingMore, setIsLoadingMore] = createSignal(false);
    // The load-more failure is stated beside its own control — the Load more
    // button at the stream's end — never in the read-level notice above the
    // page, which states read failures only (README §6.4). A read cycle that
    // lands replaces the stream wholesale and takes the failure with it: the
    // fresh stream's continuation is untested, so the old diagnosis describes
    // nothing that still exists.
    const [loadMoreNotice, setLoadMoreNotice] = createSignal<ScreenNotice | null>(null);
    // Seeded once from the arrival filter: the keyed remount owns every route
    // change, so the draft never chases a changed route within a mount.
    const [draft, setDraft] = createSignal<UsageFilterDraft>(props.filter);
    // The wire filter the rows in hand were listed under. A rolling window is
    // resolved once per cycle, never per page: a stream whose pages carried
    // different bounds could skip or repeat rows at the seam.
    const [activeFilter, setActiveFilter] = createSignal<UsageFilter>({});

    // The loader that extends the stream, beside the harness's read cycle. Its
    // async continuation checks that its controller is still the current one
    // and discards its result otherwise: a superseded call writes nothing.
    let loader: AbortController | null = null;

    onCleanup(() => {
        loader?.abort();
    });

    const read = createScreenRead({
        load: async (signal) => {
            // A page the refresh is about to replace must never land; the
            // refresh owns the stream now.
            loader?.abort();
            loader = null;
            setIsLoadingMore(false);

            const filter = draftUsageFilter(props.filter, nowMicros());

            const [entitiesResult, pageResult] = await readAll(
                signal,
                readUsageEntities(props.api),
                readUsagePage(props.api, usageListRequest(filter, null)),
            );

            // The filter is installed only once its rows have landed: a cycle
            // that fails leaves the previous filter standing, so "Load more"
            // keeps carrying the rows in hand's own filter instead of pairing
            // their cursor with a filter they never answered.
            setActiveFilter(filter);
            setEntities(entitiesResult);
            setHasEntityListings(true);

            const continuation = nextUsagePage([], pageResult.api_logs);
            setRows(continuation.rows);
            setHasMore(continuation.hasMore);
            // The fresh stream replaces the old one whole: a failure stated
            // beside the old stream's control described a continuation this
            // stream no longer has.
            setLoadMoreNotice(null);
        },
    });

    // Indexed once per entities change, so every row's label lookups are O(1).
    const index = createMemo(() => indexUsageEntities(entities()));

    /** Reads one `usage:list` page, stating a failure and answering nothing. */
    const readPage = async (
        signal: AbortSignal,
        before: number,
    ): Promise<ReadonlyArray<ApiLogResponse> | null> => {
        const outcome = await performManagementCall(() =>
            props.api.listUsage(usageListRequest(activeFilter(), before), signal),
        );

        if (outcome.status === "aborted") {
            return null;
        }

        if (outcome.status === "failed") {
            // A superseded page states nothing: its cycle was aborted because
            // the refresh is replacing the stream it was extending, and the
            // failure must not land beside the fresh stream's control.
            if (signal.aborted) {
                return null;
            }

            setLoadMoreNotice(
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
            // beside this control is the signal, and the continuation stays
            // available.
            return;
        }

        const continuation = nextUsagePage(rows(), page);
        // A retry that landed retires only the failure stated beside this
        // control; the read-level notice above stays the read's alone.
        setLoadMoreNotice(null);
        setRows(continuation.rows);
        setHasMore(continuation.hasMore);
    };

    /**
     * Applies the form to the stream by navigating to the address it names.
     * The same address re-reads instead of navigating — applying an unchanged
     * filter is a request for the newest rows, not a no-op.
     */
    const applyFilter = (): void => {
        // A malformed status list is named at its field: applying it would
        // run the stream with the entries that parse — or with no status
        // narrow at all — so the apply waits until the field parses whole.
        if (statusError(draft().statusInput) !== null) {
            return;
        }

        const target = hrefs.usage(draft());

        if (isCurrentAddress(target)) {
            read.reload();
            return;
        }

        navigate(target);
    };

    const clearFilter = (): void => {
        setDraft(EMPTY_USAGE_FILTER_DRAFT);
        navigate(hrefs.usage());
    };

    // The aggregates door follows the standing draft — the same source the
    // aggregates page's own door carries back — so an operator who narrowed
    // the form crosses with that narrow even before applying it. A draft
    // naming no single entity opens the bare aggregates page.
    const aggregatesSubject = createMemo(() => streamAggregatesSubject(draft()));

    /**
     * Whether the standing draft holds anything the aggregates door leaves
     * behind — the trade the door states beside its label, the mirror of the
     * aggregates page's own door. The crossing carries the single-entity
     * narrow alone: the window and every other filter field stay the
     * aggregates page's own, so the door states that trade exactly while the
     * draft holds one of them.
     */
    const isFilterLeftBehind = (): boolean => {
        // The one draft field the crossing carries; `aggregatesSubject` is
        // undefined exactly when no entity field is set or a compound narrow
        // stands, and then every set field counts.
        const subject = aggregatesSubject();
        const carriedField =
            subject === undefined ? null : USAGE_QUERY_SUBJECT_FIELDS[subject.dimension].draftField;

        return (
            draft().window !== "all" ||
            draft().endpointType !== "" ||
            draft().outcome !== "" ||
            draft().streamChoice !== "" ||
            draft().statusInput !== "" ||
            USAGE_QUERY_SUBJECT_DIMENSIONS.some((dimension) => {
                const field = USAGE_QUERY_SUBJECT_FIELDS[dimension].draftField;

                return field !== carriedField && draft()[field] !== "";
            })
        );
    };

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Usage"
                description="Every dispatched request, newest first — numbers only, never content. A row reporting none of the optional usage figures leaves them off whole: an all-zero tail and an unreported one cannot be told apart here."
            >
                <ButtonLink
                    href={hrefs.usageQuery(aggregatesSubject())}
                    glyph="activity"
                >
                    View aggregates
                    <Show when={isFilterLeftBehind()}>
                        {/* The stated trade, muted but visible — the same
                            pattern as the aggregates page's own door: the
                            narrow the destination shows is already its
                            subject, so the sentence names what stayed
                            behind. */}
                        <span class="text-ink-faint">
                            {" — the window and every other filter stay behind."}
                        </span>
                    </Show>
                </ButtonLink>
            </PageHeader>

            <ReadNotice
                read={read}
                retry={read.reload}
            />

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
                                // Merged functionally like the sibling
                                // controls' edits: two edits in one task — a
                                // change event following an input event —
                                // must build on the pending write, never on a
                                // field set read before the first landed.
                                setDraft((current) => ({
                                    ...current,
                                    window,
                                }))
                            }
                            isDisabled={read.isReading()}
                            hint="Rolling, resolved when the stream is read."
                        />
                        <UsageFilterControls
                            fields={draft()}
                            providers={entities().providers}
                            models={entities().models}
                            apiKeys={entities().apiKeys}
                            hasEntityListings={hasEntityListings()}
                            statusError={statusError(draft().statusInput)}
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
                            isDisabled={statusError(draft().statusInput) !== null}
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

            <SkeletonRows
                read={read}
                rowCount={3}
                rowHeight="h-24"
            />

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
                                    description="One row appears per dispatched serving request — the moment the gateway sends upstream. If traffic flows and nothing appears, check the api_logs section in the gateway's configuration file."
                                >
                                    {/* This door exists only for the unfiltered
                                        stream — a filtered answer that matches
                                        nothing states the filter instead — so
                                        the link carries no subject: there is
                                        no narrow to carry. */}
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
                        <div class="flex flex-col gap-3">
                            <Show when={loadMoreNotice()}>
                                {(failure) => (
                                    <Notice
                                        tone={failure().tone}
                                        title={failure().title}
                                        detail={failure().detail}
                                        fix={failure().fix}
                                    />
                                )}
                            </Show>
                            <div>
                                {/* The read cycle's wholesale replacement is
                                    the one refusal `loadMore` makes, and the
                                    control states it the way the shell's
                                    Refresh control states its own: disabled
                                    with the reason in its title, never a
                                    silent dead click. */}
                                <Button
                                    glyph="chev-right"
                                    isBusy={isLoadingMore()}
                                    isDisabled={read.isReading()}
                                    title={
                                        read.isReading()
                                            ? "A read cycle is replacing the stream — extend it once the cycle lands."
                                            : undefined
                                    }
                                    onClick={() => void loadMore()}
                                >
                                    {isLoadingMore() ? "Loading…" : "Load more"}
                                </Button>
                            </div>
                        </div>
                    </Show>
                </Show>
            </Show>
        </div>
    );
}

/**
 * The aggregates subject the stream's standing draft carries: its entity
 * narrow, when exactly one of provider, model, or key is named, as the
 * aggregates page's prefilled subject — so crossing to the aggregates page
 * from a stream whose form names an entity keeps asking about that entity
 * instead of silently totaling the whole gateway, applied or not. The narrow
 * is read off the wire filter [`buildUsageFilter`] spells — the one trusted
 * conversion from the form's strings to row ids — so this link's subject
 * cannot drift from the question the form holds. A draft naming no entity,
 * or more than one, carries no subject: a subject is one entity, and a
 * compound narrow belongs to the aggregates page's own filter, not to a
 * link. The window and every non-entity field stay behind — only the entity
 * subject crosses, the aggregates page's window semantics being its own.
 */
function streamAggregatesSubject(filter: UsageFilterDraft): UsageQuerySubject | undefined {
    const wire = buildUsageFilter(filter);

    // `buildUsageFilter` names a wire field with exactly one id — one draft
    // field, one wire value — so the list's first element is the id the walk
    // reads.
    return (
        soleSubject(
            namedSubjects(
                (dimension) => wire[USAGE_QUERY_SUBJECT_FIELDS[dimension].wireField]?.[0] ?? null,
            ),
        ) ?? undefined
    );
}

/** The badge behind one recorded outcome. */
function usageOutcomeBadge(outcome: ApiLogOutcome): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    const label = apiLogOutcomeLabel(outcome);

    // The failure tone reads the one shared set — the same one the Failures
    // filter narrows by and the overview's error figure counts — so a failure
    // outcome added there turns this badge with the other two views, never
    // against them.
    if (USAGE_OUTCOME_FAILURES.includes(outcome)) {
        return {
            tone: "bad",
            label,
        };
    }

    return {
        tone: outcome === "completed" ? "ok" : "warn",
        label,
    };
}

/**
 * One recorded request: when it started, how it ended, what it was, and
 * its numbers. Labels resolve through the indexed entity lists and open the
 * entity's page while it is listed; an id with no listing reads as its
 * deleted kind and stays plain.
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
                <UsageEntityLabel
                    text={model().text}
                    isListed={model().isListed}
                    href={hrefs.modelDetail(props.row.model_id)}
                />
                <span
                    aria-hidden="true"
                    class="text-ink-faint"
                >
                    ·
                </span>
                <UsageEntityLabel
                    text={provider().text}
                    isListed={provider().isListed}
                    href={hrefs.providerDetail(props.row.provider_id)}
                />
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
                    <UsageEntityLabel
                        text={apiKey().text}
                        isListed={apiKey().isListed}
                        href={hrefs.apiKeyDetail(props.row.api_key_id)}
                    />
                </span>
            </p>
            <p class="tnum mt-1 text-[12.5px] text-ink-faint">
                {props.row.http_status === null ? "—" : props.row.http_status}
                {` · ${formatDuration(props.row.duration)} · in ${formatTokenCount(props.row.input_tokens)} · out ${formatTokenCount(props.row.output_tokens)}${optionalTokenTail(
                    props.row.cached_tokens,
                    props.row.cache_write_tokens,
                    props.row.reasoning_tokens,
                )}`}
            </p>
        </li>
    );
}

/**
 * The optional token figures' tail — cached, cache write, reasoning — as the
 * row prints it when at least one of the three is a reported non-zero,
 * measured zeros and unreported dashes included verbatim; an empty string
 * when none is. An upstream that never reports the breakdown would
 * otherwise end every one of its rows in `· cached 0 · cache write 0 ·
 * reasoning 0`, printing measured zeros nothing measured. The suppression
 * gives up the per-row reading of a measured all-zero tail beside an
 * unreported one — the stream's standing description states that trade once,
 * rather than every row carrying figures that say nothing.
 */
function optionalTokenTail(
    cachedTokens: number | null,
    cacheWriteTokens: number | null,
    reasoningTokens: number | null,
): string {
    const isReportedNonZero = (value: number | null): boolean => value !== null && value !== 0;

    return isReportedNonZero(cachedTokens) ||
        isReportedNonZero(cacheWriteTokens) ||
        isReportedNonZero(reasoningTokens)
        ? ` · cached ${formatTokenCount(cachedTokens)} · cache write ${formatTokenCount(cacheWriteTokens)} · reasoning ${formatTokenCount(reasoningTokens)}`
        : "";
}

const DELETED_ENTITY_TITLE = "This row is no longer listed.";

/**
 * One entity label on a usage row: a door to the entity's own page while the
 * row is still listed, an inert caption naming its deleted kind when it is
 * not — a deleted id has no page to open.
 */
function UsageEntityLabel(props: {
    readonly text: string;
    readonly isListed: boolean;
    readonly href: string;
}): JSX.Element {
    return (
        <Show
            when={props.isListed}
            fallback={<span title={DELETED_ENTITY_TITLE}>{props.text}</span>}
        >
            <AppLink
                href={props.href}
                class="font-medium text-accent hover:text-accent-strong"
            >
                {props.text}
            </AppLink>
        </Show>
    );
}
