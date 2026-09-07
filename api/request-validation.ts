import type { ClientProtocol } from "/common/domain";
import { isJsonObject, requireJsonObject } from "/common/validation";
import { GatewayError } from "./errors";

export interface ValidatedGatewayRequest {
    body: Record<string, unknown>;
    isStreaming: boolean;
    requestedModel: string;
    requestedThinkingLevel: string | undefined;
}

/**
 * Reasoning effort is an opaque string: it passes through verbatim unless the model defines
 * a thinkingLevels override, in which case the override acts as a whitelist and any effort
 * outside it is omitted upstream ("upstream default applies" — never an error).
 */
export function effectiveThinkingLevel(
    requested: string | undefined,
    whitelist: readonly string[] | null,
): string | undefined {
    if (requested === undefined || whitelist === null) {
        return requested;
    }
    return whitelist.includes(requested) ? requested : undefined;
}

export function validateGatewayRequest(
    value: unknown,
    protocol: ClientProtocol,
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
    if (protocol === "openai_chat_completion") {
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
        protocol === "openai_response"
            ? requireReasoningObject(body["reasoning"])["effort"]
            : body["reasoning_effort"];
    let requestedThinkingLevel: string | undefined;
    if (effortValue === undefined) {
        requestedThinkingLevel = undefined;
    } else if (typeof effortValue === "string" && effortValue.length > 0) {
        requestedThinkingLevel = effortValue;
    } else {
        throw new GatewayError(
            400,
            "invalid_request_error",
            protocol === "openai_response"
                ? "reasoning.effort must be a non-empty string"
                : "reasoning_effort must be a non-empty string",
            "unsupported_parameter",
        );
    }
    return {
        body,
        isStreaming: stream ?? false,
        requestedModel: model,
        requestedThinkingLevel,
    };
}

export function requireReasoningObject(value: unknown): Record<string, unknown> {
    if (value === undefined) return {};
    if (isJsonObject(value)) return value;
    throw new GatewayError(
        400,
        "invalid_request_error",
        "reasoning must be an object",
        "invalid_json",
    );
}
