import { TranslationError } from "./errors";
import type {
    FinishReason,
    ImagePart,
    IrEvent,
    IrFunctionTool,
    IrMessage,
    IrRequest,
    IrResponseFormat,
    IrSampling,
    IrUsage,
    MessagePart,
    ToolCallPart,
    ToolOutputPart,
} from "./ir";
import {
    expectArray,
    expectJsonObject,
    expectNonEmptyString,
    expectRecord,
    expectString,
    isJsonValue,
    type JsonObject,
    type JsonValue,
    optionalBoolean,
    optionalFiniteNumber,
    optionalNonNegativeInteger,
    optionalString,
} from "./json";
import {
    parseResponseFormat,
    parseToolChoice,
    parseUsage,
    renderUsage,
    type UsageFieldNames,
    validateToolCalling,
    type WireUsage,
} from "./openai-shared";
import { encodeSse } from "./sse";

interface ChatCompletionsRequestObject {
    model: string;
    messages: ChatRequestMessage[];
    stream: boolean;
    tools?: ChatFunctionTool[];
    tool_choice?:
        | "auto"
        | "none"
        | "required"
        | {
              type: "function";
              function: {
                  name: string;
              };
          };
    temperature?: number;
    top_p?: number;
    max_completion_tokens?: number;
    stop?: string[];
    response_format?: ChatResponseFormat;
    reasoning_effort?: string;
}

interface ChatRequestMessage {
    role: "system" | "user" | "assistant" | "tool";
    content: string | null | ChatContentPart[];
    reasoning_content?: string;
    reasoning_thought_signature?: string;
    tool_calls?: ChatToolCall[];
    tool_call_id?: string;
    name?: string;
}

type ChatContentPart =
    | {
          type: "text";
          text: string;
          thought_signature?: string;
      }
    | {
          type: "image_url";
          image_url: {
              url: string;
              detail?: "auto" | "low" | "high";
          };
      };

interface ChatToolCall {
    id: string;
    type: "function";
    extra_content?: {
        google: {
            thought_signature: string;
        };
    };
    function: {
        name: string;
        arguments: string;
    };
}

interface ChatFunctionTool {
    type: "function";
    function: {
        name: string;
        description?: string;
        parameters: JsonObject;
        strict?: boolean;
    };
}

function parseGoogleThoughtSignature(
    value: Record<string, unknown>,
    context: string,
): string | undefined {
    if (value["extra_content"] !== undefined) {
        const extraContent = expectRecord(value["extra_content"], `${context}.extra_content`);
        const google = expectRecord(extraContent["google"], `${context}.extra_content.google`);
        return optionalString(
            google["thought_signature"],
            `${context}.extra_content.google.thought_signature`,
        );
    }
    // Accept JanusGate's earlier direct field during transition, but always render the documented
    // Google OpenAI-compatibility carrier.
    return optionalString(value["thought_signature"], `${context}.thought_signature`);
}

function renderGoogleThoughtSignature(
    thoughtSignature: string,
): NonNullable<ChatToolCall["extra_content"]> {
    return {
        google: {
            thought_signature: thoughtSignature,
        },
    };
}

function parseMessageReasoningThoughtSignature(
    message: Record<string, unknown>,
    context: string,
): string | undefined {
    // Rendering emits `reasoning_thought_signature` as the message-level carrier, but the
    // stream emits bare `thought_signature` on deltas, so a client that stored the stream
    // field on its history message and replays it carries only the bare key. Prefer the
    // documented carrier and fall back to the bare stream key.
    return (
        optionalString(
            message["reasoning_thought_signature"],
            `${context}.reasoning_thought_signature`,
        ) ?? optionalString(message["thought_signature"], `${context}.thought_signature`)
    );
}

type ChatResponseFormat =
    | {
          type: "json_object";
      }
    | {
          type: "json_schema";
          json_schema: {
              name: string;
              schema: JsonObject;
              description?: string;
              strict?: boolean;
          };
      };

