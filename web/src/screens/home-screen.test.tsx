import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiKeyResponse,
    ModelResponse,
    ModelRouteResponse,
    ProviderResponse,
    QueryUsageRequest,
    QueryUsageResponse,
    UsageBucket,
} from "/src/management-api/generated/v0";
import { HomeScreen } from "/src/screens/home-screen";
import {
    apiKeyRow,
    bodyText,
    createDisposals,
    linkTo,
    modelRow,
    providerRow,
    settle,
    stubListings,
    stubManagementApi,
} from "./screen-test-support";

const BASE_URL = "https://gateway.example.com";

/** One vanished model: its row exists, but the gateway does not route it. */
const vanishedModelRow: ModelResponse = modelRow({
    id: 10,
    upstream_id: "qwen3:14b",

    discovery_state: "vanished",
});

/** One OAuth provider whose authorization lapsed long ago. */
const expiredOauthProviderRow: ProviderResponse = providerRow({
    model_prefix: "",
    type: "oauth",
    id: 2,
    name: "codex",
    provider_definition_name: "codex-oauth",
    format: "openai_response",
    base_url: null,
    expires_at: 1,
});

/** The same lapsed provider parked: the operator switched it off. */
const parkedExpiredOauthProviderRow: ProviderResponse = {
    ...expiredOauthProviderRow,
    is_enabled: false,
};

/** A second, enabled provider holding no model rows at all. */
const emptyProviderRow: ProviderResponse = providerRow({
    id: 2,
    name: "spare",
});

function bucket(key: UsageBucket["key"], requests: number): UsageBucket {
    return {
        key,
        requests,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

/** The UTC day one instant falls in, spelled the way the gateway spells it. */
function utcDay(offsetDays: number): string {
    return new Date(Date.now() + offsetDays * 86_400_000).toISOString().slice(0, 10);
}

/**
 * A gateway with one provider, one routing model, one vanished model, and one
 * API key, whose accounting answers 45 requests in the rolling window — five
 * of them failures — and two days of daily buckets. The listings can be
 * replaced wholesale for screens that need a different gateway. Every other
 * route rejects, so an unexpected call fails the test loudly instead of
 * hanging it.
 */
function overviewApi(
    requests: Array<QueryUsageRequest>,
    listing: {
        readonly providers?: ReadonlyArray<ProviderResponse>;
        readonly models?: ReadonlyArray<ModelResponse>;
        readonly modelRoutes?: ReadonlyArray<ModelRouteResponse>;
        readonly apiKeys?: ReadonlyArray<ApiKeyResponse>;
    } = {},
): ManagementApi {
    return stubManagementApi({
        ...stubListings({
            providers: listing.providers ?? [
                providerRow(),
            ],
            models: listing.models ?? [
                modelRow(),
                vanishedModelRow,
            ],
            apiKeys: listing.apiKeys ?? [
                apiKeyRow(),
            ],
        }),
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [
                    ...(listing.modelRoutes ?? [
                        {
                            is_automatic: false,
                            id: 17,
                            public_id: "coding",
                            is_enabled: true,
                            target: {
                                type: "direct",
                                model_id: 9,
                            },
                        },
                        {
                            is_automatic: false,
                            id: 18,
                            public_id: "paused",
                            is_enabled: false,
                            target: {
                                type: "direct",
                                model_id: 9,
                            },
                        },
                    ]),
                ],
            }),
        queryUsage: (request): Promise<QueryUsageResponse> => {
            requests.push(request);

            return Promise.resolve({
                buckets: request.group_by.includes("outcome")
                    ? [
                          bucket(
                              {
                                  outcome: "completed",
                              },
                              40,
                          ),
                          bucket(
                              {
                                  outcome: "upstream_error",
                              },
                              5,
                          ),
                      ]
                    : [
                          bucket(
                              {
                                  day: utcDay(0),
                              },
                              40,
                          ),
                          bucket(
                              {
                                  day: utcDay(-3),
                              },
                              5,
                          ),
                      ],
            });
        },
    });
}

/** The chart's bars, by their accessible names. */
function barLabels(): ReadonlyArray<string> {
    return Array.from(document.querySelectorAll("span[role='img']")).map(
        (node) => node.getAttribute("aria-label") ?? "",
    );
}

