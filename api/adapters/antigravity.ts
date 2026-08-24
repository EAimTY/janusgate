import { TranslationError } from "../translation/errors";
import type { FinishReason, IrEvent, IrRequest, IrUsage, MessagePart } from "../translation/ir";
import {
    expectArray,
    expectNonEmptyString,
    expectRecord,
    isJsonValue,
    type JsonObject,
    optionalNonNegativeInteger,
    optionalString,
    parseJsonObjectText,
} from "../translation/json";
import type { ThinkingLevel } from "../translation/thinking";

// Installed-app credentials embedded in Antigravity. The client secret is public by design.
export const ANTIGRAVITY_CLIENT_ID =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
export const ANTIGRAVITY_CLIENT_SECRET = "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";
export const ANTIGRAVITY_AUTHORIZE_URL = "https://accounts.google.com/o/oauth2/v2/auth";
export const ANTIGRAVITY_TOKEN_URL = "https://oauth2.googleapis.com/token";
export const ANTIGRAVITY_REDIRECT_URI = "http://localhost:51121/oauth-callback";
export const ANTIGRAVITY_OAUTH_SCOPE =
    "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email";
export const ANTIGRAVITY_API_BASE_URL = "https://cloudcode-pa.googleapis.com/v1internal";
export const ANTIGRAVITY_LOAD_CODE_ASSIST_URL = `${ANTIGRAVITY_API_BASE_URL}:loadCodeAssist`;
export const ANTIGRAVITY_ONBOARD_USER_URL = `${ANTIGRAVITY_API_BASE_URL}:onboardUser`;
export const ANTIGRAVITY_USER_AGENT = "antigravity/1.107.0 darwin/arm64";
export const ANTIGRAVITY_API_CLIENT = "google-cloud-sdk vscode_cloudshelleditor/0.1";
export const ANTIGRAVITY_CLIENT_METADATA =
    '{"ideType":"ANTIGRAVITY","platform":"MACOS","pluginType":"GEMINI"}';

// The catalog is pinned because the internal model-list contract is not stable.
export const ANTIGRAVITY_MODELS: readonly string[] = [
    "gemini-3.7-flash",
    "gemini-3.6-flash",
    "gemini-3.5-flash",
    "gemini-3.1-pro-low",
    "gemini-3.1-pro-high",
];

export const CODE_ASSIST_DEFAULT_THINKING_LEVELS: readonly ThinkingLevel[] = [
    "low",
    "medium",
    "high",
];

export interface CodeAssistEnvelope {
    project: string;
    model: string;
    request: CodeAssistGenerateRequest;
}

export interface CodeAssistGenerateRequest {
    contents: CodeAssistContent[];
    systemInstruction?: {
        role: "user";
        parts: {
            text: string;
        }[];
    };
    tools?: {
        functionDeclarations: CodeAssistFunctionDeclaration[];
    }[];
    generationConfig?: CodeAssistGenerationConfig;
}

export interface CodeAssistContent {
    role: "user" | "model";
    parts: JsonObject[];
}

export interface CodeAssistFunctionDeclaration {
    name: string;
    description?: string;
    parameters: JsonObject;
}

export interface CodeAssistGenerationConfig {
    temperature?: number;
    topP?: number;
    maxOutputTokens?: number;
    stopSequences?: string[];
    responseMimeType?: "application/json";
    responseSchema?: JsonObject;
    thinkingConfig?: {
        includeThoughts: true;
        thinkingLevel: "low" | "medium" | "high";
    };
}

export interface CodeAssistRequestOptions {
    projectId: string;
    model: string;
    reasoningEffort?: ThinkingLevel;
}

export interface CodeAssistStreamState {
    isStarted: boolean;
    nextToolIndex: number;
    openToolIndexes: Map<number, string>;
}

export function codeAssistRequestUrl(projectId: string, isStreaming: boolean): string {
    const action = isStreaming ? "streamGenerateContent?alt=sse" : "generateContent";
    return `https://cloudcode-pa.googleapis.com/${encodeURIComponent(projectId)}/v1internal:${action}`;
}

