import { translation } from "../i18n";
import { base, literal, model, paramRow, secret } from "./expressions";
import type { ProviderPreset } from "./types";

/** Complete authored preset for generic-openai-responses. */
export const genericOpenaiResponsesPreset: ProviderPreset = {
    id: "generic-openai-responses",
    title: "OpenAI-compatible Responses API",
    get description() {
        return translation()(
            ($) => $.helpers.presets.anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "openai",
    provenance: "OpenAI Responses API reference (platform.openai.com/docs/api-reference/responses)",
    verification: "live",
    payload: {
        format: "openai_response",
        inference: {
            url: [
                base(),
                literal("/responses"),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: {
            request: {
                method: "GET",
                url: [
                    base(),
                    literal("/models"),
                ],
                headers: [
                    paramRow("Authorization", [
                        literal("Bearer "),
                        secret(),
                    ]),
                ],
            },
            extract: {
                mode: "array",
                array_at: [
                    "data",
                ],
                id_field: [
                    "id",
                ],
            },
            pagination: {
                type: "none",
            },
        },
    },
};

/** Complete authored preset for generic-openai-chat-completions. */
export const genericOpenaiChatCompletionsPreset: ProviderPreset = {
    id: "generic-openai-chat-completions",
    title: "OpenAI-compatible Chat Completions API",
    get description() {
        return translation()(
            ($) => $.helpers.presets.anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "openai",
    provenance:
        "OpenAI Chat Completions API reference (platform.openai.com/docs/api-reference/chat)",
    verification: "live",
    payload: {
        format: "openai_chat_completion",
        inference: {
            url: [
                base(),
                literal("/chat/completions"),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: {
            request: {
                method: "GET",
                url: [
                    base(),
                    literal("/models"),
                ],
                headers: [
                    paramRow("Authorization", [
                        literal("Bearer "),
                        secret(),
                    ]),
                ],
            },
            extract: {
                mode: "array",
                array_at: [
                    "data",
                ],
                id_field: [
                    "id",
                ],
            },
            pagination: {
                type: "none",
            },
        },
    },
};

/** Complete authored preset for generic-gemini-generate-content. */
export const genericGeminiGenerateContentPreset: ProviderPreset = {
    id: "generic-gemini-generate-content",
    title: "Gemini-compatible GenerateContent API",
    get description() {
        return translation()(
            ($) =>
                $.helpers.presets.anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "gemini",
    provenance: "Gemini API reference (ai.google.dev/api/generate-content)",
    verification: "live",
    payload: {
        format: "gemini_generate_content",
        inference: {
            url: [
                base(),
                model(),
                literal(":generateContent"),
            ],
            stream_url: [
                base(),
                model(),
                literal(":streamGenerateContent?alt=sse"),
            ],
            headers: [
                paramRow("x-goog-api-key", [
                    secret(),
                ]),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: {
            request: {
                method: "GET",
                url: [
                    base(),
                    literal("/models"),
                ],
                query: [
                    paramRow("pageSize", [
                        literal("1000"),
                    ]),
                ],
                headers: [
                    paramRow("x-goog-api-key", [
                        secret(),
                    ]),
                ],
            },
            extract: {
                mode: "array",
                array_at: [
                    "models",
                ],
                id_field: [
                    "name",
                ],
                strip_prefix: "models/",
            },
            pagination: {
                type: "token",
                request_param: "pageToken",
                response_path: [
                    "nextPageToken",
                ],
                max_pages: 10,
            },
        },
    },
};

/** Complete authored preset for generic-gemini-interactions. */
export const genericGeminiInteractionsPreset: ProviderPreset = {
    id: "generic-gemini-interactions",
    title: "Gemini-compatible Interactions API",
    get description() {
        return translation()(
            ($) => $.helpers.presets.anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "gemini",
    provenance: "Gemini Interactions API reference (ai.google.dev/api/interactions-api)",
    verification: "live",
    payload: {
        format: "gemini_interaction",
        inference: {
            url: [
                base(),
                literal("/v1beta/interactions"),
            ],
            headers: [
                paramRow("x-goog-api-key", [
                    secret(),
                ]),
                paramRow("Api-Revision", [
                    literal("2026-09-01"),
                ]),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: {
            request: {
                method: "GET",
                url: [
                    base(),
                    literal("/v1beta/models"),
                ],
                query: [
                    paramRow("pageSize", [
                        literal("1000"),
                    ]),
                ],
                headers: [
                    paramRow("x-goog-api-key", [
                        secret(),
                    ]),
                    paramRow("Api-Revision", [
                        literal("2026-09-01"),
                    ]),
                ],
            },
            extract: {
                mode: "array",
                array_at: [
                    "models",
                ],
                id_field: [
                    "name",
                ],
                strip_prefix: "models/",
            },
            pagination: {
                type: "token",
                request_param: "pageToken",
                response_path: [
                    "nextPageToken",
                ],
                max_pages: 10,
            },
        },
    },
};

/** Complete authored preset for generic-anthropic-messages. */
export const genericAnthropicMessagesPreset: ProviderPreset = {
    id: "generic-anthropic-messages",
    title: "Anthropic-compatible Messages API",
    get description() {
        return translation()(
            ($) => $.helpers.presets.anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "anthropic",
    provenance: "Anthropic Messages API reference (docs.anthropic.com/en/api/messages)",
    verification: "live",
    payload: {
        format: "anthropic_messages",
        inference: {
            url: [
                base(),
                literal("/v1/messages"),
            ],
            headers: [
                paramRow("x-api-key", [
                    secret(),
                ]),
                paramRow("anthropic-version", [
                    literal("2023-06-01"),
                ]),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: {
            request: {
                method: "GET",
                url: [
                    base(),
                    literal("/v1/models"),
                ],
                headers: [
                    paramRow("x-api-key", [
                        secret(),
                    ]),
                    paramRow("anthropic-version", [
                        literal("2023-06-01"),
                    ]),
                ],
            },
            extract: {
                mode: "array",
                array_at: [
                    "data",
                ],
                id_field: [
                    "id",
                ],
            },
            pagination: {
                type: "cursor",
                has_more_path: [
                    "has_more",
                ],
                last_id_path: [
                    "last_id",
                ],
                request_param: "after_id",
                max_pages: 10,
            },
        },
    },
};