const usageFieldNames = {
    inputTokens: "prompt_tokens",
    outputTokens: "completion_tokens",
    detailsContextPrefix: "usage.",
} as const satisfies UsageFieldNames;

interface ChatCompletionsResponseObject {
    id: string;
    object: "chat.completion";
    created: number;
    model: string;
    choices: {
        index: 0;
        message: {
            role: "assistant";
            content:
                | string
                | null
                | {
                      type: "text";
                      text: string;
                      thought_signature?: string;
                  }[];
            reasoning_content?: string;
            reasoning_thought_signature?: string;
            tool_calls?: ChatToolCall[];
        };
        finish_reason: FinishReason;
    }[];
    usage?: WireUsage<typeof usageFieldNames>;
}

interface ChatResponseMetadata {
    id: string;
    model: string;
    created: number;
}

export interface ChatStreamMetadata extends ChatResponseMetadata {
    isUsageIncluded: boolean;
}

export function parseChatCompletionsRequest(value: unknown): IrRequest {
    const root = expectRecord(value, "Chat Completions request");
    const messages = expectArray(root["messages"], "messages").map(parseMessage);
    const tools =
        root["tools"] === undefined ? [] : expectArray(root["tools"], "tools").map(parseTool);
    const sampling = parseSampling(root);
    const toolChoice = parseToolChoice(root["tool_choice"], (choice) =>
        expectNonEmptyString(
            expectRecord(choice["function"], "tool_choice.function")["name"],
            "tool_choice.function.name",
        ),
    );
    validateToolCalling(tools, toolChoice, root["parallel_tool_calls"]);
    const isStreaming = optionalBoolean(root["stream"], "stream") ?? false;

    const result: IrRequest = {
        messages,
        tools,
        sampling,
        isStreaming,
    };
    if (toolChoice !== undefined) result.toolChoice = toolChoice;
    return result;
}

function parseMessage(value: unknown, index: number): IrMessage {
    const message = expectRecord(value, `messages[${index}]`);
    const role = expectString(message["role"], `messages[${index}].role`);
    if (
        role !== "developer" &&
        role !== "system" &&
        role !== "user" &&
        role !== "assistant" &&
        role !== "tool"
    ) {
        throw new TranslationError("invalid_json", `messages[${index}].role is invalid`);
    }

    if (role === "tool") {
        const output: JsonValue = isJsonValue(message["content"])
            ? message["content"]
            : expectString(message["content"], `messages[${index}].content`);
        const part: ToolOutputPart = {
            type: "tool_output",
            callId: expectNonEmptyString(
                message["tool_call_id"],
                `messages[${index}].tool_call_id`,
            ),
            output,
        };
        const name = optionalString(message["name"], `messages[${index}].name`);
        if (name !== undefined) {
            part["name"] = name;
        }
        return {
            role,
            parts: [
                part,
            ],
        };
    }

    const parts = parseMessageContent(message["content"], index);
    if (role === "assistant" && message["tool_calls"] !== undefined) {
        for (const [toolIndex, toolValue] of expectArray(
            message["tool_calls"],
            `messages[${index}].tool_calls`,
        ).entries()) {
            const toolCall = expectRecord(toolValue, `messages[${index}].tool_calls[${toolIndex}]`);
            if (toolCall["type"] !== "function") {
                throw new TranslationError(
                    "unsupported_tool_type",
                    "only function tools are supported",
                );
            }
            const definition = expectRecord(
                toolCall["function"],
                `messages[${index}].tool_calls[${toolIndex}].function`,
            );
            const part: ToolCallPart = {
                type: "tool_call",
                id: expectNonEmptyString(
                    toolCall["id"],
                    `messages[${index}].tool_calls[${toolIndex}].id`,
                ),
                name: expectNonEmptyString(
                    definition["name"],
                    `messages[${index}].tool_calls[${toolIndex}].function.name`,
                ),
                arguments: expectString(
                    definition["arguments"],
                    `messages[${index}].tool_calls[${toolIndex}].function.arguments`,
                ),
            };
            const thoughtSignature = parseGoogleThoughtSignature(
                toolCall,
                `messages[${index}].tool_calls[${toolIndex}]`,
            );
            if (thoughtSignature !== undefined) {
                part["thoughtSignature"] = thoughtSignature;
            }
            parts.push(part);
        }
    }

    const result: IrMessage = {
        role: role === "developer" ? "system" : role,
        parts,
    };
    const reasoning = optionalString(
        message["reasoning_content"],
        `messages[${index}].reasoning_content`,
    );
    if (reasoning !== undefined) {
        result["reasoning"] = reasoning;
    }
    const reasoningThoughtSignature = parseMessageReasoningThoughtSignature(
        message,
        `messages[${index}]`,
    );
    if (reasoningThoughtSignature !== undefined) {
        result.reasoningThoughtSignature = reasoningThoughtSignature;
    }
    return result;
}