/**
 * One checklist row of the getting-started list, by its title. An undone row
 * renders its index number in place of the check, so the row's own text tells
 * the two shapes apart.
 */
function stepRow(title: string): HTMLLIElement {
    for (const candidate of document.querySelectorAll("li")) {
        if (candidate.textContent?.includes(title) === true) {
            return candidate;
        }
    }

    throw new Error(`No getting-started step reading ${title}`);
}

describe("HomeScreen", () => {
    const disposes = createDisposals();

    function mount(api: ManagementApi): void {
        disposes.push(
            render(
                () => (
                    <HomeScreen
                        api={api}
                        baseUrl={BASE_URL}
                    />
                ),
                document.body,
            ),
        );
    }

    it("states every figure as a door to the page that explains it", async () => {
        mount(overviewApi([]));
        await settle();

        expect(linkTo("/providers").textContent).toContain("Providers");
        expect(linkTo("/providers").textContent).toContain("1");

        // One routing model, one vanished row: the figure is the gateway's
        // three-input status, derived from the rows, never the row count —
        // and the hint names the listed total the figure selects from, so
        // the smaller serving count cannot read as a wrong count.
        expect(linkTo("/models").textContent).toContain("Inventory ready");
        expect(linkTo("/models").textContent).toContain("1");
        expect(linkTo("/models").textContent).toContain("ready of 2 listed");
        expect(linkTo("/routes").textContent).toContain("Public routes");
        expect(linkTo("/routes").textContent).toContain("1");
        expect(linkTo("/routes").textContent).toContain("enabled of 2 stored");

        expect(linkTo("/api-keys").textContent).toContain("API keys");
        expect(linkTo("/api-keys").textContent).toContain("1");

        // The rolling window is a route, so the figure opens the listing it
        // counted — and the error figure opens the failures among them.
        expect(linkTo("/usage?window=24h").textContent).toContain("45");
        expect(linkTo("/usage?window=24h&outcome=upstream_error,internal").textContent).toContain(
            "5",
        );
    });

    it("reads the rolling window and the daily axis from the gateway's own buckets", async () => {
        const requests: Array<QueryUsageRequest> = [];

        mount(overviewApi(requests));
        await settle();

        expect(requests).toHaveLength(2);
        expect(requests[0]?.group_by).toEqual([
            "outcome",
        ]);
        expect(requests[0]?.filter?.from).toBeTypeOf("number");
        expect(requests[1]?.group_by).toEqual([
            "day",
        ]);
        expect(requests[1]?.filter?.from).toBeTypeOf("number");
        expect(requests[1]?.filter?.to).toBeTypeOf("number");

        const bars = barLabels();

        expect(bars).toHaveLength(14);
        expect(bars.at(-1)).toBe(`${utcDay(0)}: 40 requests`);
        expect(bars.at(-4)).toBe(`${utcDay(-3)}: 5 requests`);
        // A quiet day is a zero bar, not a missing one.
        expect(bars.at(-2)).toBe(`${utcDay(-1)}: 0 requests`);
        expect(bodyText()).toContain(`45 requests · peak 40 on ${utcDay(0)}`);
    });

    it("points clients at every outward surface the gateway serves", async () => {
        mount(overviewApi([]));
        await settle();

        expect(bodyText()).toContain(`${BASE_URL}/api/openai-v1`);
        expect(bodyText()).toContain(`${BASE_URL}/api/gemini-v1beta`);
        expect(bodyText()).toContain(`${BASE_URL}/api/anthropic-v1`);
    });

    it("sends a lapsed credential to the edit page, where reauthorization lives", async () => {
        mount(
            overviewApi([], {
                providers: [
                    expiredOauthProviderRow,
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain("Credential expired");
        // The enabled shape: straight at the reauthorization, no parked
        // sentence — the operator never deferred anything here.
        expect(linkTo("/providers/2/edit").textContent).toContain("Reconnect");
        expect(bodyText()).not.toContain("The provider is disabled");
    });

    it("differentiates a parked provider's expired credential from an enabled one's", async () => {
        // The operator parked this provider: the credential still counts —
        // re-enabling it would fail every request until reconnected — but
        // the row must not read as a reauthorization order. It names the
        // disabled state and opens the read-only detail page, whose
        // *Edit provider* door reaches the edit page's switch, instead of
        // pointing straight at the edit page.
        mount(
            overviewApi([], {
                providers: [
                    parkedExpiredOauthProviderRow,
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain("Credential expired");
        expect(bodyText()).toContain("The provider is disabled — reconnect when you re-enable it.");
        expect(linkTo("/providers/2").textContent).toContain("Open provider");
    });

    it("lists no pre-satisfied step on a fresh gateway, the key issued before any client is pointed", async () => {
        mount(
            overviewApi([], {
                providers: [],
            }),
        );
        await settle();

        // Every provider is created enabled, so no step worries about the
        // switch. The order is the Endpoints card's own: a client pointed
        // without a key gets 401s, so issuing the key comes first.
        const body = bodyText();
        const keyStep = body.indexOf("Issue an API key for serving clients");
        const discoverStep = body.indexOf("Discover its models from upstream");
        const clientStep = body.indexOf("Point a client at an endpoint below");

        expect(keyStep).toBeGreaterThanOrEqual(0);
        expect(discoverStep).toBeGreaterThanOrEqual(0);
        expect(keyStep).toBeGreaterThan(discoverStep);
        expect(clientStep).toBeGreaterThan(keyStep);
        expect(body).not.toContain("Re-enable the provider");

        // The listing answers one key, and keys can exist with no provider
        // standing — the Endpoints card below links them — so the key step
        // reads done however early it was issued.
        expect(stepRow("Issue an API key for serving clients").textContent).not.toContain("5");
    });

    it("leaves the key step unchecked while no key stands", async () => {
        mount(
            overviewApi([], {
                providers: [],
                apiKeys: [],
            }),
        );
        await settle();

        expect(stepRow("Issue an API key for serving clients").textContent).toContain("4");
    });

    it("sends an enabled provider whose every model vanished to the attention list", async () => {
        // The upstream silently dropped its whole catalog: every row is
        // vanished, the provider is enabled, nothing serves — the quietest
        // failure mode, and exactly what the attention line exists for. Rows
        // that fail by themselves get the models page, not the discovery
        // promise discovery cannot keep for a switched-off row.
        mount(
            overviewApi([], {
                providers: [
                    providerRow(),
                ],
                models: [
                    vanishedModelRow,
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain(
            "has no ready inventory — every model is switched off or vanished.",
        );
        expect(linkTo("/models?provider=1").textContent).toContain("Review models");
    });

    it("points a provider with no model rows at its discovery", async () => {
        // One provider routes a model; a second enabled provider holds no
        // rows at all. Its line must not borrow the other provider's rows:
        // its cause is a missing discovery, not rows failing by themselves.
        mount(
            overviewApi([], {
                providers: [
                    providerRow(),
                    emptyProviderRow,
                ],
                models: [
                    modelRow(),
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain("has no models yet.");
        expect(bodyText()).not.toContain(
            "has no ready inventory — every model is switched off or vanished.",
        );
        expect(linkTo("/providers/2").textContent).toContain("Open and discover");
    });

    it("gives a disabled provider no attention line even while nothing of it serves", async () => {
        // The operator turned this provider off: its emptiness is a choice,
        // not a fault the overview must report.
        mount(
            overviewApi([], {
                providers: [
                    providerRow({
                        is_enabled: false,
                    }),
                ],
                models: [
                    vanishedModelRow,
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain("Stored settings reviewed.");
        expect(bodyText()).not.toContain("has no models serving yet.");
    });

    it("guides discovery when no callable model entries exist", async () => {
        mount(
            overviewApi([], {
                modelRoutes: [],
            }),
        );
        await settle();

        expect(bodyText()).toContain(
            "No models are available. Discover models on an enabled provider.",
        );
        expect(
            Array.from(document.querySelectorAll('a[href="/models"]')).some((link) =>
                link.textContent?.includes("Discover models"),
            ),
        ).toBe(true);
        expect(bodyText()).not.toContain("has no models serving yet.");
    });
});
