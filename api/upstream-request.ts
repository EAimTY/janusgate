import { assertCredentialsMatchProvider, type CredentialsCipher } from "/common/credentials";
import type {
    AntigravityOAuthCredentials,
    ClientProtocol,
    OpenAiOAuthCredentials,
    UpstreamProtocol,
} from "/common/domain";
import { buildCodeAssistRequest } from "./adapters/antigravity";
import { buildGenerateContentRequest, generateContentUrl } from "./adapters/gemini";
import {
    OPENAI_CODEX_ORIGINATOR,
    OPENAI_CODEX_RESPONSES_URL,
    OPENAI_CODEX_USER_AGENT,
} from "./adapters/openai";
import type { ResolvedModelRoute } from "./model-routing";
import type { ValidatedGatewayRequest } from "./request-validation";
import { substituteNativeModelAndThinking } from "./request-validation";
import {
    parseChatCompletionsRequest,
    renderChatCompletionsRequest,
} from "./translation/chat-completions";
import type { IrRequest } from "./translation/ir";
import { parseResponsesRequest, renderResponsesRequest } from "./translation/responses";

export interface PreparedUpstreamRequest {
    body: string;
    headers: Headers;
    isNativeProtocol: boolean;
    protocol: UpstreamProtocol;
    url: string;
}

function parseTranslatedClientRequest(
    protocol: ClientProtocol,
    body: Record<string, unknown>,
): IrRequest {
    return protocol === "openai_response"
        ? parseResponsesRequest(body)
        : parseChatCompletionsRequest(body);
}

export function upstreamProtocol(route: ResolvedModelRoute): UpstreamProtocol {
    if (route.provider.type === "antigravity_oauth") return "gemini_generate_content";
    if (route.provider.type === "openai_oauth") return "openai_response";
    if (route.provider.protocol === null)
        throw new Error("Compatible provider protocol is missing");
    return route.provider.protocol;
}

function compatibleUrl(route: ResolvedModelRoute, protocol: UpstreamProtocol): string {
    const baseUrl = route.provider.baseUrl;
    if (baseUrl === null) throw new Error("Compatible provider base URL is missing");
    if (protocol === "openai_response") return `${baseUrl}/responses`;
    if (protocol === "openai_chat_completion") return `${baseUrl}/chat/completions`;
    return generateContentUrl(baseUrl, route.upstreamName, false);
}

function requireCompatibleBaseUrl(route: ResolvedModelRoute): string {
    if (route.provider.baseUrl === null) throw new Error("Compatible provider base URL is missing");
    return route.provider.baseUrl;
}

function renderTranslatedBody(
    request: IrRequest,
    route: ResolvedModelRoute,
    protocol: UpstreamProtocol,
    effectiveThinkingLevel: ValidatedGatewayRequest["effectiveThinkingLevel"],
): unknown {
    if (protocol === "openai_response") {
        return renderResponsesRequest(request, route.upstreamName, effectiveThinkingLevel);
    }
    if (protocol === "openai_chat_completion") {
        return renderChatCompletionsRequest(request, route.upstreamName, effectiveThinkingLevel);
    }
    return buildGenerateContentRequest(request, {
        model: route.upstreamName,
        ...(effectiveThinkingLevel === undefined
            ? {}
            : {
                  reasoningEffort: effectiveThinkingLevel,
              }),
    });
}

/** API-key Gemini upstreams authenticate with the dedicated key header, not a bearer token. */
function setCompatibleAuthHeaders(
    headers: Headers,
    apiKey: string,
    protocol: UpstreamProtocol,
): void {
    if (protocol === "gemini_generate_content") {
        headers.set("x-goog-api-key", apiKey);
        return;
    }
    headers.set("Authorization", `Bearer ${apiKey}`);
}

export async function prepareUpstreamRequest(
    clientRequest: Request,
    request: ValidatedGatewayRequest,
    clientProtocol: ClientProtocol,
    route: ResolvedModelRoute,
    cipher: CredentialsCipher,
    oauthCredentials?: AntigravityOAuthCredentials | OpenAiOAuthCredentials,
): Promise<PreparedUpstreamRequest> {
    const protocol = upstreamProtocol(route);
    const isNativeProtocol = protocol === clientProtocol;
    const headers = new Headers({
        "Content-Type": "application/json",
    });
    let url: string;

    if (route.provider.type === "openai_compatible") {
        url =
            protocol === "gemini_generate_content"
                ? generateContentUrl(
                      requireCompatibleBaseUrl(route),
                      route.upstreamName,
                      request.isStreaming,
                  )
                : compatibleUrl(route, protocol);
        const credentials = assertCredentialsMatchProvider(
            "openai_compatible",
            await cipher.decrypt(route.provider.encryptedCredentials),
        );
        setCompatibleAuthHeaders(headers, credentials.apiKey, protocol);
    } else if (route.provider.type === "openai_oauth") {
        if (oauthCredentials === undefined || !("accountId" in oauthCredentials)) {
            throw new Error("OpenAI OAuth credentials are missing an account ID");
        }
        headers.set("Authorization", `Bearer ${oauthCredentials.accessToken}`);
        headers.set("chatgpt-account-id", oauthCredentials.accountId);
        headers.set("Originator", OPENAI_CODEX_ORIGINATOR);
        headers.set("User-Agent", OPENAI_CODEX_USER_AGENT);
        headers.set("Session-Id", clientRequest.headers.get("Session-Id") ?? crypto.randomUUID());
        headers.set("Accept", request.isStreaming ? "text/event-stream" : "application/json");
        url = OPENAI_CODEX_RESPONSES_URL;
    } else {
        if (oauthCredentials === undefined || !("projectId" in oauthCredentials)) {
            throw new Error("Antigravity credentials are missing a project ID");
        }
        headers.set("Authorization", `Bearer ${oauthCredentials.accessToken}`);
        const action = request.isStreaming ? ":streamGenerateContent?alt=sse" : ":generateContent";
        url = `https://cloudcode-pa.googleapis.com/${encodeURIComponent(oauthCredentials.projectId)}/v1internal${action}`;
    }

    let body: unknown;
    if (isNativeProtocol) {
        body = substituteNativeModelAndThinking(request, clientProtocol, route.upstreamName);
    } else if (protocol === "gemini_generate_content") {
        if (route.provider.type !== "antigravity_oauth") {
            body = renderTranslatedBody(
                parseTranslatedClientRequest(clientProtocol, request.body),
                route,
                protocol,
                request.effectiveThinkingLevel,
            );
        } else {
            if (oauthCredentials === undefined || !("projectId" in oauthCredentials)) {
                throw new Error("Antigravity credentials are missing a project ID");
            }
            body = buildCodeAssistRequest(
                parseTranslatedClientRequest(clientProtocol, request.body),
                {
                    model: route.upstreamName,
                    projectId: oauthCredentials.projectId,
                    ...(request.effectiveThinkingLevel === undefined
                        ? {}
                        : {
                              reasoningEffort: request.effectiveThinkingLevel,
                          }),
                },
            );
        }
    } else {
        body = renderTranslatedBody(
            parseTranslatedClientRequest(clientProtocol, request.body),
            route,
            protocol,
            request.effectiveThinkingLevel,
        );
    }
    return {
        body: JSON.stringify(body),
        headers,
        isNativeProtocol,
        protocol,
        url,
    };
}
