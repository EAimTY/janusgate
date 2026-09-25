import { render } from "@solidjs/web";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiKeyResponse,
    ApiLogResponse,
    ListUsageRequest,
    ListUsageResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { USAGE_PAGE_SIZE } from "/src/screens/usage-pagination";
import { UsageScreen } from "/src/screens/usage-screen";
import { EMPTY_USAGE_FILTER_DRAFT, type UsageFilterDraft } from "/src/usage-filter";

/**
 * One accounting row whose only distinguishing rendering is `<duration> ms`:
 * every stream assertion below marks rows by that number. The row's stamp is
 * the base instant plus the same offset, so a page's cursor derives from it.
 */
const CURSOR_BASE = 1_700_000_000_000_000;

function logRow(offset: number): ApiLogResponse {
    return {
        started_at: CURSOR_BASE + offset,
        api_key_id: 7,
        provider_id: 1,
        model_id: 9,
        endpoint_type: "openai_chat_completions",
        is_stream: false,
        http_status: 200,
        outcome: "completed",
        duration: offset,
        input_tokens: null,
        output_tokens: null,
        cached_tokens: null,
        cache_write_tokens: null,
        reasoning_tokens: null,
    };
}

/** One page of `count` rows, newest first, stamps counting down from `newest`. */
function page(newest: number, count: number): ReadonlyArray<ApiLogResponse> {
    return Array.from(
        {
            length: count,
        },
        (_, index) => logRow(newest - index),
    );
}

/** One held `usage:list` call: the request it carried and the ways the test ends it. */
type HeldPage = {
    readonly request: ListUsageRequest;
    readonly answer: (result: ListUsageResponse) => void;
    readonly refuse: (cause: unknown) => void;
};

/**
 * One `usage:list` call held back until the test ends it, rejecting with the
 * transport's abort failure the moment its cycle's signal fires — the same
 * shape a real in-flight fetch dies with.
 */
