import { translation } from "../i18n";
import {
    anthropicModelsDiscovery,
    base,
    geminiModelsDiscovery,
    literal,
    model,
    openAiModelsDiscovery,
    paramRow,
    secret,
} from "./expressions";
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
        discovery: openAiModelsDiscovery([
            base(),
            literal("/models"),
        ]),
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
        discovery: openAiModelsDiscovery([
            base(),
            literal("/models"),
        ]),
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
            // Discovery strips the `models/` resource prefix off the reported
            // names, so the inference URLs re-spell it before the bare model
            // id: `{base}/models/{model}:generateContent`.
            url: [
                base(),
                literal("/models/"),
                model(),
                literal(":generateContent"),
            ],
            stream_url: [
                base(),
                literal("/models/"),
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
        discovery: geminiModelsDiscovery([
            base(),
            literal("/models"),
        ]),
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
        discovery: geminiModelsDiscovery(
            [
                base(),
                literal("/v1beta/models"),
            ],
            [
                paramRow("Api-Revision", [
                    literal("2026-09-01"),
                ]),
            ],
        ),
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
        discovery: anthropicModelsDiscovery(
            [
                base(),
                literal("/v1/models"),
            ],
            [
                paramRow("x-api-key", [
                    secret(),
                ]),
            ],
        ),
    },
};
