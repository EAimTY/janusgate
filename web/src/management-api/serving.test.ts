import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "./generated/v0";
import {
    automaticModelNames,
    capabilitySource,
    credentialStanding,
    effectiveCompactionFacts,
    effectiveMaxInputTokens,
    effectiveMaxOutputTokens,
    effectiveThinkingLevels,
    isModelInventoryReady,
    lapsedCredentialStanding,
    modelCountsByProvider,
    readyInventoryCountsByProvider,
} from "./serving";

/**
 * The serving derivations the wire leaves to the client: the counts over the
 * model listing, the three-input inventory verdict, the effective
 * declarations (override over discovered, never invented), the compaction
 * facts resolved per mechanism with the explicit `"unknown"` override
 * suppressing the lower layers, and the credential standing read off the
 * row's own columns at the reading moment.
 */

/** One enabled static-key provider row. */
function providerRow(
    overrides: Partial<
        Extract<
            ProviderResponse,
            {
                type: "api_key";
            }
        >
    > = {},
): ProviderResponse {
    return {
        type: "api_key",
        id: 1,
        name: "acme",
        model_prefix: "acme",
        is_enabled: true,
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
        ...overrides,
    };
}

/** One enabled OAuth provider row, defaulting to no stored expiry. */
function oauthProviderRow(
    overrides: Partial<
        Extract<
            ProviderResponse,
            {
                type: "oauth";
            }
        >
    > = {},
): ProviderResponse {
    return {
        type: "oauth",
        expires_at: null,
        id: 1,
        name: "acme",
        model_prefix: "acme",
        is_enabled: true,
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
        ...overrides,
    };
}

/** One enabled, discovered model row. */
function modelRow(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 10,
        provider_id: 1,
        provider: "acme",
        upstream_id: "gpt-x",
        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: [
            "low",
            "high",
        ],
        max_input_tokens: 128_000,
        max_output_tokens: 4_096,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: {
            responses_standalone_compact: "supported",
        },
        compaction_support_override: null,
        ...overrides,
    };
}

beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-10T12:00:00Z"));
});

afterEach(() => {
    vi.useRealTimers();
});

describe("model counts", () => {
    test("count every listed model per provider without a predicate", () => {
        const counts = modelCountsByProvider([
            modelRow({
                id: 1,
            }),
            modelRow({
                id: 2,
                provider_id: 2,
            }),
            modelRow({
                id: 3,
            }),
        ]);

        expect(counts.get(1)).toBe(2);
        expect(counts.get(2)).toBe(1);
    });

    test("state which rows count through the optional predicate", () => {
        const counts = modelCountsByProvider(
            [
                modelRow({
                    id: 1,
                }),
                modelRow({
                    id: 2,
                    is_enabled: false,
                }),
            ],
            (model) => model.is_enabled,
        );

        expect(counts.get(1)).toBe(1);
        expect(counts.get(2)).toBeUndefined();
    });

    test("count ready inventory through the shared three-input verdict", () => {
        const providersById = new Map([
            [
                1,
                providerRow(),
            ],
            [
                2,
                providerRow({
                    id: 2,
                    is_enabled: false,
                }),
            ],
        ]);
        const models = [
            modelRow({
                id: 1,
                provider_id: 1,
            }),
            modelRow({
                id: 2,
                provider_id: 2,
            }),
            modelRow({
                id: 3,
                provider_id: 1,
                is_enabled: false,
            }),
        ];

        expect(readyInventoryCountsByProvider(models, providersById).get(1)).toBe(1);
        expect(readyInventoryCountsByProvider(models, providersById).get(2)).toBeUndefined();
    });
});

describe("isModelInventoryReady", () => {
    test("requires the enabled account, the enabled model, and a non-vanished row", () => {
        const provider = providerRow();

        expect(isModelInventoryReady(modelRow(), provider)).toBe(true);
        expect(
            isModelInventoryReady(
                modelRow(),
                providerRow({
                    is_enabled: false,
                }),
            ),
        ).toBe(false);
        expect(
            isModelInventoryReady(
                modelRow({
                    is_enabled: false,
                }),
                provider,
            ),
        ).toBe(false);
        expect(
            isModelInventoryReady(
                modelRow({
                    discovery_state: "vanished",
                }),
                provider,
            ),
        ).toBe(false);
        expect(isModelInventoryReady(modelRow(), undefined)).toBe(false);
        // A manual row is standing inventory, not a vanished one.
        expect(
            isModelInventoryReady(
                modelRow({
                    discovery_state: "manual",
                }),
                provider,
            ),
        ).toBe(true);
    });
});

describe("effective declarations", () => {
    test("read the override when it stands, the discovered value otherwise", () => {
        expect(
            effectiveThinkingLevels(
                modelRow({
                    thinking_levels_override: [
                        "none",
                    ],
                }),
            ),
        ).toEqual([
            "none",
        ]);
        expect(
            effectiveThinkingLevels(
                modelRow({
                    thinking_levels: null,
                }),
            ),
        ).toBeNull();

        expect(
            effectiveMaxInputTokens(
                modelRow({
                    max_input_tokens_override: 8_000,
                }),
            ),
        ).toBe(8_000);
        expect(
            effectiveMaxInputTokens(
                modelRow({
                    max_input_tokens: null,
                }),
            ),
        ).toBeNull();

        expect(
            effectiveMaxOutputTokens(
                modelRow({
                    max_output_tokens_override: 512,
                }),
            ),
        ).toBe(512);
        expect(
            effectiveMaxOutputTokens(
                modelRow({
                    max_output_tokens: null,
                }),
            ),
        ).toBeNull();
    });
});

