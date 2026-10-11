import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
} from "/src/management-api/generated/v0";
import { ModelRoutesScreen } from "/src/screens/model-routes/model-routes-screen";

function rejectUnexpected(): Promise<never> {
    return Promise.reject(new Error("This test stubs only the listing calls the screen reads."));
}

const MODELS: ReadonlyArray<ModelResponse> = [
    {
        id: 11,
        provider_id: 3,
        provider: "Acme",
        upstream_id: "large",
        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: null,
        compaction_support_override: null,
    },
];

const POOLS: ReadonlyArray<ProviderPoolResponse> = [
    {
        id: 5,
        name: "Voice",
        model_prefix: "voice/",
        is_enabled: true,
        members: [],
    },
];

const ROUTES: ReadonlyArray<ModelRouteResponse> = [
    {
        id: 21,
        public_id: "acme/large",
        is_automatic: true,
        is_enabled: true,
        target: {
            type: "direct",
            model_id: 11,
        },
    },
    {
        id: 22,
        public_id: "voice/fast",
        is_automatic: true,
        is_enabled: true,
        target: {
            type: "pool",
            pool_id: 5,
            upstream_id: "fast",
        },
    },
    {
        id: 23,
        public_id: "acme-alias",
        is_automatic: false,
        is_enabled: false,
        target: {
            type: "direct",
            model_id: 11,
        },
    },
    {
        id: 24,
        public_id: "ghost",
        is_automatic: false,
        is_enabled: true,
        target: {
            type: "direct",
            model_id: 999,
        },
    },
];

function stubApi(routes: ReadonlyArray<ModelRouteResponse>): ManagementApi {
    return {
        listProviderPools: () =>
            Promise.resolve({
                provider_pools: [
                    ...POOLS,
                ],
            }),
        createProviderPool: () => rejectUnexpected(),
        updateProviderPool: () => rejectUnexpected(),
        deleteProviderPool: () => rejectUnexpected(),
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [
                    ...routes,
                ],
            }),
        createModelRoute: () => rejectUnexpected(),
        updateModelRoute: () => rejectUnexpected(),
        deleteModelRoute: () => rejectUnexpected(),
        getProviderAvailability: () => rejectUnexpected(),
        resetProviderAvailability: () => rejectUnexpected(),
        listProviderDefinitions: () => rejectUnexpected(),
        createProviderDefinition: () => rejectUnexpected(),
        updateProviderDefinition: () => rejectUnexpected(),
        getProviderQuota: () => rejectUnexpected(),
        listProviders: () => rejectUnexpected(),
        createProvider: () => rejectUnexpected(),
        updateProvider: () => rejectUnexpected(),
        deleteProvider: () => rejectUnexpected(),
        startProviderOauth: () => rejectUnexpected(),
        completeProviderOauth: () => rejectUnexpected(),
        reauthorizeProvider: () => rejectUnexpected(),
        listModels: () =>
            Promise.resolve({
                models: [
                    ...MODELS,
                ],
            }),
        discoverModels: () => rejectUnexpected(),
        updateModel: () => rejectUnexpected(),
        createModel: () => rejectUnexpected(),
        deleteModel: () => rejectUnexpected(),
        listUsage: () => rejectUnexpected(),
        queryUsage: () => rejectUnexpected(),
        listApiKeys: () => rejectUnexpected(),
        createApiKey: () => rejectUnexpected(),
        updateApiKey: () => rejectUnexpected(),
        deleteApiKey: () => rejectUnexpected(),
    };
}

let disposeView: (() => void) | null = null;

function mount(view: () => JSX.Element): HTMLElement {
    const host = document.createElement("div");
    document.body.appendChild(host);
    disposeView = render(view, host);
    return host;
}

async function waitFor(assertion: () => void): Promise<void> {
    await vi.waitFor(() => {
        flush();
        assertion();
    });
}

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("ModelRoutesScreen", () => {
    it("routes each row to its own address: automatic entries to their owner, authored entries to their editor", async () => {
        const host = mount(() => <ModelRoutesScreen api={stubApi(ROUTES)} />);

        await waitFor(() => {
            const rows = Array.from(host.querySelectorAll("li a"));
            expect(rows.map((row) => row.getAttribute("href"))).toEqual([
                // An automatic direct entry is the model's own name; its row
                // opens the physical model.
                "/models/11",
                // An automatic pool entry opens the pool that owns it.
                "/pools/5/edit",
                // An authored alias edits itself.
                "/routes/23/edit",
                "/routes/24/edit",
            ]);
        });
    });

    it("states each row's target and serving state, and a vanished target stays visible", async () => {
        const host = mount(() => <ModelRoutesScreen api={stubApi(ROUTES)} />);

        await waitFor(() => {
            const rows = Array.from(host.querySelectorAll("li a"));
            expect(rows.map((row) => row.textContent)).toEqual([
                expect.stringContaining("acme/large"),
                expect.stringContaining("voice/fast"),
                expect.stringContaining("acme-alias"),
                expect.stringContaining("ghost"),
            ]);
            expect(rows[0]?.textContent).toContain("Automatic · Direct · Acme · large");
            expect(rows[1]?.textContent).toContain("Automatic · Pool · Voice · fast");
            // A target the inventory no longer holds keeps its row with the
            // raw identifier named.
            expect(rows[3]?.textContent).toContain("Direct · Physical model #999");

            const badges = Array.from(host.querySelectorAll("li span.rounded-full"));
            expect(badges.map((badge) => badge.textContent)).toEqual([
                "On",
                "On",
                "Off",
                "On",
            ]);
        });
    });

    it("offers creation from an empty listing", async () => {
        const host = mount(() => <ModelRoutesScreen api={stubApi([])} />);

        await waitFor(() => {
            expect(host.textContent).toContain("No public routes yet");
        });
    });
});