function heldPage(
    signal: AbortSignal | undefined,
    request: ListUsageRequest,
    held: Array<HeldPage>,
): Promise<ListUsageResponse> {
    return new Promise<ListUsageResponse>((resolve, reject) => {
        held.push({
            request,
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

/**
 * A management API whose usage listing answers only when the test answers it
 * (so a stream page stays in flight) and whose entity listings answer the
 * given rows, empty by default. Every other route rejects, so an unexpected
 * call fails the test loudly instead of hanging it.
 */
function streamApi(
    holder: {
        readonly pages: Array<HeldPage>;
    },
    entities: {
        readonly providers?: ReadonlyArray<ProviderResponse>;
        readonly models?: ReadonlyArray<ModelResponse>;
        readonly apiKeys?: ReadonlyArray<ApiKeyResponse>;
    } = {},
): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
        listProviderDefinitions: refused,
        createProviderDefinition: refused,
        updateProviderDefinition: refused,
        listProviders: () =>
            Promise.resolve({
                providers: [
                    ...(entities.providers ?? []),
                ],
            }),
        listModels: () =>
            Promise.resolve({
                models: [
                    ...(entities.models ?? []),
                ],
            }),
        listApiKeys: () =>
            Promise.resolve({
                api_keys: [
                    ...(entities.apiKeys ?? []),
                ],
            }),
        listUsage: (request, signal) => heldPage(signal, request, holder.pages),
        queryUsage: refused,
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

/** Answers the most recent held page — the newest cycle's call. */
function answerLastPage(
    holder: {
        readonly pages: Array<HeldPage>;
    },
    rows: ReadonlyArray<ApiLogResponse>,
): void {
    const pageAnswer = holder.pages.at(-1);

    if (pageAnswer === undefined) {
        throw new Error("No held page to answer");
    }

    pageAnswer.answer({
        api_logs: [
            ...rows,
        ],
    });
}

function buttonWithText(text: string): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error(`No button reading ${text}`);
}

function linkTo(href: string): HTMLAnchorElement {
    for (const candidate of document.querySelectorAll("a")) {
        if (candidate.getAttribute("href") === href) {
            return candidate;
        }
    }

    throw new Error(`No link to ${href}`);
}

function bodyText(): string {
    return document.body.textContent ?? "";
}

describe("UsageScreen stream continuation", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(api: ManagementApi, filter: UsageFilterDraft = EMPTY_USAGE_FILTER_DRAFT): void {
        disposes.push(
            render(
                () => (
                    <UsageScreen
                        api={api}
                        filter={filter}
                    />
                ),
                document.body,
            ),
        );
    }

    it("discards a load-more page the refresh superseded", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        // A full first page keeps the stream open at cursor 951.
        answerLastPage(holder, page(1050, USAGE_PAGE_SIZE));
        await settle();
        expect(bodyText()).toContain("Load more");

        buttonWithText("Load more").click();
        await settle();

        expect(bodyText()).toContain("Loading…");
        const stalePage = holder.pages.at(-1);
        expect(stalePage?.request.before).toBe(CURSOR_BASE + 951);

        // The refresh takes the stream over wholesale: the loader dies
        // mid-flight and the refresh's own first page is the only open call.
        requestRefresh();
        await settle();
        expect(holder.pages.at(-1)?.request.before).toBeUndefined();

        // The superseded page answers late — after its cycle died — and
        // writes nothing.
        stalePage?.answer({
            api_logs: [
                ...page(950, 3),
            ],
        });
        answerLastPage(holder, page(5000, 1));
        await settle();

        // The refreshed stream replaced the old one whole: the stale page's
        // rows appear neither appended nor duplicated.
        expect(bodyText()).toContain("5000 ms");
        expect(bodyText()).not.toContain("1050 ms");
        expect(bodyText()).not.toContain("950 ms");
        expect(bodyText()).toContain("That is every recorded row.");
        expect(bodyText()).not.toContain("Load more");
    });

    it("refuses to start a load while a read cycle is in flight", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        answerLastPage(holder, page(1050, USAGE_PAGE_SIZE));
        await settle();

        // A refresh is pressed: its cycle is in flight, and its first page
        // is the only open call.
        requestRefresh();
        await settle();
        expect(holder.pages).toHaveLength(2);

        buttonWithText("Load more").click();
        await settle();

        // The click is refused outright: no page call exists beyond the
        // refresh's own, and the continuation never turned busy.
        expect(holder.pages).toHaveLength(2);
        expect(bodyText()).not.toContain("Loading…");

        answerLastPage(holder, page(5000, 1));
        await settle();

        // The refresh's stream lands untouched by the refused click.
        expect(bodyText()).toContain("5000 ms");
        expect(bodyText()).toContain("That is every recorded row.");
    });

    it("keeps the continuation available after a failed page", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        answerLastPage(holder, page(1050, USAGE_PAGE_SIZE));
        await settle();

        buttonWithText("Load more").click();
        await settle();
        expect(holder.pages.at(-1)?.request.before).toBe(CURSOR_BASE + 951);

        holder.pages.at(-1)?.refuse(new Error("page load refused"));
        await settle();

        // The failure is stated and the stream is untouched — the
        // continuation stays, because only a short page ends the stream.
        expect(bodyText()).toContain("page load refused");
        expect(bodyText()).toContain("Load more");
        expect(bodyText()).not.toContain("That is every recorded row.");

        buttonWithText("Load more").click();
        await settle();

        // The retry carries the stream's own cursor: the failure never
        // displaced it.
        expect(holder.pages.at(-1)?.request.before).toBe(CURSOR_BASE + 951);

        answerLastPage(holder, page(950, 3));
        await settle();

        expect(bodyText()).toContain("950 ms");
        expect(bodyText()).toContain("948 ms");
        expect(bodyText()).toContain("That is every recorded row.");
        expect(bodyText()).not.toContain("page load refused");
    });
});

