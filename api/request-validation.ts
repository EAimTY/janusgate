import { GatewayError } from "/common/errors";
import { isJsonObject, requireJsonObject } from "/common/validation";
import { CODE_ASSIST_DEFAULT_THINKING_LEVELS } from "./adapters/antigravity";
import { OPENAI_DEFAULT_THINKING_LEVELS } from "./adapters/openai";
import type { ResolvedModelRoute } from "./model-routing";
import { clampThinkingLevel, parseThinkingLevel, type ThinkingLevel } from "./translation/thinking";

export type ClientProtocol = "chat_completions" | "responses";

export interface ValidatedGatewayRequest {
    body: Record<string, unknown>;
    isStreaming: boolean;
    requestedModel: string;
    effectiveThinkingLevel: ThinkingLevel | undefined;
}

function supportedThinkingLevels(route: ResolvedModelRoute): readonly ThinkingLevel[] {
    if (route.thinkingLevels !== null) {
        return route.thinkingLevels.map((value) => {
            const level = parseThinkingLevel(value, "stored thinking level");
            if (level === undefined) throw new Error("Stored thinking level cannot be undefined");
            return level;
        });
    }
    return route.provider.type === "antigravity_oauth"
        ? CODE_ASSIST_DEFAULT_THINKING_LEVELS
        : OPENAI_DEFAULT_THINKING_LEVELS;
}

export function validateGatewayRequest(
    value: unknown,
    protocol: ClientProtocol,
    route?: ResolvedModelRoute,
): ValidatedGatewayRequest {
    let body: Record<string, unknown>;
    try {
        body = requireJsonObject(value, "request");
    } catch (error) {
        throw new GatewayError(
            400,
            "invalid_request_error",
            error instanceof Error ? error.message : "Request must be a JSON object",
            "invalid_json",
        );
    }
    const model = body["model"];
    if (typeof model !== "string" || model.length === 0) {
        throw new GatewayError(
            400,
            "invalid_request_error",
            "model must be a non-empty string",
            "invalid_json",
        );
    }
    const stream = body["stream"];
    if (stream !== undefined && typeof stream !== "boolean") {
        throw new GatewayError(
            400,
            "invalid_request_error",
            "stream must be a boolean",
            "invalid_json",
        );
    }
    if (protocol === "chat_completions") {
        const numberOfChoices = body["n"];
        if (numberOfChoices !== undefined && typeof numberOfChoices !== "number") {
            throw new GatewayError(
                400,
                "invalid_request_error",
                "n must be a number",
                "invalid_json",
            );
        }
        if (typeof numberOfChoices === "number" && numberOfChoices > 1) {
            throw new GatewayError(
                400,
                "invalid_request_error",
                "n greater than one is not supported",
                "unsupported_parameter",
            );
        }
    }

    const effortValue =
        protocol === "responses"
            ? requireReasoningObject(body["reasoning"])["effort"]
            : body["reasoning_effort"];
    let requestedEffort: ThinkingLevel | undefined;
    try {
        requestedEffort = parseThinkingLevel(
            effortValue,
            protocol === "responses" ? "reasoning.effort" : "reasoning_effort",
        );
    } catch (error) {
        throw new GatewayError(
            400,
            "invalid_request_error",
            error instanceof Error ? error.message : "Unsupported reasoning effort",
            "unsupported_parameter",
        );
    }
    return {
        body,
        effectiveThinkingLevel:
            route === undefined
                ? requestedEffort
                : clampThinkingLevel(requestedEffort, supportedThinkingLevels(route)),
        isStreaming: stream ?? false,
        requestedModel: model,
    };
}

function requireReasoningObject(value: unknown): Record<string, unknown> {
    if (value === undefined) return {};
    if (isJsonObject(value)) return value;
    throw new GatewayError(
        400,
        "invalid_request_error",
        "reasoning must be an object",
        "invalid_json",
    );
}

export function substituteNativeModelAndThinking(
    request: ValidatedGatewayRequest,
    protocol: ClientProtocol,
    upstreamModel: string,
): Record<string, unknown> {
    const body: Record<string, unknown> = {
        ...request.body,
        model: upstreamModel,
    };
    if (protocol === "chat_completions") {
        if (body["reasoning_effort"] !== undefined) {
            if (request.effectiveThinkingLevel === undefined) delete body["reasoning_effort"];
            else body["reasoning_effort"] = request.effectiveThinkingLevel;
        }
        return body;
    }
    const reasoningValue = body["reasoning"];
    if (reasoningValue !== undefined) {
        const reasoning = {
            ...requireReasoningObject(reasoningValue),
        };
        if (reasoning["effort"] !== undefined) {
            if (request.effectiveThinkingLevel === undefined) delete reasoning["effort"];
            else reasoning["effort"] = request.effectiveThinkingLevel;
        }
        body["reasoning"] = reasoning;
    }
    return body;
}
