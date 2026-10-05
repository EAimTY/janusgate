import { antigravityPreset } from "./antigravity";
import { claudeCodePreset } from "./claude-code";
import { codexPreset } from "./codex";
import { devinPreset } from "./devin";
import {
    genericAnthropicMessagesPreset,
    genericGeminiGenerateContentPreset,
    genericGeminiInteractionsPreset,
    genericOpenaiChatCompletionsPreset,
    genericOpenaiResponsesPreset,
} from "./generic";
import { githubCopilotPreset } from "./github-copilot";
import { kimiCodePreset } from "./kimi-code";
import {
    zenAnthropicMessagesPreset,
    zenGeminiGenerateContentPreset,
    zenOpenaiChatCompletionsPreset,
    zenOpenaiResponsesPreset,
} from "./opencode-zen";
import type { ProviderPreset } from "./types";
import { xaiPreset } from "./xai";

/** Ordered nonempty catalog of authored product definitions. */
export const PROVIDER_PRESETS: readonly [
    ProviderPreset,
    ...ProviderPreset[],
] = [
    genericOpenaiResponsesPreset,
    genericOpenaiChatCompletionsPreset,
    genericGeminiGenerateContentPreset,
    genericGeminiInteractionsPreset,
    genericAnthropicMessagesPreset,
    xaiPreset,
    claudeCodePreset,
    kimiCodePreset,
    githubCopilotPreset,
    devinPreset,
    codexPreset,
    antigravityPreset,
    zenOpenaiChatCompletionsPreset,
    zenOpenaiResponsesPreset,
    zenAnthropicMessagesPreset,
    zenGeminiGenerateContentPreset,
];
