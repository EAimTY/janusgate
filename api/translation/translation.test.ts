import { test } from "vitest";
import {
    parseChatCompletionsRequest,
    parseChatCompletionsResponse,
    parseChatCompletionsStreamEvent,
    renderChatCompletionsRequest,
    renderChatCompletionsResponse,
    renderChatCompletionsStream,
} from "./chat-completions";
import { TranslationError } from "./errors";
import {
    createResponsesStreamRenderer,
    parseResponsesRequest,
    parseResponsesResponse,
    parseResponsesStreamEvent,
    renderResponsesRequest,
    renderResponsesResponse,
} from "./responses";
import { encodeSse, SseParser } from "./sse";
import { parseReasoningEffort } from "./thinking";

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

function assertTranslationError(action: () => unknown, code: string): void {
    try {
        action();
    } catch (error: unknown) {
        assert(error instanceof TranslationError, "expected TranslationError");
        assert(error.code === code, `expected ${code}, got ${error.code}`);
        return;
    }
    throw new Error(`expected ${code}`);
}

test("Responses and Chat Completions translate through the event IR", () => {
    assertEqual(
        parseReasoningEffort("xhigh", "reasoning.effort"),
        "xhigh",
        "effort strings pass through verbatim",
    );
    assertEqual(
        parseReasoningEffort("extreme", "reasoning_effort"),
        "extreme",
        "unknown effort values are opaque strings, not errors",
    );
    assertEqual(
        parseReasoningEffort(undefined, "reasoning.effort"),
        undefined,
        "omitted reasoning is never injected",
    );
    assertTranslationError(
        () => parseReasoningEffort(3, "reasoning.effort"),
        "unsupported_parameter",
    );
    assertTranslationError(
        () => parseReasoningEffort("", "reasoning_effort"),
        "unsupported_parameter",
    );

    const responseRequest = parseResponsesRequest({
        model: "public-model",
        instructions: "Be exact.",
        input: [
            {
                role: "user",
                content: [
                    {
                        type: "input_text",
                        text: "inspect",
                    },
                    {
                        type: "input_image",
                        image_url: "data:image/png;base64,AA==",
                        detail: "low",
                    },
                ],
            },
            {
                type: "function_call",
                call_id: "call_7",
                name: "read_file",
                arguments: '{"path":"a.ts"}',
            },
            {
                type: "function_call_output",
                call_id: "call_7",
                output: "contents",
            },
        ],
        tools: [
            {
                type: "function",
                name: "read_file",
                description: "Read a file",
                parameters: {
                    type: "object",
                },
                strict: true,
            },
        ],
        tool_choice: "required",
        temperature: 0.2,
        top_p: 0.9,
        max_output_tokens: 200,
        reasoning: {
            effort: "high",
        },
        stream: true,
    });
    assertEqual(responseRequest.messages[0]?.role, "system", "instructions become system context");
    assertEqual(responseRequest.messages[1]?.parts[1]?.type, "image", "Responses images enter IR");
    assertEqual(
        responseRequest.messages[2]?.parts[0]?.type,
        "tool_call",
        "Responses tool calls enter IR",
    );
    assertEqual(responseRequest.reasoningEffort, "high", "Responses effort enters IR verbatim");
    assertEqual(responseRequest.isStreaming, true, "Responses stream is preserved");
    const developerRequest = parseResponsesRequest({
        model: "m",
        input: [
            {
                type: "message",
                role: "developer",
                content: "Follow the application rules.",
            },
            {
                role: "user",
                content: "Hello",
            },
        ],
    });
    assertEqual(
        developerRequest.messages,
        [
            {
                role: "system",
                parts: [
                    {
                        type: "text",
                        text: "Follow the application rules.",
                    },
                ],
            },
            {
                role: "user",
                parts: [
                    {
                        type: "text",
                        text: "Hello",
                    },
                ],
            },
        ],
        "Responses developer messages become system context for translated upstreams",
    );
    const reasoningHistory = parseResponsesRequest({
        model: "m",
        input: [
            {
                type: "reasoning",
                thought_signature: "sig-reasoning",
                summary: [
                    {
                        type: "summary_text",
                        text: "Prior reasoning",
                    },
                ],
            },
            {
                role: "user",
                content: "Continue",
            },
        ],
    });
    assertEqual(
        reasoningHistory.messages[0],
        {
            role: "assistant",
            parts: [],
            reasoning: "Prior reasoning",
            reasoningThoughtSignature: "sig-reasoning",
        },
        "Responses reasoning history enters the translated IR",
    );
    assertTranslationError(
        () =>
            parseResponsesRequest({
                model: "m",
                input: "x",
                store: true,
            }),
        "unsupported_parameter",
    );
    assertTranslationError(
        () =>
            parseResponsesRequest({
                model: "m",
                input: "x",
                tools: [
                    {
                        type: "web_search",
                    },
                ],
            }),
        "unsupported_tool_type",
    );

    const chatRequest = renderChatCompletionsRequest(responseRequest, "wire-model", "medium");
    assertEqual(chatRequest.model, "wire-model", "translated model is substituted");
    assertEqual(chatRequest.tool_choice, "required", "tool choice survives OpenAI translation");
    assertEqual(chatRequest.reasoning_effort, "medium", "effective effort is emitted");
    assertEqual(
        chatRequest.messages[1]?.content,
        [
            {
                type: "text",
                text: "inspect",
            },
            {
                type: "image_url",
                image_url: {
                    url: "data:image/png;base64,AA==",
                    detail: "low",
                },
            },
        ],
        "IR images render to Chat Completions",
    );
    assertEqual(
        chatRequest.messages[2]?.tool_calls?.[0]?.function.arguments,
        '{"path":"a.ts"}',
        "tool arguments remain JSON text",
    );
    assertEqual(chatRequest.messages[3]?.tool_call_id, "call_7", "tool output call id is retained");

    assertEqual(
        parseChatCompletionsRequest({
            model: "m",
            messages: [
                {
                    role: "developer",
                    content: "Application rules",
                },
            ],
        }).messages[0]?.role,
        "system",
        "Chat Completions developer messages become system context",
    );

    const parsedChatRequest = parseChatCompletionsRequest({
        model: "m",
        messages: [
            {
                role: "system",
                content: "Answer briefly.",
            },
            {
                role: "user",
                content: [
                    {
                        type: "text",
                        text: "look",
                    },
                    {
                        type: "image_url",
                        image_url: {
                            url: "https://example.test/a.png",
                        },
                    },
                ],
            },
            {
                role: "assistant",
                content: null,
                reasoning_content: "checking",
                tool_calls: [
                    {
                        id: "call_1",
                        type: "function",
                        function: {
                            name: "lookup",
                            arguments: '{"q":1}',
                        },
                    },
                ],
            },
            {
                role: "tool",
                tool_call_id: "call_1",
                name: "lookup",
                content: "ok",
            },
        ],
        tools: [
            {
                type: "function",
                function: {
                    name: "lookup",
                    parameters: {
                        type: "object",
                    },
                },
            },
        ],
        response_format: {
            type: "json_schema",
            json_schema: {
                name: "answer",
                schema: {},
            },
        },
        stop: [
            "END",
        ],
        reasoning_effort: "low",
    });
    assertEqual(
        parsedChatRequest.messages[2]?.reasoning,
        "checking",
        "CC reasoning history is retained",
    );
    assertEqual(
        parsedChatRequest.messages[3]?.parts[0]?.type,
        "tool_output",
        "CC tool output enters IR",
    );
    assertEqual(
        parsedChatRequest.sampling.responseFormat?.type,
        "json_schema",
        "response format enters IR",
    );
    assertTranslationError(
        () =>
            parseChatCompletionsRequest({
                model: "m",
                messages: [],
                n: 2,
            }),
        "unsupported_parameter",
    );

    const responseEvents = parseResponsesResponse({
        id: "resp_upstream",
        model: "wire-model",
        status: "completed",
        output: [
            {
                id: "msg_1",
                type: "message",
                role: "assistant",
                content: [
                    {
                        type: "output_text",
                        text: "hello",
                    },
                ],
            },
            {
                id: "fc_1",
                type: "function_call",
                call_id: "call_1",
                name: "lookup",
                arguments: '{"q":1}',
            },
        ],
        usage: {
            input_tokens: 10,
            output_tokens: 4,
            total_tokens: 14,
            output_tokens_details: {
                reasoning_tokens: 2,
            },
        },
    });
    const chatResponse = renderChatCompletionsResponse(responseEvents, {
        id: "chat_janus",
        model: "public-model",
        created: 100,
    });
    assertEqual(
        chatResponse.choices[0]?.finish_reason,
        "tool_calls",
        "tool calls determine finish reason",
    );
    assertEqual(chatResponse.choices[0]?.message.content, "hello", "text is accumulated");
    assertEqual(
        chatResponse.usage,
        {
            prompt_tokens: 10,
            completion_tokens: 4,
            total_tokens: 14,
            completion_tokens_details: {
                reasoning_tokens: 2,
            },
        },
        "usage maps Responses to CC",
    );

    const signedChatResponse = renderChatCompletionsResponse(
        [
            {
                type: "message_start",
            },
            {
                type: "tool_call_start",
                index: 0,
                id: "signed-call",
                name: "lookup",
                thoughtSignature: "signed-thought",
            },
            {
                type: "tool_call_done",
                index: 0,
            },
            {
                type: "finish",
                reason: "tool_calls",
            },
            {
                type: "done",
            },
        ],
        {
            id: "chat_signed",
            model: "public-model",
            created: 10,
        },
    );
    assertEqual(
        signedChatResponse.choices[0]?.message.tool_calls?.[0]?.extra_content,
        {
            google: {
                thought_signature: "signed-thought",
            },
        },
        "Chat Completions emits Google's documented thought-signature carrier",
    );
    assertEqual(
        parseChatCompletionsResponse(signedChatResponse).find(
            (event) => event.type === "tool_call_start",
        ),
        {
            type: "tool_call_start",
            index: 0,
            id: "signed-call",
            name: "lookup",
            thoughtSignature: "signed-thought",
        },
        "Chat Completions parses Google's documented thought-signature carrier",
    );

    const chatEvents = parseChatCompletionsResponse({
        id: "chat_upstream",
        model: "wire-model",
        created: 10,
        choices: [
            {
                index: 0,
                message: {
                    role: "assistant",
                    content: "done",
                    reasoning_content: "work",
                    reasoning_thought_signature: "sig-work",
                    tool_calls: [],
                },
                finish_reason: "length",
            },
        ],
        usage: {
            prompt_tokens: 3,
            completion_tokens: 5,
            total_tokens: 8,
        },
    });
    const responsesResponse = renderResponsesResponse(chatEvents, {
        id: "resp_janus",
        model: "public-model",
        created: 100,
    });
    assertEqual(
        responsesResponse.output[0]?.["thought_signature"],
        "sig-work",
        "reasoning signatures survive Chat Completions to Responses translation",
    );
    assertEqual(
        responsesResponse.status,
        "incomplete",
        "length produces an incomplete Responses object",
    );
    assertEqual(
        responsesResponse.incomplete_details,
        {
            reason: "max_output_tokens",
        },
        "length reason maps to Responses",
    );
    assertEqual(
        responsesResponse.usage,
        {
            input_tokens: 3,
            output_tokens: 5,
            total_tokens: 8,
        },
        "CC usage maps to Responses",
    );

    const parsedSse: {
        event?: string;
        data: string;
    }[] = [];
    const sseParser = new SseParser((frame) => parsedSse.push(frame));
    sseParser.push('event: one\r\ndata: {"x":');
    sseParser.push("1}\r\n\r\ndata: line1\ndata: line2\n\n");
    sseParser.finish();
    assertEqual(
        parsedSse,
        [
            {
                event: "one",
                data: '{"x":1}',
            },
            {
                data: "line1\nline2",
            },
        ],
        "SSE framing survives chunk boundaries and multiline data",
    );
    const splitCrLfFrames: {
        event?: string;
        data: string;
    }[] = [];
    const splitCrLfParser = new SseParser((frame) => splitCrLfFrames.push(frame));
    splitCrLfParser.push("event: split\r");
    splitCrLfParser.push("\ndata: ok\r");
    splitCrLfParser.push("\n\r");
    splitCrLfParser.push("\n");
    splitCrLfParser.finish();
    assertEqual(
        splitCrLfFrames,
        [
            {
                event: "split",
                data: "ok",
            },
        ],
        "SSE CRLF framing remains correct when CR and LF arrive in different chunks",
    );
    assertEqual(
        encodeSse("response.completed", {
            ok: true,
        }),
        'event: response.completed\ndata: {"ok":true}\n\n',
        "SSE encoding is canonical",
    );

    const streamEvents = [
        ...parseResponsesStreamEvent({
            type: "response.created",
            response: {
                id: "resp_1",
            },
        }),
        ...parseResponsesStreamEvent({
            type: "response.output_text.delta",
            delta: "a",
        }),
        ...parseResponsesStreamEvent({
            type: "response.output_item.added",
            output_index: 1,
            item: {
                type: "function_call",
                call_id: "call_9",
                name: "lookup",
            },
        }),
        ...parseResponsesStreamEvent({
            type: "response.function_call_arguments.delta",
            output_index: 1,
            delta: '{"q":1}',
        }),
        ...parseResponsesStreamEvent({
            type: "response.completed",
            response: {
                usage: {
                    input_tokens: 2,
                    output_tokens: 3,
                    total_tokens: 5,
                },
            },
        }),
    ];
    const chatSse = renderChatCompletionsStream(streamEvents, {
        id: "chat_1",
        model: "public-model",
        created: 20,
        isUsageIncluded: true,
    });
    assert(chatSse.at(-1) === "data: [DONE]\n\n", "CC stream has a terminal DONE frame");
    assert(
        chatSse.some((frame) => frame.includes('"usage":{"prompt_tokens":2')),
        "CC stream emits requested usage chunk",
    );
    assert(
        chatSse.some((frame) => frame.includes('"tool_calls"')),
        "CC stream emits tool call deltas",
    );

    const chatStreamEvents = [
        ...parseChatCompletionsStreamEvent({
            id: "chat_1",
            choices: [
                {
                    index: 0,
                    delta: {
                        role: "assistant",
                    },
                    finish_reason: null,
                },
            ],
        }),
        ...parseChatCompletionsStreamEvent({
            choices: [
                {
                    index: 0,
                    delta: {
                        reasoning_content: "think",
                        thought_signature: "sig-stream-reasoning",
                        content: "answer",
                    },
                    finish_reason: null,
                },
            ],
        }),
        ...parseChatCompletionsStreamEvent({
            choices: [
                {
                    index: 0,
                    delta: {},
                    finish_reason: "stop",
                },
            ],
            usage: {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
            },
        }),
        {
            type: "done",
        } as const,
    ];
    const responseSse = createResponsesStreamRenderer({
        id: "resp_2",
        model: "public-model",
        created: 30,
    })(chatStreamEvents);
    assert(
        responseSse.some((frame) =>
            frame.startsWith("event: response.reasoning_summary_text.delta"),
        ),
        "Responses stream emits reasoning",
    );
    assert(
        responseSse.some((frame) => frame.includes('"thought_signature":"sig-stream-reasoning"')),
        "Responses stream preserves reasoning signatures in its final output",
    );
    assert(
        responseSse.at(-1)?.startsWith("event: response.completed") === true,
        "Responses stream completes",
    );

    const renderedResponsesRequest = renderResponsesRequest(parsedChatRequest, "wire", "high");
    assertEqual(renderedResponsesRequest.model, "wire", "IR renders a Responses request");
    assertEqual(
        renderedResponsesRequest.reasoning,
        {
            effort: "high",
        },
        "Responses renderer applies effective effort",
    );
});
