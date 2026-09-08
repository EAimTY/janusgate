import { TranslationError } from "/translation/errors";
import type { IrRequest } from "/translation/ir";
import type { JsonObject, JsonValue } from "/translation/json";
import type { GenerateContentRequest } from "./gemini";
import { buildGenerateContentRequest } from "./gemini";

// Installed-app credentials embedded in Antigravity. The client secret is public by design.
export const ANTIGRAVITY_CLIENT_ID =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
export const ANTIGRAVITY_CLIENT_SECRET = "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";
export const ANTIGRAVITY_AUTHORIZE_URL = "https://accounts.google.com/o/oauth2/v2/auth";
export const ANTIGRAVITY_TOKEN_URL = "https://oauth2.googleapis.com/token";
export const ANTIGRAVITY_REDIRECT_URI = "http://localhost:51121/oauth-callback";
export const ANTIGRAVITY_OAUTH_SCOPE = [
    "https://www.googleapis.com/auth/aicode",
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
].join(" ");
export const ANTIGRAVITY_API_BASE_URL = "https://cloudcode-pa.googleapis.com/v1internal";
export const ANTIGRAVITY_LOAD_CODE_ASSIST_URL = `${ANTIGRAVITY_API_BASE_URL}:loadCodeAssist`;
export const ANTIGRAVITY_ONBOARD_USER_URL = `${ANTIGRAVITY_API_BASE_URL}:onboardUser`;
export const ANTIGRAVITY_FETCH_MODELS_URL = `${ANTIGRAVITY_API_BASE_URL}:fetchAvailableModels`;
export const ANTIGRAVITY_USER_AGENT = "antigravity/1.15.8 darwin/arm64";
export const ANTIGRAVITY_API_CLIENT = "google-cloud-sdk vscode_cloudshelleditor/0.1";
// Request-body metadata uses the protobuf enum name, while the client fingerprint header uses
// the legacy OS name emitted by the working Antigravity client. They are not interchangeable.
export const ANTIGRAVITY_CLIENT_METADATA =
    '{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}';
export const ANTIGRAVITY_HEADER_CLIENT_METADATA =
    '{"ideType":"ANTIGRAVITY","platform":"MACOS","pluginType":"GEMINI"}';

/**
 * The pinned Antigravity client identity every Code Assist request must carry. Model-serving
 * callers additionally pass the upstream model so Claude runtimes receive their required
 * interleaved-thinking beta opt-in; model-less administrative calls (onboarding, discovery)
 * carry no family and omit it.
 */
export function antigravityRequestHeaders(
    accessToken: string,
    accept: string,
    model?: string,
): Record<string, string> {
    const headers: Record<string, string> = {
        Accept: accept,
        Authorization: `Bearer ${accessToken}`,
        "Client-Metadata": ANTIGRAVITY_HEADER_CLIENT_METADATA,
        "Content-Type": "application/json",
        "User-Agent": ANTIGRAVITY_USER_AGENT,
        "X-Goog-Api-Client": ANTIGRAVITY_API_CLIENT,
    };
    // Runtime mapping never moves a model into or out of the Claude family, so the configured
    // name decides.
    if (model?.startsWith("claude-")) {
        headers["anthropic-beta"] = "interleaved-thinking-2025-05-14";
    }
    return headers;
}

/**
 * Code Assist speaks the shared Gemini GenerateContent wire format (see ./gemini) but wraps
 * every request in a project-addressed envelope and serves it from an internal endpoint.
 */
interface CodeAssistEnvelope {
    project: string;
    model: string;
    request: GenerateContentRequest;
    requestType: "agent";
    userAgent: "antigravity";
    requestId: string;
}

interface CodeAssistRequestOptions {
    projectId: string;
    model: string;
    reasoningEffort?: string;
    sessionId?: string;
}

/** Antigravity's selectable model IDs do not always equal its requestable runtime IDs. */
export function antigravityRuntimeModel(model: string, reasoningEffort?: string): string {
    if (model === "gemini-3.7-flash") return "gemini-3.7-flash-tiered";
    if (model === "gemini-3.6-flash") {
        if (reasoningEffort === "high" || reasoningEffort === "xhigh") {
            return "gemini-3.6-flash-high";
        }
        if (reasoningEffort === "medium") return "gemini-3.6-flash-medium";
        return "gemini-3.6-flash-low";
    }
    if (model === "gemini-3.5-flash") {
        if (reasoningEffort === "high" || reasoningEffort === "xhigh") {
            return "gemini-3-flash-agent";
        }
        if (reasoningEffort === "low" || reasoningEffort === "medium") {
            return "gemini-3.5-flash-low";
        }
        return "gemini-3.5-flash-extra-low";
    }
    if (model === "gemini-3.1-pro") {
        return reasoningEffort === "high" || reasoningEffort === "xhigh"
            ? "gemini-pro-agent"
            : "gemini-3.1-pro-low";
    }
    if (model === "gemini-3.1-pro-high") return "gemini-pro-agent";
    if (model === "claude-opus-4-6") return "claude-opus-4-6-thinking";
    if (model === "gpt-oss-120b") return "gpt-oss-120b-medium";
    return model;
}