function parseMessageContent(value: unknown, messageIndex: number): MessagePart[] {
    if (value === null || value === undefined) {
        return [];
    }
    if (typeof value === "string") {
        return [
            {
                type: "text",
                text: value,
            },
        ];
    }
    return expectArray(value, `messages[${messageIndex}].content`).map((partValue, partIndex) => {
        const part = expectRecord(partValue, `messages[${messageIndex}].content[${partIndex}]`);
        if (part["type"] === "text") {
            const textPart: Extract<
                MessagePart,
                {
                    type: "text";
                }
            > = {
                type: "text",
                text: expectString(part["text"], "text content"),
            };
            const thoughtSignature = optionalString(
                part["thought_signature"],
                "text content thought_signature",
            );
            if (thoughtSignature !== undefined) textPart.thoughtSignature = thoughtSignature;
            return textPart;
        }
        if (part["type"] === "image_url") {
            const image = expectRecord(part["image_url"], "image_url");
            const detail = optionalString(image["detail"], "image_url.detail");
            if (
                detail !== undefined &&
                detail !== "auto" &&
                detail !== "low" &&
                detail !== "high"
            ) {
                throw new TranslationError("invalid_json", "image_url.detail is invalid");
            }
            const result: ImagePart = {
                type: "image",
                url: expectNonEmptyString(image["url"], "image_url.url"),
            };
            if (detail !== undefined) {
                result["detail"] = detail;
            }
            return result;
        }
        throw new TranslationError(
            "unsupported_parameter",
            "unsupported Chat Completions content part",
        );
    });
}

function parseTool(value: unknown, index: number): IrFunctionTool {
    const tool = expectRecord(value, `tools[${index}]`);
    if (tool["type"] !== "function") {
        throw new TranslationError("unsupported_tool_type", "only function tools are supported");
    }
    const definition = expectRecord(tool["function"], `tools[${index}].function`);
    const result: IrFunctionTool = {
        name: expectNonEmptyString(definition["name"], `tools[${index}].function.name`),
        parameters:
            definition["parameters"] === undefined
                ? {}
                : expectJsonObject(definition["parameters"], `tools[${index}].function.parameters`),
    };
    const description = optionalString(
        definition["description"],
        `tools[${index}].function.description`,
    );
    const isStrict = optionalBoolean(definition["strict"], `tools[${index}].function.strict`);
    if (description !== undefined) {
        result["description"] = description;
    }
    if (isStrict !== undefined) {
        result.isStrict = isStrict;
    }
    return result;
}

