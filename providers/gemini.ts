import { encodeBase64 } from "/common/base64";
import { TranslationError } from "/translation/errors";
import type { FinishReason, IrEvent, IrRequest, IrUsage, MessagePart } from "/translation/ir";
import {
    expectArray,
    expectNonEmptyString,
    expectRecord,
    isJsonValue,
    type JsonObject,
    optionalNonNegativeInteger,
    optionalString,
    parseJsonObjectText,
} from "/translation/json";

/**
 * Shared wire format for every upstream speaking Gemini `generateContent`:
 * the public Generative Language API (API-key providers) and Google Code Assist
 * (Antigravity OAuth providers, which wraps these bodies in a project envelope).
 */

type GeminiThinkingLevel = "low" | "medium" | "high";

export interface GenerateContentRequest {
    contents: GenerateContentContent[];
    systemInstruction?: {
        role: "user";
        parts: {
            text: string;
        }[];
    };
    tools?: {
        functionDeclarations: GenerateContentFunctionDeclaration[];
    }[];
    toolConfig?: {
        functionCallingConfig: {
            mode: "AUTO" | "NONE" | "ANY" | "VALIDATED";
            allowedFunctionNames?: string[];
        };
    };
    generationConfig?: GenerateContentGenerationConfig;
    sessionId?: string;
}

interface GenerateContentContent {
    role: "user" | "model";
    parts: JsonObject[];
}

interface GenerateContentFunctionDeclaration {
    name: string;
    description?: string;
    parameters?: JsonObject;
    parametersJsonSchema?: JsonObject;
}

interface GenerateContentGenerationConfig {
    temperature?: number;
    topP?: number;
    maxOutputTokens?: number;
    stopSequences?: string[];
    responseMimeType?: "application/json";
    responseJsonSchema?: JsonObject;
    thinkingConfig?: {
        includeThoughts?: true;
        thinkingLevel: GeminiThinkingLevel | Uppercase<GeminiThinkingLevel>;
    };
}

export interface GenerateContentStreamState {
    isStarted: boolean;
    nextToolIndex: number;
    /** Tool calls whose `tool_call_done` is still owed at the next finish reason. */
    openToolCallIndexes: Set<number>;
}

/** URL for the public Generative Language API; `baseUrl` carries the version path (e.g. `/v1beta`). */
export function generateContentUrl(baseUrl: string, model: string, isStreaming: boolean): string {
    const action = isStreaming ? ":streamGenerateContent?alt=sse" : ":generateContent";
    return `${baseUrl}/models/${encodeURIComponent(model)}${action}`;
}

/**
 * Effort strings pass through verbatim when the Gemini scale can express them. The Gemini
 * thinking scale only knows `low|medium|high`: values below it (`minimal`) drop the config
 * entirely, values above it (`xhigh`) clamp down to `high`, and unknown vendor-specific
 * strings mean the upstream default applies rather than a guessed mapping.
 */
export function mapGeminiThinkingLevel(effort: string): GeminiThinkingLevel | undefined {
    if (effort === "minimal") return undefined;
    if (effort === "xhigh") return "high";
    if (effort === "low" || effort === "medium" || effort === "high") return effort;
    return undefined;
}

function appendContent(
    contents: GenerateContentContent[],
    role: GenerateContentContent["role"],
    parts: JsonObject[],
): void {
    if (parts.length === 0) return;
    const previous = contents.at(-1);
    if (previous?.role === role) {
        previous.parts.push(...parts);
        return;
    }
    contents.push({
        role,
        parts,
    });
}