export function buildCodeAssistRequest(
    request: IrRequest,
    options: CodeAssistRequestOptions,
): CodeAssistEnvelope {
    const systemParts: string[] = [];
    const contents: CodeAssistContent[] = [];
    const toolNamesByCallId = new Map<string, string>();
    for (const message of request["messages"]) {
        for (const part of message["parts"]) {
            if (part["type"] === "tool_call") toolNamesByCallId.set(part["id"], part["name"]);
        }
    }

    for (const message of request["messages"]) {
        if (message["role"] === "system") {
            const text = message["parts"]
                .filter((part) => part["type"] === "text")
                .map((part) => part["text"])
                .join("");
            if (text.length > 0) systemParts.push(text);
            continue;
        }
        const parts: JsonObject[] = [];
        if (message["reasoning"] !== undefined)
            parts.push({
                text: message["reasoning"],
                thought: true,
            });
        for (const part of message["parts"])
            parts.push(renderCodeAssistPart(part, toolNamesByCallId));
        if (parts.length > 0) {
            contents.push({
                role: message["role"] === "assistant" ? "model" : "user",
                parts,
            });
        }
    }

    const generationConfig = buildGenerationConfig(request, options.reasoningEffort);
    const generateRequest: CodeAssistGenerateRequest = {
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
    if (request["tools"].length > 0) {
        generateRequest["tools"] = [
            {
                functionDeclarations: request["tools"].map((tool) => {
                    const declaration: CodeAssistFunctionDeclaration = {
                        name: tool["name"],
                        parameters: tool["parameters"],
                    };
                    if (tool["description"] !== undefined)
                        declaration["description"] = tool["description"];
                    return declaration;
                }),
            },
        ];
    }
    if (generationConfig !== undefined) generateRequest.generationConfig = generationConfig;
    return {
        project: options.projectId,
        model: options["model"],
        request: generateRequest,
    };
}

function renderCodeAssistPart(
    part: MessagePart,
    toolNamesByCallId: ReadonlyMap<string, string>,
): JsonObject {
    if (part["type"] === "text") {
        const result: JsonObject = {
            text: part["text"],
        };
        if (part["thoughtSignature"] !== undefined)
            result["thoughtSignature"] = part["thoughtSignature"];
        return result;
    }
    if (part["type"] === "image") return renderInlineData(part["url"]);
    if (part["type"] === "tool_call") {
        const result: JsonObject = {
            functionCall: {
                name: part["name"],
                args: parseJsonObjectText(part["arguments"], `arguments for ${part.name}`),
            },
        };
        if (part["thoughtSignature"] !== undefined)
            result["thoughtSignature"] = part["thoughtSignature"];
        return result;
    }
    const name = part["name"] ?? toolNamesByCallId.get(part.callId);
    if (name === undefined) {
        throw new TranslationError(
            "invalid_json",
            `tool output ${part.callId} does not match a function call in this request`,
        );
    }
    const result: JsonObject = {
        functionResponse: {
            name,
            response: {
                output: part["output"],
            },
        },
    };
    if (part["thoughtSignature"] !== undefined)
        result["thoughtSignature"] = part["thoughtSignature"];
    return result;
}

function renderInlineData(url: string): JsonObject {
    if (!url.startsWith("data:")) {
        throw new TranslationError(
            "unsupported_parameter",
            "Code Assist accepts only data-URL image inputs",
        );
    }
    const commaIndex = url.indexOf(",");
    if (commaIndex < 0) {
        throw new TranslationError("invalid_json", "image data URL is malformed");
    }
    const metadata = url.slice(5, commaIndex).split(";");
    const mimeType = metadata[0];
    if (mimeType === undefined || !mimeType.startsWith("image/")) {
        throw new TranslationError(
            "unsupported_parameter",
            "Code Assist image data URLs must use an image MIME type",
        );
    }
    const payload = url.slice(commaIndex + 1);
    if (payload.length === 0) throw new TranslationError("invalid_json", "image data URL is empty");
    const data = metadata.includes("base64")
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

function encodeBase64(bytes: Uint8Array): string {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let result = "";
    for (let index = 0; index < bytes.length; index += 3) {
        const first = bytes[index] ?? 0;
        const second = bytes[index + 1] ?? 0;
        const third = bytes[index + 2] ?? 0;
        result += alphabet[(first >> 2) & 63] ?? "";
        result += alphabet[((first & 3) << 4) | (second >> 4)] ?? "";
        result +=
            index + 1 < bytes.length ? (alphabet[((second & 15) << 2) | (third >> 6)] ?? "") : "=";
        result += index + 2 < bytes.length ? (alphabet[third & 63] ?? "") : "=";
    }
    return result;
}

function buildGenerationConfig(
    request: IrRequest,
    reasoningEffort: ThinkingLevel | undefined,
): CodeAssistGenerationConfig | undefined {
    const sampling = request.sampling;
    const result: CodeAssistGenerationConfig = {};
    if (sampling["temperature"] !== undefined) result["temperature"] = sampling["temperature"];
    if (sampling.topP !== undefined) result.topP = sampling.topP;
    if (sampling.maxOutputTokens !== undefined) result.maxOutputTokens = sampling.maxOutputTokens;
    if (sampling["stop"] !== undefined) result.stopSequences = sampling["stop"];
    if (sampling.responseFormat !== undefined) {
        result.responseMimeType = "application/json";
        if (sampling.responseFormat["type"] === "json_schema") {
            result.responseSchema = sampling.responseFormat["schema"];
        }
    }
    if (reasoningEffort !== undefined) {
        const thinkingLevel = mapCodeAssistThinkingLevel(reasoningEffort);
        if (thinkingLevel !== undefined) {
            result.thinkingConfig = {
                includeThoughts: true,
                thinkingLevel,
            };
        }
    }
    return Object.keys(result).length === 0 ? undefined : result;
}

function mapCodeAssistThinkingLevel(level: ThinkingLevel): "low" | "medium" | "high" | undefined {
    if (level === "minimal") return undefined;
    if (level === "xhigh") return "high";
    return level;
}

export function parseCodeAssistResponse(value: unknown): IrEvent[] {
    const root = unwrapCodeAssistResponse(value);
    const events: IrEvent[] = [
        {
            type: "message_start",
        },
    ];
    let nextToolIndex = 0;
    let hasToolCall = false;
    let finishReason: FinishReason = "stop";
    const candidates =
        root["candidates"] === undefined ? [] : expectArray(root["candidates"], "candidates");
    if (candidates.length > 1) {
        throw new TranslationError("upstream_failure", "Code Assist returned multiple candidates");
    }
    for (const candidateValue of candidates) {
        const candidate = expectRecord(candidateValue, "candidate");
        const content = expectRecord(candidate["content"], "candidate.content");
        for (const partValue of expectArray(content["parts"], "candidate.content.parts")) {
            const parsed = parseCodeAssistPart(partValue, nextToolIndex);
            events.push(...parsed.events);
            if (parsed.hasToolCall) {
                hasToolCall = true;
                nextToolIndex += 1;
            }
        }
        finishReason = mapCodeAssistFinishReason(candidate["finishReason"], hasToolCall);
    }
    events.push({
        type: "finish",
        reason: finishReason,
    });
    if (root["usageMetadata"] !== undefined) {
        events.push({
            type: "usage",
            usage: parseCodeAssistUsage(root["usageMetadata"]),
        });
    }
    events.push({
        type: "done",
    });
    return events;
}

function parseCodeAssistPart(
    value: unknown,
    toolIndex: number,
): {
    events: IrEvent[];
    hasToolCall: boolean;
} {
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
        if (thoughtSignature !== undefined && event["type"] === "reasoning_delta") {
            event["thoughtSignature"] = thoughtSignature;
        } else if (thoughtSignature !== undefined && event["type"] === "text_delta") {
            event["thoughtSignature"] = thoughtSignature;
        }
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
            index: toolIndex,
            id: `call_${toolIndex + 1}`,
            name: expectNonEmptyString(functionCall["name"], "functionCall.name"),
        };
        if (thoughtSignature !== undefined && start["type"] === "tool_call_start") {
            start["thoughtSignature"] = thoughtSignature;
        }
        return {
            events: [
                start,
                {
                    type: "tool_call_delta",
                    index: toolIndex,
                    delta: JSON.stringify(args),
                },
                {
                    type: "tool_call_done",
                    index: toolIndex,
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

export function parseCodeAssistStreamEvent(
    value: unknown,
    state: CodeAssistStreamState,
): IrEvent[] {
    const root = unwrapCodeAssistResponse(value);
    const events: IrEvent[] = [];
    if (!state.isStarted) {
        events.push({
            type: "message_start",
        });
        state.isStarted = true;
    }
    let finishReasonValue: unknown;
    const candidates =
        root["candidates"] === undefined ? [] : expectArray(root["candidates"], "candidates");
    if (candidates.length > 1) {
        throw new TranslationError("upstream_failure", "Code Assist returned multiple candidates");
    }
    for (const candidateValue of candidates) {
        const candidate = expectRecord(candidateValue, "candidate");
        finishReasonValue = candidate["finishReason"];
        const content =
            candidate["content"] === undefined
                ? undefined
                : expectRecord(candidate["content"], "candidate.content");
        const parts =
            content?.["parts"] === undefined
                ? []
                : expectArray(content["parts"], "candidate.content.parts");
        for (const partValue of parts) {
            const part = expectRecord(partValue, "candidate part");
            if (part["functionCall"] !== undefined) {
                const functionCall = expectRecord(part["functionCall"], "functionCall");
                const index = state.nextToolIndex;
                state.nextToolIndex += 1;
                const id = `call_${index + 1}`;
                const name = expectNonEmptyString(functionCall["name"], "functionCall.name");
                const start: IrEvent = {
                    type: "tool_call_start",
                    index,
                    id,
                    name,
                };
                const thoughtSignature = optionalString(
                    part["thoughtSignature"],
                    "part.thoughtSignature",
                );
                if (thoughtSignature !== undefined && start["type"] === "tool_call_start") {
                    start["thoughtSignature"] = thoughtSignature;
                }
                events.push(start);
                const args = functionCall["args"] === undefined ? {} : functionCall["args"];
                if (!isJsonValue(args))
                    throw new TranslationError("upstream_failure", "functionCall.args is not JSON");
                events.push({
                    type: "tool_call_delta",
                    index,
                    delta: JSON.stringify(args),
                });
                state.openToolIndexes.set(index, name);
            } else if (typeof part["text"] === "string") {
                const thoughtSignature = optionalString(
                    part["thoughtSignature"],
                    "part.thoughtSignature",
                );
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
                if (
                    thoughtSignature !== undefined &&
                    (event["type"] === "reasoning_delta" || event["type"] === "text_delta")
                ) {
                    event["thoughtSignature"] = thoughtSignature;
                }
                events.push(event);
            }
        }
    }
    if (finishReasonValue !== undefined) {
        for (const index of state.openToolIndexes.keys())
            events.push({
                type: "tool_call_done",
                index,
            });
        const hasToolCall = state.nextToolIndex > 0;
        state.openToolIndexes.clear();
        events.push({
            type: "finish",
            reason: mapCodeAssistFinishReason(finishReasonValue, hasToolCall),
        });
        if (root["usageMetadata"] !== undefined)
            events.push({
                type: "usage",
                usage: parseCodeAssistUsage(root["usageMetadata"]),
            });
        events.push({
            type: "done",
        });
    } else if (root["usageMetadata"] !== undefined) {
        events.push({
            type: "usage",
            usage: parseCodeAssistUsage(root["usageMetadata"]),
        });
    }
    return events;
}

function unwrapCodeAssistResponse(value: unknown): Record<string, unknown> {
    const envelope = expectRecord(value, "Code Assist response");
    return envelope["response"] === undefined
        ? envelope
        : expectRecord(envelope["response"], "Code Assist response.response");
}

function parseCodeAssistUsage(value: unknown): IrUsage {
    const usage = expectRecord(value, "usageMetadata");
    const result: IrUsage = {
        inputTokens: optionalNonNegativeInteger(usage["promptTokenCount"], "promptTokenCount") ?? 0,
        outputTokens:
            optionalNonNegativeInteger(usage["candidatesTokenCount"], "candidatesTokenCount") ?? 0,
        totalTokens: optionalNonNegativeInteger(usage["totalTokenCount"], "totalTokenCount") ?? 0,
    };
    const reasoningTokens = optionalNonNegativeInteger(
        usage["thoughtsTokenCount"],
        "thoughtsTokenCount",
    );
    const cachedInputTokens = optionalNonNegativeInteger(
        usage["cachedContentTokenCount"],
        "cachedContentTokenCount",
    );
    if (reasoningTokens !== undefined) result.reasoningTokens = reasoningTokens;
    if (cachedInputTokens !== undefined) result.cachedInputTokens = cachedInputTokens;
    return result;
}

function mapCodeAssistFinishReason(value: unknown, hasToolCall: boolean): FinishReason {
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
    return hasToolCall ? "tool_calls" : "stop";
}