function parseSampling(root: Record<string, unknown>): IrSampling {
    const sampling: IrSampling = {};
    const temperature = optionalFiniteNumber(root["temperature"], "temperature");
    const topP = optionalFiniteNumber(root["top_p"], "top_p");
    const maxOutputTokens = optionalNonNegativeInteger(
        root["max_completion_tokens"] ?? root["max_tokens"],
        "max_completion_tokens",
    );
    if (temperature !== undefined) sampling["temperature"] = temperature;
    if (topP !== undefined) sampling.topP = topP;
    if (maxOutputTokens !== undefined) sampling.maxOutputTokens = maxOutputTokens;
    if (root["stop"] !== undefined && root["stop"] !== null) {
        sampling["stop"] =
            typeof root["stop"] === "string"
                ? [
                      root["stop"],
                  ]
                : expectArray(root["stop"], "stop").map((entry) => expectString(entry, "stop[]"));
    }
    if (root["response_format"] !== undefined) {
        sampling.responseFormat = parseResponseFormat(
            root["response_format"],
            "response_format",
            (format) => ({
                record: expectRecord(format["json_schema"], "response_format.json_schema"),
                context: "response_format.json_schema",
            }),
        );
    }
    return sampling;
}

export function renderChatCompletionsRequest(
    request: IrRequest,
    model: string,
    reasoningEffort?: string,
): ChatCompletionsRequestObject {
    const messages: ChatRequestMessage[] = [];
    for (const message of request.messages) {
        const rendered = renderMessage(message);
        messages.push(...rendered);
    }
    const result: ChatCompletionsRequestObject = {
        model,
        messages,
        stream: request.isStreaming,
    };
    if (request.tools.length > 0) {
        result["tools"] = request.tools.map(renderTool);
    }
    if (request.toolChoice !== undefined) {
        result.tool_choice =
            request.toolChoice.functionName === undefined
                ? request.toolChoice.mode
                : {
                      type: "function",
                      function: {
                          name: request.toolChoice.functionName,
                      },
                  };
    }
    const sampling = request.sampling;
    if (sampling.temperature !== undefined) result["temperature"] = sampling.temperature;
    if (sampling.topP !== undefined) result["top_p"] = sampling.topP;
    if (sampling.maxOutputTokens !== undefined)
        result["max_completion_tokens"] = sampling.maxOutputTokens;
    if (sampling.stop !== undefined) result["stop"] = sampling.stop;
    if (sampling.responseFormat !== undefined)
        result["response_format"] = renderResponseFormat(sampling.responseFormat);
    if (reasoningEffort !== undefined) result["reasoning_effort"] = reasoningEffort;
    return result;
}

function renderMessage(message: IrMessage): ChatRequestMessage[] {
    if (message.role === "tool") {
        return message.parts.flatMap((part) => {
            if (part.type !== "tool_output") return [];
            const rendered: ChatRequestMessage = {
                role: "tool",
                content:
                    typeof part.output === "string" ? part.output : JSON.stringify(part.output),
                tool_call_id: part.callId,
            };
            if (part.name !== undefined) rendered.name = part.name;
            return [
                rendered,
            ];
        });
    }

    const textParts = message.parts.filter((part) => part.type === "text" || part.type === "image");
    const content = renderContent(textParts);
    const result: ChatRequestMessage = {
        role: message.role,
        content,
    };
    if (message.reasoning !== undefined) result["reasoning_content"] = message.reasoning;
    if (message.reasoningThoughtSignature !== undefined) {
        result.reasoning_thought_signature = message.reasoningThoughtSignature;
    }
    const toolCalls = message.parts.filter((part) => part.type === "tool_call");
    if (toolCalls.length > 0) {
        result["tool_calls"] = toolCalls.map((part) => {
            const toolCall: ChatToolCall = {
                id: part.id,
                type: "function",
                function: {
                    name: part.name,
                    arguments: part.arguments,
                },
            };
            if (part.thoughtSignature !== undefined) {
                toolCall.extra_content = renderGoogleThoughtSignature(part.thoughtSignature);
            }
            return toolCall;
        });
    }
    return [
        result,
    ];
}

