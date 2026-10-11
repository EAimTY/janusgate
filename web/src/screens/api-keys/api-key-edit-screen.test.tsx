import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse, UpdateApiKeyRequest } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ApiKeyEditScreen } from "/src/screens/api-keys/api-key-edit-screen";

const KEY_ID = 7;

function keyFixture(overrides: Partial<ApiKeyResponse> = {}): ApiKeyResponse {
    return {
        id: KEY_ID,
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

function rejectUnexpected(): Promise<never> {
    return Promise.reject(new Error("This test stubs only the calls this screen makes."));
}

/**
 * A client over a mutable stored listing, so a test can play the gateway:
 * `updateApiKey` applies the write to the stored rows, every read answers the
 * listing as it stands, and every other route is a loud refusal.
 */
function stubApi(initialStored: ReadonlyArray<ApiKeyResponse>): {
    readonly api: ManagementApi;
    readonly stored: () => ReadonlyArray<ApiKeyResponse>;
    readonly setStored: (rows: ReadonlyArray<ApiKeyResponse>) => void;
    readonly readCount: () => number;
} {
    let stored = initialStored;
    let reads = 0;

    const api: ManagementApi = {
        listProviderPools: () =>
            Promise.resolve({
                provider_pools: [],
            }),
        createProviderPool: () => rejectUnexpected(),
        updateProviderPool: () => rejectUnexpected(),
        deleteProviderPool: () => rejectUnexpected(),
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [
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
        listProviders: () =>
            Promise.resolve({
                providers: [
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
                ],
            }),
        createProvider: () => rejectUnexpected(),
        updateProvider: () => rejectUnexpected(),
        deleteProvider: () => rejectUnexpected(),
        startProviderOauth: () => rejectUnexpected(),
        completeProviderOauth: () => rejectUnexpected(),
        reauthorizeProvider: () => rejectUnexpected(),
        listModels: () =>
            Promise.resolve({
                models: [
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
                ],
            }),
        discoverModels: () => rejectUnexpected(),
        updateModel: () => rejectUnexpected(),
        createModel: () => rejectUnexpected(),
        deleteModel: () => rejectUnexpected(),
        listUsage: () => rejectUnexpected(),
        queryUsage: () => rejectUnexpected(),
        listApiKeys: () => {
            reads += 1;
            return Promise.resolve({
                api_keys: [
                    ...stored,
                ],
            });
        },
        createApiKey: () => rejectUnexpected(),
        updateApiKey: (request: UpdateApiKeyRequest) => {
            stored = stored.map((row) =>
                row.id === request.api_key_id
                    ? {
                          ...row,
                          name: request.name,
                          key: request.key,
                          is_enabled: request.is_enabled,
                          access_control: request.access_control,
                      }
                    : row,
            );
            return Promise.resolve();
        },
        deleteApiKey: (request: { api_key_id: number }) => {
            stored = stored.filter((row) => row.id !== request.api_key_id);
            return Promise.resolve();
        },
    };

    return {
        api,
        stored: () => stored,
        setStored: (rows) => {
            stored = rows;
        },
        readCount: () => reads,
    };
}

type EditHarness = {
    readonly host: HTMLElement;
    readonly stored: () => ReadonlyArray<ApiKeyResponse>;
    readonly setStored: (rows: ReadonlyArray<ApiKeyResponse>) => void;
    readonly readCount: () => number;
    readonly updateApiKey: ReturnType<
        typeof vi.fn<(request: UpdateApiKeyRequest) => Promise<void>>
    >;
    readonly deleteApiKey: ReturnType<
        typeof vi.fn<(request: { api_key_id: number }) => Promise<void>>
    >;
};

let disposeView: (() => void) | null = null;

function mountView(view: () => JSX.Element): HTMLElement {
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

function inputForLabel(host: ParentNode, labelText: string): HTMLInputElement {
    const label = Array.from(host.querySelectorAll("label")).find(
        (candidate) => candidate.textContent?.trim() === labelText,
    );
    const inputId = label?.getAttribute("for") ?? null;
    const input =
        inputId === null ? null : host.querySelector<HTMLInputElement>(`input[id='${inputId}']`);
    if (input === null) {
        throw new Error(`No input labeled "${labelText}".`);
    }

    return input;
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

function submitForm(host: HTMLElement): void {
    const form = requireElement<HTMLFormElement>(host, "form");
    form.dispatchEvent(
        new Event("submit", {
            bubbles: true,
            cancelable: true,
        }),
    );
    flush();
}

function click(element: HTMLElement): void {
    element.click();
    flush();
}

async function waitFor(assertion: () => void): Promise<void> {
    await vi.waitFor(() => {
        flush();
        assertion();
    });
}

/** Mounts the editor for key 7 and waits until its form stands prefilled. */
async function startEditor(stored: ReadonlyArray<ApiKeyResponse>): Promise<EditHarness> {
    const gateway = stubApi(stored);
    const updateApiKey = vi
        .fn<(request: UpdateApiKeyRequest) => Promise<void>>()
        .mockImplementation((request) => gateway.api.updateApiKey(request));
    const deleteApiKey = vi
        .fn<(request: { api_key_id: number }) => Promise<void>>()
        .mockImplementation((request) => gateway.api.deleteApiKey(request));
    const host = mountView(() => (
        <ApiKeyEditScreen
            api={{
                ...gateway.api,
                updateApiKey,
                deleteApiKey,
            }}
            apiKeyId={KEY_ID}
        />
    ));

    await waitFor(() => {
        expect(inputForLabel(host, "Name").value).toBe(stored[0]?.name ?? "");
    });

    return {
        host,
        stored: gateway.stored,
        setStored: gateway.setStored,
        readCount: gateway.readCount,
        updateApiKey,
        deleteApiKey,
    };
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

describe("ApiKeyEditScreen", () => {
    it("saves the complete settings the form holds and adopts the saved state", async () => {
        const { host, updateApiKey, stored } = await startEditor([
            keyFixture(),
        ]);

        typeInto(inputForLabel(host, "Name"), "staging-2");
        await waitFor(() => {
            expect(host.textContent).toContain("Unsaved changes");
        });

        submitForm(host);

        await waitFor(() => {
            expect(updateApiKey).toHaveBeenCalledTimes(1);
        });
        // The complete atomic state rides the write, not a patch of the
        // fields the operator happened to touch.
        expect(updateApiKey.mock.calls[0]?.[0]).toEqual({
            api_key_id: KEY_ID,
            name: "staging-2",
            key: "sk-staging-0001",
            is_enabled: true,
            access_control: {
                default_access: "deny",
                owner_defaults: [],
                route_overrides: [],
            },
        });

        // The save's outcome is the standing notice, the save's own reload
        // re-reads, and the form stands on the saved values.
        await waitFor(() => {
            expect(host.textContent).toContain("staging-2 saved.");
            expect(host.textContent).not.toContain("Unsaved changes");
        });
        expect(stored()[0]?.name).toBe("staging-2");
        expect(inputForLabel(host, "Name").value).toBe("staging-2");
    });

    it("holds a dirty draft against a refresh that answers a changed row, and says so", async () => {
        const { host, setStored } = await startEditor([
            keyFixture(),
        ]);

        typeInto(inputForLabel(host, "Name"), "my-local-draft");

        // The gateway's row moves while the form stands dirty.
        setStored([
            keyFixture({
                name: "gateway-renamed",
            }),
        ]);
        requestRefresh();

        await waitFor(() => {
            expect(host.textContent).toContain(
                "This key changed on the gateway while you have unsaved edits.",
            );
        });
        // The draft is exactly as it was typed, and the moved row stays held
        // back out of the form.
        expect(inputForLabel(host, "Name").value).toBe("my-local-draft");
        expect(host.textContent).not.toContain("gateway-renamed");
    });

    it("replaces a stale editor with not-found when the row is gone", async () => {
        const { host, setStored } = await startEditor([
            keyFixture(),
        ]);

        setStored([]);
        requestRefresh();

        await waitFor(() => {
            expect(host.textContent).toContain("API key not found");
        });
        expect(host.querySelector("form")).toBeNull();
        const back = Array.from(host.querySelectorAll("a")).find(
            (link) => link.getAttribute("href") === "/api-keys",
        );
        expect(back).not.toBeNull();
    });

    it("names the last enabled key's blast radius and deletes through the confirmation", async () => {
        const { host, deleteApiKey } = await startEditor([
            keyFixture(),
        ]);

        const trigger = Array.from(host.querySelectorAll("button")).find((button) =>
            button.textContent?.includes("Delete…"),
        );
        if (trigger === undefined) {
            throw new Error("No delete trigger.");
        }
        click(trigger);
        await waitFor(() => {
            expect(host.textContent).toContain(
                "Delete staging? It is the last enabled key — the gateway will serve nothing until another key is turned on.",
            );
        });

        const confirm = Array.from(host.querySelectorAll("button")).find((button) =>
            button.textContent?.includes("Delete for good"),
        );
        if (confirm === undefined) {
            throw new Error("No confirm button.");
        }
        click(confirm);

        await waitFor(() => {
            expect(deleteApiKey).toHaveBeenCalledTimes(1);
            expect(deleteApiKey.mock.calls[0]?.[0]).toEqual({
                api_key_id: KEY_ID,
            });
            expect(window.location.pathname).toBe("/api-keys");
        });
    });

    it("gates the save behind a complete, composition-valid secret", async () => {
        const { host } = await startEditor([
            keyFixture(),
        ]);

        const save = Array.from(host.querySelectorAll("button")).find((button) =>
            button.textContent?.includes("Save changes"),
        );
        if (save === undefined) {
            throw new Error("No save button.");
        }
        expect(save.disabled).toBe(true);

        typeInto(inputForLabel(host, "Key"), "");
        await waitFor(() => {
            expect(host.textContent).toContain("A key needs a secret.");
        });
        expect(save.disabled).toBe(true);

        // The gateway's own composition rule closes the gate the same way.
        typeInto(inputForLabel(host, "Key"), "has spaces");
        await waitFor(() => {
            expect(host.textContent).not.toContain("A key needs a secret.");
        });
        expect(save.disabled).toBe(true);
    });

    it("reverts the draft to the gateway's version and re-reads", async () => {
        const { host, readCount } = await startEditor([
            keyFixture(),
        ]);
        const readsBefore = readCount();

        typeInto(inputForLabel(host, "Name"), "my-local-draft");
        const revert = Array.from(host.querySelectorAll("button")).find(
            (button) => button.textContent?.trim() === "Revert",
        );
        if (revert === undefined) {
            throw new Error("No revert button.");
        }
        click(revert);

        await waitFor(() => {
            expect(inputForLabel(host, "Name").value).toBe("staging");
            expect(readCount()).toBeGreaterThan(readsBefore);
        });
    });
});
