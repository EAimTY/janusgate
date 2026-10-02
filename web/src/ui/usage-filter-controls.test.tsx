import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { UsageFilterControls } from "/src/ui/usage-filter-controls";
import { EMPTY_USAGE_FILTER_FIELDS, type UsageFilterFields } from "/src/usage-filter";
import {
    chooseSelect,
    createDisposals,
    labeledSelect,
    optionRepr,
} from "../screens/screen-test-support";

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
    thinking_levels: null,
    max_input_tokens: null,
    max_output_tokens: null,
    thinking_levels_override: null,
    max_input_tokens_override: null,
    max_output_tokens_override: null,
    compaction_support: null,
    compaction_support_override: null,
};

const apiKeyRow: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1,
};

const disposes = createDisposals();

/**
 * Mounts the controls holding the given fields, collecting the edits they
 * report. The listings stand read by default — the settled state every
 * option-list fact below is stated past; the pre-read render is pinned by the
 * mounting screens' suites.
 */
function mountControls(
    fields: UsageFilterFields,
    hasEntityListings = true,
): {
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
                    hasEntityListings={hasEntityListings}
                    statusError={null}
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

describe("UsageFilterControls listing-backed options", () => {
    it("renders a chosen id the listing no longer carries as its trailing, selected option", () => {
        mountControls({
            ...EMPTY_USAGE_FILTER_FIELDS,
            providerId: "9",
        });

        const options = Array.from(labeledSelect("Provider").options).map(optionRepr);

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
            const options = Array.from(labeledSelect(label).options).map(optionRepr);

            expect(options.some((option) => option.label.includes("no longer listed"))).toBe(false);
        }

        expect(Array.from(labeledSelect("Provider").options).map(optionRepr)).toContainEqual({
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
            const options = Array.from(labeledSelect(label).options).map(optionRepr);

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

        // The synthetic trailing option is a real choice the operator can
        // pick: choosing it reports its id, not a clearing.
        chooseSelect("Provider", "9");

        expect(edits).toEqual([
            {
                providerId: "9",
            },
        ]);

        // Picking the empty choice still clears the filter.
        chooseSelect("Provider", "");

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
