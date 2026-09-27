import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { DAY_MICROS, nowMicros, parseDateTimeInputToMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ListApiKeysResponse,
    ListModelsResponse,
    ListProvidersResponse,
    QueryUsageRequest,
    UsageBucket,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import type { UsageQuerySubject } from "/src/router";
import { UsageQueryScreen } from "/src/screens/usage-query-screen";
import {
    answerAt,
    answerLast,
    apiKeyRow,
    buttonWithText,
    dayBucket,
    type HeldCall,
    heldCall,
    labeledInput,
    labeledSelect,
    linkTo,
    modelRow,
    providerRow,
    settle,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "./screen-test-support";

/** The superseded submit's own answer: the canonical provider row under its pre-refresh name. */
const staleProviderRow = providerRow({
    name: "stale-submit",
});

/** One bucket grouped by provider 1 on the 1st — its label resolves through the entity lists. */
const providerBucket = dayBucket({
    key: {
        day: "2026-09-01",
        provider_id: 1,
    },
});

/** The three entity listings one read cycle holds back, keyed by their routes. */
type HeldListings = {
    readonly providers: Array<HeldCall<ListProvidersResponse>>;
    readonly models: Array<HeldCall<ListModelsResponse>>;
    readonly apiKeys: Array<HeldCall<ListApiKeysResponse>>;
};

/**
 * A management API whose entity listings answer only when the test answers
 * them (so a read cycle stays in flight) and whose query answers from a
 * fixture. Every other route rejects, so an unexpected call fails the test
 * loudly instead of hanging it.
 */
function screenApi(
    holder: HeldListings,
    onQueryUsage: () => void,
    buckets: ReadonlyArray<UsageBucket> = [
        dayBucket(),
    ],
): ManagementApi {
    return stubManagementApi({
        listProviders: (signal) => heldCall(signal, holder.providers),
        listModels: (_request, signal) => heldCall(signal, holder.models),
        listApiKeys: (signal) => heldCall(signal, holder.apiKeys),
        queryUsage: () => {
            onQueryUsage();

            return Promise.resolve({
                buckets: [
                    ...buckets,
                ],
            });
        },
    });
}

/** The first-read placeholder: present exactly while the screen owes its first answers. */
function skeleton(): Element | null {
    return document.querySelector(".wash");
}

/** Sets one select the way a browser delivers a choice. */
function chooseSelect(label: string, value: string): void {
    const select = labeledSelect(label);
    select.value = value;
    select.dispatchEvent(
        new Event("change", {
            bubbles: true,
        }),
    );
}

/** One bucket whose key carries exactly the requested dimensions. */
function bucketFor(dimensions: ReadonlyArray<UsageGroupBy>): UsageBucket {
    const key: UsageBucket["key"] = {};

    for (const dimension of dimensions) {
        if (dimension === "day") {
            key.day = "2026-09-01";
        }

        if (dimension === "outcome") {
            key.outcome = "completed";
        }
    }

    return {
        key,
        requests: 1,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

/** One counted bucket whose key carries exactly the given parts. */
function bucketWith(key: UsageBucket["key"]): UsageBucket {
    return {
        key,
        requests: 1,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

describe("UsageQueryScreen request assembly", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("sends the dimension set and the assembled filter to the query route", async () => {
        const requests: QueryUsageRequest[] = [];
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {});

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...api,
                            queryUsage: (request) => {
                                requests.push(request);

                                return Promise.resolve({
                                    buckets: [
                                        bucketFor(request.group_by),
                                    ],
                                });
                            },
                        }}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The untouched form asks for the day dimension and filters nothing.
        expect(requests).toEqual([
            {
                group_by: [
                    "day",
                ],
                filter: {},
            },
        ]);

        // Toggling a dimension and filling two filters composes both.
        switchWithLabel("Outcome").click();
        typeInto(labeledInput("HTTP statuses"), "200, 429");
        chooseSelect("Stream mode", "streamed");
        buttonWithText("Run query").click();
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(requests[1]).toEqual({
            group_by: [
                "day",
                "outcome",
            ],
            filter: {
                http_status: [
                    200,
                    429,
                ],
                is_stream: true,
            },
        });
    });
});

describe("UsageQueryScreen refresh racing the initial read", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("keeps the loading state up until the newer cycle settles", async () => {
        let queryCount = 0;
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        });

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The initial read is in flight under the skeleton.
        expect(skeleton()).not.toBeNull();

        // Refresh is pressed while that first read is still in flight: the
        // newer cycle aborts it and raises a loading state of its own.
        requestRefresh();
        await settle();

        // The superseded cycle's abort settles into its own cleanup only — it
        // must not drop the loading state the newer cycle raised.
        expect(skeleton()).not.toBeNull();
        expect(queryCount).toBe(0);

        // The newer cycle's reads answer and its query runs to completion.
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(queryCount).toBe(1);
        expect(skeleton()).toBeNull();
        expect(document.body.textContent).toContain("2026-09-01");
    });
});

