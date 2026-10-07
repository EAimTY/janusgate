import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import {
    bodyText,
    buttonWithText,
    chooseSelect,
    createDisposals,
    goneFailure,
    labeledCheckbox,
    labeledInput,
    linkTo,
    modelRow,
    paragraphWithText,
    settle,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "/src/test-support/management-api";
import { ModelEditScreen } from "./model-edit-screen";

const model = modelRow({
    id: 11,
    provider_id: 3,
    provider: "alpha",
    upstream_id: "gpt-x",
});

/**
 * A management API standing in for the gateway itself: its listings answer
 * from mutable holders, every read re-parses the rows into fresh objects the
 * way the real wire does, and `models:update` applies the request over the
 * held row, so a reload after a save answers the saved physical inventory row
 * the way the real gateway would.
 */
function screenApi(holder: { models: ReadonlyArray<ModelResponse> }): ManagementApi {
    return stubManagementApi({
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

                return {
                    ...row,
                    is_enabled: request.is_enabled,
                    thinking_levels_override: request.thinking_levels_override,
                    max_input_tokens_override: request.max_input_tokens_override,
                    max_output_tokens_override: request.max_output_tokens_override,
                    compaction_support_override: request.compaction_support_override,
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

/** Makes a capability edit so refresh tests exercise the held-form contract. */
async function overrideInputLimit(): Promise<HTMLInputElement> {
    switchWithLabel("Override max input tokens").click();
    await settle();
    const input = labeledInput("Max input tokens override");
    typeInto(input, "30000");
    await settle();
    return input;
}

describe("ModelEditScreen refresh against an open form", () => {
    const disposes = createDisposals();

    function mount(holder: { models: ReadonlyArray<ModelResponse> }): void {
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
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        const override = await overrideInputLimit();
        expect(document.body.textContent).toContain("Unsaved changes");

        requestRefresh();
        await settle();

        expect(labeledInput("Max input tokens override").value).toBe("30000");
        expect(override.isConnected).toBe(true);
        expect(document.body.textContent).toContain("Unsaved changes");
    });

    it("keeps a dirty form across a refresh that answers a changed row, and says so", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        await overrideInputLimit();

        holder.models = [
            {
                ...model,
                is_enabled: false,
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Max input tokens override").value).toBe("30000");
        expect(document.body.textContent).toContain("changed on the gateway");
    });

    it("re-baselines a clean form across a refresh that answers a changed row", async () => {
        const holder = {
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
        expect(switchWithLabel("Override max input tokens").getAttribute("aria-checked")).toBe(
            "false",
        );
    });

    it("picks the gateway's version up once the operator reverts a held form", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        await overrideInputLimit();
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

        expect(switchWithLabel("Override max input tokens").getAttribute("aria-checked")).toBe(
            "false",
        );
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });

    it("lands a save over a held row on the values the form sent", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        await overrideInputLimit();
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
                max_input_tokens_override: 30_000,
            },
        ]);
        expect(labeledInput("Max input tokens override").value).toBe("30000");
        expect(document.body.textContent).not.toContain("changed on the gateway");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(document.body.textContent).toContain("Model saved.");
    });

    it("saves capability overrides beside the inventory switch", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        // The levels override replaces the discovered set wholesale; the
        // token overrides replace their own values. Each switch gates its
        // own field, and an invalid token entry holds the save.
        switchWithLabel("Override reasoning levels").click();
        await settle();
        labeledCheckbox("low").click();
        await settle();
        labeledCheckbox("high").click();
        await settle();

        switchWithLabel("Override max input tokens").click();
        await settle();
        typeInto(labeledInput("Max input tokens override"), "0");
        await settle();
        expect(document.body.textContent).toContain(
            "Max input tokens override must be a positive integer.",
        );
        expect(buttonWithText("Save changes").disabled).toBe(true);

        typeInto(labeledInput("Max input tokens override"), "200000");
        await settle();

        buttonWithText("Save changes").click();
        await settle();

        expect(holder.models).toEqual([
            {
                ...model,
                thinking_levels_override: [
                    "low",
                    "high",
                ],
                max_input_tokens_override: 200_000,
                max_output_tokens_override: null,
            },
        ]);
        expect(document.body.textContent).toContain("Model saved.");
        expect(document.body.textContent).not.toContain("Unsaved changes");
    });

    it("saves the compaction-facts override wholesale, the unstated mechanisms left out", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        switchWithLabel("Override compaction facts").click();
        await settle();
        chooseSelect("Standalone compact", "supported");
        await settle();
        chooseSelect("Explicit trigger", "unknown");
        await settle();
        expect(document.body.textContent).toContain("Unsaved changes");

        buttonWithText("Save changes").click();
        await settle();

        // The override map replaces the discovered facts wholesale: the two
        // stated mechanisms ride their chosen states, and the three left at
        // Unset are absent from the write — discovery and the definition's
        // declaration keep deciding for them.
        expect(holder.models).toEqual([
            {
                ...model,
                compaction_support_override: {
                    responses_standalone_compact: "supported",
                    responses_explicit_trigger: "unknown",
                },
            },
        ]);
        expect(document.body.textContent).toContain("Model saved.");
        expect(document.body.textContent).not.toContain("Unsaved changes");
    });

    it("clears the compaction-facts override back to discovery when the switch goes off", async () => {
        const holder = {
            models: [
                modelRow({
                    id: 11,
                    provider_id: 3,
                    provider: "alpha",
                    upstream_id: "gpt-x",

                    compaction_support_override: {
                        messages_on_demand_compaction: "supported",
                    },
                }),
            ],
        };
        mount(holder);
        await settle();

        // The standing override opens the switch on; turning it off is the
        // clear, and the save restates the whole form.
        expect(switchWithLabel("Override compaction facts").getAttribute("aria-checked")).toBe(
            "true",
        );
        switchWithLabel("Override compaction facts").click();
        await settle();

        buttonWithText("Save changes").click();
        await settle();

        expect(holder.models).toEqual([
            {
                ...modelRow({
                    id: 11,
                    provider_id: 3,
                    provider: "alpha",
                    upstream_id: "gpt-x",
                }),
                compaction_support_override: null,
            },
        ]);
        expect(document.body.textContent).toContain("Model saved.");
    });

    it("reloads the listing when a save answers gone, handing over to the not-found state", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            updateModel: () => Promise.reject(goneFailure("no model 11")),
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

        await overrideInputLimit();
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
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        buttonWithText("Delete…").click();
        await settle();

        // Re-creating inventory cannot restore the direct routes deleted with it.
        expect(paragraphWithText("Delete gpt-x?").textContent).toBe(
            "Delete gpt-x? Its direct routes are deleted too. Pool routes remain stored. Rediscovery or manual creation produces a new physical row and does not restore deleted routes.",
        );
    });

    it("reloads the listing when a delete answers gone", async () => {
        const holder = {
            models: [
                model,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            deleteModel: () => Promise.reject(goneFailure("no model 11")),
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
    const disposes = createDisposals();

    /** The routing model of the shared provider row, no longer reported. */
    const vanished = modelRow({
        id: 11,
        provider_id: 3,
        provider: "alpha",
        upstream_id: "gpt-x",

        discovery_state: "vanished",
    });

    function mount(holder: { models: ReadonlyArray<ModelResponse> }): void {
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
            models: [
                vanished,
            ],
        };
        mount(holder);
        await settle();

        await overrideInputLimit();
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
