import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ModelEditScreen } from "/src/screens/model-edit-screen";
import {
    bodyText,
    buttonWithText,
    labeledInput,
    linkTo,
    modelRow,
    paragraphWithText,
    providerRow,
    settle,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "./screen-test-support";

const provider = providerRow({
    provider_definition_name: "generic-openai-responses",
    format: "openai_response",
    id: 3,
    name: "alpha",
    model_prefix: "alpha-",
    base_url: "https://alpha.example",
});

const model = modelRow({
    id: 11,
    provider_id: 3,
    provider: "alpha",
    upstream_id: "gpt-x",
    public_id: "alpha-gpt-x",
});

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
    return stubManagementApi({
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
    });
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

    it("hints the composed ID once the override is toggled off over stale field text", async () => {
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

        switchWithLabel("Override the public ID").click();
        await settle();
        typeInto(labeledInput("Public ID override"), "custom-gpt-x");
        await settle();
        switchWithLabel("Override the public ID").click();
        await settle();

        // The switch is off, so the save discards the field's stale text and
        // the gateway recomposes the public ID from the provider's prefix —
        // the off-hint must name exactly that ID, never the discarded
        // override.
        expect(paragraphWithText("Clients call").textContent).toBe("Clients call alpha-gpt-x.");
    });

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

    it("arms deletion with the true consequence and recoveries, never a same-ID claim", async () => {
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

        buttonWithText("Delete…").click();
        await settle();

        // The armed question is where the operator decides: nothing survives
        // the deletion to claim back — both recoveries return a new row with
        // a fresh identity, and a discovered rebuild carries no custom public
        // ID (README §7.1).
        expect(paragraphWithText("Delete alpha-gpt-x?").textContent).toBe(
            "Delete alpha-gpt-x? Clients calling it 404 until it exists again — a discovered model returns as new on the next discovery (without a custom public ID); a manual one can be re-added from the Create model page as a new row.",
        );
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

describe("ModelEditScreen vanished model", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** The routing model of the shared provider row, no longer reported. */
    const vanished = modelRow({
        id: 11,
        provider_id: 3,
        provider: "alpha",
        upstream_id: "gpt-x",
        public_id: "alpha-gpt-x",
        discovery_state: "vanished",
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

    it("states the suspension under the header with both recovery paths", async () => {
        // The page is reachable by deep link, and an "enable it again" edit
        // lands here — the suspension the detail page states cannot be left
        // to that page alone.
        mount({
            providers: [
                provider,
            ],
            models: [
                vanished,
            ],
        });
        await settle();

        expect(bodyText()).toContain("The upstream no longer reports this model.");
        expect(linkTo("/providers/3").textContent).toBe("Discover again on the provider's page");
        expect(linkTo("/models/new?provider=3&upstream=gpt-x").textContent).toBe(
            "claim it back from the Create model page",
        );
    });

    it("keeps the routing line off a vanished row's save notice", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                vanished,
            ],
        };
        mount(holder);
        await settle();

        await overridePublicId("custom-gpt-x");
        buttonWithText("Save changes").click();
        await settle();

        // The save succeeds, but the row stays suspended — `models:update`
        // never touches `discovery_state` — so the notice announces the save
        // alone, and the page keeps stating the suspension.
        expect(bodyText()).toContain("Model saved.");
        expect(bodyText()).not.toContain("Clients now call it");
        expect(bodyText()).toContain("The upstream no longer reports this model.");
    });
});