export function buildGenerateContentRequest(
    request: IrRequest,
    options: {
        model: string;
        reasoningEffort?: string;
    },
): GenerateContentRequest {
    const systemParts: string[] = [];
    const contents: GenerateContentContent[] = [];
    const toolNamesByCallId = new Map<string, string>();
    for (const message of request.messages) {
        for (const part of message.parts) {
            if (part.type === "tool_call") toolNamesByCallId.set(part.id, part.name);
        }
    }
    for (const message of request.messages) {
        if (message.role === "system") {
            const text = message.parts
                .filter((part) => part.type === "text")
                .map((part) => part.text)
                .join("");
            if (text.length > 0) systemParts.push(text);
            continue;
        }
        const parts: JsonObject[] = [];
        if (message.reasoning !== undefined) {
            const reasoningPart: JsonObject = {
                text: message.reasoning,
                thought: true,
            };
            if (message.reasoningThoughtSignature !== undefined) {
                reasoningPart["thoughtSignature"] = message.reasoningThoughtSignature;
            }
            parts.push(reasoningPart);
        }
        for (const part of message.parts)
            parts.push(renderGenerateContentPart(part, toolNamesByCallId));
        appendContent(contents, message.role === "assistant" ? "model" : "user", parts);
    }
    // GenerateContent requires a user turn first; reject rather than inventing prompt content.
    if (contents[0]?.role === "model") {
        throw new TranslationError(
            "unsupported_parameter",
            "Gemini upstreams do not support assistant-first conversation histories",
        );
    }

    const generationConfig = buildGenerationConfig(request, options.reasoningEffort);
    const generateRequest: GenerateContentRequest = {
        contents,
    };
    if (systemParts.length > 0) {
        generateRequest.systemInstruction = {
            role: "user",
            parts: [
                {
                    text: systemParts.join("\n\n"),
                },
            ],
        };
    }
    if (request.tools.length > 0) {
        generateRequest.tools = [
            {
                functionDeclarations: request.tools.map((tool) => {
                    const declaration: GenerateContentFunctionDeclaration = {
                        name: tool.name,
                        parameters: tool.parameters,
                    };
                    if (tool.description !== undefined) declaration.description = tool.description;
                    return declaration;
                }),
            },
        ];
    }
    if (request.toolChoice?.mode === "required" && request.tools.length === 0) {
        throw new TranslationError(
            "unsupported_parameter",
            "required tool_choice needs at least one tool",
        );
    }
    if (request.toolChoice !== undefined && request.tools.length > 0) {
        generateRequest.toolConfig = {
            functionCallingConfig: {
                mode:
                    request.toolChoice.mode === "none"
                        ? "NONE"
                        : request.toolChoice.mode === "required"
                          ? "ANY"
                          : "AUTO",
                ...(request.toolChoice.functionName === undefined
                    ? {}
                    : {
                          allowedFunctionNames: [
                              request.toolChoice.functionName,
                          ],
                      }),
            },
        };
    }
    if (generationConfig !== undefined) generateRequest.generationConfig = generationConfig;
    return generateRequest;
}

function buildGenerationConfig(
    request: IrRequest,
    reasoningEffort: string | undefined,
): GenerateContentGenerationConfig | undefined {
    const sampling = request.sampling;
    const result: GenerateContentGenerationConfig = {};
    if (sampling.temperature !== undefined) result.temperature = sampling.temperature;
    if (sampling.topP !== undefined) result.topP = sampling.topP;
    if (sampling.maxOutputTokens !== undefined) result.maxOutputTokens = sampling.maxOutputTokens;
    if (sampling.stop !== undefined) result.stopSequences = sampling.stop;
    if (sampling.responseFormat !== undefined) {
        result.responseMimeType = "application/json";
        if (sampling.responseFormat.type === "json_schema") {
            result.responseJsonSchema = sampling.responseFormat.schema;
        }
    }
    if (reasoningEffort !== undefined) {
        const thinkingLevel = mapGeminiThinkingLevel(reasoningEffort);
        if (thinkingLevel !== undefined) {
            result.thinkingConfig = {
                includeThoughts: true,
                thinkingLevel,
            };
        }
    }
    return Object.keys(result).length === 0 ? undefined : result;
}

