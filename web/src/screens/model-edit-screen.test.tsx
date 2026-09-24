import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ModelEditScreen } from "/src/screens/model-edit-screen";

const provider: ProviderResponse = {
    type: "api_key",
    provider_definition_name: "generic-openai-responses",
    provider_definition_version: 1,
    format: "openai_response",
    id: 3,
    name: "alpha",
    model_prefix: "alpha-",
    is_enabled: true,
    base_url: "https://alpha.example",
};

const model: ModelResponse = {
    id: 11,
    provider_id: 3,
    provider: "alpha",
    upstream_id: "gpt-x",
    public_id: "alpha-gpt-x",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

/**
 * A management API standing in for the gateway itself: its listings answer
 * from mutable holders, every read re-parses the rows into fresh objects the
 * way the real wire does, and `models:update` applies the request over the
 * held row — recomposing the public ID from the provider's current prefix
 * when no override stands — so a reload after a save answers the saved row
 * the way the real gateway would.
 */
function screenApi(holder: {
    providers: ReadonlyArray<ProviderResponse>;
    models: ReadonlyArray<ModelResponse>;
}): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));


    return {
        listProviderDefinitions: refused,
        createProviderDefinition: refused,
        updateProviderDefinition: refused,
        listProviders: (signal) => {
            signal?.throwIfAborted();
            return Promise.resolve({
                providers: holder.providers.map((row) => ({
                    ...row,
                })),
            });
        },
        listModels: (_request, signal) => {
            signal?.throwIfAborted();
            return Promise.resolve({
                models: holder.models.map((row) => ({
                    ...row,
                })),
            });
        },
        updateModel: (request) => {
            holder.models = holder.models.map((row) => {
                if (row.id !== request.model_id) {
                    return row;
                }

                const owner = holder.providers.find(
                    (candidate) => candidate.id === row.provider_id,
                );
                const override = request.id_override;

                return {
                    ...row,
                    id_override: override,
                    is_enabled: request.is_enabled,
                    public_id:
                        override ??
                        `${owner === undefined ? "" : owner.model_prefix}${row.upstream_id}`,
                };
            });

            const saved = holder.models.find((row) => row.id === request.model_id);

            if (saved === undefined) {
                throw new Error(`The gateway holds no model ${request.model_id}`);
            }

            return Promise.resolve({
                model: {
                    ...saved,
                },
            });
        },
        deleteModel: refused,
        discoverModels: refused,
        createProvider: refused,
        updateProvider: refused,
        deleteProvider: refused,
        startProviderOauth: refused,
        completeProviderOauth: refused,
        reauthorizeProvider: refused,
        createModel: refused,
        createApiKey: refused,
        updateApiKey: refused,
        deleteApiKey: refused,
        listApiKeys: refused,
        listUsage: refused,
        queryUsage: refused,
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

function switchWithLabel(label: string): HTMLButtonElement {
    const candidate = document.querySelector(`button[role='switch'][aria-label='${label}']`);

    if (candidate === null) {
        throw new Error(`No switch labeled ${label}`);
    }

    return candidate as HTMLButtonElement;
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

/** Opens the override and names it, the model form's dirtiest path. */
async function overridePublicId(value: string): Promise<HTMLInputElement> {
    switchWithLabel("Override the public ID").click();
    await settle();
    const override = labeledInput("Public ID override");
    typeInto(override, value);
    await settle();

    return override;
}

describe("ModelEditScreen refresh against an open form", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(holder: {
        providers: ReadonlyArray<ProviderResponse>;
        models: ReadonlyArray<ModelResponse>;
    }): void {
        disposes.push(
            render(
                () => (
                    <ModelEditScreen
                        api={screenApi(holder)}
                        modelId={11}
                    />
                ),
                document.body,
            ),
        );
    }

    it("keeps a dirty form across a refresh whose listings re-answer the same rows", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        const override = await overridePublicId("custom-gpt-x");
        expect(document.body.textContent).toContain("Unsaved changes");

        requestRefresh();
        await settle();

        expect(labeledInput("Public ID override").value).toBe("custom-gpt-x");
        expect(override.isConnected).toBe(true);
        expect(document.body.textContent).toContain("Unsaved changes");
    });

    it("keeps a dirty form across a refresh that answers a changed row, and says so", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        await overridePublicId("custom-gpt-x");

        holder.models = [
            {
                ...model,
                is_enabled: false,
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Public ID override").value).toBe("custom-gpt-x");
        expect(document.body.textContent).toContain("changed on the gateway");
    });

    it("re-baselines a clean form across a refresh that answers a changed row", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        holder.models = [
            {
                ...model,
                is_enabled: false,
            },
        ];
        requestRefresh();
        await settle();

        expect(document.body.textContent).not.toContain("changed on the gateway");
        expect(switchWithLabel("Override the public ID").getAttribute("aria-checked")).toBe(
            "false",
        );
    });

    it("picks the gateway's version up once the operator reverts a held form", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        await overridePublicId("custom-gpt-x");
        holder.models = [
            {
                ...model,
                is_enabled: false,
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Revert").click();
        await settle();

        expect(switchWithLabel("Override the public ID").getAttribute("aria-checked")).toBe(
            "false",
        );
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });

    it("lands a save over a held row on the values the form sent", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        await overridePublicId("custom-gpt-x");
        holder.models = [
            {
                ...model,
                is_enabled: false,
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Save changes").click();
        await settle();

        expect(holder.models).toEqual([
            {
                ...model,
                // The form's own settings state is what Save writes over the
                // gateway's version — the concurrent serving switch included.
                is_enabled: true,
                id_override: "custom-gpt-x",
                public_id: "custom-gpt-x",
            },
        ]);
        expect(labeledInput("Public ID override").value).toBe("custom-gpt-x");
        expect(document.body.textContent).not.toContain("changed on the gateway");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(document.body.textContent).toContain("Clients now call it custom-gpt-x.");
    });

    it("reloads the listing when a save answers gone, handing over to the not-found state", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            updateModel: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "no model 11", {
                        errno: ErrorCode.NotFound,
                        httpStatus: 404,
                    }),
                ),
        };
        disposes.push(
            render(
                () => (
                    <ModelEditScreen
                        api={api}
                        modelId={11}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        await overridePublicId("custom-gpt-x");
        holder.models = [];
        buttonWithText("Save changes").click();
        await settle();

        // The message is rendered and the listing that showed the row is
        // reloaded (web README §6.4), so the not-found state takes over
        // instead of a ghost form.
        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("Model not found");
    });

    it("reloads the listing when a delete answers gone", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            deleteModel: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "no model 11", {
                        errno: ErrorCode.NotFound,
                        httpStatus: 404,
                    }),
                ),
        };
        disposes.push(
            render(
                () => (
                    <ModelEditScreen
                        api={api}
                        modelId={11}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        holder.models = [];
        for (const candidate of document.querySelectorAll("button")) {
            if (candidate.textContent?.includes("Delete…") === true) {
                candidate.click();
            }
        }
        await settle();
        buttonWithText("Delete for good").click();
        await settle();

        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("Model not found");
    });
});
