import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ModelsScreen } from "/src/screens/models-screen";
import { settle, stubManagementApi, typeInto } from "./screen-test-support";

/**
 * A provider whose name shares nothing with its model's ids, so a search
 * matching the model can only be matching the provider the rows display.
 */
const providerRow: ProviderResponse = {
    type: "api_key",
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    id: 3,
    name: "mix-ops",
    model_prefix: "",
    is_enabled: true,
    base_url: "https://upstream.example.com/v1",
};

const modelRow: ModelResponse = {
    id: 9,
    provider_id: 3,
    provider: "mix-ops",
    upstream_id: "llama3-70b",
    public_id: "llama3-70b",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

/**
 * A management API whose two listings answer from fixtures; every other
 * route rejects, so an unexpected call fails the test loudly instead of
 * hanging it.
 */
function screenApi(options: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
}): ManagementApi {
    return stubManagementApi({
        listProviders: () =>
            Promise.resolve({
                providers: [
                    ...options.providers,
                ],
            }),
        listModels: () =>
            Promise.resolve({
                models: [
                    ...options.models,
                ],
            }),
    });
}

/** The models search field, under its fixed element id. */
function searchInput(): HTMLInputElement {
    const input = document.getElementById("models-search");

    if (input instanceof HTMLInputElement) {
        return input;
    }

    throw new Error("No models search input");
}

/** The select one label names, through the label's `for` pairing. */
function selectWithLabel(label: string): HTMLSelectElement {
    const owner = Array.from(document.querySelectorAll("label")).find(
        (candidate) => candidate.textContent === label,
    );

    if (owner === undefined) {
        throw new Error(`No field labelled ${label}`);
    }

    const control = document.getElementById(owner.htmlFor);

    if (!(control instanceof HTMLSelectElement)) {
        throw new Error(`No select labelled ${label}`);
    }

    return control;
}

/** One option's value, label, and selected state, as the DOM holds them. */
function optionRepr(option: HTMLOptionElement): {
    value: string;
    label: string;
    isSelected: boolean;
} {
    return {
        value: option.value,
        label: option.textContent ?? "",
        isSelected: option.selected,
    };
}

function bodyText(): string {
    return document.body.textContent ?? "";
}

function linkTo(href: string): HTMLAnchorElement {
    for (const candidate of document.querySelectorAll("a")) {
        if (candidate.getAttribute("href") === href) {
            return candidate;
        }
    }

    throw new Error(`No link to ${href}`);
}

describe("ModelsScreen empty gateway", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

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
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(): void {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [
                                providerRow,
                            ],
                            models: [
                                modelRow,
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

        typeInto(searchInput(), "mix");
        await settle();

        expect(bodyText()).toContain("llama3-70b");
        expect(bodyText()).not.toContain("No models match");

        typeInto(searchInput(), "zzz");
        await settle();

        expect(bodyText()).toContain('No models match "zzz"');
    });
});

describe("ModelsScreen provider filter", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the screen over a gateway with the provider but none of its models. */
    function mountFiltered(providerFilter: number | null): void {
        disposes.push(
            render(
                () => (
                    <ModelsScreen
                        api={screenApi({
                            providers: [
                                providerRow,
                            ],
                            models: [],
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

    it("keeps a filter naming an unlisted provider stated as its own option", async () => {
        // Provider 9 was deleted after a filter linked to it: the listing the
        // screen read no longer carries it, so a listing-built option list
        // alone would leave the select silently sitting on "All providers"
        // while the route still filtered.
        mountFiltered(9);
        await settle();

        const options = Array.from(selectWithLabel("Provider").options).map(optionRepr);

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