function renderGenerateContentPart(
    part: MessagePart,
    toolNamesByCallId: ReadonlyMap<string, string>,
): JsonObject {
    if (part.type === "text") {
        const result: JsonObject = {
            text: part.text,
        };
        if (part.thoughtSignature !== undefined) {
            result["thoughtSignature"] = part.thoughtSignature;
        }
        return result;
    }
    if (part.type === "image") return renderInlineData(part.url);
    if (part.type === "tool_call") {
        const result: JsonObject = {
            functionCall: {
                id: part.id,
                name: part.name,
                args: parseJsonObjectText(part.arguments, `arguments for ${part.name}`),
            },
        };
        if (part.thoughtSignature !== undefined) {
            result["thoughtSignature"] = part.thoughtSignature;
        }
        return result;
    }
    const name = part.name ?? toolNamesByCallId.get(part.callId);
    if (name === undefined) {
        throw new TranslationError(
            "invalid_json",
            `tool output ${part.callId} does not match a function call in this request`,
        );
    }
    const result: JsonObject = {
        functionResponse: {
            id: part.callId,
            name,
            response: {
                output: part.output,
            },
        },
    };
    if (part.thoughtSignature !== undefined) {
        result["thoughtSignature"] = part.thoughtSignature;
    }
    return result;
}

function renderInlineData(url: string): JsonObject {
    if (url.slice(0, 5).toLowerCase() !== "data:") {
        throw new TranslationError(
            "unsupported_parameter",
            "Gemini upstreams accept only data-URL image inputs",
        );
    }
    const commaIndex = url.indexOf(",");
    if (commaIndex < 0) {
        throw new TranslationError("invalid_json", "image data URL is malformed");
    }
    const metadata = url.slice(5, commaIndex).split(";");
    const mimeType = metadata[0]?.toLowerCase();
    if (mimeType === undefined || !mimeType.startsWith("image/")) {
        throw new TranslationError(
            "unsupported_parameter",
            "Gemini image data URLs must use an image MIME type",
        );
    }
    const payload = url.slice(commaIndex + 1);
    if (payload.length === 0) throw new TranslationError("invalid_json", "image data URL is empty");
    const data = metadata.some((entry) => entry.toLowerCase() === "base64")
        ? normalizeBase64(payload)
        : encodeBase64(decodeDataUrl(payload));
    return {
        inlineData: {
            mimeType,
            data,
        },
    };
}

function normalizeBase64(payload: string): string {
    let decoded: string;
    try {
        decoded = decodeURIComponent(payload).replaceAll(/\s/g, "");
    } catch {
        throw new TranslationError("invalid_json", "image data URL contains invalid escaping");
    }
    if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(decoded)) {
        throw new TranslationError("invalid_json", "image data URL contains invalid base64");
    }
    return decoded;
}

function decodeDataUrl(payload: string): Uint8Array {
    const bytes: number[] = [];
    for (let index = 0; index < payload.length; index += 1) {
        const character = payload[index];
        if (character === "%") {
            const encodedByte = payload.slice(index + 1, index + 3);
            if (!/^[0-9A-Fa-f]{2}$/.test(encodedByte)) {
                throw new TranslationError(
                    "invalid_json",
                    "image data URL contains invalid escaping",
                );
            }
            bytes.push(Number.parseInt(encodedByte, 16));
            index += 2;
            continue;
        }
        if (character === undefined) continue;
        const codePoint = payload.codePointAt(index);
        if (codePoint === undefined) continue;
        const literal = String.fromCodePoint(codePoint);
        bytes.push(...new TextEncoder().encode(literal));
        index += literal.length - 1;
    }
    return new Uint8Array(bytes);
}

interface ParsedGenerateContentPart {
    events: IrEvent[];
    hasToolCall: boolean;
}

/**
 * Shared per-part parser for streaming and non-streaming responses. It emits the opening
 * events (a text/reasoning delta, or tool_call_start plus the full argument delta); when a
 * tool call completes is a streaming-policy decision owned by the callers.
 */
