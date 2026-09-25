import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ModelsScreen } from "/src/screens/models-screen";

/**
 * A provider whose name shares nothing with its model's ids, so a search
 * matching the model can only be matching the provider the rows display.
 */
const providerRow: ProviderResponse = {
    type: "api_key",
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    id: 3,
    name: "mix-ops",
    model_prefix: "",
    is_enabled: true,
    base_url: "https://upstream.example.com/v1",
};

const modelRow: ModelResponse = {
    id: 9,
    provider_id: 3,
    provider: "mix-ops",
    upstream_id: "llama3-70b",
    public_id: "llama3-70b",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

/**
 * A management API whose two listings answer from fixtures; every other
 * route rejects, so an unexpected call fails the test loudly instead of
 * hanging it.
 */
function screenApi(options: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
}): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
        listProviderDefinitions: refused,
        createProviderDefinition: refused,
        updateProviderDefinition: refused,
        listProviders: () =>
            Promise.resolve({
                providers: [
                    ...options.providers,
                ],
            }),
        listModels: () =>
            Promise.resolve({
                models: [
                    ...options.models,
                ],
            }),
        listApiKeys: refused,
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

/** Types into the models search field the way a browser delivers keystrokes. */
function typeIntoSearch(value: string): void {
    const input = document.getElementById("models-search");

    if (input instanceof HTMLInputElement) {
        input.value = value;
        input.dispatchEvent(
            new Event("input", {
                bubbles: true,
            }),
        );
        return;
    }

    throw new Error("No models search input");
}

function bodyText(): string {
    return document.body.textContent ?? "";
}

function linkTo(href: string): HTMLAnchorElement {
    for (const candidate of document.querySelectorAll("a")) {
        if (candidate.getAttribute("href") === href) {
            return candidate;
        }
    }

    throw new Error(`No link to ${href}`);
}

describe("ModelsScreen empty gateway", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("makes the provider the first step when nothing exists yet", async () => {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [],
                            models: [],
                        })}
                        providerFilter={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // With no provider, model creation dead-ends in its own no-provider
        // state, so the fallback's first step is the provider — and its
        // models arrive by discovering them, never "syncing".
        expect(bodyText()).toContain(
            "Add a provider first, then discover its models — or add one by hand.",
        );

        const createProvider = linkTo("/providers/new");
        expect(createProvider.textContent).toContain("Create provider");
        expect(createProvider.className).toContain("bg-accent");

        // The model step stays offered beside it, unaccented: it leads
        // somewhere only once a provider exists.
        const createModel = Array.from(document.querySelectorAll('a[href="/models/new"]')).at(-1);
        expect(createModel?.textContent).toContain("Create model");
    });
});

describe("ModelsScreen search", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(): void {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [
                                providerRow,
                            ],
                            models: [
                                modelRow,
                            ],
                        })}
                        providerFilter={null}
                    />
                ),
                document.body,
            ),
        );
    }

    it("matches the owning provider's name, not only the model's ids", async () => {
        mount();
        await settle();

        typeIntoSearch("mix");
        await settle();

        expect(bodyText()).toContain("llama3-70b");
        expect(bodyText()).not.toContain("No models match");

        typeIntoSearch("zzz");
        await settle();

        expect(bodyText()).toContain('No models match "zzz"');
    });
});
