import { test } from "vitest";
import { parseChatCompletionsRequest } from "../translation/chat-completions";
import { TranslationError } from "../translation/errors";
import {
    ANTIGRAVITY_API_BASE_URL,
    ANTIGRAVITY_AUTHORIZE_URL,
    ANTIGRAVITY_CLIENT_ID,
    ANTIGRAVITY_CLIENT_SECRET,
    ANTIGRAVITY_MODELS,
    ANTIGRAVITY_REDIRECT_URI,
    ANTIGRAVITY_TOKEN_URL,
    buildCodeAssistRequest,
    codeAssistRequestUrl,
} from "./antigravity";
import {
    buildGenerateContentRequest,
    generateContentUrl,
    mapGeminiThinkingLevel,
    parseGenerateContentResponse,
    parseGenerateContentStreamEvent,
} from "./gemini";
import {
    OPENAI_AUTHORIZE_URL,
    OPENAI_CODEX_MODELS,
    OPENAI_CODEX_RESPONSES_URL,
    OPENAI_CODEX_USER_AGENT,
    OPENAI_OAUTH_CLIENT_ID,
    OPENAI_REDIRECT_URI,
    OPENAI_TOKEN_URL,
} from "./openai";

function assert(isTrue: boolean, message: string): asserts isTrue {
    if (!isTrue) {
        throw new Error(message);
    }
}

function canonicalize(value: unknown): unknown {
    if (Array.isArray(value)) {
        return value.map(canonicalize);
    }
    if (typeof value === "object" && value !== null) {
        return Object.fromEntries(
            Object.entries(value)
                .sort(([left], [right]) => left.localeCompare(right))
                .map(([key, entry]) => [
                    key,
                    canonicalize(entry),
                ]),
        );
    }
    return value;
}

function assertEqual(actual: unknown, expected: unknown, message: string): void {
    const actualJson = JSON.stringify(canonicalize(actual));
    const expectedJson = JSON.stringify(canonicalize(expected));
    assert(actualJson === expectedJson, `${message}: ${actualJson} !== ${expectedJson}`);
}

