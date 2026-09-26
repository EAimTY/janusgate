import { describe, expect, it } from "vitest";
import { ContractViolationError } from "./errors";

import {
    parseApiKeyEnvelope,
    parseCompleteProviderOauthResponse,
    parseDiscoverModelsResponse,
    parseEmptyResponse,
    parseErrorResponse,
    parseListApiKeysResponse,
    parseListModelsResponse,
    parseListProvidersResponse,
    parseListUsageResponse,
    parseModelEnvelope,
    parseProviderEnvelope,
    parseQueryUsageResponse,
    parseStartProviderOauthResponse,
} from "./validation";

const sharedFields = (overrides: Record<string, unknown> = {}) => ({
    id: 1,
    name: "local",
    model_prefix: "local-",
    is_enabled: true,
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    base_url: "https://upstream.example.com",
    ...overrides,
});

const compatibleProvider = {
    type: "api_key",
    ...sharedFields(),
};

const oauthProvider = {
    type: "oauth",
    ...sharedFields({
        id: 2,
        name: "chatgpt",
        model_prefix: "",
        is_enabled: false,
        provider_definition_name: "codex",
        format: "openai_response",
        base_url: null,
        expires_at: 1_800_000_000_000_000,
    }),
};

const antigravityProvider = {
    type: "oauth",
    ...sharedFields({
        id: 3,
        name: "antigravity",
        model_prefix: "ag-",
        is_enabled: true,
        provider_definition_name: "antigravity",
        format: "gemini_generate_content",
        base_url: null,
        expires_at: null,
    }),
};

const responsesProvider = {
    type: "api_key",
    ...sharedFields({
        id: 4,
        name: "responses",
        model_prefix: "",
        provider_definition_name: "generic-openai-responses",
        format: "openai_response",
        base_url: "https://api.openai.com/v1",
    }),
};

const geminiProvider = {
    type: "api_key",
    ...sharedFields({
        id: 5,
        name: "gemini",
        model_prefix: "gemini-",
        provider_definition_name: "generic-gemini-generate-content",
        format: "gemini_generate_content",
    }),
    is_enabled: false,
    base_url: "https://generativelanguage.googleapis.com",
};