function renderContent(
    parts: (MessagePart &
        (
            | {
                  type: "text";
              }
            | {
                  type: "image";
              }
        ))[],
): string | null | ChatContentPart[] {
    if (parts.length === 0) return null;
    if (parts.every((part) => part.type === "text")) {
        return parts.map((part) => (part.type === "text" ? part.text : "")).join("");
    }
    return parts.map((part) => {
        if (part.type === "text") {
            const textPart: Extract<
                ChatContentPart,
                {
                    type: "text";
                }
            > = {
                type: "text",
                text: part.text,
            };
            if (part.thoughtSignature !== undefined) {
                textPart.thought_signature = part.thoughtSignature;
            }
            return textPart;
        }
        const imageUrl: {
            url: string;
            detail?: "auto" | "low" | "high";
        } = {
            url: part.url,
        };
        if (part.detail !== undefined) imageUrl.detail = part.detail;
        return {
            type: "image_url",
            image_url: imageUrl,
        };
    });
}

function renderTool(tool: IrFunctionTool): ChatFunctionTool {
    const definition: ChatFunctionTool["function"] = {
        name: tool.name,
        parameters: tool.parameters,
    };
    if (tool.description !== undefined) definition.description = tool.description;
    if (tool.isStrict !== undefined) definition.strict = tool.isStrict;
    return {
        type: "function",
        function: definition,
    };
}

function renderResponseFormat(format: IrResponseFormat): ChatResponseFormat {
    if (format.type === "json_object")
        return {
            type: "json_object",
        };
    const jsonSchema: ChatResponseFormat & {
        type: "json_schema";
    } = {
        type: "json_schema",
        json_schema: {
            name: format.name,
            schema: format.schema,
        },
    };
    if (format.description !== undefined)
        jsonSchema.json_schema["description"] = format.description;
    if (format.isStrict !== undefined) jsonSchema.json_schema["strict"] = format.isStrict;
    return jsonSchema;
}

export function parseChatCompletionsResponse(value: unknown): IrEvent[] {
    const root = expectRecord(value, "Chat Completions response");
    const choices = expectArray(root["choices"], "choices");
    if (choices.length !== 1) {
        throw new TranslationError(
            "upstream_failure",
            "translated Chat Completions response must have one choice",
        );
    }
    const choice = expectRecord(choices[0], "choices[0]");
    const message = expectRecord(choice["message"], "choices[0].message");
    const events: IrEvent[] = [
        {
            type: "message_start",
        },
    ];
    const reasoning = optionalString(message["reasoning_content"], "message.reasoning_content");
    const reasoningThoughtSignature = parseMessageReasoningThoughtSignature(message, "message");
    if (reasoning !== undefined && reasoning.length > 0)
        events.push({
            type: "reasoning_delta",
            delta: reasoning,
            ...(reasoningThoughtSignature === undefined
                ? {}
                : {
                      thoughtSignature: reasoningThoughtSignature,
                  }),
        });
    const content = message["content"];
    if (typeof content === "string" && content.length > 0)
        events.push({
            type: "text_delta",
            delta: content,
        });
    appendChatResponseToolCalls(events, message["tool_calls"]);
    events.push({
        type: "finish",
        reason: mapChatFinishReason(choice["finish_reason"]),
    });
    if (root["usage"] !== undefined)
        events.push({
            type: "usage",
            usage: parseUsage(root["usage"], usageFieldNames),
        });
    events.push({
        type: "done",
    });
    return events;
}

