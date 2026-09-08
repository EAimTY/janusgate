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

interface ResponsesRequestObject {
    model: string;
    input: (
        | ResponsesInputMessage
        | ResponsesFunctionCall
        | ResponsesFunctionOutput
        | ResponsesReasoningItem
    )[];
    stream: boolean;
    instructions?: string;
    tools?: ResponsesFunctionTool[];
    tool_choice?:
        | "auto"
        | "none"
        | "required"
        | {
              type: "function";
              name: string;
          };
    temperature?: number;
    top_p?: number;
    max_output_tokens?: number;
    text?: {
        format: ResponsesTextFormat;
    };
    reasoning?: {
        effort: string;
    };
}

interface ResponsesInputMessage {
    type: "message";
    // The upstream `input` array accepts `system` (and `developer`) message items at any
    // position, so non-leading system messages render in position instead of being
    // hoisted into `instructions`.
    role: "system" | "user" | "assistant";
    content: ResponsesContentPart[];
}

type ResponsesContentPart =
    | {
          type: "input_text" | "output_text";
          text: string;
          thought_signature?: string;
      }
    | {
          type: "input_image";
          image_url: string;
          detail?: "auto" | "low" | "high";
      };

interface ResponsesFunctionCall {
    type: "function_call";
    call_id: string;
    name: string;
    arguments: string;
    thought_signature?: string;
}

interface ResponsesFunctionOutput {
    type: "function_call_output";
    call_id: string;
    output: JsonValue;
    thought_signature?: string;
}

// Input-side reasoning item. The official Responses schema accepts `type:"reasoning"`
// items in `input` (the `Item` union includes `ReasoningItem`); only the summary carrier
// is rendered here because the IR retains no more than the joined summary text and a
// thought signature.
interface ResponsesReasoningItem {
    type: "reasoning";
    summary: {
        type: "summary_text";
        text: string;
    }[];
    thought_signature?: string;
}

interface ResponsesFunctionTool {
    type: "function";
    name: string;
    parameters: JsonObject;
    description?: string;
    strict?: boolean;
}

type ResponsesTextFormat =
    | {
          type: "json_object";
      }
    | {
          type: "json_schema";
          name: string;
          schema: JsonObject;
          description?: string;
          strict?: boolean;
      };

const usageFieldNames = {
    inputTokens: "input_tokens",
    outputTokens: "output_tokens",
    detailsContextPrefix: "",
} as const satisfies UsageFieldNames;

interface ResponsesResponseObject {
    id: string;
    object: "response";
    created_at: number;
    model: string;
    status: "completed" | "incomplete";
    output: JsonObject[];
    usage?: WireUsage<typeof usageFieldNames>;
    incomplete_details?: {
        reason: "max_output_tokens" | "content_filter";
    };
}

interface ResponsesMetadata {
    id: string;
    model: string;
    created: number;
}

