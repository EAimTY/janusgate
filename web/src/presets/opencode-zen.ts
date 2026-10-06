import { translation } from "../i18n";
import {
    bearerAuthorization,
    literal,
    model,
    openAiModelsDiscovery,
    paramRow,
    secret,
} from "./expressions";
import type { ProviderPreset } from "./types";

/**
 * The pinned OpenCode Zen endpoint literals every variant composes. The
 * payloads are immutable and serialized, so the shared nodes are safe.
 */
const zenV1 = literal("https://opencode.ai/zen/v1");
const zenModels = literal("https://opencode.ai/zen/v1/models");

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
                zenV1,
                literal("/chat/completions"),
            ],
            headers: [
                bearerAuthorization(),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: openAiModelsDiscovery([
            zenModels,
        ]),
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
                zenV1,
                literal("/responses"),
            ],
            headers: [
                bearerAuthorization(),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: openAiModelsDiscovery([
            zenModels,
        ]),
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
                zenV1,
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
        discovery: openAiModelsDiscovery(
            [
                zenModels,
            ],
            [
                paramRow("x-api-key", [
                    secret(),
                ]),
            ],
        ),
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
                zenV1,
                model(),
                literal(":generateContent"),
            ],
            stream_url: [
                zenV1,
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
        discovery: openAiModelsDiscovery(
            [
                zenModels,
            ],
            [
                paramRow("x-goog-api-key", [
                    secret(),
                ]),
            ],
        ),
    },
};