function antigravityMaxOutputTokens(model: string): number {
    if (model.startsWith("claude-")) return 64_000;
    if (model.startsWith("gpt-oss-")) return 32_768;
    if (model.startsWith("gemini-3.1-pro") || model === "gemini-pro-agent") return 65_535;
    if (model.startsWith("gemini-")) return 65_536;
    return 8_192;
}

function hasFixedAntigravityThinkingRuntime(model: string): boolean {
    return (
        /^gemini-3\.[567]-flash-(?:extra-low|low|medium|high)$/.test(model) ||
        model === "gemini-3-flash-agent" ||
        model === "gemini-3.1-pro-low" ||
        model === "gemini-3.1-pro-high" ||
        model === "gemini-pro-agent" ||
        model.startsWith("claude-") ||
        model.startsWith("gpt-oss-")
    );
}

const CODE_ASSIST_CUSTOM_TOOL_SCHEMA_KEYS = new Set([
    "description",
    "enum",
    "items",
    "properties",
    "required",
    "type",
]);

const CODE_ASSIST_SCHEMA_METADATA_KEYS = new Set([
    "$anchor",
    "$comment",
    "$defs",
    "$dynamicAnchor",
    "$id",
    "default",
    "examples",
    "$schema",
    "$vocabulary",
    "definitions",
]);

