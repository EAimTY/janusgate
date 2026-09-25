import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { UsageFilterControls } from "/src/ui/usage-filter-controls";
import { EMPTY_USAGE_FILTER_FIELDS, type UsageFilterFields } from "/src/usage-filter";

const providerRow: ProviderResponse = {
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

const modelRow: ModelResponse = {
    id: 11,
    provider_id: 3,
    provider: "alpha",
    upstream_id: "gpt-x",
    public_id: "alpha-gpt-x",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

const apiKeyRow: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1,
};

const disposes: Array<() => void> = [];

afterEach(() => {
    for (const dispose of disposes.reverse()) {
        dispose();
    }
    disposes.length = 0;
});

/** Mounts the controls holding the given fields, collecting the edits they report. */
function mountControls(fields: UsageFilterFields): {
    edits: Array<Partial<UsageFilterFields>>;
} {
    const edits: Array<Partial<UsageFilterFields>> = [];

    disposes.push(
        render(
            () => (
                <UsageFilterControls
                    fields={fields}
                    providers={[
                        providerRow,
                    ]}
                    models={[
                        modelRow,
                    ]}
                    apiKeys={[
                        apiKeyRow,
                    ]}
                    isDisabled={false}
                    onEdit={(edit) => {
                        edits.push(edit);
                    }}
                />
            ),
            document.body,
        ),
    );

    return {
        edits,
    };
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

describe("UsageFilterControls listing-backed options", () => {
    it("renders a chosen id the listing no longer carries as its trailing, selected option", () => {
        mountControls({
            ...EMPTY_USAGE_FILTER_FIELDS,
            providerId: "9",
        });

        const options = Array.from(selectWithLabel("Provider").options).map(optionRepr);

        expect(options.map((option) => option.label)).toContain("9 (no longer listed)");
        expect(options.at(-1)).toEqual({
            value: "9",
            label: "9 (no longer listed)",
            isSelected: true,
        });
        // The listed row's own option is untouched.
        expect(options).toContainEqual({
            value: "3",
            label: "alpha",
            isSelected: false,
        });
    });

    it("leaves the options of a listed or empty value untouched", () => {
        mountControls({
            ...EMPTY_USAGE_FILTER_FIELDS,
            providerId: "3",
        });

        for (const label of [
            "Provider",
            "Model",
            "API key",
        ]) {
            const options = Array.from(selectWithLabel(label).options).map(optionRepr);

            expect(options.some((option) => option.label.includes("no longer listed"))).toBe(false);
        }

        expect(Array.from(selectWithLabel("Provider").options).map(optionRepr)).toContainEqual({
            value: "3",
            label: "alpha",
            isSelected: true,
        });
    });

    it("wires the same treatment into the model and key selects", () => {
        mountControls({
            ...EMPTY_USAGE_FILTER_FIELDS,
            modelId: "12",
            apiKeyId: "8",
        });

        for (const [label, id] of [
            [
                "Model",
                "12",
            ],
            [
                "API key",
                "8",
            ],
        ] as const) {
            const options = Array.from(selectWithLabel(label).options).map(optionRepr);

            expect(options.at(-1)).toEqual({
                value: id,
                label: `${id} (no longer listed)`,
                isSelected: true,
            });
        }
    });

    it("reports the option's value when the no-longer-listed choice is picked", () => {
        const { edits } = mountControls({
            ...EMPTY_USAGE_FILTER_FIELDS,
            providerId: "9",
        });

        const select = selectWithLabel("Provider");

        // The synthetic trailing option is a real choice the operator can
        // pick: choosing it reports its id, not a clearing.
        select.value = "9";
        select.dispatchEvent(new Event("change"));

        expect(edits).toEqual([
            {
                providerId: "9",
            },
        ]);

        // Picking the empty choice still clears the filter.
        select.value = "";
        select.dispatchEvent(new Event("change"));

        expect(edits).toEqual([
            {
                providerId: "9",
            },
            {
                providerId: "",
            },
        ]);
    });
});
