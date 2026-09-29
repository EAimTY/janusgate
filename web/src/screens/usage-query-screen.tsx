import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import {
    dateTimeInputValue,
    formatTokenCount,
    nowMicros,
    parseDateTimeInputToMicros,
} from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    QueryUsageRequest,
    UsageBucket,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { USAGE_GROUP_BY } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, type UsageQuerySubject } from "/src/router";
import { Button, ButtonLink, Switch, TextField } from "/src/ui/controls";
import { Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { UsageFilterControls } from "/src/ui/usage-filter-controls";
import {
    buildUsageFilter,
    EMPTY_USAGE_FILTER_FIELDS,
    hasUsageFilterFields,
    statusError,
    streamDraftOf,
    subjectFilterFields,
    USAGE_WINDOW_OPTIONS,
    type UsageFilterFields,
    type UsageWindowChoice,
    windowBoundMicros,
} from "/src/usage-filter";
import { isSkeletonWindowOpen, ReadCallFailure, readOne, type ScreenNotice } from "./read";
import { StatedNotice } from "./read-notice";
import {
    daysNewestFirst,
    indexUsageEntities,
    type UsageEntityIndex,
    usageDimensionLabel,
} from "./usage-labels";
import { EMPTY_USAGE_ENTITIES, readUsageBuckets, readUsageEntities } from "./usage-read";
import { BucketCell, BucketHeader, UsageDaysUtcNote, UsageTable } from "./usage-table";

/**
 * The grouping the address arrives with: the default day dimension, plus the
 * carried subject's own, so its buckets split the subject out beside every
 * day.
 */
function arrivalGroupBy(subject: UsageQuerySubject | null): ReadonlyArray<UsageGroupBy> {
    return subject === null
        ? [
              "day",
          ]
        : [
              "day",
              subject.dimension,
          ];
}

/**
 * The raw verdict on one hand-typed bound: any non-blank value must be a
 * complete date and time. The submit gate reads it ungated, and each field's
 * inline error reads it once that field was typed in.
 */
function boundError(label: string, value: string): string | null {
    if (value.trim() === "" || parseDateTimeInputToMicros(value) !== null) {
        return null;
    }

    return `${label} must be a complete date and time, like 2026-09-23T10:00 — or blank for no bound.`;
}

/**
 * What a cycle states when it reaches the query while a field stands marked —
 * the half-run made observable. Only the shell's Refresh can produce the
 * state (the submit is guarded at its button, and every form-driven cycle
 * starts from fields the guards released), so the notice says what the cycle
 * did instead of naming the way it was started, and points at the marked
 * field's own inline error as the thing to fix.
 */
const HELD_QUERY_NOTICE: ScreenNotice = {
    tone: "warn",
    title: "The query did not run — a filter value is invalid.",
    detail: "The answer below is still the previous query's — the field showing an inline error names the problem.",
    fix: "Fix that field, then run the query again.",
};

/**
 * Request accounting as aggregates: one bucket per distinct combination of
 * the requested dimensions inside the filter. The dimensions are a set of
 * switches — none selected totals every row — and the filter narrows what the
 * gateway counts. Bucket keys are composite: one part per requested dimension,
 * resolved through the entity lists and the vocabularies the screen holds.
 * The address may carry a subject — one entity dimension and its row — which
 * arrives through the route's payload: the route Match keys this screen, so
 * every address is a fresh mount that opens with that dimension grouped
 * beside the day and its id filled into the filter field. The header's
 * *View requests* door carries the standing filter fields into the stream's
 * draft — the crossing back that opens a failing combination's recorded
 * requests without a filter rebuild — while the From and To bounds stay
 * behind, the stream's own window row governing time there; while a
 * hand-typed bound stands set, the door states that trade visibly beside its
 * label.
 */
export function UsageQueryScreen(props: {
    readonly api: ManagementApi;
    readonly subject: UsageQuerySubject | null;
}): JSX.Element {
    const [entities, setEntities] = createSignal(EMPTY_USAGE_ENTITIES);
    // The entity listings' first successful read flips this, permanently for
    // the mount: only a read listing states a row's absence, so the filter
    // fields carry the `(no longer listed)` verdict only past it. The write
    // sits beside the listings' install — which a superseded cycle never
    // reaches and a failed read never survives — so the flag never retracts.
    const [hasEntityListings, setHasEntityListings] = createSignal(false);
    const [groupBy, setGroupBy] = createSignal<ReadonlyArray<UsageGroupBy>>(
        arrivalGroupBy(props.subject),
    );
    const [fromInput, setFromInput] = createSignal("");
    const [toInput, setToInput] = createSignal("");
    // The bounds' errors open behind input to their own field — a pristine
    // form is not an error, the hints state the format quietly (the app's
    // lazy-error convention) — and an edit that completes a bound retracts
    // its error without another submit.
    const [hasFromInput, setHasFromInput] = createSignal(false);
    const [hasToInput, setHasToInput] = createSignal(false);
    // The arrival subject prefills the shared filter fields through the one
    // subject→fields derivation — the same one the stream card's *View
    // requests* door reads — so the first query already narrows to the
    // subject the aggregates link carried.
    const [fields, setFields] = createSignal<UsageFilterFields>(subjectFilterFields(props.subject));
    const [buckets, setBuckets] = createSignal<ReadonlyArray<UsageBucket>>([]);
    // The dimensions the displayed buckets were grouped by — the request's own
    // set, not the live switches: a key carries exactly its request's
    // dimensions, so toggling a switch must not re-render the previous
    // answer's keys against a question it never answered.
    const [responseDimensions, setResponseDimensions] = createSignal<ReadonlyArray<UsageGroupBy>>(
        [],
    );
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [isQuerying, setIsQuerying] = createSignal(false);
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);

    let controller: AbortController | null = null;

    // Indexed once per entities change, so every bucket key's id lookups are O(1).
    const index = createMemo(() => indexUsageEntities(entities()));

    /**
     * The buckets as the table displays them: while the standing grouping
     * includes day, the shared `daysNewestFirst` orders the series
     * newest-day-first — the same direction the detail usage card renders its
     * per-day series, so arriving through its aggregates door never flips the
     * series; any other grouping keeps the gateway's order untouched.
     */
    const displayedBuckets = createMemo(() => {
        if (!responseDimensions().includes("day")) {
            return buckets();
        }

        return daysNewestFirst(buckets());
    });

    /** Adds the dimension to the requested set, or removes it from it. */
    const toggleDimension = (dimension: UsageGroupBy, isOn: boolean): void => {
        setGroupBy((current) =>
            isOn
                ? current.includes(dimension)
                    ? current
                    : [
                          ...current,
                          dimension,
                      ]
                : current.filter((candidate) => candidate !== dimension),
        );
    };

    const queryRequest = (): QueryUsageRequest => ({
        group_by: [
            ...groupBy(),
        ],
        filter: buildUsageFilter(fields(), {
            from: parseDateTimeInputToMicros(fromInput()),
            to: parseDateTimeInputToMicros(toInput()),
        }),
    });

    /** The From bound's inline error, open only once the field was typed in. */
    const fromError = (): string | null =>
        hasFromInput() ? boundError("From", fromInput()) : null;

    /** The To bound's inline error, open only once the field was typed in. */
    const toError = (): string | null => (hasToInput() ? boundError("To", toInput()) : null);

    /**
     * Whether every hand-typed value — both bounds and the status list —
     * parses: the query never runs while one does not. The bounds are judged
     * raw, not through their gated inline errors: a field that was never
     * typed into holds no error text, but a malformed value holds the submit
     * all the same.
     */
    const isFormValid = (): boolean =>
        boundError("From", fromInput()) === null &&
        boundError("To", toInput()) === null &&
        statusError(fields().statusInput) === null;

    /**
     * Fills both bounds with one rolling window — the stream page's own
     * window semantics, resolved once here against the current moment through
     * the shared window arithmetic: the lower bound one window back, the
     * upper left open. `all` empties both fields, which is the form's own
     * spelling of no bounds. The fields stay hand-editable; the fill is not
     * input, so it opens no inline error.
     */
    const applyWindow = (window: UsageWindowChoice): void => {
        const from = windowBoundMicros(window, nowMicros());

        setFromInput(from === null ? "" : dateTimeInputValue(new Date(from / 1000)));
        setToInput("");
    };

    /** Runs the aggregation query, answering whether it produced buckets. */
    const runQuery = async (signal: AbortSignal): Promise<boolean> => {
        // A malformed typed value is named at its field: running without it
        // would silently widen the question, so the query waits until it is
        // fixed. A cycle that arrives here anyway — only the shell's refresh
        // can, the submit being guarded at its button — states the hold on
        // the failure channel instead of half-running silently; the next
        // successful cycle clears it.
        if (!isFormValid()) {
            setFailure(HELD_QUERY_NOTICE);
            return false;
        }

        try {
            const request = queryRequest();

            const result = await readOne(signal, readUsageBuckets(props.api, request));

            if (signal.aborted) {
                // Superseded after the read resolved but before this
                // continuation ran: the cycle is dead either way, and the
                // newer cycle owns the screen — nothing installs, not even
                // the standing failure's lift.
                return false;
            }

            setFailure(null);
            setBuckets(result.buckets);
            setResponseDimensions(request.group_by);
            return true;
        } catch (error) {
            // A failed read propagates its `ReadCallFailure` to the cycle's
            // one catch — the same treatment the entity listings' failure
            // already gets — instead of stating it here; an abort dies here
            // quietly, the cycle that superseded it owning the screen.
            if (!signal.aborted) {
                throw error;
            }

            return false;
        }
    };

    /**
     * Runs one full cycle: the entity listings and the aggregation query, in
     * that order, under one controller. This screen stays outside the shared
     * read harness because the query is also a form submission: the submit
     * and the screen read run this same cycle and share one controller, so a
     * submit stops an in-flight read rather than racing it — and reads the
     * entity lists itself, so superseding a read never leaves the labels
     * resolving against lists the screen never loaded.
     */
    const runCycle = async (isSubmit: boolean): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);
        setIsQuerying(isSubmit);

        try {
            const listings = await readOne(call.signal, readUsageEntities(props.api));

            if (call.signal.aborted) {
                // Superseded after the listings resolved but before this
                // continuation ran: the cycle is dead either way, and the
                // newer cycle owns the screen — nothing installs, and the
                // query below is never asked on the dead signal.
                return;
            }

            setEntities(listings);
            setHasEntityListings(true);

            if (await runQuery(call.signal)) {
                setHasRead(true);
                reportRefresh();
            }
        } catch (error) {
            if (error instanceof ReadCallFailure) {
                setFailure(error.notice);
            } else if (!call.signal.aborted) {
                throw error;
            }
        } finally {
            // A superseded cycle's abort must not drop the loading states the
            // cycle that superseded it raised (as the read harness does, its
            // abort path installs nothing): only the cycle still holding the
            // controller resets them.
            if (!call.signal.aborted) {
                setIsReading(false);
                setIsQuerying(false);
            }
        }
    };

    const submit = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();

        if (isQuerying() || !isFormValid()) {
            return;
        }

        await runCycle(true);
    };

    /** Whether either hand-typed bound holds a value — the View requests door's date trade. */
    const hasBoundInput = (): boolean => fromInput().trim() !== "" || toInput().trim() !== "";

    /** Whether any filter field or bound holds a value — the Clear filters button's visibility. */
    const isAnyFilterSet = (): boolean => hasUsageFilterFields(fields()) || hasBoundInput();

    /**
     * Resets the filter fields and the bounds — the dimension switches stay,
     * so a carried subject's grouping survives — and re-runs the cycle, the
     * form-driven path the submit button is: the reset fields are whole by
     * construction, so the cycle's query can never arrive held.
     */
    const clearFilters = (): void => {
        setFields(EMPTY_USAGE_FILTER_FIELDS);
        setFromInput("");
        setToInput("");
        void runCycle(true);
    };

    createEffect(
        () => refreshRequest(),
        () => {
            void runCycle(false);

            return () => controller?.abort();
        },
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Usage aggregates"
                description="One bucket per dimension combination inside the filter — counts and token sums, never content."
                backHref={hrefs.usage()}
                backLabel="Usage"
            >
                <ButtonLink href={hrefs.usage(streamDraftOf(fields()))}>
                    View requests
                    <Show when={hasBoundInput()}>
                        {/* The stated trade, muted but visible — the same
                            sentence every crossing door states while what it
                            drops stands set. */}
                        <span class="text-ink-faint">{" — From and To do not apply there."}</span>
                    </Show>
                </ButtonLink>
            </PageHeader>

            <StatedNotice notice={failure} />

            <Card>
                <CardHeader title="Question" />
                <form
                    novalidate={true}
                    class="mt-4 flex flex-col gap-4"
                    onSubmit={(event) => void submit(event)}
                >
                    <div>
                        <p class="text-[13px] font-medium text-ink">Group by</p>
                        <div class="mt-2 grid gap-x-6 gap-y-3 sm:grid-cols-2 lg:grid-cols-4">
                            <For each={USAGE_GROUP_BY.options}>
                                {(option) => (
                                    <Switch
                                        label={option.label}
                                        isOn={groupBy().includes(option.value)}
                                        isDisabled={isQuerying()}
                                        onToggle={(isOn) => toggleDimension(option.value, isOn)}
                                    />
                                )}
                            </For>
                        </div>
                        <p class="mt-2 max-w-[70ch] text-[13px] text-ink-faint">
                            Any dimension set works — none selected totals every matching row.
                        </p>
                    </div>
                    <div>
                        <p class="text-[13px] font-medium text-ink">Window</p>
                        <div class="mt-2 flex flex-wrap gap-2">
                            <For each={USAGE_WINDOW_OPTIONS}>
                                {(option) => (
                                    <Button
                                        tone="ghost"
                                        isDisabled={isQuerying()}
                                        onClick={() => applyWindow(option.value)}
                                    >
                                        {option.label}
                                    </Button>
                                )}
                            </For>
                        </div>
                        <p class="mt-2 max-w-[70ch] text-[13px] text-ink-faint">
                            Fills the bounds below — from one window back, to left open — resolved
                            now; the fields stay hand-editable.
                        </p>
                    </div>
                    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                        <TextField
                            label="From"
                            value={fromInput()}
                            onInput={(value) => {
                                setHasFromInput(true);
                                setFromInput(value);
                            }}
                            isDisabled={isQuerying()}
                            error={fromError()}
                            hint="Inclusive, in this browser's zone — like 2026-09-23T10:00. Blank sets no bound."
                        />
                        <TextField
                            label="To"
                            value={toInput()}
                            onInput={(value) => {
                                setHasToInput(true);
                                setToInput(value);
                            }}
                            isDisabled={isQuerying()}
                            error={toError()}
                            hint="Exclusive, in this browser's zone — like 2026-09-23T10:00. Blank sets no bound."
                        />
                        <UsageFilterControls
                            fields={fields()}
                            providers={entities().providers}
                            models={entities().models}
                            apiKeys={entities().apiKeys}
                            hasEntityListings={hasEntityListings()}
                            statusError={statusError(fields().statusInput)}
                            isDisabled={isQuerying()}
                            onEdit={(edits) =>
                                setFields((current) => ({
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
                            isBusy={isQuerying()}
                            isDisabled={!isFormValid()}
                        >
                            {isQuerying() ? "Querying…" : "Run query"}
                        </Button>
                        <Show when={isAnyFilterSet()}>
                            <Button
                                tone="ghost"
                                isDisabled={isQuerying()}
                                onClick={clearFilters}
                            >
                                Clear filters
                            </Button>
                        </Show>
                    </div>
                </form>
            </Card>

            <Show when={isSkeletonWindowOpen(isReading(), hasRead(), failure())}>
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={buckets().length > 0}
                    fallback={
                        // One behavior with two wordings, split exactly on the
                        // Clear filters action's visibility: a set filter makes
                        // the emptiness the filter's — the notice names the
                        // action that drops it — while a bare question matching
                        // nothing is the record itself, with no action on the
                        // page to point at.
                        <Show
                            when={isAnyFilterSet()}
                            fallback={
                                <Notice
                                    tone="info"
                                    title="Nothing recorded in this window."
                                    detail="No filter or bound narrows the question, so an empty answer is the gateway's whole record — it holds no usage yet."
                                />
                            }
                        >
                            <Notice
                                tone="info"
                                title="No rows in this window."
                                detail="Nothing recorded matches these filters — widen the bounds, drop a filter, or query without one. Clear filters sits in the form above."
                            />
                        </Show>
                    }
                >
                    <Card>
                        <UsageTable minWidthClass="min-w-[640px]">
                            <thead>
                                <tr class="text-ink-dim">
                                    <BucketHeader label="Bucket" />
                                    <BucketHeader
                                        label="Requests"
                                        isNumeric
                                    />
                                    <BucketHeader
                                        label="Input"
                                        isNumeric
                                    />
                                    <BucketHeader
                                        label="Output"
                                        isNumeric
                                    />
                                    <BucketHeader
                                        label="Cached"
                                        isNumeric
                                    />
                                    <BucketHeader
                                        label="Cache write"
                                        isNumeric
                                    />
                                    <BucketHeader
                                        label="Reasoning"
                                        isNumeric
                                    />
                                </tr>
                            </thead>
                            <tbody>
                                <For each={displayedBuckets()}>
                                    {(bucket) => (
                                        <tr class="border-t border-rule">
                                            <BucketKeyCell
                                                bucket={bucket}
                                                dimensions={responseDimensions()}
                                                index={index()}
                                            />
                                            <BucketCell value={formatTokenCount(bucket.requests)} />
                                            <BucketCell
                                                value={formatTokenCount(bucket.input_tokens)}
                                            />
                                            <BucketCell
                                                value={formatTokenCount(bucket.output_tokens)}
                                            />
                                            <BucketCell
                                                value={formatTokenCount(bucket.cached_tokens)}
                                            />
                                            <BucketCell
                                                value={formatTokenCount(bucket.cache_write_tokens)}
                                            />
                                            <BucketCell
                                                value={formatTokenCount(bucket.reasoning_tokens)}
                                            />
                                        </tr>
                                    )}
                                </For>
                            </tbody>
                        </UsageTable>
                        <p class="mt-3 text-[12.5px] text-ink-faint">
                            Token totals skip rows where the upstream reported no usage — an absent
                            breakdown never inflates or deflates a total.
                        </p>
                        <Show when={responseDimensions().includes("day")}>
                            <UsageDaysUtcNote class="mt-1" />
                        </Show>
                    </Card>
                </Show>
            </Show>
        </div>
    );
}

/**
 * A bucket's composite key in the words the operator knows: one part per
 * requested dimension, in request order — ids resolved through the indexed
 * entity lists, the enums through their vocabularies. The total bucket's
 * empty key reads as "Total".
 */
function BucketKeyCell(props: {
    readonly bucket: UsageBucket;
    readonly dimensions: ReadonlyArray<UsageGroupBy>;
    readonly index: UsageEntityIndex;
}): JSX.Element {
    const label = (): string =>
        props.dimensions.length === 0
            ? "Total"
            : props.dimensions
                  .map((dimension) => usageDimensionLabel(dimension, props.bucket.key, props.index))
                  .join(" · ");

    return (
        <th
            scope="row"
            class="max-w-64 truncate px-3 py-2 font-medium"
            title={label()}
        >
            {label()}
        </th>
    );
}
