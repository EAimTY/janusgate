import { translation } from "../i18n";
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
                {
                    type: "literal",
                    value: "https://opencode.ai/zen/v1",
                },
                {
                    type: "literal",
                    value: "/chat/completions",
                },
            ],
            headers: [
                {
                    name: "Authorization",
                    value: [
                        {
                            type: "literal",
                            value: "Bearer ",
                        },
                        {
                            type: "secret",
                        },
                    ],
                },
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
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1/models",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
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
                {
                    type: "literal",
                    value: "https://opencode.ai/zen/v1",
                },
                {
                    type: "literal",
                    value: "/responses",
                },
            ],
            headers: [
                {
                    name: "Authorization",
                    value: [
                        {
                            type: "literal",
                            value: "Bearer ",
                        },
                        {
                            type: "secret",
                        },
                    ],
                },
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
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1/models",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
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
                {
                    type: "literal",
                    value: "https://opencode.ai/zen/v1",
                },
                {
                    type: "literal",
                    value: "/v1/messages",
                },
            ],
            headers: [
                {
                    name: "x-api-key",
                    value: [
                        {
                            type: "secret",
                        },
                    ],
                },
                {
                    name: "anthropic-version",
                    value: [
                        {
                            type: "literal",
                            value: "2023-06-01",
                        },
                    ],
                },
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
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1/models",
                    },
                ],
                headers: [
                    {
                        name: "x-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
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
                {
                    type: "literal",
                    value: "https://opencode.ai/zen/v1",
                },
                {
                    type: "model",
                },
                {
                    type: "literal",
                    value: ":generateContent",
                },
            ],
            stream_url: [
                {
                    type: "literal",
                    value: "https://opencode.ai/zen/v1",
                },
                {
                    type: "model",
                },
                {
                    type: "literal",
                    value: ":streamGenerateContent?alt=sse",
                },
            ],
            headers: [
                {
                    name: "x-goog-api-key",
                    value: [
                        {
                            type: "secret",
                        },
                    ],
                },
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
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1/models",
                    },
                ],
                headers: [
                    {
                        name: "x-goog-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
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