test("pinned OpenAI and Antigravity adapters map the complete IR", () => {
    assert(OPENAI_OAUTH_CLIENT_ID === "app_EmoamVZd6MmPgQohKd7qKLFF", "OpenAI client is pinned");
    assert(
        OPENAI_AUTHORIZE_URL === "https://auth.openai.com/authorize",
        "OpenAI authorize URL is pinned",
    );
    assert(
        OPENAI_TOKEN_URL === "https://auth.openai.com/oauth/token",
        "OpenAI token URL is pinned",
    );
    assert(
        OPENAI_REDIRECT_URI === "http://localhost:1455/auth/callback",
        "OpenAI redirect is pinned",
    );
    assert(
        OPENAI_CODEX_RESPONSES_URL.endsWith("/backend-api/codex/responses"),
        "Codex endpoint is pinned",
    );
    assert(OPENAI_CODEX_USER_AGENT.startsWith("codex_cli_rs/"), "Codex user agent is pinned");
    assert(OPENAI_CODEX_MODELS.length > 0, "Codex discovery catalog is non-empty");

    assert(
        ANTIGRAVITY_CLIENT_ID.endsWith(".apps.googleusercontent.com"),
        "Antigravity client is pinned",
    );
    assert(ANTIGRAVITY_CLIENT_SECRET.startsWith("GOCSPX-"), "installed-app secret is pinned");
    assert(
        ANTIGRAVITY_AUTHORIZE_URL.includes("accounts.google.com"),
        "Google authorize URL is pinned",
    );
    assert(
        ANTIGRAVITY_TOKEN_URL === "https://oauth2.googleapis.com/token",
        "Google token URL is pinned",
    );
    assert(
        ANTIGRAVITY_REDIRECT_URI.startsWith("http://localhost:"),
        "Google loopback redirect is pinned",
    );
    assert(
        ANTIGRAVITY_API_BASE_URL === "https://cloudcode-pa.googleapis.com/v1internal",
        "Code Assist API is pinned",
    );
    assert(ANTIGRAVITY_MODELS.includes("gemini-3.1-pro-high"), "Gemini catalog is pinned");
    assert(
        codeAssistRequestUrl("project/with spaces", false).endsWith(
            "/project%2Fwith%20spaces/v1internal:generateContent",
        ),
        "project path is escaped",
    );
    assert(
        codeAssistRequestUrl("p", true).endsWith("/p/v1internal:streamGenerateContent?alt=sse"),
        "stream URL selects SSE",
    );

    assertEqual(
        mapGeminiThinkingLevel("medium"),
        "medium",
        "known Gemini efforts pass through verbatim",
    );
    assertEqual(
        mapGeminiThinkingLevel("xhigh"),
        "high",
        "efforts above the Gemini scale clamp down",
    );
    assertEqual(
        mapGeminiThinkingLevel("minimal"),
        undefined,
        "efforts below the Gemini scale drop the config",
    );
    assertEqual(
        mapGeminiThinkingLevel("extreme"),
        undefined,
        "unknown effort strings never guess a mapping",
    );
    assertEqual(
        generateContentUrl(
            "https://generativelanguage.googleapis.com/v1beta",
            "gemini-wire",
            false,
        ),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-wire:generateContent",
        "API-key URLs address the model action directly",
    );
    assert(
        generateContentUrl("https://x.test/v1beta", "m", true).endsWith(
            "/models/m:streamGenerateContent?alt=sse",
        ),
        "API-key streaming selects SSE",
    );

    const request = parseChatCompletionsRequest({
        model: "public",
        messages: [
            {
                role: "system",
                content: "System A",
            },
            {
                role: "system",
                content: "System B",
            },
            {
                role: "user",
                content: [
                    {
                        type: "text",
                        text: "inspect",
                    },
                    {
                        type: "image_url",
                        image_url: {
                            url: "data:image/png;base64,AA==",
                        },
                    },
                ],
            },
            {
                role: "assistant",
                content: "calling",
                tool_calls: [
                    {
                        id: "call_4",
                        type: "function",
                        function: {
                            name: "lookup",
                            arguments: '{"q":1}',
                        },
                        thought_signature: "sig-call",
                    },
                ],
            },
            {
                role: "tool",
                tool_call_id: "call_4",
                name: "lookup",
                content: "result",
            },
        ],
        tools: [
            {
                type: "function",
                function: {
                    name: "lookup",
                    description: "Lookup",
                    parameters: {
                        type: "object",
                        properties: {
                            q: {
                                type: "number",
                            },
                        },
                    },
                },
            },
        ],
        temperature: 0.4,
        top_p: 0.8,
        max_completion_tokens: 500,
        stop: [
            "END",
        ],
        response_format: {
            type: "json_object",
        },
        reasoning_effort: "medium",
        stream: true,
    });
    const codeAssist = buildCodeAssistRequest(request, {
        model: "gemini-wire",
        projectId: "project-1",
        reasoningEffort: "medium",
    });
    assertEqual(codeAssist.project, "project-1", "project appears in the envelope");
    assertEqual(codeAssist.model, "gemini-wire", "model appears in the envelope");
    assertEqual(
        codeAssist.request.systemInstruction?.parts,
        [
            {
                text: "System A\n\nSystem B",
            },
        ],
        "system messages merge in order",
    );
    assertEqual(
        codeAssist.request.contents[0]?.parts[1],
        {
            inlineData: {
                mimeType: "image/png",
                data: "AA==",
            },
        },
        "data URL images map to inlineData",
    );
    const percentEncodedImage = buildCodeAssistRequest(
        parseChatCompletionsRequest({
            model: "m",
            messages: [
                {
                    role: "user",
                    content: [
                        {
                            type: "image_url",
                            image_url: {
                                url: "data:image/svg+xml,%3Csvg%3E",
                            },
                        },
                    ],
                },
            ],
        }),
        {
            model: "m",
            projectId: "p",
        },
    );
    assertEqual(
        percentEncodedImage.request.contents[0]?.parts[0],
        {
            inlineData: {
                mimeType: "image/svg+xml",
                data: "PHN2Zz4=",
            },
        },
        "percent-encoded data URLs are converted to inlineData base64",
    );
    assertEqual(
        codeAssist.request.contents[1]?.parts[1],
        {
            functionCall: {
                name: "lookup",
                args: {
                    q: 1,
                },
            },
            thoughtSignature: "sig-call",
        },
        "tool calls and signatures map to Code Assist",
    );
    assertEqual(
        codeAssist.request.contents[2]?.parts[0],
        {
            functionResponse: {
                name: "lookup",
                response: {
                    output: "result",
                },
            },
        },
        "tool outputs match the previous call by id/name",
    );
    assertEqual(
        codeAssist.request.tools,
        [
            {
                functionDeclarations: [
                    {
                        name: "lookup",
                        description: "Lookup",
                        parameters: {
                            type: "object",
                            properties: {
                                q: {
                                    type: "number",
                                },
                            },
                        },
                    },
                ],
            },
        ],
        "function declarations map to Code Assist",
    );
    assertEqual(
        codeAssist.request.generationConfig,
        {
            temperature: 0.4,
            topP: 0.8,
            maxOutputTokens: 500,
            stopSequences: [
                "END",
            ],
            responseMimeType: "application/json",
            thinkingConfig: {
                includeThoughts: true,
                thinkingLevel: "medium",
            },
        },
        "sampling, JSON mode, and thinking map to Code Assist",
    );

    const bareGemini = buildGenerateContentRequest(request, {
        model: "gemini-wire",
        reasoningEffort: "xhigh",
    });
    assertEqual(
        bareGemini.generationConfig?.thinkingConfig,
        {
            includeThoughts: true,
            thinkingLevel: "high",
        },
        "API-key bodies are unwrapped and clamp xhigh to high",
    );
    assertEqual(
        Object.keys(bareGemini).sort(),
        [
            "contents",
            "generationConfig",
            "systemInstruction",
            "tools",
        ],
        "API-key requests carry no project envelope",
    );
    assertEqual(
        buildGenerateContentRequest(request, {
            model: "m",
            reasoningEffort: "extreme",
        }).generationConfig?.thinkingConfig,
        undefined,
        "unmappable efforts omit thinkingConfig entirely",
    );

    try {
        buildCodeAssistRequest(
            parseChatCompletionsRequest({
                model: "m",
                messages: [
                    {
                        role: "user",
                        content: [
                            {
                                type: "image_url",
                                image_url: {
                                    url: "https://example.test/a.png",
                                },
                            },
                        ],
                    },
                ],
            }),
            {
                model: "m",
                projectId: "p",
            },
        );
        throw new Error("expected non-data image rejection");
    } catch (error: unknown) {
        assert(error instanceof TranslationError, "non-data image should be a translation error");
        assert(error.code === "unsupported_parameter", "non-data image uses unsupported_parameter");
    }

    const responseEvents = parseGenerateContentResponse({
        response: {
            candidates: [
                {
                    content: {
                        role: "model",
                        parts: [
                            {
                                text: "private work",
                                thought: true,
                                thoughtSignature: "sig-thought",
                            },
                            {
                                text: "answer",
                            },
                            {
                                functionCall: {
                                    name: "lookup",
                                    args: {
                                        q: 2,
                                    },
                                },
                                thoughtSignature: "sig-tool",
                            },
                        ],
                    },
                    finishReason: "SAFETY",
                },
            ],
            usageMetadata: {
                promptTokenCount: 4,
                candidatesTokenCount: 6,
                totalTokenCount: 10,
                thoughtsTokenCount: 3,
                cachedContentTokenCount: 1,
            },
        },
    });
    assertEqual(
        responseEvents.map((event) => event.type),
        [
            "message_start",
            "reasoning_delta",
            "text_delta",
            "tool_call_start",
            "tool_call_delta",
            "tool_call_done",
            "finish",
            "usage",
            "done",
        ],
        "Gemini object maps to ordered IR events",
    );
    assertEqual(
        responseEvents[3],
        {
            type: "tool_call_start",
            index: 0,
            id: "call_1",
            name: "lookup",
            thoughtSignature: "sig-tool",
        },
        "Gemini synthesizes stable call ids and preserves signatures",
    );
    assertEqual(
        responseEvents[6],
        {
            type: "finish",
            reason: "content_filter",
        },
        "SAFETY maps to content_filter",
    );
    assertEqual(
        responseEvents[7],
        {
            type: "usage",
            usage: {
                inputTokens: 4,
                outputTokens: 6,
                totalTokens: 10,
                reasoningTokens: 3,
                cachedInputTokens: 1,
            },
        },
        "Gemini usage maps to IR",
    );

    const streamState = {
        isStarted: false,
        nextToolIndex: 0,
        openToolIndexes: new Map<number, string>(),
    };
    const firstStreamEvents = parseGenerateContentStreamEvent(
        {
            response: {
                candidates: [
                    {
                        content: {
                            parts: [
                                {
                                    text: "thinking",
                                    thought: true,
                                },
                                {
                                    functionCall: {
                                        name: "one",
                                        args: {
                                            a: 1,
                                        },
                                    },
                                },
                            ],
                        },
                    },
                ],
            },
        },
        streamState,
    );
    const secondStreamEvents = parseGenerateContentStreamEvent(
        {
            response: {
                candidates: [
                    {
                        content: {
                            parts: [
                                {
                                    text: "answer",
                                },
                            ],
                        },
                        finishReason: "STOP",
                    },
                ],
                usageMetadata: {
                    promptTokenCount: 1,
                    candidatesTokenCount: 2,
                    totalTokenCount: 3,
                },
            },
        },
        streamState,
    );
    assertEqual(firstStreamEvents[0]?.type, "message_start", "Code Assist stream starts once");
    assertEqual(
        firstStreamEvents[2],
        {
            type: "tool_call_start",
            index: 0,
            id: "call_1",
            name: "one",
        },
        "stream tool id synthesis starts at call_1",
    );
    assert(
        secondStreamEvents.some((event) => event.type === "tool_call_done"),
        "open streamed tool calls are closed",
    );
    assert(secondStreamEvents.at(-1)?.type === "done", "finished Gemini stream terminates");
});