function appendChatResponseToolCalls(events: IrEvent[], value: unknown): void {
    if (value === undefined) return;
    for (const [index, toolValue] of expectArray(value, "message.tool_calls").entries()) {
        const tool = expectRecord(toolValue, `message.tool_calls[${index}]`);
        const definition = expectRecord(tool["function"], `message.tool_calls[${index}].function`);
        const start: IrEvent = {
            type: "tool_call_start",
            index,
            id: expectNonEmptyString(tool["id"], "tool call id"),
            name: expectNonEmptyString(definition["name"], "tool function name"),
        };
        const thoughtSignature = parseGoogleThoughtSignature(tool, `message.tool_calls[${index}]`);
        if (thoughtSignature !== undefined) {
            start.thoughtSignature = thoughtSignature;
        }
        events.push(start);
        const argumentsDelta = expectString(definition["arguments"], "tool function arguments");
        if (argumentsDelta.length > 0)
            events.push({
                type: "tool_call_delta",
                index,
                delta: argumentsDelta,
            });
        events.push({
            type: "tool_call_done",
            index,
        });
    }
}

export function parseChatCompletionsStreamEvent(value: unknown): IrEvent[] {
    // The "[DONE]" wire sentinel never reaches this parser: the SSE framing layer that decodes
    // frames owns the terminal rule.
    const root = expectRecord(value, "Chat Completions stream chunk");
    const events: IrEvent[] = [];
    if (root["usage"] !== undefined)
        events.push({
            type: "usage",
            usage: parseUsage(root["usage"], usageFieldNames),
        });
    const choices = root["choices"] === undefined ? [] : expectArray(root["choices"], "choices");
    for (const choiceValue of choices) {
        const choice = expectRecord(choiceValue, "choice");
        const delta = expectRecord(choice["delta"], "choice.delta");
        if (delta["role"] === "assistant")
            events.push({
                type: "message_start",
            });
        const reasoning = optionalString(delta["reasoning_content"], "delta.reasoning_content");
        const thoughtSignature = optionalString(
            delta["thought_signature"],
            "delta.thought_signature",
        );
        if (reasoning !== undefined && reasoning.length > 0)
            events.push({
                type: "reasoning_delta",
                delta: reasoning,
                ...(thoughtSignature === undefined
                    ? {}
                    : {
                          thoughtSignature,
                      }),
            });
        const text = optionalString(delta["content"], "delta.content");
        if (text !== undefined && text.length > 0)
            events.push({
                type: "text_delta",
                delta: text,
            });
        if (delta["tool_calls"] !== undefined) {
            for (const toolValue of expectArray(delta["tool_calls"], "delta.tool_calls")) {
                const tool = expectRecord(toolValue, "delta.tool_calls[]");
                const index = optionalNonNegativeInteger(tool["index"], "tool index") ?? 0;
                const definition =
                    tool["function"] === undefined
                        ? {}
                        : expectRecord(tool["function"], "tool function");
                const id = optionalString(tool["id"], "tool id");
                const name = optionalString(definition["name"], "tool name");
                if (id !== undefined || name !== undefined) {
                    const start: IrEvent = {
                        type: "tool_call_start",
                        index,
                        id: id ?? `call_${index + 1}`,
                        name: name ?? "",
                    };
                    const thoughtSignature = parseGoogleThoughtSignature(
                        tool,
                        "delta.tool_calls[]",
                    );
                    if (thoughtSignature !== undefined) {
                        start.thoughtSignature = thoughtSignature;
                    }
                    events.push(start);
                }
                const argumentsDelta = optionalString(definition["arguments"], "tool arguments");
                if (argumentsDelta !== undefined && argumentsDelta.length > 0)
                    events.push({
                        type: "tool_call_delta",
                        index,
                        delta: argumentsDelta,
                    });
            }
        }
        if (choice["finish_reason"] !== null && choice["finish_reason"] !== undefined) {
            events.push({
                type: "finish",
                reason: mapChatFinishReason(choice["finish_reason"]),
            });
        }
    }
    return events;
}

function mapChatFinishReason(value: unknown): FinishReason {
    if (value === "length") return "length";
    if (value === "tool_calls" || value === "function_call") return "tool_calls";
    if (value === "content_filter") return "content_filter";
    return "stop";
}

