import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type { ApiKeyResponse, UpdateApiKeyRequest } from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ApiKeyEditScreen } from "/src/screens/api-key-edit-screen";
import { settle, stubManagementApi } from "./screen-test-support";

const key: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
};

/**
 * A management API standing in for the gateway itself: its key listing
 * answers from a mutable holder (so a test can change what the gateway
 * answers between refreshes), and `api-keys:update` applies the request's
 * complete desired state back into the holder and records it, so a reload
 * after a save answers the saved row the way the real gateway would.
 */
function screenApi(
    holder: {
        keys: ReadonlyArray<ApiKeyResponse>;
    },
    sent: UpdateApiKeyRequest[] = [],
): ManagementApi {
    return stubManagementApi({
        listApiKeys: (signal) => {
            signal?.throwIfAborted();
            return Promise.resolve({
                api_keys: [
                    ...holder.keys,
                ],
            });
        },
        updateApiKey: (request) => {
            sent.push(request);

            holder.keys = holder.keys.map((row) =>
                row.id === request.api_key_id
                    ? {
                          ...row,
                          name: request.name,
                          key: request.key,
                          is_enabled: request.is_enabled,
                      }
                    : row,
            );

            return Promise.resolve();
        },
    });
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

    it("lands a save over a held row on the sent values, carrying the rotated secret", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const sent: UpdateApiKeyRequest[] = [];
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={screenApi(holder, sent)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(labeledInput("Name"), "saved-here");
        typeInto(labeledInput("Key"), "sk-rotated");
        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toEqual([
            {
                api_key_id: 7,
                name: "saved-here",
                key: "sk-rotated",
                is_enabled: true,
            },
        ]);
        expect(holder.keys).toEqual([
            {
                ...key,
                name: "saved-here",
                key: "sk-rotated",
            },
        ]);
        expect(labeledInput("Name").value).toBe("saved-here");
        expect(labeledInput("Key").value).toBe("sk-rotated");
        expect(document.body.textContent).not.toContain("changed on the gateway");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(document.body.textContent).toContain("saved-here saved.");
    });

    it("states the last-enabled-key consequence when a save turns the only enabled key off", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const sent: UpdateApiKeyRequest[] = [];
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={screenApi(holder, sent)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        switchWithLabel("Enabled").click();
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent[0]?.is_enabled).toBe(false);
        expect(holder.keys).toEqual([
            {
                ...key,
                is_enabled: false,
            },
        ]);
        expect(document.body.textContent).toContain(
            "This was the last enabled key: the gateway now serves nothing until a key is turned on.",
        );
    });

    it("names the emptied secret and holds the save until one is typed back", async () => {
        // api-keys:update is a full-state write, so unlike the provider
        // form's write-only field the secret is restated here: an emptied Key
        // cannot save, and the disabled save must name that, not sit mute.
        mount({
            keys: [
                key,
            ],
        });
        await settle();

        typeInto(labeledInput("Key"), "");
        await settle();

        expect(document.body.textContent).toContain("A key needs a secret.");
        expect(document.body.textContent).toContain("Unsaved changes");
        expect(buttonWithText("Save changes").disabled).toBe(true);

        typeInto(labeledInput("Key"), "sk-restored");
        await settle();

        expect(document.body.textContent).not.toContain("A key needs a secret.");
        expect(buttonWithText("Save changes").disabled).toBe(false);
    });

    it("reloads the listing when a save answers gone, handing over to the not-found state", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            updateApiKey: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "no key 7", {
                        errno: ErrorCode.NotFound,
                        httpStatus: 404,
                    }),
                ),
        };
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={api}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(labeledInput("Name"), "renamed");
        await settle();
        holder.keys = [];
        buttonWithText("Save changes").click();
        await settle();

        // The message is rendered and the listing that showed the row is
        // reloaded (web README §6.4), so the not-found state takes over
        // instead of a ghost form.
        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("API key not found");
    });

    it("reloads the listing when a delete answers gone", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            deleteApiKey: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "no key 7", {
                        errno: ErrorCode.NotFound,
                        httpStatus: 404,
                    }),
                ),
        };
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={api}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        holder.keys = [];
        for (const candidate of document.querySelectorAll("button")) {
            if (candidate.textContent?.includes("Delete…") === true) {
                candidate.click();
            }
        }
        await settle();
        buttonWithText("Delete for good").click();
        await settle();

        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("API key not found");
    });
});
