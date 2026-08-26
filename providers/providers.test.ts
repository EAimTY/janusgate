import { test } from "vitest";
import { parseChatCompletionsRequest } from "/translation/chat-completions";
import { TranslationError } from "/translation/errors";
import {
    ANTIGRAVITY_API_BASE_URL,
    ANTIGRAVITY_AUTHORIZE_URL,
    ANTIGRAVITY_CLIENT_ID,
    ANTIGRAVITY_CLIENT_METADATA,
    ANTIGRAVITY_CLIENT_SECRET,
    ANTIGRAVITY_HEADER_CLIENT_METADATA,
    ANTIGRAVITY_OAUTH_SCOPE,
    ANTIGRAVITY_REDIRECT_URI,
    ANTIGRAVITY_TOKEN_URL,
    ANTIGRAVITY_USER_AGENT,
    antigravityRuntimeModel,
    buildCodeAssistRequest,
    codeAssistRequestUrl,
    normalizeCodeAssistJsonSchema,
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
    for (const scope of [
        "aicode",
        "cloud-platform",
        "userinfo.email",
        "userinfo.profile",
        "cclog",
        "experimentsandconfigs",
    ]) {
        assert(ANTIGRAVITY_OAUTH_SCOPE.includes(scope), `Google OAuth requests ${scope}`);
    }
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
    assert(
        ANTIGRAVITY_USER_AGENT === "antigravity/1.15.8 darwin/arm64",
        "working Antigravity client identity is pinned",
    );
    assertEqual(
        JSON.parse(ANTIGRAVITY_CLIENT_METADATA),
        {
            ideType: "ANTIGRAVITY",
            platform: "PLATFORM_UNSPECIFIED",
            pluginType: "GEMINI",
        },
        "request-body metadata uses the protobuf platform enum",
    );
    assertEqual(
        JSON.parse(ANTIGRAVITY_HEADER_CLIENT_METADATA),
        {
            ideType: "ANTIGRAVITY",
            platform: "MACOS",
            pluginType: "GEMINI",
        },
        "client fingerprint metadata uses the legacy platform name",
    );
    assertEqual(
        antigravityRuntimeModel("gemini-3.7-flash"),
        "gemini-3.7-flash-tiered",
        "Gemini 3.7 uses its requestable tiered runtime",
    );
    assertEqual(
        antigravityRuntimeModel("gemini-3.6-flash", "high"),
        "gemini-3.6-flash-high",
        "Gemini 3.6 effort selects its runtime tier",
    );
    assertEqual(
        antigravityRuntimeModel("gemini-3.5-flash", "medium"),
        "gemini-3.5-flash-low",
        "Gemini 3.5 effort selects a requestable runtime tier",
    );
    assertEqual(
        antigravityRuntimeModel("gemini-3.1-pro-high"),
        "gemini-pro-agent",
        "Gemini 3.1 High avoids the advertised runtime that rejects requests",
    );
    assertEqual(
        antigravityRuntimeModel("gemini-3.1-pro", "high"),
        "gemini-pro-agent",
        "Gemini 3.1 logical IDs route by requested effort",
    );
    assertEqual(
        normalizeCodeAssistJsonSchema({
            type: "object",
            properties: {
                mode: {
                    const: "safe",
                    default: "safe",
                    examples: [
                        "safe",
                    ],
                },
            },
        }),
        {
            type: "object",
            properties: {
                mode: {
                    enum: [
                        "safe",
                    ],
                },
            },
        },
        "Code Assist schema normalization preserves const semantics and strips annotations",
    );
    try {
        normalizeCodeAssistJsonSchema({
            type: "object",
            properties: {
                value: {
                    $ref: "#/$defs/missing",
                },
            },
        });
        throw new Error("expected unresolved schema reference rejection");
    } catch (error: unknown) {
        assert(error instanceof TranslationError, "unresolved schemas fail with translation error");
        assert(error.code === "unsupported_parameter", "unresolved schemas fail locally");
    }
    assert(
        codeAssistRequestUrl(false) ===
            "https://cloudcode-pa.googleapis.com/v1internal:generateContent",
        "Code Assist sends project routing in the body rather than the URL path",
    );
    assert(
        codeAssistRequestUrl(true) ===
            "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse",
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
                        extra_content: {
                            google: {
                                thought_signature: "sig-call",
                            },
                        },
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
                        $schema: "https://json-schema.org/draft/2020-12/schema",
                        $defs: {
                            query: {
                                type: "number",
                            },
                        },
                        type: "object",
                        properties: {
                            q: {
                                $ref: "#/$defs/query",
                            },
                        },
                        required: [
                            "q",
                        ],
                    },
                },
            },
        ],
        tool_choice: {
            type: "function",
            function: {
                name: "lookup",
            },
        },
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
    assertEqual(codeAssist.requestType, "agent", "agent request type appears in the envelope");
    assertEqual(codeAssist.userAgent, "antigravity", "client identity appears in the envelope");
    assert(
        codeAssist.requestId.startsWith("agent-") && codeAssist.requestId.length > "agent-".length,
        "each envelope carries an agent request ID",
    );
    assertEqual(
        buildCodeAssistRequest(request, {
            model: "gemini-3.7-flash",
            projectId: "project-1",
        }).request.generationConfig?.thinkingConfig,
        {
            thinkingLevel: "LOW",
        },
        "tiered Gemini 3.7 always receives the required uppercase thinking level",
    );
    assertEqual(
        buildCodeAssistRequest(request, {
            model: "gemini-3.6-flash-low",
            projectId: "project-1",
            reasoningEffort: "high",
        }).request.generationConfig?.thinkingConfig,
        undefined,
        "discovered runtime-tier IDs do not receive a conflicting thinking config",
    );
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
    const mixedCaseImage = buildCodeAssistRequest(
        parseChatCompletionsRequest({
            model: "m",
            messages: [
                {
                    role: "user",
                    content: [
                        {
                            type: "image_url",
                            image_url: {
                                url: "DATA:IMAGE/PNG;BASE64,AA==",
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
        mixedCaseImage.request.contents[0]?.parts[0],
        {
            inlineData: {
                mimeType: "image/png",
                data: "AA==",
            },
        },
        "data URL scheme, MIME type, and base64 marker are case-insensitive",
    );
    const normalizedTurns = buildCodeAssistRequest(
        parseChatCompletionsRequest({
            model: "m",
            messages: [
                {
                    role: "user",
                    content: "first",
                },
                {
                    role: "user",
                    content: "second",
                },
                {
                    role: "assistant",
                    content: "reply",
                },
            ],
        }),
        {
            model: "m",
            projectId: "p",
        },
    ).request.contents;
    assertEqual(
        normalizedTurns,
        [
            {
                role: "user",
                parts: [
                    {
                        text: "first",
                    },
                    {
                        text: "second",
                    },
                ],
            },
            {
                role: "model",
                parts: [
                    {
                        text: "reply",
                    },
                ],
            },
        ],
        "consecutive same-role messages merge into valid GenerateContent turns",
    );
    try {
        buildCodeAssistRequest(
            parseChatCompletionsRequest({
                model: "m",
                messages: [
                    {
                        role: "assistant",
                        content: "prefill",
                    },
                ],
            }),
            {
                model: "m",
                projectId: "p",
            },
        );
        throw new Error("expected assistant-first history rejection");
    } catch (error: unknown) {
        assert(error instanceof TranslationError, "assistant-first history should fail locally");
        assert(
            error.code === "unsupported_parameter",
            "assistant-first history uses unsupported_parameter",
        );
    }
    assertEqual(
        codeAssist.request.contents[1]?.parts[1],
        {
            functionCall: {
                id: "call_4",
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
                id: "call_4",
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
                        parametersJsonSchema: {
                            type: "object",
                            properties: {
                                q: {
                                    type: "number",
                                },
                            },
                            required: [
                                "q",
                            ],
                        },
                    },
                ],
            },
        ],
        "function declarations map to Code Assist",
    );
    assertEqual(
        codeAssist.request.toolConfig,
        {
            functionCallingConfig: {
                mode: "ANY",
                allowedFunctionNames: [
                    "lookup",
                ],
            },
        },
        "tool choice maps to Code Assist function-calling config",
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

    const structuredCodeAssist = buildCodeAssistRequest(
        parseChatCompletionsRequest({
            model: "m",
            messages: [
                {
                    role: "user",
                    content: "json",
                },
            ],
            response_format: {
                type: "json_schema",
                json_schema: {
                    name: "answer",
                    schema: {
                        type: "string",
                        const: "ok",
                    },
                },
            },
        }),
        {
            model: "m",
            projectId: "p",
        },
    );
    assertEqual(
        structuredCodeAssist.request.generationConfig?.responseJsonSchema,
        {
            type: "string",
            enum: [
                "ok",
            ],
        },
        "Code Assist structured output uses responseJsonSchema",
    );

    const customToolRequest = parseChatCompletionsRequest({
        model: "m",
        messages: [
            {
                role: "user",
                content: "use a tool",
            },
        ],
        tools: [
            {
                type: "function",
                function: {
                    name: "lookup",
                    parameters: {
                        type: "object",
                        properties: {
                            query: {
                                type: [
                                    "string",
                                    "null",
                                ],
                                format: "custom",
                            },
                        },
                    },
                },
            },
        ],
        reasoning_effort: "high",
    });
    const claudeCodeAssist = buildCodeAssistRequest(customToolRequest, {
        model: "claude-opus-4-6",
        projectId: "p",
        reasoningEffort: "high",
    });
    assertEqual(claudeCodeAssist.model, "claude-opus-4-6-thinking", "Claude runtime routes");
    assertEqual(
        claudeCodeAssist.request.generationConfig,
        {
            maxOutputTokens: 64000,
        },
        "Claude uses its output limit and no separate thinking config",
    );
    assertEqual(
        claudeCodeAssist.request.tools?.[0]?.functionDeclarations[0]?.parameters,
        {
            type: "object",
            properties: {
                query: {
                    type: "string",
                },
            },
        },
        "Claude uses the allowlisted legacy custom-tool schema",
    );
    assertEqual(
        claudeCodeAssist.request.toolConfig,
        {
            functionCallingConfig: {
                mode: "VALIDATED",
            },
        },
        "Claude defaults tool calling to validated mode",
    );
    const gptCodeAssist = buildCodeAssistRequest(
        parseChatCompletionsRequest({
            model: "m",
            messages: [
                {
                    role: "user",
                    content: "do not call tools",
                },
            ],
            tools: [
                {
                    type: "function",
                    function: {
                        name: "lookup",
                    },
                },
            ],
            tool_choice: "none",
        }),
        {
            model: "gpt-oss-120b",
            projectId: "p",
        },
    );
    assertEqual(gptCodeAssist.model, "gpt-oss-120b-medium", "GPT-OSS runtime routes");
    assertEqual(
        gptCodeAssist.request.generationConfig?.maxOutputTokens,
        32768,
        "GPT-OSS uses its output limit",
    );
    assertEqual(
        gptCodeAssist.request.toolConfig,
        {
            functionCallingConfig: {
                mode: "NONE",
            },
        },
        "GPT-OSS preserves explicit tool choice",
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
            "toolConfig",
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
                                    id: "upstream-call",
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
                totalTokenCount: 13,
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
            id: "upstream-call",
            name: "lookup",
            thoughtSignature: "sig-tool",
        },
        "Gemini synthesizes stable call ids and preserves signatures",
    );
    const synthesizedToolIds = [
        parseGenerateContentResponse({
            candidates: [
                {
                    content: {
                        parts: [
                            {
                                functionCall: {
                                    name: "first",
                                    args: {},
                                },
                            },
                        ],
                    },
                },
            ],
        }).find((event) => event.type === "tool_call_start"),
        parseGenerateContentResponse({
            candidates: [
                {
                    content: {
                        parts: [
                            {
                                functionCall: {
                                    name: "second",
                                    args: {},
                                },
                            },
                        ],
                    },
                },
            ],
        }).find((event) => event.type === "tool_call_start"),
    ];
    assert(
        synthesizedToolIds[0]?.type === "tool_call_start" &&
            synthesizedToolIds[1]?.type === "tool_call_start" &&
            synthesizedToolIds[0].id !== synthesizedToolIds[1].id,
        "synthesized tool IDs remain unique across independent turns",
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
                outputTokens: 9,
                totalTokens: 13,
                reasoningTokens: 3,
                cachedInputTokens: 1,
            },
        },
        "Gemini usage maps to IR",
    );
    assertEqual(
        parseGenerateContentResponse({
            promptFeedback: {
                blockReason: "SAFETY",
            },
        }).find((event) => event.type === "finish"),
        {
            type: "finish",
            reason: "content_filter",
        },
        "prompt-level blocks without candidate content map to content_filter",
    );
    for (const invalidResponse of [
        {
            error: {
                message: "quota",
            },
        },
        {
            candidates: [
                {
                    content: {
                        parts: [],
                    },
                    finishReason: "MALFORMED_FUNCTION_CALL",
                },
            ],
        },
    ]) {
        try {
            parseGenerateContentResponse(invalidResponse);
            throw new Error("expected invalid Gemini response rejection");
        } catch (error: unknown) {
            assert(error instanceof TranslationError, "invalid Gemini responses fail parsing");
            assert(
                error.code === "upstream_failure",
                "invalid Gemini responses are upstream failures",
            );
        }
    }

    const streamState = {
        isStarted: false,
        nextToolIndex: 0,
        openToolCallIndexes: new Set<number>(),
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
                                        id: "stream-call",
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
            id: "stream-call",
            name: "one",
        },
        "stream tool calls preserve upstream IDs",
    );
    assert(
        secondStreamEvents.some((event) => event.type === "tool_call_done"),
        "open streamed tool calls are closed",
    );
    assert(secondStreamEvents.at(-1)?.type === "done", "finished Gemini stream terminates");
});