describe("UsageQueryScreen submit during the initial read", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("reads the entity lists the labels resolve through before the query lands", async () => {
        let queryCount = 0;
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        }, [
            providerBucket,
        ]);

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The initial read is in flight under the skeleton.
        expect(skeleton()).not.toBeNull();
        expect(holder.providers).toHaveLength(1);

        // The operator picks the provider grouping and submits while that
        // first read is still in flight.
        switchWithLabel("Provider").click();
        buttonWithText("Run query").click();
        await settle();

        // The submit superseded the read and reads the entity lists itself:
        // a second held round of listings, and the query waits behind them.
        expect(holder.providers).toHaveLength(2);
        expect(queryCount).toBe(0);
        expect(buttonWithText("Querying…").disabled).toBe(true);

        answerLast(holder.providers, {
            providers: [
                providerRow(),
            ],
        });
        answerLast(holder.models, {
            models: [
                modelRow(),
            ],
        });
        answerLast(holder.apiKeys, {
            api_keys: [
                apiKeyRow(),
            ],
        });
        await settle();

        // The bucket's label resolves through the freshly read lists — an
        // existing provider never reads as a deleted row.
        expect(queryCount).toBe(1);
        expect(skeleton()).toBeNull();
        expect(document.body.textContent).toContain("local");
        expect(document.body.textContent).not.toContain("Deleted provider");
    });
});

describe("UsageQueryScreen refresh racing an in-flight submit", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("renders the refresh over the submit and lets the submit's late answer write nothing", async () => {
        let queryCount = 0;
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        }, [
            providerBucket,
        ]);

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The initial read is in flight under the skeleton.
        expect(skeleton()).not.toBeNull();

        // The operator picks the provider grouping and submits while that
        // first read is still in flight; the submit's own entity listings
        // are held.
        switchWithLabel("Provider").click();
        buttonWithText("Run query").click();
        await settle();

        expect(holder.providers).toHaveLength(2);
        expect(buttonWithText("Querying…").disabled).toBe(true);

        // The shell Refresh is pressed while that submit is in flight — the
        // one supersede that starts while the busy state is the submit's.
        requestRefresh();
        await settle();

        // The refresh raises its own cycle and reads the entity lists
        // itself: a third held round, with both cycles' queries still
        // waiting behind their listings.
        expect(holder.providers).toHaveLength(3);
        expect(queryCount).toBe(0);

        // The superseded submit's cleanup settles into its own guard only —
        // it must not drop the loading state the refresh's cycle raised.
        expect(skeleton()).not.toBeNull();

        // The refresh's cycle answers and its query runs to completion.
        answerLast(holder.providers, {
            providers: [
                providerRow(),
            ],
        });
        answerLast(holder.models, {
            models: [
                modelRow(),
            ],
        });
        answerLast(holder.apiKeys, {
            api_keys: [
                apiKeyRow(),
            ],
        });
        await settle();

        // The busy state is neither stuck nor handed back by the superseded
        // cycle: the submit button reads its idle label, enabled.
        expect(queryCount).toBe(1);
        expect(skeleton()).toBeNull();
        expect(buttonWithText("Run query").disabled).toBe(false);

        // The rendered bucket resolves through the refresh's entity lists.
        expect(document.body.textContent).toContain("local");
        expect(document.body.textContent).not.toContain("Deleted provider");

        // The submit's own listing answers, arriving after everything has
        // settled, write nothing: the screen stays exactly as the refresh's
        // cycle left it.
        answerAt(holder.providers, 1, {
            providers: [
                staleProviderRow,
            ],
        });
        await settle();

        expect(queryCount).toBe(1);
        expect(document.body.textContent).toContain("local");
        expect(document.body.textContent).not.toContain("stale-submit");
    });
});