describe("effectiveCompactionFacts", () => {
    test("answers every mechanism, the override member winning over the discovered fact", () => {
        const facts = effectiveCompactionFacts(
            modelRow({
                compaction_support: {
                    responses_standalone_compact: "supported",
                },
                compaction_support_override: {
                    responses_standalone_compact: "unsupported",
                },
            }),
        );

        expect(facts.responses_standalone_compact).toEqual({
            support: "unsupported",
            source: "override",
        });
        expect(facts.responses_explicit_trigger).toEqual({
            support: "unknown",
            source: "unknown",
        });
    });

    test("reads the discovered fact where no override member stands", () => {
        const facts = effectiveCompactionFacts(
            modelRow({
                compaction_support: {
                    messages_on_demand_compaction: "unsupported",
                },
            }),
        );

        expect(facts.messages_on_demand_compaction).toEqual({
            support: "unsupported",
            source: "discovered",
        });
    });

    test("lets an explicit unknown override suppress the discovered fact", () => {
        const facts = effectiveCompactionFacts(
            modelRow({
                compaction_support: {
                    responses_standalone_compact: "supported",
                },
                compaction_support_override: {
                    responses_standalone_compact: "unknown",
                },
            }),
        );

        expect(facts.responses_standalone_compact).toEqual({
            support: "unknown",
            source: "override",
        });
    });
});

describe("capabilitySource", () => {
    test("answers override, discovered, or unknown by presence alone", () => {
        expect(capabilitySource([], null)).toBe("override");
        expect(
            capabilitySource(null, [
                "low",
            ]),
        ).toBe("discovered");
        expect(capabilitySource(null, null)).toBe("unknown");
        // A standing override replaces the declaration wholesale, even where the base is unknown.
        expect(capabilitySource(0, null)).toBe("override");
    });
});

describe("credential standing", () => {
    test("answers no standing for a static-key row", () => {
        expect(credentialStanding(providerRow())).toBeNull();
        expect(lapsedCredentialStanding(providerRow())).toBeNull();
    });

    test("answers valid while the stored expiry is ahead, expired once it has passed", () => {
        const ahead = oauthProviderRow({
            expires_at: Date.now() * 1000 + 3_600_000_000,
        });
        const behind = oauthProviderRow({
            expires_at: Date.now() * 1000 - 3_600_000_000,
        });

        expect(credentialStanding(ahead)).toBe("valid");
        expect(lapsedCredentialStanding(ahead)).toBeNull();
        expect(credentialStanding(behind)).toBe("expired");
        expect(lapsedCredentialStanding(behind)).toBe("expired");
    });

    test("counts an expiry exactly at the reading moment as expired", () => {
        const now = oauthProviderRow({
            expires_at: Date.now() * 1000,
        });

        expect(credentialStanding(now)).toBe("expired");
    });

    test("answers never-fresh for an OAuth row storing no expiry", () => {
        const neverFresh = oauthProviderRow();

        expect(credentialStanding(neverFresh)).toBe("never-fresh");
        expect(lapsedCredentialStanding(neverFresh)).toBe("never-fresh");
    });
});

describe("automaticModelNames", () => {
    /** One automatic direct route over model 10. */
    const DIRECT: ModelRouteResponse = {
        id: 100,
        public_id: "acme/gpt-x",
        is_automatic: true,
        is_enabled: true,
        target: {
            type: "direct",
            model_id: 10,
        },
    };
    /** One automatic pool route over upstream `gpt-x` in pool 7. */
    const POOLED: ModelRouteResponse = {
        id: 101,
        public_id: "fast/gpt-x",
        is_automatic: true,
        is_enabled: true,
        target: {
            type: "pool",
            pool_id: 7,
            upstream_id: "gpt-x",
        },
    };
    /** One authored route, which never belongs to a model's automatic names. */
    const AUTHORED: ModelRouteResponse = {
        id: 102,
        public_id: "aliased",
        is_automatic: false,
        is_enabled: true,
        target: {
            type: "direct",
            model_id: 10,
        },
    };

    const POOL: ProviderPoolResponse = {
        id: 7,
        name: "fast",
        model_prefix: "fast",
        is_enabled: true,
        members: [
            {
                provider_id: 1,
                priority: 0,
            },
        ],
    };

    test("collects the direct automatic route over the model's own row id", () => {
        expect(
            automaticModelNames(
                modelRow(),
                [
                    DIRECT,
                    AUTHORED,
                ],
                [
                    POOL,
                ],
            ),
        ).toEqual([
            "acme/gpt-x",
        ]);
    });

    test("collects a pool route only when the model's provider sits in that exact pool", () => {
        expect(
            automaticModelNames(
                modelRow(),
                [
                    POOLED,
                ],
                [
                    POOL,
                ],
            ),
        ).toEqual([
            "fast/gpt-x",
        ]);
        expect(
            automaticModelNames(
                modelRow(),
                [
                    POOLED,
                ],
                [],
            ),
        ).toEqual([]);
        expect(
            automaticModelNames(
                modelRow({
                    provider_id: 2,
                }),
                [
                    POOLED,
                ],
                [
                    {
                        ...POOL,
                        members: [
                            {
                                provider_id: 3,
                                priority: 0,
                            },
                        ],
                    },
                ],
            ),
        ).toEqual([]);
    });
});
