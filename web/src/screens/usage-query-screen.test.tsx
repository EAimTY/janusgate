import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ListApiKeysResponse,
    ListModelsResponse,
    ListProvidersResponse,
    UsageBucket,
} from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { UsageQueryScreen } from "/src/screens/usage-query-screen";

const dayBucket: UsageBucket = {
    key: "2026-09-01",
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
                    {
                        ...dayBucket,
                    },
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

/** The first-read placeholder: present exactly while the screen owes its first answers. */
function skeleton(): Element | null {
    return document.querySelector(".wash");
}

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
