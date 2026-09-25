import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ProvidersScreen } from "/src/screens/providers-screen";

/**
 * A provider whose name and credential kind share nothing with its wire
 * format's label, so a search matching it can only be matching the format
 * the row displays.
 */
const providerRow: ProviderResponse = {
    type: "api_key",
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    id: 4,
    name: "mix-ops",
    model_prefix: "mix-",
    is_enabled: true,
    base_url: "https://upstream.example.com/v1",
};

/** A management API whose two listings answer from fixtures; every other route rejects. */
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

/** Types into the provider search field the way a browser delivers keystrokes. */
function typeIntoSearch(value: string): void {
    const input = document.querySelector('input[aria-label="Search providers"]');

    if (input instanceof HTMLInputElement) {
        input.value = value;
        input.dispatchEvent(
            new Event("input", {
                bubbles: true,
            }),
        );
        return;
    }

    throw new Error("No provider search input");
}

function bodyText(): string {
    return document.body.textContent ?? "";
}

describe("ProvidersScreen search", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("matches the wire format the rows display", async () => {
        disposes.push(
            render(
                () => (
                    <ProvidersScreen
                        api={screenApi({
                            providers: [
                                providerRow,
                            ],
                            models: [],
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeIntoSearch("openai");
        await settle();

        expect(bodyText()).toContain("mix-ops");
        expect(bodyText()).not.toContain("No providers match");

        typeIntoSearch("zzz");
        await settle();

        expect(bodyText()).toContain('No providers match "zzz"');
    });
});
