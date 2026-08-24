import type { IrRequest } from "../translation/ir";
import type { GenerateContentRequest } from "./gemini";
import { buildGenerateContentRequest } from "./gemini";

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

/**
 * Code Assist speaks the shared Gemini GenerateContent wire format (see ./gemini) but wraps
 * every request in a project-addressed envelope and serves it from an internal endpoint.
 */
export interface CodeAssistEnvelope {
    project: string;
    model: string;
    request: GenerateContentRequest;
}

/** Kept as the named envelope field type; the shape itself lives in ./gemini. */
export type CodeAssistGenerateRequest = GenerateContentRequest;

export interface CodeAssistRequestOptions {
    projectId: string;
    model: string;
    reasoningEffort?: string;
}

export function codeAssistRequestUrl(projectId: string, isStreaming: boolean): string {
    const action = isStreaming ? ":streamGenerateContent?alt=sse" : ":generateContent";
    return `https://cloudcode-pa.googleapis.com/${encodeURIComponent(projectId)}/v1internal${action}`;
}

export function buildCodeAssistRequest(
    request: IrRequest,
    options: CodeAssistRequestOptions,
): CodeAssistEnvelope {
    return {
        project: options.projectId,
        model: options["model"],
        request: buildGenerateContentRequest(request, {
            model: options["model"],
            ...(options.reasoningEffort === undefined
                ? {}
                : {
                      reasoningEffort: options.reasoningEffort,
                  }),
        }),
    };
}
