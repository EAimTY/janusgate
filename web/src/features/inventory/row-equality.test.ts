import { describe, expect, test } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { isSameApiKeyRow } from "../api-keys/row-equality";
import { isSameModelRow, isSameProviderRow, isSameRowFields } from "./row-equality";

/**
 * The field-for-field row equality the refresh identity pins ride: the walk
 * over every generated column, arrays compared element-wise, structured
 * fields by their named comparators — so a refresh that changed nothing
 * never remounts a form, and a changed answer adopts the fresh row.
 */

function provider(
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

function oauthProvider(
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

function model(overrides: Partial<ModelResponse> = {}): ModelResponse {
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
        max_input_tokens: null,
        max_output_tokens: null,
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

function apiKey(overrides: Partial<ApiKeyResponse> = {}): ApiKeyResponse {
    return {
        id: 5,
        name: "batch",
        key: "jkc-1",
        is_enabled: true,
        created_at: 1_700_000_000_000_000,
        access_control: {
            default_access: "deny",
            owner_defaults: [],
            route_overrides: [],
        },
        ...overrides,
    };
}

describe("isSameRowFields", () => {
    test("answers equal for content-equal rows and unequal on any changed column", () => {
        expect(isSameRowFields(provider(), provider())).toBe(true);
        expect(
            isSameRowFields(
                provider(),
                provider({
                    name: "renamed",
                }),
            ),
        ).toBe(false);
        expect(
            isSameRowFields(
                provider(),
                provider({
                    is_enabled: false,
                }),
            ),
        ).toBe(false);
    });

    test("compares array values element-wise, not by reference", () => {
        // A nullable array column, the shape the generated model rows carry.
        const row: {
            id: number;
            levels: string[] | null;
            empty: null;
        } = {
            id: 1,
            levels: [
                "low",
                "high",
            ],
            empty: null,
        };
        const twin = {
            id: 1,
            levels: [
                "low",
                "high",
            ],
            empty: null,
        };

        expect(isSameRowFields(row, twin)).toBe(true);
        expect(
            isSameRowFields(row, {
                ...twin,
                levels: [
                    "high",
                    "low",
                ],
            }),
        ).toBe(false);
        expect(
            isSameRowFields(row, {
                ...twin,
                levels: null,
            }),
        ).toBe(false);
        expect(
            isSameRowFields(row, {
                ...twin,
                levels: [
                    "low",
                ],
            }),
        ).toBe(false);
    });

    test("never answers equal across a tagged union's members", () => {
        // The members' key sets differ, so the key-count check ends the walk.
        expect(isSameRowFields(provider(), oauthProvider())).toBe(false);
        expect(isSameRowFields(oauthProvider(), provider())).toBe(false);
    });

    test("compares a structured field by its named comparator", () => {
        const withOverride = {
            name: "x",
            compaction: {
                a: 1,
            },
        };
        expect(
            isSameRowFields(
                withOverride,
                {
                    name: "x",
                    compaction: {
                        a: 1,
                    },
                },
                {
                    compaction: (previous, fresh) =>
                        JSON.stringify(previous) === JSON.stringify(fresh),
                },
            ),
        ).toBe(true);
        expect(
            isSameRowFields(
                withOverride,
                {
                    name: "x",
                    compaction: {
                        a: 2,
                    },
                },
                {
                    compaction: (previous, fresh) =>
                        JSON.stringify(previous) === JSON.stringify(fresh),
                },
            ),
        ).toBe(false);
    });

    test("requires the fresh row to carry exactly the previous row's keys", () => {
        // The walk's domain is rows whose key sets the static types do not pin
        // equal — the union members and any grown column — so the fixtures say so.
        const narrow: {
            id: number;
            name?: string;
            extra?: boolean;
        } = {
            id: 1,
            name: "x",
        };

        expect(
            isSameRowFields(narrow, {
                id: 1,
                name: "x",
                extra: true,
            }),
        ).toBe(false);
        expect(
            isSameRowFields(narrow, {
                id: 1,
            }),
        ).toBe(false);
    });
});

describe("isSameProviderRow", () => {
    test("keeps a provider row's identity across a content-equal re-read", () => {
        const row = provider();

        expect(isSameProviderRow(row, provider())).toBe(true);
        expect(
            isSameProviderRow(
                row,
                provider({
                    base_url: "https://api.example.test",
                }),
            ),
        ).toBe(false);
    });

    test("compares the OAuth member's expiry column", () => {
        const row = oauthProvider({
            expires_at: 1_800_000_000_000_000,
        });

        expect(
            isSameProviderRow(
                row,
                oauthProvider({
                    expires_at: 1_800_000_000_000_000,
                }),
            ),
        ).toBe(true);
        expect(
            isSameProviderRow(
                row,
                oauthProvider({
                    expires_at: null,
                }),
            ),
        ).toBe(false);
    });
});

describe("isSameModelRow", () => {
    test("compares the compaction-facts maps by their entries, key order never carrying facts", () => {
        const row = model();

        expect(isSameModelRow(row, model())).toBe(true);
        expect(
            isSameModelRow(
                row,
                model({
                    compaction_support: {
                        responses_standalone_compact: "supported",
                    },
                }),
            ),
        ).toBe(true);
        expect(
            isSameModelRow(
                row,
                model({
                    compaction_support: null,
                }),
            ),
        ).toBe(false);
        expect(
            isSameModelRow(
                row,
                model({
                    compaction_support: {
                        responses_standalone_compact: "unsupported",
                    },
                }),
            ),
        ).toBe(false);
    });

    test("answers null meets null and null never meets a record", () => {
        const row = model({
            compaction_support: null,
            compaction_support_override: null,
        });
        const fresh = model({
            compaction_support: null,
            compaction_support_override: null,
        });

        expect(isSameModelRow(row, fresh)).toBe(true);
        expect(isSameModelRow(fresh, model())).toBe(false);
    });
});

describe("isSameApiKeyRow", () => {
    test("compares the access policy structurally, rules in any order equal", () => {
        const row = apiKey({
            access_control: {
                default_access: "deny",
                owner_defaults: [
                    {
                        type: "provider",
                        provider_id: 3,
                        access: "allow",
                    },
                    {
                        type: "pool",
                        pool_id: 8,
                        access: "deny",
                    },
                ],
                route_overrides: [
                    {
                        model_route_id: 12,
                        access: "allow",
                    },
                ],
            },
        });

        const reordered = apiKey({
            access_control: {
                default_access: "deny",
                owner_defaults: [
                    {
                        type: "pool",
                        pool_id: 8,
                        access: "deny",
                    },
                    {
                        type: "provider",
                        provider_id: 3,
                        access: "allow",
                    },
                ],
                route_overrides: [
                    {
                        model_route_id: 12,
                        access: "allow",
                    },
                ],
            },
        });

        expect(isSameApiKeyRow(row, reordered)).toBe(true);
        expect(
            isSameApiKeyRow(
                row,
                apiKey({
                    access_control: {
                        ...row.access_control,
                        default_access: "allow",
                    },
                }),
            ),
        ).toBe(false);
    });

    test("distinguishes an explicit equal-default pin from an absent rule", () => {
        const pinned = apiKey({
            access_control: {
                default_access: "deny",
                owner_defaults: [
                    {
                        type: "provider",
                        provider_id: 3,
                        access: "deny",
                    },
                ],
                route_overrides: [],
            },
        });

        expect(isSameApiKeyRow(pinned, apiKey())).toBe(false);
    });
});
