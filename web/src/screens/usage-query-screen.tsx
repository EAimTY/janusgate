import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { formatTokenCount, parseDateTimeInputToMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    QueryUsageRequest,
    UsageBucket,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { USAGE_GROUP_BY_OPTIONS } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { Button, Switch, TextField } from "/src/ui/controls";
import { Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { UsageFilterControls } from "/src/ui/usage-filter-controls";
import {
    buildUsageFilter,
    EMPTY_USAGE_FILTER_FIELDS,
    type UsageFilterFields,
} from "/src/usage-filter";
import { ReadCallFailure, readOne, type ScreenNotice } from "./read";
import { indexUsageEntities, type UsageEntityIndex, usageDimensionLabel } from "./usage-labels";
import { readUsageBuckets, readUsageEntities, type UsageEntities } from "./usage-read";

/**
 * Request accounting as aggregates: one bucket per distinct combination of
 * the requested dimensions inside the filter. The dimensions are a set of
 * switches — none selected totals every row — and the filter narrows what the
 * gateway counts. Bucket keys are composite: one part per requested dimension,
 * resolved through the entity lists and the vocabularies the screen holds.
 */
export function UsageQueryScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [entities, setEntities] = createSignal<UsageEntities>({
        providers: [],
        models: [],
        apiKeys: [],
    });
    const [groupBy, setGroupBy] = createSignal<ReadonlyArray<UsageGroupBy>>([
        "day",
    ]);
    const [fromInput, setFromInput] = createSignal("");
    const [toInput, setToInput] = createSignal("");
    const [fields, setFields] = createSignal<UsageFilterFields>(EMPTY_USAGE_FILTER_FIELDS);
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

    /** Runs the aggregation query, answering whether it produced buckets. */
    const runQuery = async (signal: AbortSignal): Promise<boolean> => {
        try {
            const request = queryRequest();

            const result = await readOne(signal, readUsageBuckets(props.api, request));

            setFailure(null);
            setBuckets(result.buckets);
            setResponseDimensions(request.group_by);
            return true;
        } catch (error) {
            if (error instanceof ReadCallFailure) {
                setFailure(error.notice);
                return false;
            }

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
            setEntities(await readOne(call.signal, readUsageEntities(props.api)));

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

        if (isQuerying()) {
            return;
        }

        await runCycle(true);
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
            />

            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

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
                            <For each={USAGE_GROUP_BY_OPTIONS}>
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
                    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                        <TextField
                            label="From"
                            type="datetime-local"
                            value={fromInput()}
                            onInput={setFromInput}
                            isDisabled={isQuerying()}
                            hint="Inclusive, in this browser's zone. Blank sets no bound."
                        />
                        <TextField
                            label="To"
                            type="datetime-local"
                            value={toInput()}
                            onInput={setToInput}
                            isDisabled={isQuerying()}
                            hint="Exclusive, in this browser's zone. Blank sets no bound."
                        />
                        <UsageFilterControls
                            fields={fields()}
                            providers={entities().providers}
                            models={entities().models}
                            apiKeys={entities().apiKeys}
                            isDisabled={isQuerying()}
                            onEdit={(edits) =>
                                setFields((current) => ({
                                    ...current,
                                    ...edits,
                                }))
                            }
                        />
                    </div>
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={isQuerying()}
                        >
                            {isQuerying() ? "Querying…" : "Run query"}
                        </Button>
                    </div>
                </form>
            </Card>

            <Show when={isReading() && !hasRead() && failure() === null}>
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={buckets().length > 0}
                    fallback={
                        <Notice
                            tone="info"
                            title="No rows in this window."
                            detail="Nothing recorded matches these filters — widen the window or clear a filter."
                        />
                    }
                >
                    <Card>
                        <div class="overflow-x-auto">
                            <table class="tnum w-full min-w-[640px] text-left text-[13px]">
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
                                    <For each={buckets()}>
                                        {(bucket) => (
                                            <tr class="border-t border-rule">
                                                <BucketKeyCell
                                                    bucket={bucket}
                                                    dimensions={responseDimensions()}
                                                    index={index()}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(bucket.requests)}
                                                />
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
                                                    value={formatTokenCount(
                                                        bucket.cache_write_tokens,
                                                    )}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(
                                                        bucket.reasoning_tokens,
                                                    )}
                                                />
                                            </tr>
                                        )}
                                    </For>
                                </tbody>
                            </table>
                        </div>
                        <p class="mt-3 text-[12.5px] text-ink-faint">
                            Token totals skip rows where the upstream reported no usage — an absent
                            breakdown never inflates or deflates a total.
                        </p>
                    </Card>
                </Show>
            </Show>
        </div>
    );
}

function BucketHeader(props: {
    readonly label: string;
    readonly isNumeric?: boolean;
}): JSX.Element {
    return (
        <th
            scope="col"
            class={`px-3 py-2 text-[12.5px] font-medium ${props.isNumeric === true ? "text-right" : ""}`}
        >
            {props.label}
        </th>
    );
}

function BucketCell(props: { readonly value: string }): JSX.Element {
    return <td class="px-3 py-2 text-right">{props.value}</td>;
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
