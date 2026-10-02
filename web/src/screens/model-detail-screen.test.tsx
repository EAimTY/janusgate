import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ModelDetailScreen } from "/src/screens/model-detail-screen";
import {
    bodyText,
    cardWithTitle,
    createDisposals,
    modelRow,
    providerRow,
    settle,
    stubManagementApi,
} from "./screen-test-support";

const provider = providerRow({
    id: 5,
    base_url: "https://upstream.example.com",
});

const model = modelRow({
    provider_id: 5,
});

/** A management API whose entity listings answer the fixtures; usage answers empty. */
function screenApi(provider: ProviderResponse | null, model: ModelResponse): ManagementApi {
    return stubManagementApi({
        listProviders: () =>
            Promise.resolve({
                providers:
                    provider === null
                        ? []
                        : [
                              provider,
                          ],
            }),
        listModels: () =>
            Promise.resolve({
                models: [
                    model,
                ],
            }),
        queryUsage: () =>
            Promise.resolve({
                buckets: [],
            }),
    });
}

describe("ModelDetailScreen facts", () => {
    const disposes = createDisposals();

    function mount(provider: ProviderResponse | null, model: ModelResponse): void {
        disposes.push(
            render(
                () => (
                    <ModelDetailScreen
                        api={screenApi(provider, model)}
                        modelId={9}
                    />
                ),
                document.body,
            ),
        );
    }

    it("speaks the enabled vocabulary on the Details card under a Status term", async () => {
        mount(provider, model);
        await settle();

        // The detail-page vocabulary, under the same Status term the provider
        // page's Details card uses; the tight list-row value never appears.
        const details = cardWithTitle("Details");
        expect(details.textContent).toContain("Status");
        expect(details.textContent).toContain("Enabled");
        expect(details.textContent).not.toContain("Off");
        expect(bodyText().split("Enabled").length - 1).toBe(1);
    });

    it("keeps the serving badge's ladder word in the header while Details says Disabled", async () => {
        mount(
            provider,
            modelRow({
                provider_id: 5,
                is_enabled: false,
            }),
        );
        await settle();

        // The header badge stays the effective-serving ladder — its rung for a
        // switched-off model is "Off" — while the Details card speaks the
        // enabled vocabulary the other detail pages share.
        expect(bodyText()).toContain("Off");
        const details = cardWithTitle("Details");
        expect(details.textContent).toContain("Disabled");
        expect(details.textContent).not.toContain("Off");
        expect(bodyText()).not.toContain("Enabled");
    });
});

describe("ModelDetailScreen capabilities", () => {
    const disposes = createDisposals();

    function mount(model: ModelResponse): void {
        disposes.push(
            render(
                () => (
                    <ModelDetailScreen
                        api={screenApi(provider, model)}
                        modelId={9}
                    />
                ),
                document.body,
            ),
        );
    }

    it("states each effective declaration with its provenance", async () => {
        mount(
            modelRow({
                provider_id: 5,
                thinking_levels: [
                    "low",
                    "high",
                ],
                max_input_tokens: 200_000,
                max_output_tokens_override: 32_768,
            }),
        );
        await settle();

        const capabilities = cardWithTitle("Capabilities");
        expect(capabilities.textContent).toContain("low, high");
        expect(capabilities.textContent).toContain("From the latest discovery.");
        expect(capabilities.textContent).toContain("200,000");
        expect(capabilities.textContent).toContain("32,768");
        expect(capabilities.textContent).toContain("Overridden by administrator.");
    });

    it("states unknown declarations as omitted from the listings", async () => {
        mount(
            modelRow({
                provider_id: 5,
            }),
        );
        await settle();

        const capabilities = cardWithTitle("Capabilities");
        expect(capabilities.textContent).toContain("Unknown");
        expect(capabilities.textContent).toContain("omitted from model listings");
    });

    it("states an empty level set as the positive declaration of no levels", async () => {
        mount(
            modelRow({
                provider_id: 5,
                thinking_levels_override: [],
            }),
        );
        await settle();

        const capabilities = cardWithTitle("Capabilities");
        expect(capabilities.textContent).toContain("No levels");
        expect(capabilities.textContent).toContain("Overridden by administrator.");
    });
});

