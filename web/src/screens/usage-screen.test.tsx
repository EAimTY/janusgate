import { render } from "@solidjs/web";
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
import {
    API_LOG_CURSOR_BASE,
    apiKeyRow,
    apiLogRow,
    bodyText,
    buttonWithText,
    labeledInput,
    labeledSelect,
    linkTo,
    modelRow,
    paragraphWithText,
    providerRow,
    settle,
    stubListings,
    stubManagementApi,
    typeInto,
} from "./screen-test-support";

/**
 * One accounting row whose only distinguishing exact rendering is
 * `in <offset>`: every stream assertion below marks rows by that number. The
 * shared row's duration is the offset too, but the row prints durations as
 * human durations, whose rounding can collide across offsets; the input
 * count never rounds. The remaining token columns are blanked, so the row's
 * optional figures stay out of these assertions' way.
 */
function logRow(offset: number): ApiLogResponse {
    return apiLogRow(offset, {
        duration: offset,
        input_tokens: offset,
        output_tokens: null,
    });
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
    return stubManagementApi({
        ...stubListings(entities),
        listUsage: (request, signal) => heldPage(signal, request, holder.pages),
    });
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
        expect(stalePage?.request.before).toBe(API_LOG_CURSOR_BASE + 951);

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
        expect(bodyText()).toContain("in 5,000");
        expect(bodyText()).not.toContain("in 1,050");
        expect(bodyText()).not.toContain("in 950");
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

        // The refusal is the disabled control's: the suppressed click never
        // reaches the app, so no page call can follow it and the continuation
        // never turns busy — the assertions pin the disabled face.
        expect(holder.pages).toHaveLength(2);
        expect(bodyText()).not.toContain("Loading…");

        // The refusal is visible at the control itself: the button is
        // disabled while the read cycle owns the stream, its title stating
        // why — the Refresh control's disabled-with-a-reason treatment.
        expect(buttonWithText("Load more").disabled).toBe(true);
        expect(buttonWithText("Load more").title).toContain("replacing the stream");

        answerLastPage(holder, page(5000, 1));
        await settle();

        // The refresh's stream lands untouched by the refused click.
        expect(bodyText()).toContain("in 5,000");
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
        expect(holder.pages.at(-1)?.request.before).toBe(API_LOG_CURSOR_BASE + 951);

        holder.pages.at(-1)?.refuse(new Error("page load refused"));
        await settle();

        // The failure is stated and the stream is untouched — the
        // continuation stays, because only a short page ends the stream.
        expect(bodyText()).toContain("page load refused");
        expect(bodyText()).toContain("Load more");
        expect(bodyText()).not.toContain("That is every recorded row.");

        // The failure is stated beside the control that failed: its container
        // holds the Load more button and none of the stream's rows — never
        // the screen-level notice above the page.
        const failure = [
            ...document.querySelectorAll('[role="alert"]'),
        ].find((candidate) => candidate.textContent?.includes("page load refused"));
        expect(failure).toBeDefined();
        const container = failure?.parentElement;
        expect(container?.contains(buttonWithText("Load more"))).toBe(true);
        expect(container?.textContent).not.toContain("in 1,050");

        buttonWithText("Load more").click();
        await settle();

        // The retry carries the stream's own cursor: the failure never
        // displaced it.
        expect(holder.pages.at(-1)?.request.before).toBe(API_LOG_CURSOR_BASE + 951);

        answerLastPage(holder, page(950, 3));
        await settle();

        expect(bodyText()).toContain("in 950");
        expect(bodyText()).toContain("in 948");
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

        expect(next?.before).toBe(API_LOG_CURSOR_BASE + 951);
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
        // A rolling window is resolved when the stream is read: the refresh's
        // own cycle resolves later bounds than the first read did, so a
        // failed refresh leaves two candidate filters to continue under —
        // exactly the pairing this screen must not get wrong.
        mount(streamApi(holder), {
            ...EMPTY_USAGE_FILTER_DRAFT,
            window: "24h",
        });
        await settle();

        const first = holder.pages.at(-1)?.request;
        // The unfiltered-but-bounded stream's first full page is the rows in
        // hand.
        answerLastPage(holder, page(1050, USAGE_PAGE_SIZE));
        await settle();

        // A moment passes so the refresh resolves a strictly later window
        // bound than the first read did; the assertion below proves it.
        await new Promise((resolve) => setTimeout(resolve, 2));

        requestRefresh();
        await settle();

        const refreshed = holder.pages.at(-1)?.request;
        expect(refreshed?.filter?.from).toBeGreaterThan(first?.filter?.from ?? 0);

        // The refresh cycle fails. The rows in hand are still the first
        // stream's, so the continuation must stay on their own bounds — not
        // the failed cycle's.
        holder.pages.at(-1)?.refuse(new Error("refresh refused"));
        await settle();
        expect(bodyText()).toContain("refresh refused");
        expect(bodyText()).toContain("Load more");

        buttonWithText("Load more").click();
        await settle();

        const continued = holder.pages.at(-1)?.request;

        expect(continued?.before).toBe(API_LOG_CURSOR_BASE + 951);
        expect(continued?.filter?.from).toBe(first?.filter?.from);

        answerLastPage(holder, page(950, 3));
        await settle();

        // A landed continuation retires only its own failure; the failed
        // refresh's diagnosis stays stated where reads state theirs.
        expect(bodyText()).toContain("refresh refused");
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

    it("holds Apply filters while the typed status list does not parse", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();
        answerLastPage(holder, page(1050, 1));
        await settle();

        // The glob habit "2xx" fills the field and would ride the route, but
        // narrows to nothing on the wire — the unfiltered answer: the apply
        // is held until the field parses whole.
        typeInto(labeledInput("HTTP statuses"), "2xx");
        await settle();

        expect(bodyText()).toContain("HTTP statuses must be plain integers");
        expect(buttonWithText("Apply filters").disabled).toBe(true);

        buttonWithText("Apply filters").click();
        await settle();
        expect(`${window.location.pathname}${window.location.search}`).toBe("/usage");

        // Clearing the mistake releases the apply, and the whole list rides
        // the route.
        typeInto(labeledInput("HTTP statuses"), "200,429");
        await settle();

        expect(buttonWithText("Apply filters").disabled).toBe(false);
        buttonWithText("Apply filters").click();
        await settle();

        expect(`${window.location.pathname}${window.location.search}`).toBe(
            "/usage?status=200,429",
        );
    });

    it("carries the stream's one-entity narrow as the aggregates link's subject", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        // Each entity dimension arrives under its own parameter — the key's
        // dimension spells `api_key` on the wire but rides as `key=` — and the
        // window stays behind: the aggregates page's window semantics are its
        // own, so only the entity subject crosses.
        mount(streamApi(holder), {
            ...EMPTY_USAGE_FILTER_DRAFT,
            window: "24h",
            providerId: "3",
        });
        mount(streamApi(holder), {
            ...EMPTY_USAGE_FILTER_DRAFT,
            modelId: "9",
        });
        mount(streamApi(holder), {
            ...EMPTY_USAGE_FILTER_DRAFT,
            apiKeyId: "7",
        });
        await settle();

        // The 24h window stays behind — the aggregates page totals all time —
        // so that door states the trade beside its label, while a draft
        // holding nothing beyond the one entity narrow reads its bare label:
        // there, nothing the operator set stays behind.
        expect(linkTo("/usage/query?provider=3").textContent).toBe(
            "View aggregates — the window and every other filter stay behind.",
        );
        expect(linkTo("/usage/query?model=9").textContent).toBe("View aggregates");
        expect(linkTo("/usage/query?key=7").textContent).toBe("View aggregates");

        for (const held of holder.pages) {
            held.answer({
                api_logs: [],
            });
        }
        await settle();
    });

    it("links the bare aggregates page when the narrow names no single entity", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        // Two entity fields at once: a subject is one entity, so a compound
        // narrow carries none rather than picking one.
        mount(streamApi(holder), {
            ...EMPTY_USAGE_FILTER_DRAFT,
            providerId: "3",
            modelId: "9",
        });
        // An entity-free narrow — window, outcome, status only — and the
        // every-row stream land on the bare page alike.
        mount(streamApi(holder), filtered);
        mount(streamApi(holder));
        await settle();

        // The compound narrow crosses no subject and everything set stays
        // behind, so the bare page's door states that trade beside its label.
        expect(linkTo("/usage/query").textContent).toBe(
            "View aggregates — the window and every other filter stay behind.",
        );
        expect(() => linkTo("/usage/query?provider=3")).toThrow(
            "No link to /usage/query?provider=3",
        );
        expect(() => linkTo("/usage/query?key=7")).toThrow("No link to /usage/query?key=7");

        for (const held of holder.pages) {
            held.answer({
                api_logs: [],
            });
        }
        await settle();
    });

    it("follows the standing draft's entity narrow across the aggregates door before it is applied", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        const doorProvider = providerRow({
            id: 3,
            name: "door-provider",
            model_prefix: "door-",
        });
        mount(
            streamApi(holder, {
                providers: [
                    doorProvider,
                ],
            }),
        );
        await settle();
        answerLastPage(holder, page(1050, 1));
        await settle();

        // The unfiltered stream's door opens the bare aggregates page, its
        // bare label stating no trade — nothing the operator set stays
        // behind.
        expect(linkTo("/usage/query").textContent).toBe("View aggregates");

        // The operator narrows the form's provider field without applying:
        // the door crosses with the standing draft's narrow — the same source
        // the aggregates page's own door carries back — so a narrow the
        // operator holds in the form cannot be silently dropped by the
        // crossing.
        const providerSelect = labeledSelect("Provider");
        providerSelect.value = "3";
        providerSelect.dispatchEvent(
            new Event("change", {
                bubbles: true,
            }),
        );
        await settle();

        // The provider narrow alone crosses — the window and the rest stand
        // untouched — so the door still reads its bare label.
        expect(linkTo("/usage/query?provider=3").textContent).toBe("View aggregates");
        expect(() => linkTo("/usage/query")).toThrow("No link to /usage/query");
    });

    it("lands a window change delivered in the same batch as a field edit", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();
        answerLastPage(holder, page(1050, 1));
        await settle();

        // Both edits ride one task — the status field's input event, then the
        // window's change event. Signal writes defer to a microtask, so the
        // window's merge must be functional: a field set read inside its own
        // setter would build on the pre-edit draft and drop the typed status.
        typeInto(labeledInput("HTTP statuses"), "200,429");
        const windowSelect = labeledSelect("Window");
        windowSelect.value = "24h";
        windowSelect.dispatchEvent(
            new Event("change", {
                bubbles: true,
            }),
        );
        await settle();

        // Both edits landed: the applied route carries them together.
        buttonWithText("Apply filters").click();
        await settle();

        expect(`${window.location.pathname}${window.location.search}`).toBe(
            "/usage?window=24h&status=200,429",
        );
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
                    providerRow(),
                ],
                models: [
                    modelRow(),
                ],
                apiKeys: [
                    apiKeyRow(),
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

describe("UsageScreen row figures", () => {
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
                    <UsageScreen
                        api={api}
                        filter={EMPTY_USAGE_FILTER_DRAFT}
                    />
                ),
                document.body,
            ),
        );
    }

    it("omits the optional token tail when none of its figures is reported non-zero", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        // Three rows covering the whole suppression surface: the unreported
        // breakdown, the measured all-zero one, and a mixed zero-and-null
        // one. None carries a reported non-zero figure, so none may end in
        // the three-figure tail — a tail of measured-looking zeros no
        // upstream measured.
        answerLastPage(holder, [
            apiLogRow(30, {
                cached_tokens: null,
                cache_write_tokens: null,
                reasoning_tokens: null,
            }),
            apiLogRow(20, {
                cached_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            }),
            apiLogRow(10, {
                cached_tokens: 0,
                cache_write_tokens: null,
                reasoning_tokens: null,
            }),
        ]);
        await settle();

        for (const figure of [
            "cached 0",
            "cache write",
            "reasoning",
        ]) {
            expect(bodyText()).not.toContain(figure);
        }

        // The suppression is stated once, on the stream's standing
        // description: the measured-all-zero reading it gives up is a
        // documented trade, not a silent one.
        expect(bodyText()).toContain("cannot be told apart");
    });

    it("renders the optional token tail whole when any figure is reported", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        // One reported reasoning figure among zeros and unreported columns:
        // the tail renders whole, the measured zero and the unreported
        // dashes exactly as they always did.
        answerLastPage(holder, [
            apiLogRow(30, {
                cached_tokens: 0,
                cache_write_tokens: null,
                reasoning_tokens: 8,
            }),
        ]);
        await settle();

        const figures = paragraphWithText("reasoning 8").textContent ?? "";
        expect(figures).toContain("cached 0");
        expect(figures).toContain("cache write —");
        expect(figures).toContain("reasoning 8");
    });

    it("prints a row's duration as a human duration, not raw milliseconds", async () => {
        const holder: {
            readonly pages: Array<HeldPage>;
        } = {
            pages: [],
        };
        mount(streamApi(holder));
        await settle();

        answerLastPage(holder, [
            apiLogRow(30, {
                duration: 8_100,
            }),
            apiLogRow(20, {
                duration: 123_000,
            }),
        ]);
        await settle();

        const short = paragraphWithText("8.1s");
        expect(short.textContent).toContain("in 10");
        expect(bodyText()).toContain("2m 03s");
        expect(bodyText()).not.toContain(" ms");
    });
});
