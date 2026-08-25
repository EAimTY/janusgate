import type { OAuthFlowType, UpstreamProtocol } from "/common/domain";
import { ValidationError } from "/common/errors";
import {
    parseBaseUrl,
    parseModelPrefix,
    parseNonEmptyString,
    parseOAuthFlowType,
    parsePositiveInteger,
    parseProviderName,
    parseThinkingLevels,
    parseUpstreamProtocol,
    requireJsonObject,
} from "/common/validation";
import { PROVIDER_NAME_MAX_LENGTH, type ThinkingLevel } from "/config";

export interface SaveProviderInput {
    readonly name: string;
    readonly type?: "openai_compatible";
    readonly protocol?: UpstreamProtocol;
    readonly baseUrl?: string;
    readonly apiKey?: string;
    readonly enabled?: boolean;
    readonly modelPrefix?: string;
}

export interface DeleteProviderInput {
    readonly providerName: string;
}

export interface StartOAuthInput {
    readonly type: OAuthFlowType;
    readonly providerName?: string;
}

export interface CompleteOAuthInput {
    readonly redirectUrl: string;
    readonly name?: string;
    readonly modelPrefix?: string;
    readonly projectId?: string;
}

export interface SaveModelInput {
    readonly id?: number;
    readonly providerName: string;
    readonly upstreamName: string;
    readonly name?: string;
    readonly enabled?: boolean;
    readonly thinkingLevels?: readonly ThinkingLevel[] | null;
}

export interface DeleteModelInput {
    readonly id: number;
}

export interface DiscoverModelsInput {
    readonly providerName: string;
}

function assertAllowedFields(
    object: Record<string, unknown>,
    allowedFields: readonly string[],
): void {
    const allowed = new Set(allowedFields);
    const unexpected = Object.keys(object).filter((field) => !allowed.has(field));
    if (unexpected.length > 0) {
        throw new ValidationError(`Unexpected field: ${unexpected.sort()[0]}`);
    }
}

function parseBoolean(value: unknown, fieldName: string): boolean {
    if (typeof value !== "boolean") {
        throw new ValidationError(`${fieldName} must be a boolean`);
    }
    return value;
}

function parseProviderNameField(object: Record<string, unknown>, fieldName: string): string {
    const value = object[fieldName];
    try {
        return parseProviderName(value);
    } catch {
        throw new ValidationError(
            `${fieldName} must match [a-z0-9_-]{1,${PROVIDER_NAME_MAX_LENGTH}}`,
        );
    }
}

export function parseSaveProviderInput(value: unknown): SaveProviderInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "apiKey",
        "baseUrl",
        "enabled",
        "modelPrefix",
        "name",
        "protocol",
        "type",
    ]);

    const input: {
        name: string;
        type?: "openai_compatible";
        protocol?: UpstreamProtocol;
        baseUrl?: string;
        apiKey?: string;
        enabled?: boolean;
        modelPrefix?: string;
    } = {
        name: parseProviderName(object["name"]),
    };

    if (Object.hasOwn(object, "type")) {
        if (object["type"] !== "openai_compatible") {
            throw new ValidationError("type must be openai_compatible");
        }
        input.type = "openai_compatible";
    }
    if (Object.hasOwn(object, "protocol"))
        input.protocol = parseUpstreamProtocol(object["protocol"]);
    if (Object.hasOwn(object, "baseUrl")) input.baseUrl = parseBaseUrl(object["baseUrl"]);
    if (Object.hasOwn(object, "apiKey")) {
        input.apiKey = parseNonEmptyString(object["apiKey"], "apiKey");
    }
    if (Object.hasOwn(object, "enabled"))
        input.enabled = parseBoolean(object["enabled"], "enabled");
    if (Object.hasOwn(object, "modelPrefix")) {
        input.modelPrefix = parseModelPrefix(object["modelPrefix"]);
    }
    return input;
}

export function parseDeleteProviderInput(value: unknown): DeleteProviderInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "providerName",
    ]);
    return {
        providerName: parseProviderNameField(object, "providerName"),
    };
}

export function parseStartOAuthInput(value: unknown): StartOAuthInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "providerName",
        "type",
    ]);
    const input: {
        type: OAuthFlowType;
        providerName?: string;
    } = {
        type: parseOAuthFlowType(object["type"]),
    };
    if (Object.hasOwn(object, "providerName")) {
        input.providerName = parseProviderNameField(object, "providerName");
    }
    return input;
}

export function parseCompleteOAuthInput(value: unknown): CompleteOAuthInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "modelPrefix",
        "name",
        "projectId",
        "redirectUrl",
    ]);
    const input: {
        redirectUrl: string;
        name?: string;
        modelPrefix?: string;
        projectId?: string;
    } = {
        redirectUrl: parseNonEmptyString(object["redirectUrl"], "redirectUrl"),
    };
    if (Object.hasOwn(object, "name")) input.name = parseProviderName(object["name"]);
    if (Object.hasOwn(object, "modelPrefix")) {
        input.modelPrefix = parseModelPrefix(object["modelPrefix"]);
    }
    if (Object.hasOwn(object, "projectId")) {
        input.projectId = parseNonEmptyString(object["projectId"], "projectId");
    }
    return input;
}

export function parseSaveModelInput(value: unknown): SaveModelInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "enabled",
        "id",
        "name",
        "providerName",
        "thinkingLevels",
        "upstreamName",
    ]);
    const input: {
        id?: number;
        providerName: string;
        upstreamName: string;
        name?: string;
        enabled?: boolean;
        thinkingLevels?: readonly ThinkingLevel[] | null;
    } = {
        providerName: parseProviderNameField(object, "providerName"),
        upstreamName: parseNonEmptyString(object["upstreamName"], "upstreamName"),
    };
    if (Object.hasOwn(object, "id")) input.id = parsePositiveInteger(object["id"], "id");
    if (Object.hasOwn(object, "name")) {
        input.name = parseNonEmptyString(object["name"], "name");
    }
    if (Object.hasOwn(object, "enabled"))
        input.enabled = parseBoolean(object["enabled"], "enabled");
    if (Object.hasOwn(object, "thinkingLevels")) {
        input.thinkingLevels = parseThinkingLevels(object["thinkingLevels"]);
    }
    return input;
}

export function parseDeleteModelInput(value: unknown): DeleteModelInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "id",
    ]);
    return {
        id: parsePositiveInteger(object["id"], "id"),
    };
}

export function parseDiscoverModelsInput(value: unknown): DiscoverModelsInput {
    const object = requireJsonObject(value, "request body");
    assertAllowedFields(object, [
        "providerName",
    ]);
    return {
        providerName: parseProviderNameField(object, "providerName"),
    };
}
