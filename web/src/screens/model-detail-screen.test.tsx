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