describe("UsageQueryScreen arrival subject", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    async function mountAndAnswer(
        requests: Array<QueryUsageRequest>,
        holder: HeldListings,
        subject: UsageQuerySubject | null = null,
    ): Promise<void> {
        const api = screenApi(holder, () => {}, []);

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...api,
                            queryUsage: (request) => {
                                requests.push(request);

                                return Promise.resolve({
                                    buckets: [],
                                });
                            },
                        }}
                        subject={subject}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();
    }

    it("prefills the route's subject as the grouping and the filter", async () => {
        const requests: QueryUsageRequest[] = [];
        await mountAndAnswer(
            requests,
            {
                providers: [],
                models: [],
                apiKeys: [],
            },
            {
                dimension: "provider",
                id: 1,
            },
        );

        // The subject preselects its dimension beside the day default and
        // fills its own filter field: the arrival query already asks about
        // exactly the subject the aggregates link carried.
        expect(requests).toEqual([
            {
                group_by: [
                    "day",
                    "provider",
                ],
                filter: {
                    provider_id: [
                        1,
                    ],
                },
            },
        ]);
    });

    it("asks the bare page's default question when the route carries no subject", async () => {
        const requests: QueryUsageRequest[] = [];
        await mountAndAnswer(requests, {
            providers: [],
            models: [],
            apiKeys: [],
        });

        expect(requests).toEqual([
            {
                group_by: [
                    "day",
                ],
                filter: {},
            },
        ]);
    });
});

describe("UsageQueryScreen hand-typed bounds", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("opens a bound's error only behind input to its field, then holds the query until it is fixed", async () => {
        const requests: QueryUsageRequest[] = [];
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {});

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...api,
                            queryUsage: (request) => {
                                requests.push(request);

                                return Promise.resolve({
                                    buckets: [],
                                });
                            },
                        }}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The blank-bounds mount query ran as always.
        expect(requests).toHaveLength(1);

        // A pristine form is not an error: the format lesson lives in the
        // fields' permanent hints, never as alert-red from first paint — the
        // bounds' errors open behind input to their own field (the app's
        // lazy-error convention).
        expect(document.body.textContent).toContain("like 2026-09-23T10:00");
        expect(document.body.textContent).not.toContain("From must be a complete date and time");
        expect(document.body.textContent).not.toContain("To must be a complete date and time");

        // A partially-typed bound is a mistake the moment the field receives
        // it: named inline, with the submit held, so the query can never run
        // with the bound silently dropped.
        typeInto(labeledInput("From"), "2026-09-23T1");
        await settle();

        expect(document.body.textContent).toContain("From must be a complete date and time");
        expect(buttonWithText("Run query").disabled).toBe(true);

        buttonWithText("Run query").click();
        await settle();
        expect(requests).toHaveLength(1);

        // Completing the value retracts the error without another submit and
        // releases the query, and the bound rides it.
        typeInto(labeledInput("From"), "2026-09-23T10:00");
        await settle();

        expect(document.body.textContent).not.toContain("From must be a complete date and time");
        expect(buttonWithText("Run query").disabled).toBe(false);
        buttonWithText("Run query").click();
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(requests).toHaveLength(2);
        expect(requests[1]?.filter?.from).toBeTypeOf("number");
        expect(requests[1]?.filter?.to).toBeUndefined();
    });
});

