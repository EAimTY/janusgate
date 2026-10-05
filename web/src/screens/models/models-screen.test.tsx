import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import {
    bodyText,
    createDisposals,
    labeledSelect,
    linkTo,
    modelRow,
    optionRepr,
    providerRow,
    searchInput,
    settle,
    stubListings,
    stubManagementApi,
    typeInto,
} from "/src/test-support/management-api";
import { ModelsScreen } from "./models-screen";

/**
 * A management API whose listings answer from the given rows; every other
 * route rejects, so an unexpected call fails the test loudly instead of
 * hanging it.
 */
function screenApi(options: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
}): ManagementApi {
    return stubManagementApi(stubListings(options));
}

describe("ModelsScreen empty gateway", () => {
    const disposes = createDisposals();

    it("makes the provider the first step when nothing exists yet", async () => {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [],
                            models: [],
                        })}
                        providerFilter={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // With no provider, model creation dead-ends in its own no-provider
        // state, so the fallback's first step is the provider — and its
        // models arrive by discovering them, never "syncing".
        expect(bodyText()).toContain(
            "Add a provider first, then discover its models — or add one by hand.",
        );

        const createProvider = linkTo("/providers/new");
        expect(createProvider.textContent).toContain("Create provider");
        expect(createProvider.className).toContain("bg-accent");

        // The model step stays offered beside it, unaccented: it leads
        // somewhere only once a provider exists.
        const createModel = Array.from(document.querySelectorAll('a[href="/models/new"]')).at(-1);
        expect(createModel?.textContent).toContain("Create model");
    });
});

describe("ModelsScreen search", () => {
    const disposes = createDisposals();

    function mount(): void {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [
                                // A provider whose name shares nothing with its
                                // model's ids, so a search matching the model
                                // can only be matching the provider the rows
                                // display.
                                providerRow({
                                    id: 3,
                                    name: "mix-ops",
                                }),
                            ],
                            models: [
                                modelRow({
                                    provider_id: 3,
                                    provider: "mix-ops",
                                    upstream_id: "llama3-70b",
                                }),
                            ],
                        })}
                        providerFilter={null}
                    />
                ),
                document.body,
            ),
        );
    }

    it("matches the owning provider's name, not only the model's ids", async () => {
        mount();
        await settle();

        typeInto(searchInput("Search models"), "mix");
        await settle();

        expect(bodyText()).toContain("llama3-70b");
        expect(bodyText()).not.toContain("No models match");

        typeInto(searchInput("Search models"), "zzz");
        await settle();

        expect(bodyText()).toContain('No models match "zzz"');
    });
});

describe("ModelsScreen count line", () => {
    const disposes = createDisposals();

    /**
     * Five models of one provider, two of them qwen: the fraction a standing
     * search reports needs a needle that narrows the listing without
     * emptying it.
     */
    const models: ReadonlyArray<ModelResponse> = [
        "qwen3-32b",
        "qwen3-8b",
        "llama3-70b",
        "llama3-8b",
        "mistral-7b",
    ].map((publicId) =>
        modelRow({
            provider_id: 3,
            provider: "mix-ops",

            upstream_id: publicId,
        }),
    );

    function mount(): void {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [
                                providerRow({
                                    id: 3,
                                    name: "mix-ops",
                                }),
                            ],
                            models,
                        })}
                        providerFilter={null}
                    />
                ),
                document.body,
            ),
        );
    }

    it("states the plain count until a search stands, then the fraction", async () => {
        mount();
        await settle();

        // With no search every row is visible, so a fraction would read as a
        // narrowed table and send the operator looking for the narrowing.
        expect(bodyText()).toContain("5 models");
        expect(bodyText()).not.toContain("of 5 models");

        typeInto(searchInput("Search models"), "qwen");
        await settle();

        // Under a search the fraction carries the narrowing the search made.
        expect(bodyText()).toContain("2 of 5 models");

        typeInto(searchInput("Search models"), "");
        await settle();

        // Clearing the search returns the line to the plain count.
        expect(bodyText()).toContain("5 models");
        expect(bodyText()).not.toContain("of 5 models");
    });
});

describe("ModelsScreen provider filter", () => {
    const disposes = createDisposals();

    /** Mounts the screen over a gateway holding the provider and the given models. */
    function mountFiltered(
        providerFilter: number | null,
        models: ReadonlyArray<ModelResponse> = [],
    ): void {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [
                                providerRow({
                                    id: 3,
                                    name: "mix-ops",
                                }),
                            ],
                            models: [
                                ...models,
                            ],
                        })}
                        providerFilter={providerFilter}
                    />
                ),
                document.body,
            ),
        );
    }

    it("states the whole gateway's emptiness when nothing is filtered", async () => {
        mountFiltered(null);
        await settle();

        // The unfiltered table is about no provider in particular, so the
        // notice must not assert "this provider" about one it never named.
        expect(bodyText()).toContain("No models on any provider yet — discover or add one.");
        expect(bodyText()).not.toContain("This provider holds no models.");
    });

    it("names the filtered provider's emptiness when the filter is set", async () => {
        mountFiltered(3);
        await settle();

        expect(bodyText()).toContain("This provider holds no models.");
    });

    it("names both narrows when a search runs under a filter", async () => {
        mountFiltered(3, [
            modelRow({
                provider_id: 3,
                provider: "mix-ops",
                upstream_id: "llama3-70b",
            }),
        ]);
        await settle();

        typeInto(searchInput("Search models"), "zzz");
        await settle();

        expect(bodyText()).toContain('No models of this provider match "zzz"');
        // The filter's narrow is part of the answer, never folded away into
        // the search-only wording.
        expect(bodyText()).not.toContain('No models match "zzz"');
    });

    it("states a no-longer-listed filter instead of its models", async () => {
        mountFiltered(9);
        await settle();

        // The empty filtered list proves nothing about the provider's models
        // — the provider list is the authority on existence — so the notice
        // states the listing fact the select's option already names and
        // points there.
        expect(bodyText()).toContain(
            "This provider is no longer listed — pick another in the Provider filter.",
        );
        expect(bodyText()).not.toContain("This provider holds no models.");
    });

    it("keeps a filter naming an unlisted provider stated as its own option", async () => {
        // Provider 9 was deleted after a filter linked to it: the listing the
        // screen read no longer carries it, so a listing-built option list
        // alone would leave the select silently sitting on "All providers"
        // while the route still filtered.
        mountFiltered(9);
        await settle();

        const options = Array.from(labeledSelect("Provider").options).map(optionRepr);

        expect(options.at(-1)).toEqual({
            value: "9",
            label: "9 (no longer listed)",
            isSelected: true,
        });
        // The listed provider's own option is untouched.
        expect(options).toContainEqual({
            value: "3",
            label: "mix-ops",
            isSelected: false,
        });
    });
});