export function parseResponsesRequest(value: unknown): IrRequest {
    const root = expectRecord(value, "Responses request");
    if (root["store"] === true || root["previous_response_id"] !== undefined) {
        throw new TranslationError(
            "unsupported_parameter",
            "store:true and previous_response_id require native Responses passthrough",
        );
    }
    const messages: IrMessage[] = [];
    const instructions = optionalString(root["instructions"], "instructions");
    if (instructions !== undefined)
        messages.push({
            role: "system",
            parts: [
                {
                    type: "text",
                    text: instructions,
                },
            ],
        });
    messages.push(...parseResponsesInput(root["input"]));
    const tools =
        root["tools"] === undefined
            ? []
            : expectArray(root["tools"], "tools").map(parseResponsesTool);
    const sampling = parseResponsesSampling(root);
    const toolChoice = parseToolChoice(root["tool_choice"], (choice) =>
        expectNonEmptyString(choice["name"], "tool_choice.name"),
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

function parseResponsesInput(value: unknown): IrMessage[] {
    if (typeof value === "string")
        return [
            {
                role: "user",
                parts: [
                    {
                        type: "text",
                        text: value,
                    },
                ],
            },
        ];
    const input = expectArray(value, "input");
    return input.map((itemValue, index) => {
        const item = expectRecord(itemValue, `input[${index}]`);
        if (item["type"] === "function_call") {
            const part: ToolCallPart = {
                type: "tool_call",
                id: expectNonEmptyString(item["call_id"] ?? item["id"], `input[${index}].call_id`),
                name: expectNonEmptyString(item["name"], `input[${index}].name`),
                arguments: expectString(item["arguments"], `input[${index}].arguments`),
            };
            const thoughtSignature = optionalString(
                item["thought_signature"],
                `input[${index}].thought_signature`,
            );
            if (thoughtSignature !== undefined) part.thoughtSignature = thoughtSignature;
            return {
                role: "assistant",
                parts: [
                    part,
                ],
            };
        }
        if (item["type"] === "reasoning") {
            const summary =
                item["summary"] === undefined
                    ? []
                    : expectArray(item["summary"], `input[${index}].summary`);
            const reasoning = summary
                .map((summaryValue, summaryIndex) => {
                    const summaryPart = expectRecord(
                        summaryValue,
                        `input[${index}].summary[${summaryIndex}]`,
                    );
                    return expectString(
                        summaryPart["text"],
                        `input[${index}].summary[${summaryIndex}].text`,
                    );
                })
                .join("");
            const thoughtSignature = optionalString(
                item["thought_signature"],
                `input[${index}].thought_signature`,
            );
            return {
                role: "assistant",
                parts: [],
                ...(reasoning.length === 0
                    ? {}
                    : {
                          reasoning,
                      }),
                ...(thoughtSignature === undefined
                    ? {}
                    : {
                          reasoningThoughtSignature: thoughtSignature,
                      }),
            };
        }
        if (item["type"] === "function_call_output") {
            const output = item["output"];
            if (!isJsonValue(output))
                throw new TranslationError("invalid_json", `input[${index}].output must be JSON`);
            const part: ToolOutputPart = {
                type: "tool_output",
                callId: expectNonEmptyString(item["call_id"], `input[${index}].call_id`),
                output,
            };
            const thoughtSignature = optionalString(
                item["thought_signature"],
                `input[${index}].thought_signature`,
            );
            if (thoughtSignature !== undefined) part.thoughtSignature = thoughtSignature;
            return {
                role: "tool",
                parts: [
                    part,
                ],
            };
        }
        const role = expectString(item["role"], `input[${index}].role`);
        if (role !== "developer" && role !== "system" && role !== "user" && role !== "assistant") {
            throw new TranslationError("invalid_json", `input[${index}].role is invalid`);
        }
        const content = item["content"];
        const parts =
            typeof content === "string"
                ? [
                      {
                          type: "text" as const,
                          text: content,
                      },
                  ]
                : expectArray(content, `input[${index}].content`).map((part, partIndex) =>
                      parseResponsesContentPart(part, index, partIndex),
                  );
        return {
            // `developer` and `system` both normalize to the IR's system role, and their
            // position is preserved end to end: the Responses request renderer emits
            // non-leading system messages as in-position `role:"system"` input items and
            // lifts only the leading all-text system message into `instructions`. (Gemini
            // is the lossy one: its single `systemInstruction` channel forces every system
            // message to the front.)
            role: role === "developer" ? "system" : role,
            parts,
        };
    });
}

function parseResponsesContentPart(
    value: unknown,
    messageIndex: number,
    partIndex: number,
): MessagePart {
    const part = expectRecord(value, `input[${messageIndex}].content[${partIndex}]`);
    if (
        part["type"] === "input_text" ||
        part["type"] === "output_text" ||
        part["type"] === "text"
    ) {
        const textPart: Extract<
            MessagePart,
            {
                type: "text";
            }
        > = {
            type: "text",
            text: expectString(part["text"], "text"),
        };
        const thoughtSignature = optionalString(
            part["thought_signature"],
            "text thought_signature",
        );
        if (thoughtSignature !== undefined) textPart.thoughtSignature = thoughtSignature;
        return textPart;
    }
    if (part["type"] === "input_image") {
        const detail = optionalString(part["detail"], "input image detail");
        if (detail !== undefined && detail !== "auto" && detail !== "low" && detail !== "high") {
            throw new TranslationError("invalid_json", "input image detail is invalid");
        }
        const image: ImagePart = {
            type: "image",
            url: expectNonEmptyString(part["image_url"], "input image URL"),
        };
        if (detail !== undefined) image["detail"] = detail;
        return image;
    }
    throw new TranslationError("unsupported_parameter", "unsupported Responses content part");
}

function parseResponsesTool(value: unknown, index: number): IrFunctionTool {
    const tool = expectRecord(value, `tools[${index}]`);
    if (tool["type"] !== "function") {
        throw new TranslationError("unsupported_tool_type", "only function tools are supported");
    }
    const result: IrFunctionTool = {
        name: expectNonEmptyString(tool["name"], `tools[${index}].name`),
        parameters:
            tool["parameters"] === undefined
                ? {}
                : expectJsonObject(tool["parameters"], `tools[${index}].parameters`),
    };
    const description = optionalString(tool["description"], `tools[${index}].description`);
    const isStrict = optionalBoolean(tool["strict"], `tools[${index}].strict`);
    if (description !== undefined) result["description"] = description;
    if (isStrict !== undefined) result.isStrict = isStrict;
    return result;
}

function parseResponsesSampling(root: Record<string, unknown>): IrSampling {
    const sampling: IrSampling = {};
    const temperature = optionalFiniteNumber(root["temperature"], "temperature");
    const topP = optionalFiniteNumber(root["top_p"], "top_p");
    const maxOutputTokens = optionalNonNegativeInteger(
        root["max_output_tokens"],
        "max_output_tokens",
    );
    if (temperature !== undefined) sampling["temperature"] = temperature;
    if (topP !== undefined) sampling.topP = topP;
    if (maxOutputTokens !== undefined) sampling.maxOutputTokens = maxOutputTokens;
    if (root["text"] !== undefined) {
        const text = expectRecord(root["text"], "text");
        if (text["format"] !== undefined) {
            sampling.responseFormat = parseResponseFormat(
                text["format"],
                "text.format",
                (format) => ({
                    record: format,
                    context: "text.format",
                }),
            );
        }
    }
    return sampling;
}

export function renderResponsesRequest(
    request: IrRequest,
    model: string,
    reasoningEffort?: string,
): ResponsesRequestObject {
    // `instructions` on the Responses upstream is documented as "a system (or developer)
    // message inserted into the model's context", i.e. it always lands at the front. The
    // leading all-text system message therefore renders into `instructions` — the exact
    // inverse of the parser, which turns `instructions` into a leading system message.
    // Every other system message renders in position as a `role:"system"` input message
    // item, which the upstream `input` array accepts at any offset; hoisting a
    // mid-conversation developer correction to the front would silently reorder it. A
    // system message with non-text parts also stays in position because a string cannot
    // carry them. (Gemini is the exception: its single `systemInstruction` channel forces
    // every system message to the front.)
    let instructions: string | undefined;
    const input: ResponsesRequestObject["input"] = [];
    for (const [messageIndex, message] of request.messages.entries()) {
        const isLeadingSystem = messageIndex === 0 && message.role === "system";
        if (isLeadingSystem) {
            const textParts = message.parts.filter((part) => part.type === "text");
            // A string cannot carry non-text parts, so a leading system message that has
            // any renders in position like any other system message.
            const isAllText = textParts.length === message.parts.length;
            if (isAllText) {
                const text = textParts.map((part) => part.text).join("");
                if (text.length > 0) instructions = text;
                continue;
            }
        }
        // Reasoning retained by the parsers (Responses `reasoning` summaries, Chat
        // Completions `reasoning_content`) renders back as an in-position
        // `type:"reasoning"` input item. The official Responses schema accepts reasoning
        // items in `input` — the `ReasoningItem` description says to "be sure to include
        // these items in your `input` ... for subsequent turns" — with `summary` as the
        // required carrier and `encrypted_content` optional. The IR retains only the
        // joined summary text plus an optional thought signature (never item ids or
        // encrypted content), so exactly that re-renders here instead of vanishing.
        // Reasoning precedes the rest of the assistant turn, so it is emitted ahead of
        // the message's parts.
        if (message.reasoning !== undefined) {
            const reasoningItem: ResponsesReasoningItem = {
                type: "reasoning",
                summary: [
                    {
                        type: "summary_text",
                        text: message.reasoning,
                    },
                ],
            };
            if (message.reasoningThoughtSignature !== undefined) {
                reasoningItem.thought_signature = message.reasoningThoughtSignature;
            }
            input.push(reasoningItem);
        }
        for (const part of message.parts) {
            if (part.type === "tool_call") {
                const functionCall: ResponsesFunctionCall = {
                    type: "function_call",
                    call_id: part.id,
                    name: part.name,
                    arguments: part.arguments,
                };
                if (part.thoughtSignature !== undefined) {
                    functionCall.thought_signature = part.thoughtSignature;
                }
                input.push(functionCall);
            } else if (part.type === "tool_output") {
                const functionOutput: ResponsesFunctionOutput = {
                    type: "function_call_output",
                    call_id: part.callId,
                    output: part.output,
                };
                if (part.thoughtSignature !== undefined) {
                    functionOutput.thought_signature = part.thoughtSignature;
                }
                input.push(functionOutput);
            }
        }
        const contentParts = message.parts
            .filter((part) => part.type === "text" || part.type === "image")
            .map((part) => renderResponsesContentPart(part, message.role));
        if (
            contentParts.length > 0 &&
            (message.role === "system" || message.role === "user" || message.role === "assistant")
        ) {
            input.push({
                type: "message",
                role: message.role,
                content: contentParts,
            });
        }
    }
    const result: ResponsesRequestObject = {
        model,
        input,
        stream: request.isStreaming,
    };
    if (instructions !== undefined) result["instructions"] = instructions;
    if (request.tools.length > 0) result["tools"] = request.tools.map(renderResponsesTool);
    if (request.toolChoice !== undefined) {
        result.tool_choice =
            request.toolChoice.functionName === undefined
                ? request.toolChoice.mode
                : {
                      type: "function",
                      name: request.toolChoice.functionName,
                  };
    }
    const sampling = request.sampling;
    if (sampling.temperature !== undefined) result["temperature"] = sampling.temperature;
    if (sampling.topP !== undefined) result["top_p"] = sampling.topP;
    if (sampling.maxOutputTokens !== undefined)
        result["max_output_tokens"] = sampling.maxOutputTokens;
    if (sampling.responseFormat !== undefined)
        result["text"] = {
            format: renderResponsesTextFormat(sampling.responseFormat),
        };
    if (reasoningEffort !== undefined)
        result["reasoning"] = {
            effort: reasoningEffort,
        };
    return result;
}

function renderResponsesContentPart(
    part: MessagePart,
    role: "system" | "user" | "assistant" | "tool",
): ResponsesContentPart {
    if (part.type === "text") {
        const textPart: Extract<
            ResponsesContentPart,
            {
                type: "input_text" | "output_text";
            }
        > = {
            type: role === "assistant" ? "output_text" : "input_text",
            text: part.text,
        };
        if (part.thoughtSignature !== undefined) {
            textPart.thought_signature = part.thoughtSignature;
        }
        return textPart;
    }
    if (part.type === "image") {
        const image: Extract<
            ResponsesContentPart,
            {
                type: "input_image";
            }
        > = {
            type: "input_image",
            image_url: part.url,
        };
        if (part.detail !== undefined) image["detail"] = part.detail;
        return image;
    }
    throw new Error("non-content IR part reached Responses content renderer");
}

function renderResponsesTool(tool: IrFunctionTool): ResponsesFunctionTool {
    const result: ResponsesFunctionTool = {
        type: "function",
        name: tool.name,
        parameters: tool.parameters,
    };
    if (tool.description !== undefined) result["description"] = tool.description;
    if (tool.isStrict !== undefined) result["strict"] = tool.isStrict;
    return result;
}

function renderResponsesTextFormat(format: IrResponseFormat): ResponsesTextFormat {
    if (format.type === "json_object")
        return {
            type: "json_object",
        };
    const result: Extract<
        ResponsesTextFormat,
        {
            type: "json_schema";
        }
    > = {
        type: "json_schema",
        name: format.name,
        schema: format.schema,
    };
    if (format.description !== undefined) result["description"] = format.description;
    if (format.isStrict !== undefined) result["strict"] = format.isStrict;
    return result;
}

export function parseResponsesResponse(value: unknown): IrEvent[] {
    const root = expectRecord(value, "Responses response");
    // The upstream response id is deliberately dropped: outward ids are always
    // synthesized from renderer metadata, so the IR carries no message id.
    const events: IrEvent[] = [
        {
            type: "message_start",
        },
    ];
    let hasToolCall = false;
    for (const [outputIndex, itemValue] of expectArray(root["output"], "output").entries()) {
        const item = expectRecord(itemValue, `output[${outputIndex}]`);
        if (item["type"] === "message") {
            for (const contentValue of expectArray(
                item["content"],
                `output[${outputIndex}].content`,
            )) {
                const content = expectRecord(contentValue, "output content");
                if (
                    (content["type"] === "output_text" || content["type"] === "text") &&
                    typeof content["text"] === "string"
                ) {
                    const event: IrEvent = {
                        type: "text_delta",
                        delta: content["text"],
                    };
                    const thoughtSignature = optionalString(
                        content["thought_signature"],
                        "output text thought_signature",
                    );
                    if (thoughtSignature !== undefined) {
                        event.thoughtSignature = thoughtSignature;
                    }
                    events.push(event);
                }
            }
        } else if (item["type"] === "function_call") {
            hasToolCall = true;
            const start: IrEvent = {
                type: "tool_call_start",
                index: outputIndex,
                id: expectNonEmptyString(item["call_id"] ?? item["id"], "function call id"),
                name: expectNonEmptyString(item["name"], "function call name"),
            };
            const thoughtSignature = optionalString(
                item["thought_signature"],
                "function call thought_signature",
            );
            if (thoughtSignature !== undefined) {
                start.thoughtSignature = thoughtSignature;
            }
            events.push(start);
            const argumentsDelta = expectString(item["arguments"], "function call arguments");
            if (argumentsDelta.length > 0)
                events.push({
                    type: "tool_call_delta",
                    index: outputIndex,
                    delta: argumentsDelta,
                });
            events.push({
                type: "tool_call_done",
                index: outputIndex,
            });
        } else if (item["type"] === "reasoning") {
            const summaries =
                item["summary"] === undefined
                    ? []
                    : expectArray(item["summary"], "reasoning.summary");
            for (const summaryValue of summaries) {
                const summary = expectRecord(summaryValue, "reasoning summary");
                const text = optionalString(summary["text"], "reasoning summary text");
                if (text !== undefined) {
                    const event: IrEvent = {
                        type: "reasoning_delta",
                        delta: text,
                    };
                    const thoughtSignature = optionalString(
                        summary["thought_signature"] ?? item["thought_signature"],
                        "reasoning thought_signature",
                    );
                    if (thoughtSignature !== undefined) {
                        event.thoughtSignature = thoughtSignature;
                    }
                    events.push(event);
                }
            }
        }
    }
    events.push({
        type: "finish",
        reason: mapResponsesFinishReason(root, hasToolCall),
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

export function parseResponsesStreamEvent(value: unknown): IrEvent[] {
    const root = expectRecord(value, "Responses stream event");
    const type = expectString(root["type"], "event.type");
    if (type === "response.created" || type === "response.in_progress") {
        return [
            {
                type: "message_start",
            },
        ];
    }
    if (type === "response.output_text.delta") {
        const event: IrEvent = {
            type: "text_delta",
            delta: expectString(root["delta"], "event.delta"),
        };
        const thoughtSignature = optionalString(
            root["thought_signature"],
            "event.thought_signature",
        );
        if (thoughtSignature !== undefined) {
            event.thoughtSignature = thoughtSignature;
        }
        return [
            event,
        ];
    }
    if (
        type === "response.reasoning_summary_text.delta" ||
        type === "response.reasoning_text.delta"
    ) {
        const event: IrEvent = {
            type: "reasoning_delta",
            delta: expectString(root["delta"], "event.delta"),
        };
        const thoughtSignature = optionalString(
            root["thought_signature"],
            "event.thought_signature",
        );
        if (thoughtSignature !== undefined) {
            event.thoughtSignature = thoughtSignature;
        }
        return [
            event,
        ];
    }
    if (type === "response.output_item.added") {
        const item = expectRecord(root["item"], "event.item");
        if (item["type"] !== "function_call") return [];
        const event: IrEvent = {
            type: "tool_call_start",
            index: optionalNonNegativeInteger(root["output_index"], "output_index") ?? 0,
            id: expectNonEmptyString(item["call_id"] ?? item["id"], "function call id"),
            name: expectNonEmptyString(item["name"], "function call name"),
        };
        const thoughtSignature = optionalString(
            item["thought_signature"],
            "function call thought_signature",
        );
        if (thoughtSignature !== undefined) {
            event.thoughtSignature = thoughtSignature;
        }
        return [
            event,
        ];
    }
    if (type === "response.function_call_arguments.delta") {
        return [
            {
                type: "tool_call_delta",
                index: optionalNonNegativeInteger(root["output_index"], "output_index") ?? 0,
                delta: expectString(root["delta"], "event.delta"),
            },
        ];
    }
    if (type === "response.output_item.done") {
        const item = expectRecord(root["item"], "event.item");
        return item["type"] === "function_call"
            ? [
                  {
                      type: "tool_call_done",
                      index: optionalNonNegativeInteger(root["output_index"], "output_index") ?? 0,
                  },
              ]
            : [];
    }
    if (type === "response.completed" || type === "response.incomplete") {
        const response = expectRecord(root["response"], "event.response");
        const events: IrEvent[] = [
            {
                type: "finish",
                reason: mapResponsesFinishReason(response, hasResponsesToolCall(response)),
            },
        ];
        if (response["usage"] !== undefined)
            events.push({
                type: "usage",
                usage: parseUsage(response["usage"], usageFieldNames),
            });
        events.push({
            type: "done",
        });
        return events;
    }
    return [];
}

function hasResponsesToolCall(response: Record<string, unknown>): boolean {
    if (!Array.isArray(response["output"])) return false;
    return response["output"].some((item) => {
        return (
            typeof item === "object" &&
            item !== null &&
            "type" in item &&
            item.type === "function_call"
        );
    });
}

function mapResponsesFinishReason(
    response: Record<string, unknown>,
    hasToolCall: boolean,
): FinishReason {
    if (response["status"] === "incomplete") {
        const details =
            response["incomplete_details"] === undefined
                ? {}
                : expectRecord(response["incomplete_details"], "incomplete_details");
        return details["reason"] === "content_filter" ? "content_filter" : "length";
    }
    return hasToolCall ? "tool_calls" : "stop";
}

export function renderResponsesResponse(
    events: readonly IrEvent[],
    metadata: ResponsesMetadata,
): ResponsesResponseObject {
    let text = "";
    let textThoughtSignature: string | undefined;
    let reasoning = "";
    let reasoningThoughtSignature: string | undefined;
    let finishReason: FinishReason = "stop";
    let usage: IrUsage | undefined;
    const tools = new Map<
        number,
        {
            id: string;
            name: string;
            arguments: string;
            thoughtSignature?: string;
        }
    >();
    for (const event of events) {
        if (event.type === "text_delta") {
            text += event.delta;
            // Last-wins: a thought signature covers only the content issued up to it, so the
            // latest signature is the one that matches the fully coalesced text or reasoning.
            if (event.thoughtSignature !== undefined) {
                textThoughtSignature = event.thoughtSignature;
            }
        } else if (event.type === "reasoning_delta") {
            reasoning += event.delta;
            if (event.thoughtSignature !== undefined) {
                reasoningThoughtSignature = event.thoughtSignature;
            }
        } else if (event.type === "tool_call_start") {
            const tool: {
                id: string;
                name: string;
                arguments: string;
                thoughtSignature?: string;
            } = {
                id: event.id,
                name: event.name,
                arguments: "",
            };
            if (event.thoughtSignature !== undefined) {
                tool.thoughtSignature = event.thoughtSignature;
            }
            tools.set(event.index, tool);
        } else if (event.type === "tool_call_delta") {
            const tool = tools.get(event.index);
            if (tool !== undefined) tool.arguments += event.delta;
        } else if (event.type === "finish") finishReason = event.reason;
        else if (event.type === "usage") usage = event.usage;
    }
    const output: JsonObject[] = [];
    if (reasoning.length > 0) {
        const reasoningItem: JsonObject = {
            id: `${metadata.id}_reasoning`,
            type: "reasoning",
            summary: [
                {
                    type: "summary_text",
                    text: reasoning,
                },
            ],
        };
        if (reasoningThoughtSignature !== undefined) {
            reasoningItem["thought_signature"] = reasoningThoughtSignature;
        }
        output.push(reasoningItem);
    }
    if (text.length > 0) {
        const textPart: JsonObject = {
            type: "output_text",
            text,
            annotations: [],
        };
        if (textThoughtSignature !== undefined) {
            textPart["thought_signature"] = textThoughtSignature;
        }
        output.push({
            id: `${metadata.id}_message`,
            type: "message",
            role: "assistant",
            status: "completed",
            content: [
                textPart,
            ],
        });
    }
    for (const [, tool] of [
        ...tools.entries(),
    ].sort(([left], [right]) => left - right)) {
        const toolItem: JsonObject = {
            id: `${metadata.id}_tool_${output.length}`,
            type: "function_call",
            status: "completed",
            call_id: tool.id,
            name: tool.name,
            arguments: tool.arguments,
        };
        if (tool.thoughtSignature !== undefined) {
            toolItem["thought_signature"] = tool.thoughtSignature;
        }
        output.push(toolItem);
    }
    const isIncomplete = finishReason === "length" || finishReason === "content_filter";
    const result: ResponsesResponseObject = {
        id: metadata.id,
        object: "response",
        created_at: metadata.created,
        model: metadata.model,
        status: isIncomplete ? "incomplete" : "completed",
        output,
    };
    if (isIncomplete)
        result["incomplete_details"] = {
            reason: finishReason === "content_filter" ? "content_filter" : "max_output_tokens",
        };
    if (usage !== undefined) result["usage"] = renderUsage(usage, usageFieldNames);
    return result;
}

type ResponsesStreamRenderer = (events: readonly IrEvent[]) => string[];

export function createResponsesStreamRenderer(
    metadata: ResponsesMetadata,
): ResponsesStreamRenderer {
    const outputItems = new Map<number, JsonObject>();
    const tools = new Map<
        number,
        {
            outputIndex: number;
            item: JsonObject;
            arguments: string;
            isDone: boolean;
        }
    >();
    let nextOutputIndex = 0;
    let textOutputIndex: number | undefined;
    let text = "";
    let textThoughtSignature: string | undefined;
    let reasoningOutputIndex: number | undefined;
    let reasoning = "";
    let reasoningThoughtSignature: string | undefined;
    let usage: IrUsage | undefined;
    let finishReason: FinishReason = "stop";
    const responseBase: JsonObject = {
        id: metadata.id,
        object: "response",
        created_at: metadata.created,
        model: metadata.model,
    };
    let isResponseCreated = false;

    return (events): string[] => {
        const frames: string[] = [];
        if (!isResponseCreated) {
            isResponseCreated = true;
            frames.push(
                encodeSse("response.created", {
                    type: "response.created",
                    response: {
                        ...responseBase,
                        status: "in_progress",
                        output: [],
                    },
                }),
            );
        }
        for (const event of events) {
            if (event.type === "message_start") continue;
            if (event.type === "text_delta") {
                if (textOutputIndex === undefined) {
                    textOutputIndex = nextOutputIndex;
                    nextOutputIndex += 1;
                    const item: JsonObject = {
                        id: `${metadata.id}_message`,
                        type: "message",
                        role: "assistant",
                        status: "in_progress",
                        content: [],
                    };
                    outputItems.set(textOutputIndex, item);
                    frames.push(
                        encodeSse("response.output_item.added", {
                            type: "response.output_item.added",
                            response_id: metadata.id,
                            output_index: textOutputIndex,
                            item,
                        }),
                        encodeSse("response.content_part.added", {
                            type: "response.content_part.added",
                            response_id: metadata.id,
                            output_index: textOutputIndex,
                            content_index: 0,
                            part: {
                                type: "output_text",
                                text: "",
                                annotations: [],
                            },
                        }),
                    );
                }
                text += event.delta;
                if (event.thoughtSignature !== undefined) {
                    textThoughtSignature = event.thoughtSignature;
                }
                frames.push(
                    encodeSse("response.output_text.delta", {
                        type: "response.output_text.delta",
                        response_id: metadata.id,
                        output_index: textOutputIndex,
                        content_index: 0,
                        delta: event.delta,
                        ...(event.thoughtSignature === undefined
                            ? {}
                            : {
                                  thought_signature: event.thoughtSignature,
                              }),
                    }),
                );
            } else if (event.type === "reasoning_delta") {
                if (reasoningOutputIndex === undefined) {
                    reasoningOutputIndex = nextOutputIndex;
                    nextOutputIndex += 1;
                    const item: JsonObject = {
                        id: `${metadata.id}_reasoning`,
                        type: "reasoning",
                        status: "in_progress",
                        summary: [],
                    };
                    outputItems.set(reasoningOutputIndex, item);
                    frames.push(
                        encodeSse("response.output_item.added", {
                            type: "response.output_item.added",
                            response_id: metadata.id,
                            output_index: reasoningOutputIndex,
                            item,
                        }),
                        encodeSse("response.reasoning_summary_part.added", {
                            type: "response.reasoning_summary_part.added",
                            response_id: metadata.id,
                            output_index: reasoningOutputIndex,
                            summary_index: 0,
                            part: {
                                type: "summary_text",
                                text: "",
                            },
                        }),
                    );
                }
                reasoning += event.delta;
                if (event.thoughtSignature !== undefined) {
                    reasoningThoughtSignature = event.thoughtSignature;
                }
                frames.push(
                    encodeSse("response.reasoning_summary_text.delta", {
                        type: "response.reasoning_summary_text.delta",
                        response_id: metadata.id,
                        output_index: reasoningOutputIndex,
                        summary_index: 0,
                        delta: event.delta,
                        ...(event.thoughtSignature === undefined
                            ? {}
                            : {
                                  thought_signature: event.thoughtSignature,
                              }),
                    }),
                );
            } else if (event.type === "tool_call_start") {
                // Starts can legally repeat for one call: the Chat Completions parser
                // emits a start whenever a delta chunk carries an id or a name, and
                // Groq-style Responses upstreams re-send both for a call that is already
                // in flight. A repeat must update the existing item in place —
                // allocating a second output item would orphan the first one as an empty
                // `in_progress` entry that still lands in the final `response.completed`
                // output. Arguments accumulate on the map entry and keep flowing through
                // `tool_call_delta`.
                const knownTool = tools.get(event.index);
                if (knownTool !== undefined) {
                    knownTool.item["call_id"] = event.id;
                    knownTool.item["name"] = event.name;
                    // Last-wins, matching the text and reasoning signature semantics.
                    if (event.thoughtSignature !== undefined) {
                        knownTool.item["thought_signature"] = event.thoughtSignature;
                    }
                    continue;
                }
                const index = nextOutputIndex;
                nextOutputIndex += 1;
                const item: JsonObject = {
                    id: `${metadata.id}_tool_${index}`,
                    type: "function_call",
                    status: "in_progress",
                    call_id: event.id,
                    name: event.name,
                    arguments: "",
                };
                if (event.thoughtSignature !== undefined) {
                    item["thought_signature"] = event.thoughtSignature;
                }
                outputItems.set(index, item);
                tools.set(event.index, {
                    outputIndex: index,
                    item,
                    arguments: "",
                    isDone: false,
                });
                frames.push(
                    encodeSse("response.output_item.added", {
                        type: "response.output_item.added",
                        response_id: metadata.id,
                        output_index: index,
                        item,
                    }),
                );
            } else if (event.type === "tool_call_delta") {
                const tool = tools.get(event.index);
                if (tool !== undefined) {
                    tool.arguments += event.delta;
                    frames.push(
                        encodeSse("response.function_call_arguments.delta", {
                            type: "response.function_call_arguments.delta",
                            response_id: metadata.id,
                            output_index: tool.outputIndex,
                            delta: event.delta,
                        }),
                    );
                }
            } else if (event.type === "tool_call_done") {
                const tool = tools.get(event.index);
                if (tool !== undefined && !tool.isDone) {
                    tool.isDone = true;
                    tool.item["status"] = "completed";
                    tool.item["arguments"] = tool.arguments;
                    frames.push(
                        encodeSse("response.function_call_arguments.done", {
                            type: "response.function_call_arguments.done",
                            response_id: metadata.id,
                            output_index: tool.outputIndex,
                            arguments: tool.arguments,
                        }),
                        encodeSse("response.output_item.done", {
                            type: "response.output_item.done",
                            response_id: metadata.id,
                            output_index: tool.outputIndex,
                            item: tool.item,
                        }),
                    );
                }
            } else if (event.type === "finish") finishReason = event.reason;
            else if (event.type === "usage") usage = event.usage;
            else if (event.type === "done") {
                if (reasoningOutputIndex !== undefined) {
                    const item = outputItems.get(reasoningOutputIndex);
                    if (item !== undefined) {
                        item["status"] = "completed";
                        const summaryPart: JsonObject = {
                            type: "summary_text",
                            text: reasoning,
                        };
                        if (reasoningThoughtSignature !== undefined) {
                            item["thought_signature"] = reasoningThoughtSignature;
                            summaryPart["thought_signature"] = reasoningThoughtSignature;
                        }
                        item["summary"] = [
                            summaryPart,
                        ];
                        frames.push(
                            encodeSse("response.reasoning_summary_text.done", {
                                type: "response.reasoning_summary_text.done",
                                response_id: metadata.id,
                                output_index: reasoningOutputIndex,
                                summary_index: 0,
                                text: reasoning,
                            }),
                            encodeSse("response.reasoning_summary_part.done", {
                                type: "response.reasoning_summary_part.done",
                                response_id: metadata.id,
                                output_index: reasoningOutputIndex,
                                summary_index: 0,
                                part: summaryPart,
                            }),
                            encodeSse("response.output_item.done", {
                                type: "response.output_item.done",
                                response_id: metadata.id,
                                output_index: reasoningOutputIndex,
                                item,
                            }),
                        );
                    }
                }
                if (textOutputIndex !== undefined) {
                    const item = outputItems.get(textOutputIndex);
                    if (item !== undefined) {
                        item["status"] = "completed";
                        const textPart: JsonObject = {
                            type: "output_text",
                            text,
                            annotations: [],
                        };
                        if (textThoughtSignature !== undefined) {
                            textPart["thought_signature"] = textThoughtSignature;
                        }
                        item["content"] = [
                            textPart,
                        ];
                        frames.push(
                            encodeSse("response.output_text.done", {
                                type: "response.output_text.done",
                                response_id: metadata.id,
                                output_index: textOutputIndex,
                                content_index: 0,
                                text,
                            }),
                            encodeSse("response.content_part.done", {
                                type: "response.content_part.done",
                                response_id: metadata.id,
                                output_index: textOutputIndex,
                                content_index: 0,
                                part: textPart,
                            }),
                            encodeSse("response.output_item.done", {
                                type: "response.output_item.done",
                                response_id: metadata.id,
                                output_index: textOutputIndex,
                                item,
                            }),
                        );
                    }
                }
                for (const tool of tools.values()) {
                    if (tool.isDone) continue;
                    tool.isDone = true;
                    tool.item["status"] = "completed";
                    tool.item["arguments"] = tool.arguments;
                    frames.push(
                        encodeSse("response.function_call_arguments.done", {
                            type: "response.function_call_arguments.done",
                            response_id: metadata.id,
                            output_index: tool.outputIndex,
                            arguments: tool.arguments,
                        }),
                        encodeSse("response.output_item.done", {
                            type: "response.output_item.done",
                            response_id: metadata.id,
                            output_index: tool.outputIndex,
                            item: tool.item,
                        }),
                    );
                }
                const isIncomplete = finishReason === "length" || finishReason === "content_filter";
                const response: JsonObject = {
                    ...responseBase,
                    status: isIncomplete ? "incomplete" : "completed",
                    output: [
                        ...outputItems.entries(),
                    ]
                        .sort(([left], [right]) => left - right)
                        .map((entry) => entry[1]),
                };
                if (isIncomplete)
                    response["incomplete_details"] = {
                        reason:
                            finishReason === "content_filter"
                                ? "content_filter"
                                : "max_output_tokens",
                    };
                if (usage !== undefined) response["usage"] = renderUsage(usage, usageFieldNames);
                const type = isIncomplete ? "response.incomplete" : "response.completed";
                frames.push(
                    encodeSse(type, {
                        type,
                        response,
                    }),
                );
            }
        }
        return frames;
    };
}