function isJsonObject(value: JsonValue | undefined): value is JsonObject {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeCodeAssistCustomToolSchema(value: JsonValue): JsonValue {
    if (Array.isArray(value))
        return value.map((entry) => normalizeCodeAssistCustomToolSchema(entry));
    if (!isJsonObject(value)) return value;
    const result: JsonObject = {};
    for (const [key, child] of Object.entries(value)) {
        if (!CODE_ASSIST_CUSTOM_TOOL_SCHEMA_KEYS.has(key)) continue;
        if (key === "type" && Array.isArray(child)) {
            const scalarType = child.find((entry) => typeof entry === "string" && entry !== "null");
            if (scalarType !== undefined) result[key] = scalarType;
            continue;
        }
        if (key === "properties" && isJsonObject(child)) {
            const properties: JsonObject = {};
            for (const [propertyName, propertySchema] of Object.entries(child)) {
                properties[propertyName] = normalizeCodeAssistCustomToolSchema(propertySchema);
            }
            result[key] = properties;
            continue;
        }
        if (key === "enum" && Array.isArray(child)) {
            if (child.every((entry) => typeof entry === "string")) result[key] = child;
            continue;
        }
        result[key] = normalizeCodeAssistCustomToolSchema(child);
    }
    return result;
}

/** Resolve local schema definitions and remove metadata rejected by Code Assist. */
export function normalizeCodeAssistJsonSchema(
    schema: JsonObject,
    shouldEnsureRootObject = true,
): JsonObject {
    function resolveReference(reference: string): JsonValue | undefined {
        if (!reference.startsWith("#/")) return undefined;
        let current: JsonValue = schema;
        for (const encodedToken of reference.slice(2).split("/")) {
            const token = encodedToken.replaceAll("~1", "/").replaceAll("~0", "~");
            if (Array.isArray(current)) {
                if (!/^\d+$/.test(token)) return undefined;
                const entry: JsonValue | undefined = current[Number(token)];
                if (entry === undefined) return undefined;
                current = entry;
            } else if (isJsonObject(current)) {
                const entry: JsonValue | undefined = current[token];
                if (entry === undefined) return undefined;
                current = entry;
            } else return undefined;
        }
        return current;
    }

    function normalize(value: JsonValue, activeReferences: ReadonlySet<string>): JsonValue {
        if (Array.isArray(value)) return value.map((entry) => normalize(entry, activeReferences));
        if (!isJsonObject(value)) return value;

        const reference = value["$ref"];
        let base: JsonObject = {};
        if (reference !== undefined) {
            if (typeof reference !== "string" || activeReferences.has(reference)) {
                throw new TranslationError(
                    "unsupported_parameter",
                    "Code Assist does not support cyclic or malformed JSON Schema references",
                );
            }
            const referencedValue = resolveReference(reference);
            if (referencedValue === undefined) {
                throw new TranslationError(
                    "unsupported_parameter",
                    "Code Assist does not support unresolved or external JSON Schema references",
                );
            }
            const normalizedReference = normalize(
                referencedValue,
                new Set([
                    ...activeReferences,
                    reference,
                ]),
            );
            if (!isJsonObject(normalizedReference)) {
                throw new TranslationError(
                    "unsupported_parameter",
                    "Code Assist requires referenced JSON Schemas to be objects",
                );
            }
            base = normalizedReference;
        }

        const result: JsonObject = {
            ...base,
        };
        for (const [key, entry] of Object.entries(value)) {
            if (key === "$ref" || CODE_ASSIST_SCHEMA_METADATA_KEYS.has(key)) continue;
            if (key === "const") {
                if (value["enum"] !== undefined) {
                    throw new TranslationError(
                        "unsupported_parameter",
                        "Code Assist cannot combine JSON Schema const and enum",
                    );
                }
                result["enum"] = [
                    normalize(entry, activeReferences),
                ];
                continue;
            }
            result[key] = normalize(entry, activeReferences);
        }
        return result;
    }

    const normalized = normalize(schema, new Set());
    if (!isJsonObject(normalized)) throw new Error("Normalized tool schema is not an object");
    if (shouldEnsureRootObject) {
        if (normalized["type"] === undefined) normalized["type"] = "object";
        if (normalized["properties"] === undefined) normalized["properties"] = {};
    }
    return normalized;
}

export function codeAssistRequestUrl(isStreaming: boolean): string {
    const action = isStreaming ? ":streamGenerateContent?alt=sse" : ":generateContent";
    return `${ANTIGRAVITY_API_BASE_URL}${action}`;
}

export function buildCodeAssistRequest(
    request: IrRequest,
    options: CodeAssistRequestOptions,
): CodeAssistEnvelope {
    const runtimeModel = antigravityRuntimeModel(options.model, options.reasoningEffort);
    const generateRequest = buildGenerateContentRequest(request, {
        model: runtimeModel,
        ...(options.reasoningEffort === undefined
            ? {}
            : {
                  reasoningEffort: options.reasoningEffort,
              }),
    });
    if (options.sessionId !== undefined) generateRequest.sessionId = options.sessionId;
    // generationConfig is guaranteed from here on: the maxOutputTokens clamp below always
    // applies on Antigravity runtimes.
    generateRequest.generationConfig ??= {};
    generateRequest.generationConfig.maxOutputTokens = Math.min(
        generateRequest.generationConfig.maxOutputTokens ??
            antigravityMaxOutputTokens(runtimeModel),
        antigravityMaxOutputTokens(runtimeModel),
    );
    if (generateRequest.generationConfig.responseJsonSchema !== undefined) {
        generateRequest.generationConfig.responseJsonSchema = normalizeCodeAssistJsonSchema(
            generateRequest.generationConfig.responseJsonSchema,
            false,
        );
    }
    const isCustomToolRuntime =
        runtimeModel.startsWith("claude-") || runtimeModel.startsWith("gpt-oss-");
    for (const toolGroup of generateRequest.tools ?? []) {
        for (const declaration of toolGroup.functionDeclarations) {
            const normalizedSchema = normalizeCodeAssistJsonSchema(
                declaration.parametersJsonSchema ?? declaration.parameters ?? {},
            );
            if (isCustomToolRuntime) {
                const customToolSchema = normalizeCodeAssistCustomToolSchema(normalizedSchema);
                if (!isJsonObject(customToolSchema)) {
                    throw new Error("Code Assist custom tool schema root must be an object");
                }
                declaration.parameters = customToolSchema;
                delete declaration.parametersJsonSchema;
            } else {
                declaration.parametersJsonSchema = normalizedSchema;
                delete declaration.parameters;
            }
        }
    }
    if (
        runtimeModel.startsWith("claude-") &&
        generateRequest.tools !== undefined &&
        generateRequest.toolConfig === undefined
    ) {
        generateRequest.toolConfig = {
            functionCallingConfig: {
                mode: "VALIDATED",
            },
        };
    }
    if (runtimeModel === "gemini-3.7-flash-tiered") {
        generateRequest.generationConfig.thinkingConfig = {
            thinkingLevel:
                options.reasoningEffort === "high" || options.reasoningEffort === "xhigh"
                    ? "HIGH"
                    : options.reasoningEffort === "medium"
                      ? "MEDIUM"
                      : "LOW",
        };
    } else if (
        hasFixedAntigravityThinkingRuntime(runtimeModel) &&
        generateRequest.generationConfig.thinkingConfig !== undefined
    ) {
        // These runtimes encode their thinking tier in the model ID or expose one fixed
        // runtime. Code Assist rejects a separate thinkingConfig for them.
        delete generateRequest.generationConfig.thinkingConfig;
    }
    return {
        project: options.projectId,
        model: runtimeModel,
        request: generateRequest,
        requestType: "agent",
        userAgent: "antigravity",
        requestId: `agent-${crypto.randomUUID()}`,
    };
}
