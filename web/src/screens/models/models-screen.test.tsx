import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ListModelsRequest,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { ModelsScreen } from "/src/screens/models/models-screen";

function rejectUnexpected(): Promise<never> {
    return Promise.reject(new Error("This test stubs only the listing calls the screen reads."));
}

const PROVIDERS: ReadonlyArray<ProviderResponse> = [
    {
        type: "api_key",
        id: 3,
        name: "Acme",
        model_prefix: "acme/",
        is_enabled: true,
        provider_definition_name: "acme",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
    },
    {
        type: "api_key",
        id: 4,
        name: "Beta",
        model_prefix: "",
        is_enabled: true,
        provider_definition_name: "beta",
        provider_definition_version: 1,
        format: "anthropic_messages",
        base_url: null,
    },
];

function modelFixture(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
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
        ...overrides,
    };
}

/** The inventory a test asks for: `withModels` and `withProviders` seed the listings. */
function stubApi(
    withModels: ReadonlyArray<ModelResponse>,
    withProviders: ReadonlyArray<ProviderResponse> = PROVIDERS,
): ManagementApi {
    return {
        listProviderPools: () =>
            Promise.resolve({
                provider_pools: [],
            }),
        createProviderPool: () => rejectUnexpected(),
        updateProviderPool: () => rejectUnexpected(),
        deleteProviderPool: () => rejectUnexpected(),
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [],
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
        listProviders: () =>
            Promise.resolve({
                providers: [
                    ...withProviders,
                ],
            }),
        createProvider: () => rejectUnexpected(),
        updateProvider: () => rejectUnexpected(),
        deleteProvider: () => rejectUnexpected(),
        startProviderOauth: () => rejectUnexpected(),
        completeProviderOauth: () => rejectUnexpected(),
        reauthorizeProvider: () => rejectUnexpected(),
        listModels: (request: ListModelsRequest) =>
            Promise.resolve({
                models: withModels.filter(
                    (model) =>
                        request.provider_id === undefined ||
                        model.provider_id === request.provider_id,
                ),
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

function requireElement<TElement extends Element>(host: ParentNode, selectors: string): TElement {
    const element = host.querySelector<TElement>(selectors);
    if (element === null) {
        throw new Error(`Missing element for "${selectors}".`);
    }

    return element;
}

async function waitFor(assertion: () => void): Promise<void> {
    await vi.waitFor(() => {
        flush();
        assertion();
    });
}

function setSelectValue(element: HTMLSelectElement, value: string): void {
    element.value = value;
    element.dispatchEvent(
        new Event("change", {
            bubbles: true,
        }),
    );
    flush();
}

function typeInto(element: HTMLInputElement, value: string): void {
    element.value = value;
    element.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
    flush();
}

beforeEach(() => {
    window.scrollTo = () => {};
    window.history.replaceState(null, "", "/");
});

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
    window.history.replaceState(null, "", "/");
});

describe("ModelsScreen", () => {
    it("rows open the model's detail page and the search narrows by upstream ID or provider name", async () => {
        const api = stubApi([
            modelFixture(),
            modelFixture({
                id: 12,
                provider_id: 4,
                provider: "Beta",
                upstream_id: "small",
            }),
        ]);
        const host = mount(() => (
            <ModelsScreen
                api={api}
                providerFilter={null}
            />
        ));

        await waitFor(() => {
            const rows = Array.from(host.querySelectorAll("a.group"));
            expect(rows.map((row) => row.getAttribute("href"))).toEqual([
                "/models/11",
                "/models/12",
            ]);
        });

        const search = requireElement<HTMLInputElement>(host, "input[type='search']");
        typeInto(search, "ACME");
        await waitFor(() => {
            const rows = Array.from(host.querySelectorAll("a.group"));
            expect(rows.map((row) => row.getAttribute("href"))).toEqual([
                "/models/11",
            ]);
        });
    });

    it("keeps an unlisted filter's value in force in the provider select", async () => {
        const api = stubApi([]);
        const host = mount(() => (
            <ModelsScreen
                api={api}
                providerFilter={99}
            />
        ));

        await waitFor(() => {
            const select = requireElement<HTMLSelectElement>(host, "select");
            const trailing = select.options[select.options.length - 1];
            expect(trailing?.value).toBe("99");
            expect(trailing?.textContent).toBe("99 (no longer listed)");
            expect(select.value).toBe("99");
        });
    });

    it("applies the filter by navigating to the filtered address", async () => {
        const api = stubApi([
            modelFixture(),
        ]);
        const host = mount(() => (
            <ModelsScreen
                api={api}
                providerFilter={null}
            />
        ));

        await waitFor(() => {
            expect(host.querySelectorAll("a.group").length).toBe(1);
        });

        const select = requireElement<HTMLSelectElement>(host, "select");
        setSelectValue(select, "4");

        expect(window.location.pathname).toBe("/models");
        expect(window.location.search).toBe("?provider=4");
    });

    it("states exactly the narrows an empty answer names", async () => {
        // Nothing at all: the gateway holds no inventory yet.
        const bare = mount(() => (
            <ModelsScreen
                api={stubApi([], [])}
                providerFilter={null}
            />
        ));
        await waitFor(() => {
            expect(bare.textContent).toContain("No models yet");
        });

        // A listed provider holding no models: the provider's own verdict.
        const filtered = mount(() => (
            <ModelsScreen
                api={stubApi([])}
                providerFilter={4}
            />
        ));
        await waitFor(() => {
            expect(filtered.textContent).toContain("This provider holds no models.");
        });

        // A provider the listing no longer carries: the listing fact, whose
        // select option already states the same.
        const unlisted = mount(() => (
            <ModelsScreen
                api={stubApi([])}
                providerFilter={99}
            />
        ));
        await waitFor(() => {
            expect(unlisted.textContent).toContain(
                "This provider is no longer listed — pick another in the Provider filter.",
            );
        });

        // A standing search that matches nothing quotes the query.
        const searching = mount(() => (
            <ModelsScreen
                api={stubApi([
                    modelFixture(),
                ])}
                providerFilter={null}
            />
        ));
        await waitFor(() => {
            expect(searching.querySelectorAll("a.group").length).toBe(1);
        });
        typeInto(requireElement<HTMLInputElement>(searching, "input[type='search']"), "zzz");
        await waitFor(() => {
            expect(searching.textContent).toContain('No models match "zzz".');
        });
    });
});