describe("UsageScreen filtering", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }

        disposes.length = 0;
    });

    function mount(api: ManagementApi, filter: UsageFilterDraft = EMPTY_USAGE_FILTER_DRAFT): void {
        disposes.push(
            render(
                () => (
                    <UsageScreen
                        api={api}
                        filter={filter}
                    />
                ),
                document.body,
            ),
        );
    }

    const filtered: UsageFilterDraft = {
        ...EMPTY_USAGE_FILTER_DRAFT,
        window: "24h",
        outcome: "failures",
        statusInput: "500",
    };

    it("carries the route's filter, resolved once, into every page of the stream", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder), filtered);
        await settle();

        const first = holder.pages.at(-1)?.request;

        expect(first?.filter?.outcome).toEqual([
            "upstream_error",
            "internal",
        ]);
        expect(first?.filter?.http_status).toEqual([
            500,
        ]);
        expect(first?.filter?.from).toBeTypeOf("number");

        answerLastPage(holder, page(1050, USAGE_PAGE_SIZE));
        await settle();

        buttonWithText("Load more").click();
        await settle();

        const next = holder.pages.at(-1)?.request;

        expect(next?.before).toBe(CURSOR_BASE + 951);
        // The rolling window is resolved once per cycle: pages carrying
        // different bounds could skip or repeat rows at the seam.
        expect(next?.filter).toEqual(first?.filter);
    });

    it("keeps the continuation on the rows' own filter when a refresh cycle fails", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        // The route owns the filter, so the screen sees a changed filter the
        // way a navigation delivers it: through the prop.
        const [filter, setFilter] = createSignal<UsageFilterDraft>(EMPTY_USAGE_FILTER_DRAFT);
        disposes.push(
            render(
                () => (
                    <UsageScreen
                        api={streamApi(holder)}
                        filter={filter()}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The unfiltered stream's first full page is the rows in hand.
        answerLastPage(holder, page(1050, USAGE_PAGE_SIZE));
        await settle();

        // A new filter navigates: the refresh cycle starts, and its page is
        // the only open call — under the new filter.
        setFilter({
            ...EMPTY_USAGE_FILTER_DRAFT,
            outcome: "failures",
        });
        await settle();
        expect(holder.pages.at(-1)?.request.filter?.outcome).toEqual([
            "upstream_error",
            "internal",
        ]);

        // The refresh cycle fails. The rows in hand are still the unfiltered
        // stream's, so the continuation must stay on their own filter — not
        // the failed cycle's.
        holder.pages.at(-1)?.refuse(new Error("refresh refused"));
        await settle();
        expect(bodyText()).toContain("refresh refused");
        expect(bodyText()).toContain("Load more");

        buttonWithText("Load more").click();
        await settle();

        const continued = holder.pages.at(-1)?.request;

        expect(continued?.before).toBe(CURSOR_BASE + 951);
        expect(continued?.filter).toBeUndefined();
    });

    it("sends no filter at all for the unfiltered stream", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        expect(holder.pages.at(-1)?.request).toEqual({
            limit: USAGE_PAGE_SIZE,
        });
    });

    it("states a filtered empty answer as a filter that matched nothing", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder), filtered);
        await settle();

        answerLastPage(holder, []);
        await settle();

        expect(bodyText()).toContain("No recorded requests match this filter.");
        expect(bodyText()).not.toContain("No requests recorded yet");
    });

    it("clears the filter back to the unfiltered address", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder), filtered);
        await settle();
        answerLastPage(holder, []);
        await settle();

        buttonWithText("Clear filters").click();
        await settle();

        expect(`${window.location.pathname}${window.location.search}`).toBe("/usage");
    });
});

describe("UsageScreen row links", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }

        disposes.length = 0;
    });

    const providerRow: ProviderResponse = {
        type: "api_key",
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        id: 1,
        name: "local",
        model_prefix: "local-",
        is_enabled: true,
        base_url: "https://upstream.example.com/v1",
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

    function mount(api: ManagementApi): void {
        disposes.push(
            render(
                () => (
                    <UsageScreen
                        api={api}
                        filter={EMPTY_USAGE_FILTER_DRAFT}
                    />
                ),
                document.body,
            ),
        );
    }

    it("links a row's listed entities to their pages and leaves deleted kinds plain", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        // One row whose provider, model, and key are all still listed, and
        // one whose provider was deleted after it was recorded.
        mount(
            streamApi(holder, {
                providers: [
                    providerRow,
                ],
                models: [
                    modelRow,
                ],
                apiKeys: [
                    apiKeyRow,
                ],
            }),
        );
        await settle();
        answerLastPage(holder, [
            ...page(1050, 1),
            {
                ...logRow(1049),
                provider_id: 2,
            },
        ]);
        await settle();

        expect(linkTo("/providers/1").textContent).toContain("local");
        expect(linkTo("/models/9").textContent).toContain("local-qwen3-32b");
        expect(linkTo("/api-keys/7").textContent).toContain("service-a");

        // A deleted row is named by its kind, never turned into a door that
        // leads nowhere.
        expect(bodyText()).toContain("Deleted provider");
        expect(() => linkTo("/providers/2")).toThrow("No link to /providers/2");
    });
});
