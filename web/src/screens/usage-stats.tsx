import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { formatTokenCount } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { UsageBucket, UsageFilter } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { Notice, Skeleton, Stat } from "/src/ui/feedback";
import { Card, CardHeader } from "/src/ui/layout";
import {
    EMPTY_USAGE_FILTER_DRAFT,
    USAGE_QUERY_SUBJECT_FIELDS,
    type UsageFilterDraft,
    type UsageQuerySubject,
    type UsageSubjectFilter,
    usageQuerySubjectOfFilter,
} from "/src/usage-filter";
import { createScreenRead, readOne } from "./read";
import { ReadNotice } from "./read-notice";
import { bucketKeyField, daysNewestFirst } from "./usage-labels";
import { readUsageBuckets } from "./usage-read";

/**
 * The subject as the shared `usage` filter the query route carries: the one
 * id, listed under the dimension table's wire field for the dimension the
 * subject read names. The values ride as the subject slice of the filter —
 * the table's fields optional, exactly one set — and `UsageFilter` leaves
 * every field the subject never names absent.
 */
function subjectFilter(filter: UsageSubjectFilter): UsageFilter {
    const subject = usageQuerySubjectOfFilter(filter);
    const wireField = USAGE_QUERY_SUBJECT_FIELDS[subject.dimension].wireField;
    const values: Partial<Record<typeof wireField, number[]>> = {
        [wireField]: [
            subject.id,
        ],
    };

    return values;
}

/**
 * The stream draft one subject narrows: the unfiltered listing with the
 * subject's id alone filled into its dimension's draft field, the field name
 * read from the shared table — the same pairing the stream route reads back —
 * so the card's two doors can never spell one entity differently. The window
 * and every other field stay at their default: the stream's own semantics
 * are its own.
 */
function subjectStreamDraft(subject: UsageQuerySubject): UsageFilterDraft {
    const field = USAGE_QUERY_SUBJECT_FIELDS[subject.dimension].draftField;
    const narrow: Partial<Record<typeof field, string>> = {
        [field]: String(subject.id),
    };

    return {
        ...EMPTY_USAGE_FILTER_DRAFT,
        ...narrow,
    };
}

/**
 * One subject's request accounting as aggregates: totals plus one row per
 * day, newest first. The header carries the card's two doors — *Open
 * aggregates* onto the aggregates page with the subject preselected, and
 * *View requests* onto the stream narrowed to the subject alone, the reverse
 * crossing of the stream header's own aggregates door. The gateway counts
 * rows and sums tokens; names never ride along, so this card shows numbers
 * only. A failed read states its
 * diagnosis in place, like every other card that reads, and a failed re-read
 * keeps the last-good totals and table standing beside it; the no-requests
 * empty state — the claim a failure cannot prove — stands only where a read
 * succeeded and found nothing.
 *
 * The buckets arrive through `readUsageBuckets`, which proved inside the read
 * cycle that every key carries the `day` its request asked for; the
 * `bucketKeyField` read in the table is the fatal assert on that proven
 * invariant, never the guard that would discover a gateway/bindings skew.
 */