describe("UsageQueryScreen rolling-window quick-sets", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /**
     * Mounts the bare screen, answers its first cycle's held listings so the
     * mount query lands, and hands the listings holder over for the cycles
     * the test still runs.
     */
    async function mountAndAnswerFirstCycle(
        requests: Array<QueryUsageRequest>,
    ): Promise<HeldListings> {
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...screenApi(holder, () => {}),
                            queryUsage: (request) => {
                                requests.push(request);

                                return Promise.resolve({
                                    buckets: [],
                                });
                            },
                        }}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        return holder;
    }

    /** Runs the submit through its own held listings, answering them. */
    async function runQueryAndAnswer(holder: HeldListings): Promise<void> {
        buttonWithText("Run query").click();
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();
    }

    it("fills both bounds with the stream's window and runs the query with them", async () => {
        const requests: QueryUsageRequest[] = [];
        const holder = await mountAndAnswerFirstCycle(requests);

        // The mount query ran bare.
        expect(requests).toHaveLength(1);

        const before = nowMicros();

        buttonWithText("Last 24 hours").click();
        await settle();

        // The fill lands in the hand-typed fields themselves — From one
        // complete spelling a window back, To left open — the stream page's
        // own rolling-window semantics, in the fields' own minute-precision
        // shape.
        expect(labeledInput("To").value).toBe("");

        const from = parseDateTimeInputToMicros(labeledInput("From").value);
        expect(from).not.toBeNull();
        expect(from).toBeLessThanOrEqual(before - DAY_MICROS);
        expect(from).toBeGreaterThan(before - DAY_MICROS - 2 * 60_000_000);

        await runQueryAndAnswer(holder);

        // The filled bounds ride the query exactly as a hand-typed pair would.
        expect(requests).toHaveLength(2);
        expect(requests[1]?.filter?.from).toBe(from);
        expect(requests[1]?.filter?.to).toBeUndefined();
    });

    it("empties both bounds for all time, overwriting a hand-typed one", async () => {
        const requests: QueryUsageRequest[] = [];
        const holder = await mountAndAnswerFirstCycle(requests);

        typeInto(labeledInput("From"), "2026-09-23T10:00");
        buttonWithText("All time").click();
        await settle();

        // All time is the fields' own empty state: both bounds drop,
        // whatever they held.
        expect(labeledInput("From").value).toBe("");
        expect(labeledInput("To").value).toBe("");

        await runQueryAndAnswer(holder);

        expect(requests).toHaveLength(2);
        expect(requests[1]?.filter).toEqual({});
    });
});

describe("UsageQueryScreen hand-typed status list", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("names an unparseable status inline and holds the query until it is fixed", async () => {
        const requests: QueryUsageRequest[] = [];
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {});

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...api,
                            queryUsage: (request) => {
                                requests.push(request);

                                return Promise.resolve({
                                    buckets: [],
                                });
                            },
                        }}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The blank-field mount query ran as always.
        expect(requests).toHaveLength(1);

        // The glob habit "2xx" is no status list: the mistake is named
        // inline and the submit is held, so the query can never silently
        // run with the status narrow it names dropped.
        typeInto(labeledInput("HTTP statuses"), "2xx");
        await settle();

        expect(document.body.textContent).toContain("HTTP statuses must be plain integers");
        expect(buttonWithText("Run query").disabled).toBe(true);

        buttonWithText("Run query").click();
        await settle();
        expect(requests).toHaveLength(1);

        // Clearing the field releases the query, and no status narrow rides it.
        typeInto(labeledInput("HTTP statuses"), "");
        await settle();

        expect(buttonWithText("Run query").disabled).toBe(false);
        buttonWithText("Run query").click();
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(requests).toHaveLength(2);
        expect(requests[1]?.filter).toEqual({});
    });
});