function parseGenerateContentPart(
    value: unknown,
    toolCallIndex: number,
): ParsedGenerateContentPart {
    const part = expectRecord(value, "candidate part");
    const thoughtSignature = optionalString(part["thoughtSignature"], "part.thoughtSignature");
    if (typeof part["text"] === "string") {
        const event: IrEvent =
            part["thought"] === true
                ? {
                      type: "reasoning_delta",
                      delta: part["text"],
                  }
                : {
                      type: "text_delta",
                      delta: part["text"],
                  };
        if (thoughtSignature !== undefined) event["thoughtSignature"] = thoughtSignature;
        return {
            events: [
                event,
            ],
            hasToolCall: false,
        };
    }
    if (part["functionCall"] !== undefined) {
        const functionCall = expectRecord(part["functionCall"], "functionCall");
        const args = functionCall["args"] === undefined ? {} : functionCall["args"];
        if (!isJsonValue(args))
            throw new TranslationError("upstream_failure", "functionCall.args is not JSON");
        const start: IrEvent = {
            type: "tool_call_start",
            index: toolCallIndex,
            id: generateToolCallId(functionCall),
            name: expectNonEmptyString(functionCall["name"], "functionCall.name"),
        };
        if (thoughtSignature !== undefined) start["thoughtSignature"] = thoughtSignature;
        return {
            events: [
                start,
                {
                    type: "tool_call_delta",
                    index: toolCallIndex,
                    delta: JSON.stringify(args),
                },
            ],
            hasToolCall: true,
        };
    }
    return {
        events: [],
        hasToolCall: false,
    };
}

function generateToolCallId(functionCall: Record<string, unknown>): string {
    const upstreamId = optionalString(functionCall["id"], "functionCall.id");
    return upstreamId === undefined || upstreamId.length === 0
        ? `call_${crypto.randomUUID().replaceAll("-", "")}`
        : upstreamId;
}

function expectSingleCandidate(root: Record<string, unknown>): unknown[] {
    const candidates =
        root["candidates"] === undefined ? [] : expectArray(root["candidates"], "candidates");
    if (candidates.length > 1) {
        throw new TranslationError("upstream_failure", "Gemini returned multiple candidates");
    }
    return candidates;
}

function candidateParts(candidateValue: unknown): unknown[] {
    const candidate = expectRecord(candidateValue, "candidate");
    const content =
        candidate["content"] === undefined
            ? undefined
            : expectRecord(candidate["content"], "candidate.content");
    return content?.["parts"] === undefined
        ? []
        : expectArray(content["parts"], "candidate.content.parts");
}

export function parseGenerateContentResponse(value: unknown): IrEvent[] {
    const root = unwrapGenerateContentResponse(value);
    const events: IrEvent[] = [
        {
            type: "message_start",
        },
    ];
    let nextToolIndex = 0;
    let hasToolCall = false;
    const promptFeedback =
        root["promptFeedback"] === undefined
            ? undefined
            : expectRecord(root["promptFeedback"], "promptFeedback");
    let finishReason = mapGeminiFinishReason(promptFeedback?.["blockReason"], false);
    for (const candidateValue of expectSingleCandidate(root)) {
        const candidate = expectRecord(candidateValue, "candidate");
        for (const partValue of candidateParts(candidateValue)) {
            const parsed = parseGenerateContentPart(partValue, nextToolIndex);
            events.push(...parsed.events);
            if (parsed.hasToolCall) {
                events.push({
                    type: "tool_call_done",
                    index: nextToolIndex,
                });
                hasToolCall = true;
                nextToolIndex += 1;
            }
        }
        // A blocked promptFeedback must survive candidates that carry no finishReason of their own.
        finishReason = mapGeminiFinishReason(
            candidate["finishReason"] ?? promptFeedback?.["blockReason"],
            hasToolCall,
        );
    }
    events.push({
        type: "finish",
        reason: finishReason,
    });
    if (root["usageMetadata"] !== undefined) {
        events.push({
            type: "usage",
            usage: parseGenerateContentUsage(root["usageMetadata"]),
        });
    }
    events.push({
        type: "done",
    });
    return events;
}

