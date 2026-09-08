import { assertCredentialsMatchProvider, type CredentialsCipher } from "/common/credentials";
import type {
    AntigravityOAuthCredentials,
    ClientProtocol,
    OpenAiOAuthCredentials,
    ProviderSummary,
    UpstreamProtocol,
} from "/common/domain";
import {
    antigravityRequestHeaders,
    buildCodeAssistRequest,
    codeAssistRequestUrl,
} from "/providers/antigravity";
import { buildGenerateContentRequest, generateContentUrl } from "/providers/gemini";
import {
    OPENAI_CODEX_ORIGINATOR,
    OPENAI_CODEX_RESPONSES_URL,
    OPENAI_CODEX_USER_AGENT,
} from "/providers/openai";
import {
    parseChatCompletionsRequest,
    renderChatCompletionsRequest,
} from "/translation/chat-completions";
import type { IrRequest } from "/translation/ir";
import { parseResponsesRequest, renderResponsesRequest } from "/translation/responses";
import type { ResolvedModelRoute } from "./model-routing";
import {
    effectiveThinkingLevel,
    requireReasoningObject,
    type ValidatedGatewayRequest,
} from "./request-validation";

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

/** OAuth kinds imply their wire format; only a compatible provider chooses one explicitly. */
function upstreamProtocol(summary: ProviderSummary): UpstreamProtocol {
    switch (summary.type) {
        case "antigravity_oauth":
            return "gemini_generate_content";
        case "openai_oauth":
            return "openai_response";
        case "openai_compatible":
            // The summary invariant guarantees a compatible provider carries its protocol.
            return summary.protocol;
    }
}

function compatibleUrl(baseUrl: string, protocol: UpstreamProtocol): string {
    if (protocol === "openai_response") return `${baseUrl}/responses`;
    return `${baseUrl}/chat/completions`;
}

function renderTranslatedBody(
    request: IrRequest,
    route: ResolvedModelRoute,
    protocol: UpstreamProtocol,
    reasoningEffort: string | undefined,
): unknown {
    if (protocol === "openai_response") {
        return renderResponsesRequest(request, route.upstreamName, reasoningEffort);
    }
    if (protocol === "openai_chat_completion") {
        return renderChatCompletionsRequest(request, route.upstreamName, reasoningEffort);
    }
    return buildGenerateContentRequest(request, {
        model: route.upstreamName,
        ...(reasoningEffort === undefined
            ? {}
            : {
                  reasoningEffort,
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
    const summary = route.providerSummary;
    const protocol = upstreamProtocol(summary);
    const isNativeProtocol = protocol === clientProtocol;
    const reasoningEffort = effectiveThinkingLevel(
        request.requestedThinkingLevel,
        route.thinkingLevels,
    );
    // A missing or empty Session-Id means "generate one" (§8.3); both consuming branches below
    // work from this single normalized value.
    const clientSessionId = clientRequest.headers.get("Session-Id");
    const sessionId =
        clientSessionId === null || clientSessionId.length === 0
            ? crypto.randomUUID()
            : clientSessionId;
    const headers = new Headers({
        "Content-Type": "application/json",
    });
    let url: string;
    let body: unknown;

    if (summary.type === "openai_compatible") {
        url =
            protocol === "gemini_generate_content"
                ? generateContentUrl(summary.baseUrl, route.upstreamName, request.isStreaming)
                : compatibleUrl(summary.baseUrl, protocol);
        const credentials = assertCredentialsMatchProvider(
            "openai_compatible",
            await cipher.decrypt(route.provider.encryptedCredentials),
        );
        setCompatibleAuthHeaders(headers, credentials.apiKey, protocol);
    } else if (summary.type === "openai_oauth") {
        if (oauthCredentials === undefined || !("accountId" in oauthCredentials)) {
            throw new Error("OpenAI OAuth credentials are missing an account ID");
        }
        headers.set("Authorization", `Bearer ${oauthCredentials.accessToken}`);
        headers.set("chatgpt-account-id", oauthCredentials.accountId);
        headers.set("Originator", OPENAI_CODEX_ORIGINATOR);
        headers.set("User-Agent", OPENAI_CODEX_USER_AGENT);
        headers.set("Session-Id", sessionId);
        headers.set("Accept", request.isStreaming ? "text/event-stream" : "application/json");
        url = OPENAI_CODEX_RESPONSES_URL;
    } else {
        if (oauthCredentials === undefined || !("projectId" in oauthCredentials)) {
            throw new Error("Antigravity credentials are missing a project ID");
        }
        for (const [name, value] of Object.entries(
            antigravityRequestHeaders(
                oauthCredentials.accessToken,
                request.isStreaming ? "text/event-stream" : "application/json",
                route.upstreamName,
            ),
        )) {
            headers.set(name, value);
        }
        url = codeAssistRequestUrl(request.isStreaming);
        // Antigravity upstreams speak GenerateContent, which no client speaks, so the request
        // is always translated and belongs with the rest of the Antigravity specifics.
        body = buildCodeAssistRequest(parseTranslatedClientRequest(clientProtocol, request.body), {
            model: route.upstreamName,
            projectId: oauthCredentials.projectId,
            sessionId,
            ...(reasoningEffort === undefined
                ? {}
                : {
                      reasoningEffort,
                  }),
        });
    }

    if (body === undefined) {
        body = isNativeProtocol
            ? substituteNativeModelAndThinking(
                  request,
                  clientProtocol,
                  route.upstreamName,
                  reasoningEffort,
              )
            : renderTranslatedBody(
                  parseTranslatedClientRequest(clientProtocol, request.body),
                  route,
                  protocol,
                  reasoningEffort,
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

function substituteNativeModelAndThinking(
    request: ValidatedGatewayRequest,
    protocol: ClientProtocol,
    upstreamModel: string,
    reasoningEffort: string | undefined,
): Record<string, unknown> {
    const body: Record<string, unknown> = {
        ...request.body,
        model: upstreamModel,
    };
    if (protocol === "openai_chat_completion") {
        if (body["reasoning_effort"] !== undefined) {
            if (reasoningEffort === undefined) delete body["reasoning_effort"];
            else body["reasoning_effort"] = reasoningEffort;
        }
        return body;
    }
    const reasoningValue = body["reasoning"];
    if (reasoningValue !== undefined) {
        const reasoning = {
            ...requireReasoningObject(reasoningValue),
        };
        if (reasoning["effort"] !== undefined) {
            if (reasoningEffort === undefined) delete reasoning["effort"];
            else reasoning["effort"] = reasoningEffort;
        }
        body["reasoning"] = reasoning;
    }
    return body;
}
