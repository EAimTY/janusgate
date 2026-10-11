import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { ApiKeysScreen } from "/src/screens/api-keys/api-keys-screen";

function rejectUnexpected(): Promise<never> {
    return Promise.reject(new Error("This test stubs only the listing calls the screen reads."));
}

/** A client whose listing methods answer; every other route is a loud refusal. */
function stubApi(overrides: Partial<ManagementApi> = {}): ManagementApi {
    const base: ManagementApi = {
        listProviderPools: () => rejectUnexpected(),
        createProviderPool: () => rejectUnexpected(),
        updateProviderPool: () => rejectUnexpected(),
        deleteProviderPool: () => rejectUnexpected(),
        listModelRoutes: () => rejectUnexpected(),
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
        listModels: () => rejectUnexpected(),
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

    return {
        ...base,
        ...overrides,
    };
}

function apiKeyFixture(overrides: Partial<ApiKeyResponse> = {}): ApiKeyResponse {
    return {
        id: 7,
        name: "staging",
        key: "sk-staging-0001",
        is_enabled: true,
        access_control: {
            default_access: "deny",
            owner_defaults: [],
            route_overrides: [],
        },
        created_at: 1_760_000_000_000_000,
        ...overrides,
    };
}

let disposeView: (() => void) | null = null;

function mount(view: () => JSX.Element): HTMLElement {
    const host = document.createElement("div");
    document.body.appendChild(host);
    disposeView = render(view, host);
    return host;
}

/** Retries the assertion while the screen's read cycle is still landing. */
async function settled(assertion: () => void): Promise<void> {
    await vi.waitFor(() => {
        flush();
        assertion();
    });
}

function requireElement<TElement extends Element>(host: ParentNode, selectors: string): TElement {
    const element = host.querySelector<TElement>(selectors);
    if (element === null) {
        throw new Error(`Missing element for "${selectors}".`);
    }

    return element;
}

function setInputValue(element: HTMLInputElement, value: string): void {
    element.value = value;
    element.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
    flush();
}

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("ApiKeysScreen", () => {
    it("carries each key's identity, secret, serving state, and per-key copy action", async () => {
        const api = stubApi({
            listApiKeys: () =>
                Promise.resolve({
                    api_keys: [
                        apiKeyFixture(),
                        apiKeyFixture({
                            id: 9,
                            name: "production",
                            key: "sk-production-0002",
                            is_enabled: false,
                        }),
                    ],
                }),
        });
        const host = mount(() => <ApiKeysScreen api={api} />);

        await settled(() => {
            const rows = Array.from(host.querySelectorAll("li a"));
            expect(rows.map((row) => row.getAttribute("href"))).toEqual([
                "/api-keys/7",
                "/api-keys/9",
            ]);
        });

        expect(host.textContent).toContain("staging");
        expect(host.textContent).toContain("sk-staging-0001");
        expect(host.textContent).toContain("production");
        expect(host.textContent).toContain("sk-production-0002");

        const badges = Array.from(host.querySelectorAll("li span.rounded-full"));
        expect(badges.map((badge) => badge.textContent)).toEqual([
            "On",
            "Off",
        ]);

        // Every row repeats the visible Copy label, so the accessible name
        // says which key's secret is taken.
        const copyButtons = Array.from(host.querySelectorAll("button[aria-label]"));
        expect(copyButtons.map((button) => button.getAttribute("aria-label"))).toEqual([
            "Copy the secret of staging",
            "Copy the secret of production",
        ]);
    });

    it("narrows by name as one types and states the fraction a standing search shows", async () => {
        const api = stubApi({
            listApiKeys: () =>
                Promise.resolve({
                    api_keys: [
                        apiKeyFixture(),
                        apiKeyFixture({
                            id: 9,
                            name: "production",
                            key: "sk-production-0002",
                            is_enabled: false,
                        }),
                    ],
                }),
        });
        const host = mount(() => <ApiKeysScreen api={api} />);

        await settled(() => {
            expect(host.querySelectorAll("li a").length).toBe(2);
        });

        const search = requireElement<HTMLInputElement>(host, "input[type='search']");
        setInputValue(search, "STAGING");
        await settled(() => {
            expect(host.querySelectorAll("li a").length).toBe(1);
            expect(host.querySelector("p[role='status']")?.textContent).toBe(
                "1 key shown · 1 of 2 keys enabled",
            );
        });
    });

    it("states the query when nothing matches instead of an empty listing", async () => {
        const api = stubApi({
            listApiKeys: () =>
                Promise.resolve({
                    api_keys: [
                        apiKeyFixture(),
                    ],
                }),
        });
        const host = mount(() => <ApiKeysScreen api={api} />);

        await settled(() => {
            expect(host.querySelectorAll("li a").length).toBe(1);
        });

        const search = requireElement<HTMLInputElement>(host, "input[type='search']");
        setInputValue(search, "zzz");
        await settled(() => {
            expect(host.querySelectorAll("li a").length).toBe(0);
            expect(host.textContent).toContain('No API keys match "zzz".');
        });
    });

    it("offers creation from an empty listing", async () => {
        const api = stubApi({
            listApiKeys: () =>
                Promise.resolve({
                    api_keys: [],
                }),
        });
        const host = mount(() => <ApiKeysScreen api={api} />);

        await settled(() => {
            expect(host.textContent).toContain("No API keys yet");
            const create = Array.from(host.querySelectorAll("a")).find((link) =>
                link.textContent?.includes("Create API key"),
            );
            expect(create?.getAttribute("href")).toBe("/api-keys/new");
        });
    });
});
