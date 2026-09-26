import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { QueryUsageResponse, UsageBucket } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { EntityUsageCard } from "/src/screens/usage-stats";
import type { UsageSubjectFilter } from "/src/usage-filter";
import { bodyText, linkTo, settle, stubManagementApi } from "./screen-test-support";

/** One day bucket: the last-good table a failed re-read must keep standing. */
const dayBucket: UsageBucket = {
    key: {
        day: "2026-09-01",
    },
    requests: 3,
    input_tokens: 120,
    output_tokens: 45,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
};

/** One counted day bucket on the given day. */
function dayBucketOn(day: string): UsageBucket {
    return {
        key: {
            day,
        },
        requests: 1,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

/** One held `usage:query` call: the ways the test ends it. */
type HeldQuery = {
    readonly answer: (result: QueryUsageResponse) => void;
    readonly refuse: (cause: unknown) => void;
};

/**
 * One `usage:query` call held back until the test ends it, rejecting with the
 * transport's abort failure the moment its cycle's signal fires — the same
 * shape a real in-flight fetch dies with.
 */
function heldQuery(
    signal: AbortSignal | undefined,
    held: Array<HeldQuery>,
): Promise<QueryUsageResponse> {
    return new Promise<QueryUsageResponse>((resolve, reject) => {
        held.push({
            answer: resolve,
            refuse: reject,
        });
        signal?.addEventListener(
            "abort",
            () => {
                reject(new DOMException("This read cycle was aborted.", "AbortError"));
            },
            {
                once: true,
            },
        );
    });
}

/** A management API whose query answers only when the test answers it. */
function heldQueryApi(holder: { readonly queries: Array<HeldQuery> }): ManagementApi {
    return stubManagementApi({
        queryUsage: (_request, signal) => heldQuery(signal, holder.queries),
    });
}

/** Answers the most recent held query — the newest cycle's call. */
function answerLastQuery(
    holder: {
        readonly queries: Array<HeldQuery>;
    },
    buckets: ReadonlyArray<UsageBucket>,
): void {
    const held = holder.queries.at(-1);

    if (held === undefined) {
        throw new Error("No held query to answer");
    }

    held.answer({
        buckets: [
            ...buckets,
        ],
    });
}

describe("EntityUsageCard read failures", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(api: ManagementApi): void {
        disposes.push(
            render(
                () => (
                    <EntityUsageCard
                        api={api}
                        filter={{
                            provider_id: 1,
                        }}
                        subjectKind="provider"
                    />
                ),
                document.body,
            ),
        );
    }

    it("keeps the last-good table beside a failed re-read's notice", async () => {
        const holder: {
            readonly queries: Array<HeldQuery>;
        } = {
            queries: [],
        };
        mount(heldQueryApi(holder));
        await settle();
        answerLastQuery(holder, [
            dayBucket,
        ]);
        await settle();

        expect(bodyText()).toContain("2026-09-01");

        // The refresh fails after a read has succeeded: the card keeps its
        // last-good totals and table beside the failure notice — the buckets
        // signal still holds them — and the empty state, the claim the
        // failure cannot prove, stays away.
        requestRefresh();
        await settle();
        holder.queries.at(-1)?.refuse(new Error("re-read refused"));
        await settle();

        expect(bodyText()).toContain("re-read refused");
        expect(bodyText()).toContain("2026-09-01");
        expect(bodyText()).not.toContain("No requests recorded for this provider yet.");
    });

    it("states the empty state only where a read succeeded and found nothing", async () => {
        const holder: {
            readonly queries: Array<HeldQuery>;
        } = {
            queries: [],
        };
        // A read that has never succeeded shows its diagnosis alone: no
        // table, and no empty state the failure cannot rule out.
        mount(heldQueryApi(holder));
        await settle();
        holder.queries.at(-1)?.refuse(new Error("first read refused"));
        await settle();

        expect(bodyText()).toContain("first read refused");
        expect(bodyText()).not.toContain("No requests recorded for this provider yet.");
        expect(bodyText()).not.toContain("2026-09-01");

        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;

        // A read that succeeded on an empty gateway is what states the
        // emptiness.
        mount(
            stubManagementApi({
                queryUsage: () =>
                    Promise.resolve({
                        buckets: [],
                    }),
            }),
        );
        await settle();

        expect(bodyText()).toContain("No requests recorded for this provider yet.");
        expect(bodyText()).not.toContain("first read refused");
    });
});

describe("EntityUsageCard per-day series", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** The usage table's row labels in rendered order — the displayed series. */
    function dayRows(): Array<string | null> {
        return Array.from(
            document.querySelectorAll<HTMLTableCellElement>("tbody th[scope=row]"),
        ).map((cell) => cell.textContent);
    }

    it("renders the days newest-first and states the UTC boundary under the table", async () => {
        disposes.push(
            render(
                () => (
                    <EntityUsageCard
                        api={stubManagementApi({
                            queryUsage: () =>
                                Promise.resolve({
                                    buckets: [
                                        dayBucketOn("2026-09-01"),
                                        dayBucketOn("2026-09-02"),
                                        dayBucketOn("2026-09-03"),
                                    ],
                                }),
                        })}
                        filter={{
                            provider_id: 1,
                        }}
                        subjectKind="provider"
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The gateway answers ascending by day; the card's series reads
        // newest-first, and the boundary the days split on is stated under
        // the table.
        expect(dayRows()).toEqual([
            "2026-09-03",
            "2026-09-02",
            "2026-09-01",
        ]);
        expect(bodyText()).toContain("Days are UTC — the gateway's own day boundary.");
    });
});

describe("EntityUsageCard doors", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("carries the subject through both doors — the stream's narrow and the aggregates preselection", async () => {
        // One case per subject dimension: each door fills the subject into
        // the route parameter that dimension's field names — the stream's
        // own parameter spelling — and nothing else rides along.
        const cases: ReadonlyArray<{
            readonly filter: UsageSubjectFilter;
            readonly subjectKind: string;
            readonly streamHref: string;
            readonly aggregatesHref: string;
        }> = [
            {
                filter: {
                    provider_id: 1,
                },
                subjectKind: "provider",
                streamHref: "/usage?provider=1",
                aggregatesHref: "/usage/query?provider=1",
            },
            {
                filter: {
                    model_id: 9,
                },
                subjectKind: "model",
                streamHref: "/usage?model=9",
                aggregatesHref: "/usage/query?model=9",
            },
            {
                filter: {
                    api_key_id: 7,
                },
                subjectKind: "api key",
                streamHref: "/usage?key=7",
                aggregatesHref: "/usage/query?key=7",
            },
        ];

        for (const testCase of cases) {
            disposes.push(
                render(
                    () => (
                        <EntityUsageCard
                            api={stubManagementApi({
                                queryUsage: () =>
                                    Promise.resolve({
                                        buckets: [],
                                    }),
                            })}
                            filter={testCase.filter}
                            subjectKind={testCase.subjectKind}
                        />
                    ),
                    document.body,
                ),
            );
            await settle();

            expect(linkTo(testCase.streamHref).textContent).toContain("View requests");
            expect(linkTo(testCase.aggregatesHref).textContent).toContain("Open aggregates");

            for (const dispose of disposes.reverse()) {
                dispose();
            }
            disposes.length = 0;
        }
    });
});