describe("ModelDetailScreen compaction facts", () => {
    const disposes = createDisposals();

    const responsesProvider = providerRow({
        id: 5,
        base_url: "https://upstream.example.com",
        format: "openai_response",
    });

    const messagesProvider = providerRow({
        id: 5,
        base_url: "https://upstream.example.com",
        format: "anthropic_messages",
    });

    function mount(provider: ProviderResponse, model: ModelResponse): void {
        disposes.push(
            render(
                () => (
                    <ModelDetailScreen
                        api={screenApi(provider, model)}
                        modelId={9}
                    />
                ),
                document.body,
            ),
        );
    }

    it("states the format's mechanisms with their row provenance", async () => {
        mount(
            responsesProvider,
            modelRow({
                provider_id: 5,
                compaction_support: {
                    responses_standalone_compact: "supported",
                },
                compaction_support_override: {
                    responses_explicit_trigger: "unsupported",
                },
            }),
        );
        await settle();

        const capabilities = cardWithTitle("Capabilities");

        // The discovered fact carries its layer; the override carries its
        // own; the mechanism neither states reads as the row-level unknown
        // that leaves the decision to the definition's declaration.
        expect(capabilities.textContent).toContain("Standalone compact");
        expect(capabilities.textContent).toContain("Explicit trigger");
        expect(capabilities.textContent).toContain("Automatic compaction");
        expect(capabilities.textContent).toContain("Supported");
        expect(capabilities.textContent).toContain("Unsupported");
        expect(capabilities.textContent).toContain("From the latest discovery.");
        expect(capabilities.textContent).toContain("Overridden by administrator.");
        expect(capabilities.textContent).toContain(
            "No fact on this row — the definition's declaration decides.",
        );

        // A Responses provider never shows the Messages mechanisms: the
        // definition cannot declare them, so the row's facts for them are
        // unstatable here.
        expect(capabilities.textContent).not.toContain("On-demand compaction");
        expect(capabilities.textContent).not.toContain("Threshold compaction");
    });

    it("states the Messages mechanisms on an anthropic_messages provider", async () => {
        mount(
            messagesProvider,
            modelRow({
                provider_id: 5,
                compaction_support: {
                    messages_on_demand_compaction: "supported",
                    messages_threshold_compaction: "supported",
                },
            }),
        );
        await settle();

        const capabilities = cardWithTitle("Capabilities");

        expect(capabilities.textContent).toContain("On-demand compaction");
        expect(capabilities.textContent).toContain("Threshold compaction");
        expect(capabilities.textContent).not.toContain("Standalone compact");
    });

    it("states a format that cannot carry compaction without a compaction section", async () => {
        mount(
            providerRow({
                id: 5,
                base_url: "https://upstream.example.com",
            }),
            modelRow({
                provider_id: 5,
                compaction_support: {
                    messages_on_demand_compaction: "supported",
                },
            }),
        );
        await settle();

        expect(cardWithTitle("Capabilities").textContent).not.toContain("Compaction");
    });

    it("reads an explicit unknown override as the administrator's own no-fact", async () => {
        mount(
            messagesProvider,
            modelRow({
                provider_id: 5,
                compaction_support: {
                    messages_on_demand_compaction: "supported",
                },
                compaction_support_override: {
                    messages_on_demand_compaction: "unknown",
                },
            }),
        );
        await settle();

        const compaction = cardWithTitle("Capabilities").textContent ?? "";

        expect(compaction).toContain("On-demand compaction");
        expect(compaction).toContain("Unknown");
        expect(compaction).toContain("Overridden by administrator.");
    });
});