export function EntityUsageCard(props: {
    readonly api: ManagementApi;
    readonly filter: UsageSubjectFilter;
    readonly subjectKind: string;
}): JSX.Element {
    const [buckets, setBuckets] = createSignal<ReadonlyArray<UsageBucket>>([]);

    // The subject is fixed for the card's whole life — the owning screen
    // remounts per subject — so the read follows the refresh pulse alone.
    const read = createScreenRead({
        load: async (signal) => {
            const result = await readOne(
                signal,
                readUsageBuckets(props.api, {
                    group_by: [
                        "day",
                    ],
                    filter: subjectFilter(props.filter),
                }),
            );

            setBuckets(result.buckets);
        },
    });

    // The per-day series as every usage display shows it: the shared
    // `daysNewestFirst` orders it newest-day-first — the same direction the
    // aggregates screen's table reads.
    const newestFirst = createMemo(() => daysNewestFirst(buckets()));

    const totals = createMemo(() => {
        let requests = 0;
        let input = 0;
        let output = 0;

        for (const bucket of buckets()) {
            requests += bucket.requests;
            input += bucket.input_tokens;
            output += bucket.output_tokens;
        }

        return {
            requests,
            input,
            output,
        };
    });

    // The subject is fixed for the card's whole life — the owning screen
    // remounts per subject — so both doors' addresses read it once here.
    const subject = usageQuerySubjectOfFilter(props.filter);

    return (
        <Card>
            <CardHeader
                title="Usage"
                description="Requests and tokens per day — numbers only, never content."
            >
                <ButtonLink href={hrefs.usageQuery(subject)}>Open aggregates</ButtonLink>
                <ButtonLink href={hrefs.usage(subjectStreamDraft(subject))}>
                    View requests
                </ButtonLink>
            </CardHeader>

            <div class="mt-4">
                <ReadNotice read={read} />

                <Show when={read.isFirstRead()}>
                    <div class="grid gap-3 sm:grid-cols-3">
                        <Skeleton class="h-16 w-full" />
                        <Skeleton class="h-16 w-full" />
                        <Skeleton class="h-16 w-full" />
                    </div>
                </Show>

                {/* The last-good answer survives a failed re-read: the
                    totals and table stand beside the failure notice, while
                    the empty state is the one piece the notice withholds —
                    a failed read cannot prove the subject recorded nothing. */}
                <Show when={read.hasRead()}>
                    <Show when={buckets().length > 0}>
                        <div class="grid gap-3 sm:grid-cols-3">
                            <Stat
                                value={formatTokenCount(totals().requests)}
                                label="Requests"
                                hint="Dispatched, all days"
                            />
                            <Stat
                                value={formatTokenCount(totals().input)}
                                label="Input tokens"
                                hint="Summed, usage reported"
                            />
                            <Stat
                                value={formatTokenCount(totals().output)}
                                label="Output tokens"
                                hint="Summed, usage reported"
                            />
                        </div>

                        <div class="mt-4 overflow-x-auto">
                            <table class="tnum w-full min-w-[480px] text-left text-[13px]">
                                <thead>
                                    <tr class="text-ink-dim">
                                        <th
                                            scope="col"
                                            class="px-3 py-2 text-[12.5px] font-medium"
                                        >
                                            Day
                                        </th>
                                        <th
                                            scope="col"
                                            class="px-3 py-2 text-right text-[12.5px] font-medium"
                                        >
                                            Requests
                                        </th>
                                        <th
                                            scope="col"
                                            class="px-3 py-2 text-right text-[12.5px] font-medium"
                                        >
                                            Input
                                        </th>
                                        <th
                                            scope="col"
                                            class="px-3 py-2 text-right text-[12.5px] font-medium"
                                        >
                                            Output
                                        </th>
                                    </tr>
                                </thead>
                                <tbody>
                                    <For each={newestFirst()}>
                                        {(bucket) => (
                                            <tr class="border-t border-rule">
                                                <th
                                                    scope="row"
                                                    class="px-3 py-2 font-medium"
                                                >
                                                    {bucketKeyField(bucket.key.day, "day")}
                                                </th>
                                                <td class="px-3 py-2 text-right">
                                                    {formatTokenCount(bucket.requests)}
                                                </td>
                                                <td class="px-3 py-2 text-right">
                                                    {formatTokenCount(bucket.input_tokens)}
                                                </td>
                                                <td class="px-3 py-2 text-right">
                                                    {formatTokenCount(bucket.output_tokens)}
                                                </td>
                                            </tr>
                                        )}
                                    </For>
                                </tbody>
                            </table>
                        </div>

                        <p class="mt-3 text-[12.5px] text-ink-faint">
                            Days are UTC — the gateway's own day boundary.
                        </p>
                    </Show>

                    <Show when={read.notice() === null && buckets().length === 0}>
                        <Notice
                            tone="info"
                            title={`No requests recorded for this ${props.subjectKind} yet.`}
                            detail="One row appears per dispatched serving request — the moment the gateway sends upstream."
                        />
                    </Show>
                </Show>
            </div>
        </Card>
    );
}
