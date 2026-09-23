import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiKeyResponse,
    ListApiKeysResponse,
    ListModelsResponse,
    ListProvidersResponse,
    ModelResponse,
    ProviderResponse,
    QueryUsageRequest,
    UsageBucket,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { UsageQueryScreen } from "/src/screens/usage-query-screen";

const providerRow: ProviderResponse = {
    type: "openai_chat_completion",
    id: 1,
    name: "local",
    model_prefix: "local-",
    is_enabled: true,
    base_url: "https://upstream.example.com/v1",
};

/** The superseded submit's own answer: the same row under its pre-refresh name. */
const staleProviderRow: ProviderResponse = {
    ...providerRow,
    name: "stale-submit",
};

const modelRow: ModelResponse = {
    id: 9,
    provider_id: 1,
    provider: "local",
    upstream_id: "qwen3:32b",
    public_id: "local-qwen3-32b",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

const apiKeyRow: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
};

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

/** One bucket grouped by provider 1 on the 1st — its label resolves through the entity lists. */
const providerBucket: UsageBucket = {
    key: {
        day: "2026-09-01",
        provider_id: 1,
    },
    requests: 3,
    input_tokens: 120,
    output_tokens: 45,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
};

/**
 * One list call held back until the test answers it, rejecting with the
 * transport's abort failure the moment its cycle's signal fires — the same
 * shape a real in-flight fetch dies with.
 */
