import { assertCredentialsMatchProvider, type CredentialsCipher } from "/common/credentials";
import type { AntigravityOAuthCredentials, OpenAiOAuthCredentials } from "/common/domain";
import { buildCodeAssistRequest, codeAssistRequestUrl } from "./adapters/antigravity";
import {
    OPENAI_CODEX_ORIGINATOR,
    OPENAI_CODEX_RESPONSES_URL,
    OPENAI_CODEX_USER_AGENT,
} from "./adapters/openai";
import type { ResolvedModelRoute } from "./model-routing";
import type { ClientProtocol, ValidatedGatewayRequest } from "./request-validation";
import { substituteNativeModelAndThinking } from "./request-validation";
import {
    parseChatCompletionsRequest,
    renderChatCompletionsRequest,
} from "./translation/chat-completions";
import type { IrRequest } from "./translation/ir";
import { parseResponsesRequest, renderResponsesRequest } from "./translation/responses";

export type UpstreamProtocol = ClientProtocol | "code_assist";

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
    return protocol === "responses"
        ? parseResponsesRequest(body)
        : parseChatCompletionsRequest(body);
}

function upstreamProtocol(route: ResolvedModelRoute): UpstreamProtocol {
    if (route.provider.type === "antigravity_oauth") return "code_assist";
    if (route.provider.type === "openai_oauth") return "responses";
    if (route.provider.protocol === null)
        throw new Error("Compatible provider protocol is missing");
    return route.provider.protocol;
}

function compatibleUrl(route: ResolvedModelRoute, protocol: ClientProtocol): string {
    if (route.provider.baseUrl === null) throw new Error("Compatible provider base URL is missing");
    return protocol === "responses"
        ? `${route.provider.baseUrl}/responses`
        : `${route.provider.baseUrl}/chat/completions`;
}

function renderTranslatedBody(
    request: IrRequest,
    route: ResolvedModelRoute,
    protocol: UpstreamProtocol,
    credentials: AntigravityOAuthCredentials | OpenAiOAuthCredentials | undefined,
    effectiveThinkingLevel: ValidatedGatewayRequest["effectiveThinkingLevel"],
): unknown {
    if (protocol === "responses") {
        return renderResponsesRequest(request, route.upstreamName, effectiveThinkingLevel);
    }
    if (protocol === "chat_completions") {
        return renderChatCompletionsRequest(request, route.upstreamName, effectiveThinkingLevel);
    }
    if (credentials === undefined || !("projectId" in credentials)) {
        throw new Error("Code Assist credentials are missing a project ID");
    }
    return buildCodeAssistRequest(request, {
        model: route.upstreamName,
        projectId: credentials.projectId,
        ...(effectiveThinkingLevel === undefined
            ? {}
            : {
                  reasoningEffort: effectiveThinkingLevel,
              }),
    });
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
        const credentials = assertCredentialsMatchProvider(
            "openai_compatible",
            await cipher.decrypt(route.provider.encryptedCredentials),
        );
        headers.set("Authorization", `Bearer ${credentials.apiKey}`);
        url = compatibleUrl(route, protocol === "code_assist" ? clientProtocol : protocol);
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
        url = codeAssistRequestUrl(oauthCredentials.projectId, request.isStreaming);
    }

    const body = isNativeProtocol
        ? substituteNativeModelAndThinking(request, clientProtocol, route.upstreamName)
        : renderTranslatedBody(
              parseTranslatedClientRequest(clientProtocol, request.body),
              route,
              protocol,
              oauthCredentials,
              request.effectiveThinkingLevel,
          );
    return {
        body: JSON.stringify(body),
        headers,
        isNativeProtocol,
        protocol,
        url,
    };
}