export function parseGenerateContentStreamEvent(
    value: unknown,
    state: GenerateContentStreamState,
): IrEvent[] {
    const root = unwrapGenerateContentResponse(value);
    const events: IrEvent[] = [];
    if (!state.isStarted) {
        events.push({
            type: "message_start",
        });
        state.isStarted = true;
    }
    const promptFeedback =
        root["promptFeedback"] === undefined
            ? undefined
            : expectRecord(root["promptFeedback"], "promptFeedback");
    let finishReasonValue: unknown = promptFeedback?.["blockReason"];
    for (const candidateValue of expectSingleCandidate(root)) {
        const candidate = expectRecord(candidateValue, "candidate");
        // Keep an earlier promptFeedback block signal when the candidate carries no finishReason.
        finishReasonValue = candidate["finishReason"] ?? finishReasonValue;
        for (const partValue of candidateParts(candidateValue)) {
            const parsed = parseGenerateContentPart(partValue, state.nextToolIndex);
            events.push(...parsed.events);
            if (parsed.hasToolCall) {
                state.openToolCallIndexes.add(state.nextToolIndex);
                state.nextToolIndex += 1;
            }
        }
    }
    if (finishReasonValue !== undefined) {
        for (const index of state.openToolCallIndexes)
            events.push({
                type: "tool_call_done",
                index,
            });
        const hasToolCall = state.nextToolIndex > 0;
        state.openToolCallIndexes.clear();
        events.push({
            type: "finish",
            reason: mapGeminiFinishReason(finishReasonValue, hasToolCall),
        });
        if (root["usageMetadata"] !== undefined)
            events.push({
                type: "usage",
                usage: parseGenerateContentUsage(root["usageMetadata"]),
            });
        events.push({
            type: "done",
        });
    } else if (root["usageMetadata"] !== undefined) {
        events.push({
            type: "usage",
            usage: parseGenerateContentUsage(root["usageMetadata"]),
        });
    }
    return events;
}

/** Public API responses are bare objects; Code Assist wraps them in an extra `.response` layer. */
function unwrapGenerateContentResponse(value: unknown): Record<string, unknown> {
    const envelope = expectRecord(value, "Gemini response");
    if (envelope["error"] !== undefined) {
        throw new TranslationError("upstream_failure", "Gemini returned an embedded error");
    }
    return envelope["response"] === undefined
        ? envelope
        : expectRecord(envelope["response"], "Gemini response.response");
}

function parseGenerateContentUsage(value: unknown): IrUsage {
    const usage = expectRecord(value, "usageMetadata");
    const candidateTokens =
        optionalNonNegativeInteger(usage["candidatesTokenCount"], "candidatesTokenCount") ?? 0;
    const reasoningTokens = optionalNonNegativeInteger(
        usage["thoughtsTokenCount"],
        "thoughtsTokenCount",
    );
    const result: IrUsage = {
        inputTokens: optionalNonNegativeInteger(usage["promptTokenCount"], "promptTokenCount") ?? 0,
        outputTokens: candidateTokens + (reasoningTokens ?? 0),
        totalTokens: optionalNonNegativeInteger(usage["totalTokenCount"], "totalTokenCount") ?? 0,
    };
    const cachedInputTokens = optionalNonNegativeInteger(
        usage["cachedContentTokenCount"],
        "cachedContentTokenCount",
    );
    if (reasoningTokens !== undefined) result.reasoningTokens = reasoningTokens;
    if (cachedInputTokens !== undefined) result.cachedInputTokens = cachedInputTokens;
    return result;
}

function mapGeminiFinishReason(value: unknown, hasToolCall: boolean): FinishReason {
    if (value === "MAX_TOKENS") return "length";
    if (
        value === "SAFETY" ||
        value === "RECITATION" ||
        value === "BLOCKLIST" ||
        value === "PROHIBITED_CONTENT" ||
        value === "SPII"
    ) {
        return "content_filter";
    }
    if (hasToolCall) return "tool_calls";
    if (value === undefined || value === "STOP" || value === "FINISH_REASON_UNSPECIFIED") {
        return "stop";
    }
    throw new TranslationError("upstream_failure", "Gemini returned an unsuccessful finish reason");
}
