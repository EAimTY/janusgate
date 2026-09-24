import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { formatTokenCount } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { UsageBucket, UsageFilter } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { ReadNotice } from "/src/screens/read-notice";
import { ButtonLink } from "/src/ui/controls";
import { Notice, Skeleton, Stat } from "/src/ui/feedback";
import { Card, CardHeader } from "/src/ui/layout";
import { createScreenRead, readOne } from "./read";
import { readUsageBuckets } from "./usage-read";

/** Which recorded rows the stats count: exactly one subject's. */
type UsageSubjectFilter =
    | {
          readonly provider_id: number;
      }
    | {
          readonly model_id: number;
      }
    | {
          readonly api_key_id: number;
      };

/** The subject as the shared `usage` filter the query route carries: the one id, listed. */
function subjectFilter(filter: UsageSubjectFilter): UsageFilter {
    if ("provider_id" in filter) {
        return {
            provider_id: [
                filter.provider_id,
            ],
        };
    }

    if ("model_id" in filter) {
        return {
            model_id: [
                filter.model_id,
            ],
        };
    }

    return {
        api_key_id: [
            filter.api_key_id,
        ],
    };
}

/**
 * One subject's request accounting as aggregates: totals plus one row per
 * day, newest first. The gateway counts rows and sums tokens; names never
 * ride along, so this card shows numbers only. A failed read states its
 * diagnosis in place, like every other card that reads.
 */
export function EntityUsageCard(props: {
    readonly api: ManagementApi;
    readonly filter: UsageSubjectFilter;
    readonly subjectKind: string;
}): JSX.Element {
    const [buckets, setBuckets] = createSignal<ReadonlyArray<UsageBucket>>([]);

    const read = createScreenRead({
        reReadOn: () => props.filter,
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

    const newestFirst = createMemo(() =>
        [
            ...buckets(),
        ].reverse(),
    );

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

    return (
        <Card>
            <CardHeader
                title="Usage"
                description="Requests and tokens per day — numbers only, never content."
            >
                <ButtonLink href={hrefs.usageQuery()}>Open aggregates</ButtonLink>
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

                <Show when={read.hasRead() && read.notice() === null}>
                    <Show
                        when={buckets().length > 0}
                        fallback={
                            <Notice
                                tone="info"
                                title={`No requests recorded for this ${props.subjectKind} yet.`}
                                detail="One row appears per dispatched serving request — the moment the gateway sends upstream."
                            />
                        }
                    >
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
                                                    {bucket.key.day ?? ""}
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
                    </Show>
                </Show>
            </div>
        </Card>
    );
}