export function renderChatCompletionsResponse(
    events: readonly IrEvent[],
    metadata: ChatResponseMetadata,
): ChatCompletionsResponseObject {
    let content = "";
    let textThoughtSignature: string | undefined;
    let reasoning = "";
    let reasoningThoughtSignature: string | undefined;
    let finishReason: FinishReason = "stop";
    let usage: IrUsage | undefined;
    const toolCalls = new Map<number, ChatToolCall>();
    for (const event of events) {
        if (event.type === "text_delta") {
            content += event.delta;
            // Last-wins: a thought signature covers only the content issued up to it, so the
            // latest signature is the one that matches the fully coalesced text.
            if (event.thoughtSignature !== undefined) {
                textThoughtSignature = event.thoughtSignature;
            }
        } else if (event.type === "reasoning_delta") {
            reasoning += event.delta;
            // Last-wins: a thought signature covers only the content issued up to it, so the
            // latest signature is the one that matches the fully coalesced reasoning text.
            if (event.thoughtSignature !== undefined) {
                reasoningThoughtSignature = event.thoughtSignature;
            }
        } else if (event.type === "tool_call_start") {
            const toolCall: ChatToolCall = {
                id: event.id,
                type: "function",
                function: {
                    name: event.name,
                    arguments: "",
                },
            };
            if (event.thoughtSignature !== undefined) {
                toolCall.extra_content = renderGoogleThoughtSignature(event.thoughtSignature);
            }
            toolCalls.set(event.index, toolCall);
        } else if (event.type === "tool_call_delta") {
            const tool = toolCalls.get(event.index);
            if (tool !== undefined) tool.function["arguments"] += event.delta;
        } else if (event.type === "finish") finishReason = event.reason;
        else if (event.type === "usage") usage = event.usage;
    }
    if (toolCalls.size > 0 && !events.some((event) => event.type === "finish"))
        finishReason = "tool_calls";
    const message: ChatCompletionsResponseObject["choices"][number]["message"] = {
        role: "assistant",
        content: content.length > 0 ? content : null,
    };
    if (textThoughtSignature !== undefined && content.length > 0) {
        // A bare string has nowhere to carry a signature, so emit the coalesced text the way
        // the request side accepts it: a single text content part with `thought_signature`.
        message.content = [
            {
                type: "text",
                text: content,
                thought_signature: textThoughtSignature,
            },
        ];
    }
    if (reasoning.length > 0) message.reasoning_content = reasoning;
    if (reasoningThoughtSignature !== undefined) {
        message.reasoning_thought_signature = reasoningThoughtSignature;
    }
    if (toolCalls.size > 0)
        message.tool_calls = [
            ...toolCalls.entries(),
        ]
            .sort(([left], [right]) => left - right)
            .map((entry) => entry[1]);
    const result: ChatCompletionsResponseObject = {
        id: metadata.id,
        object: "chat.completion",
        created: metadata.created,
        model: metadata.model,
        choices: [
            {
                index: 0,
                message,
                finish_reason: finishReason,
            },
        ],
    };
    if (usage !== undefined) result["usage"] = renderUsage(usage, usageFieldNames);
    return result;
}

