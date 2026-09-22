import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ApiKeyEditScreen } from "/src/screens/api-key-edit-screen";

const key: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
};

/**
 * A management API whose key listing answers from a mutable holder (so a test
 * can change what the gateway answers between refreshes) and whose write
 * surface records nothing — this screen is only ever read here.
 */
function screenApi(holder: { readonly keys: ReadonlyArray<ApiKeyResponse> }): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
        listApiKeys: (signal) => {
            signal?.throwIfAborted();
            return Promise.resolve({
                api_keys: [
                    ...holder.keys,
                ],
            });
        },
        listProviders: refused,
        listModels: refused,
        discoverModels: refused,
        createProvider: refused,
        updateProvider: refused,
        deleteProvider: refused,
        startProviderOauth: refused,
        completeProviderOauth: refused,
        reauthorizeProvider: refused,
        createModel: refused,
        updateModel: refused,
        deleteModel: refused,
        listUsage: refused,
        queryUsage: refused,
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

describe("ApiKeyEditScreen refresh against an open form", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(holder: { readonly keys: ReadonlyArray<ApiKeyResponse> }): void {
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={screenApi(holder)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
    }

    it("keeps a dirty form across a refresh that answers the same row", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        const name = labeledInput("Name");
        typeInto(name, "renamed");
        await settle();
        expect(document.body.textContent).toContain("Unsaved changes");

        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed");
    });

    it("keeps a dirty form across a refresh that answers a changed row, and says so", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");

        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed");
        expect(document.body.textContent).toContain("changed on the gateway");
    });

    it("re-baselines a clean form across a refresh that answers a changed row", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed-elsewhere");
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });

    it("picks the gateway's version up once the operator reverts a held form", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");
        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();
        expect(labeledInput("Name").value).toBe("renamed");

        buttonWithText("Revert").click();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed-elsewhere");
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });
});