describe("UsageQueryScreen refresh holding on a marked field", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("states the hold on the failure channel and keeps the standing buckets", async () => {
        let queryCount = 0;
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        });

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The mount query ran and its answer stands.
        expect(queryCount).toBe(1);
        expect(document.body.textContent).toContain("2026-09-01");

        // A malformed bound marks its field. The submit button is guarded,
        // so only the shell's Refresh can start a cycle that reaches the
        // query while a field stands marked.
        typeInto(labeledInput("From"), "2026-09-23 10:00");
        await settle();

        requestRefresh();
        await settle();

        // The refresh's cycle re-reads the entity lists — a second held
        // round.
        expect(holder.providers).toHaveLength(2);
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The query never ran again, and the half-run is stated: the hold
        // notice names the way out while the standing buckets stay visible
        // beside it.
        expect(queryCount).toBe(1);
        expect(document.body.textContent).toContain(
            "The query did not run — a filter value is invalid.",
        );
        expect(document.body.textContent).toContain("2026-09-01");
    });

    it("clears the hold notice when a later cycle succeeds", async () => {
        let queryCount = 0;
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        });

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The hold arrives the way the screen can only produce it: a shell
        // refresh landing on a marked field.
        typeInto(labeledInput("From"), "2026-09-23 10:00");
        await settle();
        requestRefresh();
        await settle();

        expect(holder.providers).toHaveLength(2);
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(document.body.textContent).toContain(
            "The query did not run — a filter value is invalid.",
        );

        // Correcting the field releases the submit, and the successful
        // cycle replaces both the buckets and the notice.
        typeInto(labeledInput("From"), "2026-09-23T10:00");
        await settle();
        buttonWithText("Run query").click();
        await settle();

        expect(holder.providers).toHaveLength(3);
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(queryCount).toBe(2);
        expect(document.body.textContent).not.toContain(
            "The query did not run — a filter value is invalid.",
        );
        expect(document.body.textContent).toContain("2026-09-01");
    });
});

describe("UsageQueryScreen failed query", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("states the query's diagnosis once and clears it when a later cycle lands", async () => {
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        let isQueryFailing = true;
        const api = {
            ...screenApi(holder, () => {}),
            queryUsage: () =>
                isQueryFailing
                    ? Promise.reject(new Error("query refused"))
                    : Promise.resolve({
                          buckets: [
                              dayBucket(),
                          ],
                      }),
        };

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The first cycle's query fails: its diagnosis is stated on the
        // failure channel, the skeleton is gone, and the form stands
        // re-enabled for the retry.
        expect(document.body.textContent).toContain("query refused");
        expect(skeleton()).toBeNull();
        expect(buttonWithText("Run query").disabled).toBe(false);

        // A later cycle that lands replaces the diagnosis with the answer.
        isQueryFailing = false;
        buttonWithText("Run query").click();
        await settle();
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        expect(document.body.textContent).not.toContain("query refused");
        expect(document.body.textContent).toContain("2026-09-01");
    });
});

describe("UsageQueryScreen clear filters", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("resets the fields and bounds, keeps the dimensions, and hides once nothing is set", async () => {
        const requests: QueryUsageRequest[] = [];
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...screenApi(holder, () => {}),
                            queryUsage: (request) => {
                                requests.push(request);

                                // Each answer carries exactly the key its
                                // request's dimensions name, as the label
                                // resolver's contract demands.
                                const key: UsageBucket["key"] = {
                                    day: "2026-09-01",
                                };

                                if (request.group_by.includes("provider")) {
                                    key.provider_id = 1;
                                }

                                if (request.group_by.includes("outcome")) {
                                    key.outcome = "completed";
                                }

                                return Promise.resolve({
                                    buckets: [
                                        {
                                            key,
                                            requests: 1,
                                            input_tokens: 0,
                                            output_tokens: 0,
                                            cached_tokens: 0,
                                            cache_write_tokens: 0,
                                            reasoning_tokens: 0,
                                        },
                                    ],
                                });
                            },
                        }}
                        subject={{
                            dimension: "provider",
                            id: 1,
                        }}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The arrival subject's prefilled field is a set filter, so the
        // clear action stands beside the submit from the start.
        expect(requests).toHaveLength(1);
        expect(buttonWithText("Clear filters")).toBeInstanceOf(HTMLButtonElement);

        // A bound, a status narrow, and one more dimension join the
        // question — the clear must reset the first two and keep the
        // third.
        typeInto(labeledInput("From"), "2026-09-23T10:00");
        typeInto(labeledInput("HTTP statuses"), "429");
        switchWithLabel("Outcome").click();
        await settle();

        buttonWithText("Clear filters").click();
        await settle();

        // The clear re-runs the whole cycle — a second held round of
        // listings behind the query.
        expect(holder.providers).toHaveLength(2);
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The subject's id narrow, the status list, and the bound are
        // gone, while the dimension switches — the arrival subject's
        // provider included — stay in the grouping.
        expect(requests[1]).toEqual({
            group_by: [
                "day",
                "provider",
                "outcome",
            ],
            filter: {},
        });

        // With nothing set the action is gone from the form.
        expect(() => buttonWithText("Clear filters")).toThrow("No button reading Clear filters");
    });
});

