import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    QueryUsageRequest,
    QueryUsageResponse,
    UsageBucket,
} from "/src/management-api/generated/v0";
import { HomeScreen } from "/src/screens/home-screen";
import { settle, stubManagementApi } from "./screen-test-support";

const BASE_URL = "https://gateway.example.com";

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

/** One model of the enabled provider: effectively routing, by the gateway's three inputs. */
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

/** One vanished model: its row exists, but the gateway does not route it. */
const vanishedModelRow: ModelResponse = {
    ...modelRow,
    id: 10,
    upstream_id: "qwen3:14b",
    public_id: "local-qwen3-14b",
    discovery_state: "vanished",
};

const apiKeyRow: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
};

/** One OAuth provider whose authorization lapsed long ago. */
const expiredOauthProviderRow: ProviderResponse = {
    type: "oauth",
    id: 2,
    name: "codex",
    model_prefix: "codex-",
    is_enabled: true,
    provider_definition_name: "codex-oauth",
    provider_definition_version: 1,
    format: "openai_response",
    base_url: null,
    expires_at: 1,
};

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
    } = {},
): ManagementApi {
    return stubManagementApi({
        listProviders: () =>
            Promise.resolve({
                providers: [
                    ...(listing.providers ?? [
                        providerRow,
                    ]),
                ],
            }),
        listModels: () =>
            Promise.resolve({
                models: [
                    ...(listing.models ?? [
                        modelRow,
                        vanishedModelRow,
                    ]),
                ],
            }),
        listApiKeys: () =>
            Promise.resolve({
                api_keys: [
                    apiKeyRow,
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

/** The chart's bars, by their accessible names. */
function barLabels(): ReadonlyArray<string> {
    return Array.from(document.querySelectorAll("span[role='img']")).map(
        (node) => node.getAttribute("aria-label") ?? "",
    );
}

describe("HomeScreen", () => {
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
        // three-input status, derived from the rows, never the row count.
        expect(linkTo("/models").textContent).toContain("Models routing");
        expect(linkTo("/models").textContent).toContain("1");

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
        expect(linkTo("/providers/2/edit").textContent).toContain("Reconnect");
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
        const clientStep = body.indexOf("Point a client at an endpoint below");

        expect(keyStep).toBeGreaterThanOrEqual(0);
        expect(clientStep).toBeGreaterThan(keyStep);
        expect(body).not.toContain("Re-enable the provider");
    });

    it("sends an enabled provider whose every model vanished to the attention list", async () => {
        // The upstream silently dropped its whole catalog: every row is
        // vanished, the provider is enabled, nothing serves — the quietest
        // failure mode, and exactly what the attention line exists for.
        mount(
            overviewApi([], {
                providers: [
                    providerRow,
                ],
                models: [
                    vanishedModelRow,
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain("has no models serving yet.");
        expect(linkTo("/providers/1").textContent).toContain("Open and discover");
    });

    it("gives a disabled provider no attention line even while nothing of it serves", async () => {
        // The operator turned this provider off: its emptiness is a choice,
        // not a fault the overview must report.
        mount(
            overviewApi([], {
                providers: [
                    {
                        ...providerRow,
                        is_enabled: false,
                    },
                ],
                models: [
                    vanishedModelRow,
                ],
            }),
        );
        await settle();

        expect(bodyText()).toContain("Everything looks healthy.");
        expect(bodyText()).not.toContain("has no models serving yet.");
    });

    it("gives a provider whose models serve no attention line", async () => {
        mount(overviewApi([]));
        await settle();

        expect(bodyText()).toContain("Everything looks healthy.");
        expect(bodyText()).not.toContain("has no models serving yet.");
    });
});
