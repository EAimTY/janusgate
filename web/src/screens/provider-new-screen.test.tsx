import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { CreateProviderRequest } from "/src/management-api/generated/v0";
import { ProviderNewScreen } from "/src/screens/provider-new-screen";

/**
 * A management API standing in for the gateway: `providers:create` records
 * its request for assertions on what the form sent; every other route
 * refuses, so an unexpected call fails the test loudly.
 */
function screenApi(sent: CreateProviderRequest[] = []): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
        listProviders: () =>
            Promise.resolve({
                providers: [],
            }),
        createProvider: (request) => {
            sent.push(request);
            return Promise.resolve({
                provider: {
                    type: "openai_response",
                    id: 1,
                    name: request.name,
                    model_prefix: request.model_prefix,
                    is_enabled: true,
                    base_url: "https://upstream.example.com",
                },
            });
        },
        updateProvider: refused,
        deleteProvider: refused,
        startProviderOauth: refused,
        completeProviderOauth: refused,
        reauthorizeProvider: refused,
        listModels: refused,
        discoverModels: refused,
        updateModel: refused,
        createModel: refused,
        deleteModel: refused,
        listUsage: refused,
        queryUsage: refused,
        listApiKeys: refused,
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

function labeledInput(label: string): HTMLInputElement {
    for (const candidate of document.querySelectorAll("label")) {
        const id = candidate.getAttribute("for");

        if (candidate.textContent === label && id !== null) {
            const input = document.getElementById(id);

            if (input instanceof HTMLInputElement) {
                return input;
            }
        }
    }

    throw new Error(`No input labeled ${label}`);
}

function buttonWithText(text: string): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error(`No button reading ${text}`);
}

/** Types into an input the way a browser delivers an edit. */
function typeInto(input: HTMLInputElement, value: string): void {
    input.value = value;
    input.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
}

function switchWithLabel(label: string): HTMLButtonElement {
    const candidate = document.querySelector(`button[role='switch'][aria-label='${label}']`);

    if (candidate === null) {
        throw new Error(`No switch labeled ${label}`);
    }

    return candidate as HTMLButtonElement;
}

describe("ProviderNewScreen creation", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("sends a typed custom prefix byte-for-byte in the create request", async () => {
        const sent: CreateProviderRequest[] = [];
        disposes.push(render(() => <ProviderNewScreen api={screenApi(sent)} />, document.body));
        await settle();

        // The typed prefix carries its own trailing slash, and the field
        // promises it is used exactly as typed (gateway README §7 stores the
        // prefix verbatim and recomposes every unoverridden public ID from
        // exactly what the write states) — creation must match the edit
        // path's byte-for-byte rule.
        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "beta/");
        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        typeInto(labeledInput("API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.model_prefix).toBe("beta/");
    });
});
