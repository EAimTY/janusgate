import { describe, expect, it } from "vitest";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import type { BadgeTone } from "/src/ui/feedback";
import { modelCompactionBadges, modelInventoryBadge } from "./model-list";

const enabledProvider: ProviderResponse = {
    model_prefix: "",
    type: "api_key",
    id: 4,
    name: "local",
    is_enabled: true,
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    base_url: "https://upstream.example.com/v1",
};

const disabledProvider: ProviderResponse = {
    ...enabledProvider,
    is_enabled: false,
};

function model(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 9,
        provider_id: 4,
        provider: "local",
        upstream_id: "qwen3:32b",

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
        ...overrides,
    };
}

/**
 * Every rung of the badge's precedence ladder: the serving verdict first,
 * then the one cause that keeps the row from serving — the row's own switch
 * outranks the suspension, and the suspension outranks the provider's hold.
 */
const LADDER: ReadonlyArray<{
    readonly name: string;
    readonly model: ModelResponse;
    readonly provider: ProviderResponse | undefined;
    readonly tone: BadgeTone;
    readonly label: string;
}> = [
    {
        name: "a listed row with both switches on serves",
        model: model(),
        provider: enabledProvider,
        tone: "ok",
        label: "Ready",
    },
    {
        name: "a manual row with both switches on serves",
        model: model({
            discovery_state: "manual",
        }),
        provider: enabledProvider,
        tone: "ok",
        label: "Ready",
    },
    {
        name: "the row's own switch reads Off over an enabled provider",
        model: model({
            is_enabled: false,
        }),
        provider: enabledProvider,
        tone: "muted",
        label: "Off",
    },
    {
        name: "the row's own switch reads Off even when the row is vanished",
        model: model({
            is_enabled: false,
            discovery_state: "vanished",
        }),
        provider: enabledProvider,
        tone: "muted",
        label: "Off",
    },
    {
        name: "the row's own switch reads Off even when the provider is off",
        model: model({
            is_enabled: false,
        }),
        provider: disabledProvider,
        tone: "muted",
        label: "Off",
    },
    {
        name: "a vanished row with both switches on reads Suspended",
        model: model({
            discovery_state: "vanished",
        }),
        provider: enabledProvider,
        tone: "warn",
        label: "Suspended",
    },
    {
        name: "a vanished row reads Suspended even when the provider is off",
        model: model({
            discovery_state: "vanished",
        }),
        provider: disabledProvider,
        tone: "warn",
        label: "Suspended",
    },
    {
        name: "a vanished row reads Suspended when its provider row is absent",
        model: model({
            discovery_state: "vanished",
        }),
        provider: undefined,
        tone: "warn",
        label: "Suspended",
    },
    {
        name: "a listed row behind an off provider is Held by provider",
        model: model(),
        provider: disabledProvider,
        tone: "warn",
        label: "Held by provider",
    },
    {
        name: "a manual row behind an off provider is Held by provider",
        model: model({
            discovery_state: "manual",
        }),
        provider: disabledProvider,
        tone: "warn",
        label: "Held by provider",
    },
    {
        name: "a listed row whose provider row is absent reads Unknown",
        model: model(),
        provider: undefined,
        tone: "muted",
        label: "Unknown",
    },
    {
        name: "a manual row whose provider row is absent reads Unknown",
        model: model({
            discovery_state: "manual",
        }),
        provider: undefined,
        tone: "muted",
        label: "Unknown",
    },
];

describe("modelInventoryBadge", () => {
    it("walks the precedence ladder one rung per cause", () => {
        for (const rung of LADDER) {
            expect(modelInventoryBadge(rung.model, rung.provider), rung.name).toEqual({
                tone: rung.tone,
                label: rung.label,
            });
        }
    });
});

describe("modelCompactionBadges", () => {
    it("omits every mechanism a row with no facts carries", () => {
        expect(modelCompactionBadges(model())).toEqual([]);
    });

    it("states each discovered fact, positive and negative, per mechanism", () => {
        expect(
            modelCompactionBadges(
                model({
                    compaction_support: {
                        messages_on_demand_compaction: "supported",
                        messages_threshold_compaction: "unsupported",
                    },
                }),
            ),
        ).toEqual([
            {
                tone: "ok",
                label: "on-demand",
            },
            {
                tone: "muted",
                label: "no threshold",
            },
        ]);
    });

    it("reads the override over the discovered facts, per mechanism", () => {
        expect(
            modelCompactionBadges(
                model({
                    compaction_support: {
                        messages_on_demand_compaction: "supported",
                    },
                    compaction_support_override: {
                        // The explicit unknown is a positive "no fact": the
                        // badge omits the mechanism rather than claiming a
                        // direction nobody stated.
                        messages_on_demand_compaction: "unknown",
                        responses_standalone_compact: "supported",
                    },
                }),
            ),
        ).toEqual([
            {
                tone: "ok",
                label: "compact",
            },
        ]);
    });
});