const model = {
    id: 9,
    provider_id: 1,
    provider: "local",
    upstream_id: "qwen3:32b",
    public_id: "local-qwen3-32b",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

const apiLog = {
    started_at: 1_780_000_000_000_000,
    api_key_id: 7,
    provider_id: 1,
    model_id: 9,
    endpoint_type: "openai_responses",
    is_stream: false,
    http_status: 200,
    outcome: "completed",
    duration: 812,
    input_tokens: 120,
    output_tokens: 340,
    cached_tokens: null,
    cache_write_tokens: null,
    reasoning_tokens: 12,
};

const usageBucket = {
    key: {
        day: "2026-09-22",
    },
    requests: 3,
    input_tokens: 360,
    output_tokens: 1020,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 36,
};

describe("response validation", () => {
    it("parses every provider variant", () => {
        const response = parseListProvidersResponse({
            providers: [
                responsesProvider,
                compatibleProvider,
                geminiProvider,
                oauthProvider,
                antigravityProvider,
            ],
        });

        expect(response.providers).toEqual([
            responsesProvider,
            compatibleProvider,
            geminiProvider,
            oauthProvider,
            antigravityProvider,
        ]);
    });

    it("validates each variant's own field on the branch its type selects", () => {
        expect(() =>
            parseListProvidersResponse({
                providers: [
                    {
                        ...oauthProvider,
                        expires_at: "soon",
                    },
                ],
            }),
        ).toThrow(/response\.providers\[0\]\.expires_at must be an integer or null/);

        expect(() =>
            parseListProvidersResponse({
                providers: [
                    {
                        ...compatibleProvider,
                        base_url: 7,
                    },
                ],
            }),
        ).toThrow(/response\.providers\[0\]\.base_url must be a string/);
    });

    it("tolerates unknown extra keys but reads every required one", () => {
        const response = parseListProvidersResponse({
            providers: [
                {
                    ...compatibleProvider,
                    future_field: "ignored",
                },
            ],
            future_envelope_field: 7,
        });

        expect(response.providers).toEqual([
            compatibleProvider,
        ]);
    });

    it("rejects a provider that is missing a field of its variant", () => {
        const { base_url: _baseUrl, ...withoutBaseUrl } = compatibleProvider;

        expect(() =>
            parseListProvidersResponse({
                providers: [
                    withoutBaseUrl,
                ],
            }),
        ).toThrow(/response\.providers\[0\]\.base_url must be a string/);
    });

    it("rejects a provider whose type is not part of the union", () => {
        expect(() =>
            parseListProvidersResponse({
                providers: [
                    {
                        ...compatibleProvider,
                        type: "gemini_oauth",
                    },
                ],
            }),
        ).toThrow(/response\.providers\[0\]\.type is not a known credential kind/);
    });

    it("rejects a non-object response and a non-array listing", () => {
        expect(() => parseListProvidersResponse("nope")).toThrow(ContractViolationError);

        expect(() =>
            parseListProvidersResponse({
                providers: {},
            }),
        ).toThrow(/response\.providers must be an array/);
    });

    it("keeps the nullable mirror column of a model", () => {
        const overridden = {
            ...model,
            id_override: "custom-id",
            discovery_state: "manual",
        };

        const vanished = {
            ...model,
            id: 10,
            is_enabled: true,
            discovery_state: "vanished",
        };

        const response = parseListModelsResponse({
            models: [
                model,
                overridden,
                vanished,
            ],
        });

        expect(response.models).toEqual([
            model,
            overridden,
            vanished,
        ]);
    });

    it("rejects a discovery state outside the closed set", () => {
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...model,
                        discovery_state: "forgotten",
                    },
                ],
            }),
        ).toThrow(/response\.models\[0\]\.discovery_state is not a discovery state/);
    });

    it("rejects a model whose row identifier is not an integer", () => {
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...model,
                        id: "local-qwen3-32b",
                    },
                ],
            }),
        ).toThrow(/response\.models\[0\]\.id must be an integer/);
    });

    it("rejects an omitted required-nullable field", () => {
        const { id_override: _idOverride, ...withoutOverride } = model;

        expect(() =>
            parseListModelsResponse({
                models: [
                    withoutOverride,
                ],
            }),
        ).toThrow(/response\.models\[0\]\.id_override must be a string or null/);
    });

    it("parses envelopes and discovery listings", () => {
        expect(
            parseProviderEnvelope({
                provider: oauthProvider,
            }),
        ).toEqual({
            provider: oauthProvider,
        });

        expect(
            parseModelEnvelope({
                model,
            }),
        ).toEqual({
            model,
        });

        expect(
            parseDiscoverModelsResponse({
                models: [
                    model,
                ],
            }),
        ).toEqual({
            models: [
                model,
            ],
        });

        expect(
            parseStartProviderOauthResponse({
                grant: "authorization_code",
                authorize_url: "https://auth.example.com/authorize",
                state: "opaque",
                expires_at: 1_800_000_600_000_000,
            }),
        ).toEqual({
            grant: "authorization_code",
            authorize_url: "https://auth.example.com/authorize",
            state: "opaque",
            expires_at: 1_800_000_600_000_000,
        });

        expect(
            parseCompleteProviderOauthResponse({
                status: "pending",
            }),
        ).toEqual({
            status: "pending",
        });
    });

    it("parses the usage listing and its aggregation", () => {
        expect(
            parseListUsageResponse({
                api_logs: [
                    apiLog,
                ],
            }),
        ).toEqual({
            api_logs: [
                apiLog,
            ],
        });

        // A row without usage and without a produced response keeps both as
        // null; labels never ride along (rule 7).
        const untouched = {
            ...apiLog,
            http_status: null,
            outcome: "client_aborted",
            input_tokens: null,
            output_tokens: null,
            reasoning_tokens: null,
        };

        expect(
            parseListUsageResponse({
                api_logs: [
                    untouched,
                ],
            }).api_logs,
        ).toEqual([
            untouched,
        ]);

        // The composite key types every present dimension and passes absence
        // through; a `NULL` status bucket reads as null.
        const modelBucket = {
            ...usageBucket,
            key: {
                day: "2026-09-22",
                model_id: 9,
                http_status: null,
            },
        };

        expect(
            parseQueryUsageResponse({
                buckets: [
                    usageBucket,
                    modelBucket,
                ],
            }),
        ).toEqual({
            buckets: [
                usageBucket,
                modelBucket,
            ],
        });

        // A key requesting every dimension types them all in one pass.
        const fullBucket = {
            ...usageBucket,
            key: {
                day: "2026-09-22",
                model_id: 9,
                provider_id: 1,
                api_key_id: 7,
                endpoint_type: "openai_responses",
                outcome: "completed",
                is_stream: true,
                http_status: null,
            },
        };

        expect(
            parseQueryUsageResponse({
                buckets: [
                    fullBucket,
                ],
            }),
        ).toEqual({
            buckets: [
                fullBucket,
            ],
        });

        // A present key field must still type-check and stay in vocabulary.
        expect(() =>
            parseQueryUsageResponse({
                buckets: [
                    {
                        ...usageBucket,
                        key: {
                            day: 5,
                        },
                    },
                ],
            }),
        ).toThrow(/response\.buckets\[0\]\.key\.day must be a string/);

        expect(() =>
            parseQueryUsageResponse({
                buckets: [
                    {
                        ...usageBucket,
                        key: {
                            outcome: "succeeded",
                        },
                    },
                ],
            }),
        ).toThrow(/response\.buckets\[0\]\.key\.outcome is not a request outcome/);

        expect(() =>
            parseListUsageResponse({
                api_logs: [
                    {
                        ...apiLog,
                        outcome: "succeeded",
                    },
                ],
            }),
        ).toThrow(/response\.api_logs\[0\]\.outcome is not a request outcome/);

        expect(() =>
            parseListUsageResponse({
                api_logs: [
                    {
                        ...apiLog,
                        endpoint_type: "openai_embeddings",
                    },
                ],
            }),
        ).toThrow(/response\.api_logs\[0\]\.endpoint_type is not an endpoint type/);
    });

    it("parses the api-keys listing and its envelope", () => {
        const row = {
            id: 7,
            name: "service-a",
            key: "sk-live",
            is_enabled: true,
            created_at: 1_700_000_000_000_000,
        };

        expect(
            parseListApiKeysResponse({
                api_keys: [
                    row,
                ],
            }),
        ).toEqual({
            api_keys: [
                row,
            ],
        });

        expect(
            parseApiKeyEnvelope({
                api_key: row,
            }),
        ).toEqual({
            api_key: row,
        });

        expect(() =>
            parseListApiKeysResponse({
                api_keys: [
                    {
                        ...row,
                        key: 7,
                    },
                ],
            }),
        ).toThrow(/response\.api_keys\[0\]\.key must be a string/);
    });

    it("accepts only an object as an empty response", () => {
        expect(parseEmptyResponse({})).toEqual({});
        expect(() => parseEmptyResponse([])).toThrow(/response must be an object/);
        expect(() => parseEmptyResponse(null)).toThrow(ContractViolationError);
    });
});

describe("parseErrorResponse", () => {
    it("reads the flat errno-shaped body", () => {
        expect(
            parseErrorResponse({
                errno: 2,
                message: "invalid management token",
            }),
        ).toEqual({
            errno: 2,
            message: "invalid management token",
        });
    });

    it("rejects bodies outside the closed taxonomy and malformed bodies", () => {
        expect(
            parseErrorResponse({
                errno: 99,
                message: "unknown",
            }),
        ).toBeNull();

        expect(
            parseErrorResponse({
                errno: 2,
            }),
        ).toBeNull();

        expect(
            parseErrorResponse({
                errno: "2",
                message: "text errno",
            }),
        ).toBeNull();

        expect(parseErrorResponse("plain text")).toBeNull();
        expect(parseErrorResponse(null)).toBeNull();
    });
});
