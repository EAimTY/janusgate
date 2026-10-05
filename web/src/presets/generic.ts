import { translation } from "../i18n";
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
                {
                    type: "base",
                },
                {
                    type: "literal",
                    value: "/responses",
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
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/models",
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
                {
                    type: "base",
                },
                {
                    type: "literal",
                    value: "/chat/completions",
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
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/models",
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
                {
                    type: "base",
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
                    type: "base",
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
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/models",
                    },
                ],
                query: [
                    {
                        name: "pageSize",
                        value: [
                            {
                                type: "literal",
                                value: "1000",
                            },
                        ],
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
                {
                    type: "base",
                },
                {
                    type: "literal",
                    value: "/v1beta/interactions",
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
                {
                    name: "Api-Revision",
                    value: [
                        {
                            type: "literal",
                            value: "2026-09-01",
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
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/v1beta/models",
                    },
                ],
                query: [
                    {
                        name: "pageSize",
                        value: [
                            {
                                type: "literal",
                                value: "1000",
                            },
                        ],
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
                    {
                        name: "Api-Revision",
                        value: [
                            {
                                type: "literal",
                                value: "2026-09-01",
                            },
                        ],
                    },
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
                {
                    type: "base",
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
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/v1/models",
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