function heldCall<TValue>(
    signal: AbortSignal | undefined,
    answers: Array<(answer: TValue) => void>,
): Promise<TValue> {
    return new Promise<TValue>((resolve, reject) => {
        answers.push(resolve);
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

/**
 * A management API whose entity listings answer only when the test answers
 * them (so a read cycle stays in flight) and whose query answers from a
 * fixture. Every other route rejects, so an unexpected call fails the test
 * loudly instead of hanging it.
 */
function screenApi(
    holder: {
        readonly providers: Array<(answer: ListProvidersResponse) => void>;
        readonly models: Array<(answer: ListModelsResponse) => void>;
        readonly apiKeys: Array<(answer: ListApiKeysResponse) => void>;
    },
    onQueryUsage: () => void,
    buckets: ReadonlyArray<UsageBucket> = [
        dayBucket,
    ],
): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
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
        createProvider: refused,
        updateProvider: refused,
        deleteProvider: refused,
        startProviderOauth: refused,
        completeProviderOauth: refused,
        reauthorizeProvider: refused,
        discoverModels: refused,
        updateModel: refused,
        createModel: refused,
        deleteModel: refused,
        listUsage: refused,
        createApiKey: refused,
        updateApiKey: refused,
        deleteApiKey: refused,
    };
}

/** Drains the pending promises and scheduled work of one screen exchange. */
async function settle(): Promise<void> {
    for (let round = 0; round < 10; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

/** Answers the most recent held call of one route — the newest cycle's call. */
function answerLast<TValue>(pending: Array<(answer: TValue) => void>, value: TValue): void {
    const answer = pending.at(-1);

    if (answer === undefined) {
        throw new Error("No held call to answer");
    }

    answer(value);
}

/** Answers one held call by position — a superseded cycle's own, late answer. */
function answerAt<TValue>(
    pending: Array<(answer: TValue) => void>,
    position: number,
    value: TValue,
): void {
    const answer = pending.at(position);

    if (answer === undefined) {
        throw new Error(`No held call at position ${position}`);
    }

    answer(value);
}

/** The first-read placeholder: present exactly while the screen owes its first answers. */
function skeleton(): Element | null {
    return document.querySelector(".wash");
}

function buttonWithText(text: string): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error(`No button reading ${text}`);
}

/** Toggles one group-by switch the way a browser delivers a click. */
function labeledSwitch(label: string): void {
    const control = document.querySelector(`button[role="switch"][aria-label="${label}"]`);

    if (control instanceof HTMLButtonElement) {
        control.click();
        return;
    }

    throw new Error(`No switch labeled ${label}`);
}

/** Picks one select the way a browser delivers a choice. */
function labeledSelect(label: string, value: string): void {
    for (const candidate of document.querySelectorAll("label")) {
        const id = candidate.getAttribute("for");

        if (candidate.textContent === label && id !== null) {
            const select = document.getElementById(id);

            if (select instanceof HTMLSelectElement) {
                select.value = value;
                select.dispatchEvent(
                    new Event("change", {
                        bubbles: true,
                    }),
                );
                return;
            }
        }
    }

    throw new Error(`No select labeled ${label}`);
}

/** Types into one labeled text field the way a browser delivers keystrokes. */
function labeledInput(label: string, value: string): void {
    for (const candidate of document.querySelectorAll("label")) {
        const id = candidate.getAttribute("for");

        if (candidate.textContent === label && id !== null) {
            const input = document.getElementById(id);

            if (input instanceof HTMLInputElement) {
                input.value = value;
                input.dispatchEvent(
                    new Event("input", {
                        bubbles: true,
                    }),
                );
                return;
            }
        }
    }

    throw new Error(`No input labeled ${label}`);
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
        const holder: {
            readonly providers: Array<(answer: ListProvidersResponse) => void>;
            readonly models: Array<(answer: ListModelsResponse) => void>;
            readonly apiKeys: Array<(answer: ListApiKeysResponse) => void>;
        } = {
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
                                    buckets: [bucketFor(request.group_by)],
                                });
                            },
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
        labeledSwitch("Outcome");
        labeledInput("HTTP statuses", "200, 429");
        labeledSelect("Stream mode", "streamed");
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
        const holder: {
            readonly providers: Array<(answer: ListProvidersResponse) => void>;
            readonly models: Array<(answer: ListModelsResponse) => void>;
            readonly apiKeys: Array<(answer: ListApiKeysResponse) => void>;
        } = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        });

        disposes.push(render(() => <UsageQueryScreen api={api} />, document.body));
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
        const holder: {
            readonly providers: Array<(answer: ListProvidersResponse) => void>;
            readonly models: Array<(answer: ListModelsResponse) => void>;
            readonly apiKeys: Array<(answer: ListApiKeysResponse) => void>;
        } = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        }, [
            providerBucket,
        ]);

        disposes.push(render(() => <UsageQueryScreen api={api} />, document.body));
        await settle();

        // The initial read is in flight under the skeleton.
        expect(skeleton()).not.toBeNull();
        expect(holder.providers).toHaveLength(1);

        // The operator picks the provider grouping and submits while that
        // first read is still in flight.
        labeledSwitch("Provider");
        buttonWithText("Run query").click();
        await settle();

        // The submit superseded the read and reads the entity lists itself:
        // a second held round of listings, and the query waits behind them.
        expect(holder.providers).toHaveLength(2);
        expect(queryCount).toBe(0);
        expect(buttonWithText("Querying…").disabled).toBe(true);

        answerLast(holder.providers, {
            providers: [
                providerRow,
            ],
        });
        answerLast(holder.models, {
            models: [
                modelRow,
            ],
        });
        answerLast(holder.apiKeys, {
            api_keys: [
                apiKeyRow,
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
        const holder: {
            readonly providers: Array<(answer: ListProvidersResponse) => void>;
            readonly models: Array<(answer: ListModelsResponse) => void>;
            readonly apiKeys: Array<(answer: ListApiKeysResponse) => void>;
        } = {
            providers: [],
            models: [],
            apiKeys: [],
        };
        const api = screenApi(holder, () => {
            queryCount += 1;
        }, [
            providerBucket,
        ]);

        disposes.push(render(() => <UsageQueryScreen api={api} />, document.body));
        await settle();

        // The initial read is in flight under the skeleton.
        expect(skeleton()).not.toBeNull();

        // The operator picks the provider grouping and submits while that
        // first read is still in flight; the submit's own entity listings
        // are held.
        labeledSwitch("Provider");
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
                providerRow,
            ],
        });
        answerLast(holder.models, {
            models: [
                modelRow,
            ],
        });
        answerLast(holder.apiKeys, {
            api_keys: [
                apiKeyRow,
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