export function renderChatCompletionsStream(
    events: readonly IrEvent[],
    metadata: ChatStreamMetadata,
): string[] {
    const frames: string[] = [];
    let usage: IrUsage | undefined;
    for (const event of events) {
        if (event.type === "usage") {
            usage = event.usage;
            continue;
        }
        if (event.type === "done") {
            if (metadata.isUsageIncluded && usage !== undefined)
                frames.push(
                    encodeSse(
                        undefined,
                        chatChunk(metadata, [], renderUsage(usage, usageFieldNames)),
                    ),
                );
            frames.push(encodeSse(undefined, "[DONE]"));
            continue;
        }
        const delta: Record<string, JsonValue> = {};
        let finishReason: FinishReason | null = null;
        if (event.type === "message_start") delta["role"] = "assistant";
        else if (event.type === "text_delta") {
            delta["content"] = event.delta;
            if (event.thoughtSignature !== undefined) {
                delta["thought_signature"] = event.thoughtSignature;
            }
        } else if (event.type === "reasoning_delta") {
            delta["reasoning_content"] = event.delta;
            if (event.thoughtSignature !== undefined) {
                delta["thought_signature"] = event.thoughtSignature;
            }
        } else if (event.type === "tool_call_start") {
            const toolCall: JsonObject = {
                index: event.index,
                id: event.id,
                type: "function",
                function: {
                    name: event.name,
                    arguments: "",
                },
            };
            if (event.thoughtSignature !== undefined) {
                toolCall["extra_content"] = renderGoogleThoughtSignature(event.thoughtSignature);
            }
            delta["tool_calls"] = [
                toolCall,
            ];
        } else if (event.type === "tool_call_delta")
            delta["tool_calls"] = [
                {
                    index: event.index,
                    function: {
                        arguments: event.delta,
                    },
                },
            ];
        else if (event.type === "finish") finishReason = event.reason;
        else continue;
        frames.push(
            encodeSse(
                undefined,
                chatChunk(metadata, [
                    {
                        index: 0,
                        delta,
                        finish_reason: finishReason,
                    },
                ]),
            ),
        );
    }
    return frames;
}

function chatChunk(
    metadata: ChatResponseMetadata,
    choices: JsonValue[],
    usage?: JsonValue,
): JsonObject {
    const chunk: JsonObject = {
        id: metadata.id,
        object: "chat.completion.chunk",
        created: metadata.created,
        model: metadata.model,
        choices,
    };
    if (usage !== undefined) chunk["usage"] = usage;
    return chunk;
}

type ChatCompletionsStreamRenderer = (events: readonly IrEvent[]) => string[];

export function createChatCompletionsStreamRenderer(
    metadata: ChatStreamMetadata,
): ChatCompletionsStreamRenderer {
    // A Chat Completions stream carries usage at most once, in the chunk adjacent to the
    // terminal frame, yet upstreams deliver the usage chunk at any earlier position
    // (typically the final empty-choices chunk). Usage is therefore buffered across calls
    // and injected before `done`, and repeated `message_start` events (upstreams may
    // re-send the assistant role on continuation chunks) collapse to the first one.
    let hasStarted = false;
    let usage: IrUsage | undefined;
    // The IR tool index is the upstream output position (a Responses `output_index` counts
    // reasoning and message items too), while Chat Completions `delta.tool_calls[].index`
    // must densely number tool calls alone: clients assemble the array by index, so sparse
    // indexes would leave holes. Each stream therefore maps IR indexes to dense indexes in
    // first-seen order, persistently across calls because argument deltas arrive in later
    // upstream frames than their start.
    const denseToolIndexes = new Map<number, number>();
    let nextDenseToolIndex = 0;
    const denseToolIndex = (index: number): number => {
        const mapped = denseToolIndexes.get(index);
        if (mapped !== undefined) return mapped;
        denseToolIndexes.set(index, nextDenseToolIndex);
        nextDenseToolIndex += 1;
        return nextDenseToolIndex - 1;
    };
    return (events): string[] => {
        const immediate: IrEvent[] = [];
        for (const event of events) {
            if (event.type === "message_start") {
                if (!hasStarted) {
                    hasStarted = true;
                    immediate.push(event);
                }
            } else if (event.type === "usage") usage = event.usage;
            else if (event.type === "done") {
                if (usage !== undefined)
                    immediate.push({
                        type: "usage",
                        usage,
                    });
                immediate.push(event);
            } else if (event.type === "tool_call_start" || event.type === "tool_call_delta")
                immediate.push({
                    ...event,
                    index: denseToolIndex(event.index),
                });
            else immediate.push(event);
        }
        return renderChatCompletionsStream(immediate, metadata);
    };
}