describe("UsageQueryScreen empty answer", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the screen against a gateway whose query answers empty, and answers its listings. */
    async function mountAndAnswer(subject: UsageQuerySubject | null): Promise<void> {
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {}, []);

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={api}
                        subject={subject}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();
    }

    it("states the filter mismatch, naming the clear action it stands beside, while filters are set", async () => {
        await mountAndAnswer({
            dimension: "provider",
            id: 1,
        });

        // The carried subject's own narrow stands, so the empty answer is
        // the filter's doing: every clause of the notice is true — the
        // widening, the dropping, and the action it names all exist.
        expect(document.body.textContent).toContain("No rows in this window.");
        expect(document.body.textContent).toContain("Nothing recorded matches these filters");
        expect(document.body.textContent).toContain("Clear filters sits in the form above.");
        expect(buttonWithText("Clear filters")).toBeInstanceOf(HTMLButtonElement);
    });

    it("states the bare question's empty record without naming filters or a clear action", async () => {
        await mountAndAnswer(null);

        // Nothing stands set — no filter to drop, no bound to widen, and no
        // clear action on the page — so the notice states the record itself
        // instead of a mismatch a control that is not there would clear.
        expect(document.body.textContent).toContain("Nothing recorded in this window.");
        expect(document.body.textContent).not.toContain("No rows in this window.");
        expect(document.body.textContent).not.toContain("these filters");
        expect(document.body.textContent).not.toContain("Clear filters");
    });
});

