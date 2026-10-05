import { translation } from "../i18n";
import { literal, model, paramRow, secret } from "./expressions";
import type { ProviderPreset } from "./types";

/** Complete authored preset for zen-openai-chat-completions. */
export const zenOpenaiChatCompletionsPreset: ProviderPreset = {
    id: "zen-openai-chat-completions",
    title: "OpenCode Zen — Chat Completions",
    get description() {
        return translation()(
            ($) => $.helpers.presets.zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "zen",
    provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
    verification: "unverified",
    payload: {
        format: "openai_chat_completion",
        inference: {
            url: [
                literal("https://opencode.ai/zen/v1"),
                literal("/chat/completions"),
            ],
            headers: [
                paramRow("Authorization", [
                    literal("Bearer "),
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
                    literal("https://opencode.ai/zen/v1/models"),
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

/** Complete authored preset for zen-openai-responses. */
export const zenOpenaiResponsesPreset: ProviderPreset = {
    id: "zen-openai-responses",
    title: "OpenCode Zen — Responses",
    get description() {
        return translation()(
            ($) => $.helpers.presets.zenOverTheOpenAIResponsesProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "zen",
    provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
    verification: "unverified",
    payload: {
        format: "openai_response",
        inference: {
            url: [
                literal("https://opencode.ai/zen/v1"),
                literal("/responses"),
            ],
            headers: [
                paramRow("Authorization", [
                    literal("Bearer "),
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
                    literal("https://opencode.ai/zen/v1/models"),
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

/** Complete authored preset for zen-anthropic-messages. */
export const zenAnthropicMessagesPreset: ProviderPreset = {
    id: "zen-anthropic-messages",
    title: "OpenCode Zen — Anthropic Messages",
    get description() {
        return translation()(
            ($) => $.helpers.presets.zenOverTheAnthropicMessagesProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "zen",
    provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
    verification: "unverified",
    payload: {
        format: "anthropic_messages",
        inference: {
            url: [
                literal("https://opencode.ai/zen/v1"),
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
                    literal("https://opencode.ai/zen/v1/models"),
                ],
                headers: [
                    paramRow("x-api-key", [
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

/** Complete authored preset for zen-gemini-generate-content. */
export const zenGeminiGenerateContentPreset: ProviderPreset = {
    id: "zen-gemini-generate-content",
    title: "OpenCode Zen — Gemini GenerateContent",
    get description() {
        return translation()(
            ($) => $.helpers.presets.zenOverTheGeminiGenerateContentProtocolWithAStaticKey,
        );
    },
    grant: null,
    suggestedName: "zen",
    provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
    verification: "unverified",
    payload: {
        format: "gemini_generate_content",
        inference: {
            url: [
                literal("https://opencode.ai/zen/v1"),
                model(),
                literal(":generateContent"),
            ],
            stream_url: [
                literal("https://opencode.ai/zen/v1"),
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
                    literal("https://opencode.ai/zen/v1/models"),
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