describe("UsageQueryScreen displayed bucket order", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** The table's row labels in rendered order — the displayed bucket order. */
    function rowLabels(): Array<string | null> {
        return Array.from(
            document.querySelectorAll<HTMLTableCellElement>("tbody th[scope=row]"),
        ).map((cell) => cell.textContent);
    }

    /** Answers one cycle's own held entity listings — a submit reads them itself. */
    function answerListings(holder: HeldListings): void {
        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
    }

    /**
     * Mounts the bare page whose query answers whatever `answer` currently
     * holds, and settles its first cycle — the held entity listings included.
     * The mutable answer lets a test run a second query under a different
     * grouping without remounting.
     */
    async function mountWithAnswer(answer: { buckets: Array<UsageBucket> }): Promise<HeldListings> {
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={{
                            ...screenApi(holder, () => {}),
                            queryUsage: () =>
                                Promise.resolve({
                                    buckets: [
                                        ...answer.buckets,
                                    ],
                                }),
                        }}
                        subject={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerListings(holder);
        await settle();

        return holder;
    }

    it("displays a day grouping newest-first and states the UTC boundary", async () => {
        const answer: {
            buckets: Array<UsageBucket>;
        } = {
            buckets: [
                bucketWith({
                    day: "2026-09-01",
                }),
                bucketWith({
                    day: "2026-09-02",
                }),
                bucketWith({
                    day: "2026-09-03",
                }),
            ],
        };
        await mountWithAnswer(answer);

        // The gateway answers ascending by day; the table reads newest-first
        // — the detail usage card's own direction — and the boundary the days
        // split on is stated under the table.
        expect(rowLabels()).toEqual([
            "2026-09-03",
            "2026-09-02",
            "2026-09-01",
        ]);
        expect(document.body.textContent).toContain(
            "Days are UTC — the gateway's own day boundary.",
        );
    });

    it("keeps the gateway's order within equal days of a day grouping", async () => {
        const answer: {
            buckets: Array<UsageBucket>;
        } = {
            buckets: [
                bucketWith({
                    day: "2026-09-01",
                }),
            ],
        };
        const holder = await mountWithAnswer(answer);

        // The outcome dimension joins the day: the next query's keys are
        // composite, and the gateway answers ascending by day, then outcome.
        switchWithLabel("Outcome").click();
        answer.buckets = [
            bucketWith({
                day: "2026-09-01",
                outcome: "completed",
            }),
            bucketWith({
                day: "2026-09-01",
                outcome: "upstream_error",
            }),
            bucketWith({
                day: "2026-09-02",
                outcome: "completed",
            }),
        ];
        buttonWithText("Run query").click();
        await settle();

        answerListings(holder);
        await settle();

        // Newest day first; within the equal 09-01 days the gateway's own
        // order — completed before upstream error — stands.
        expect(rowLabels()).toEqual([
            "2026-09-02 · Completed",
            "2026-09-01 · Completed",
            "2026-09-01 · Upstream error",
        ]);
    });

    it("keeps a grouping without day in the gateway's order and states no boundary", async () => {
        const answer: {
            buckets: Array<UsageBucket>;
        } = {
            buckets: [
                bucketWith({
                    day: "2026-09-01",
                }),
            ],
        };
        const holder = await mountWithAnswer(answer);

        // The day dimension leaves the question: the next query's buckets
        // split on the outcome alone.
        switchWithLabel("Day").click();
        switchWithLabel("Outcome").click();
        answer.buckets = [
            bucketWith({
                outcome: "upstream_error",
            }),
            bucketWith({
                outcome: "completed",
            }),
        ];
        buttonWithText("Run query").click();
        await settle();

        answerListings(holder);
        await settle();

        // Nothing reorders the answer — the gateway's order stands as it
        // came — and no day boundary is stated where no day is rendered.
        expect(rowLabels()).toEqual([
            "Upstream error",
            "Completed",
        ]);
        expect(document.body.textContent).not.toContain("Days are UTC");
    });
});

describe("UsageQueryScreen aggregates-to-stream door", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("carries the shared filter fields into the stream draft the header door's href spells, bounds behind", async () => {
        const holder: HeldListings = {
            providers: [],
            models: [],
            apiKeys: [],
        };

        disposes.push(
            render(
                () => (
                    <UsageQueryScreen
                        api={screenApi(holder, () => {})}
                        subject={{
                            dimension: "provider",
                            id: 1,
                        }}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        answerLast(holder.providers, {
            providers: [],
        });
        answerLast(holder.models, {
            models: [],
        });
        answerLast(holder.apiKeys, {
            api_keys: [],
        });
        await settle();

        // The shared filter fields stand set the way an operator's diagnostic
        // question holds them: the arrival subject's provider narrow, the
        // grouped failure outcome, the stream mode, and one status.
        chooseSelect("Outcome", "failures");
        chooseSelect("Stream mode", "streamed");
        typeInto(labeledInput("HTTP statuses"), "429");
        await settle();

        // With no bound set the door reads its bare label — the date trade is
        // announced only while a From or To bound stands.
        expect(
            linkTo("/usage?provider=1&outcome=upstream_error,internal&stream=streamed&status=429")
                .textContent,
        ).toBe("View requests");

        // A From bound joins the question.
        typeInto(labeledInput("From"), "2026-09-23T10:00");
        await settle();

        // The door crosses to the stream with the shared fields as its draft —
        // the grouped outcome spelled as the pair the stream route reads back —
        // while the From bound stays behind with no stream spelling to become:
        // the stream's own window row governs time there. The door states that
        // trade beside its label as visible text — no screen-reader-only
        // aside, the sentence sighted operators read too.
        const door = linkTo(
            "/usage?provider=1&outcome=upstream_error,internal&stream=streamed&status=429",
        );
        expect(door.textContent).toBe("View requests — From and To do not apply there.");
        expect(door.querySelector(".sr-only")).toBeNull();
    });
});
