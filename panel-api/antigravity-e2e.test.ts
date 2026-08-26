import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { handleGatewayRequest } from "/api/gateway";
import { CredentialsCipher, parseAntigravityOAuthCredentials } from "/common/credentials";
import type { AntigravityOAuthCredentials } from "/common/domain";
import {
    ANTIGRAVITY_FETCH_MODELS_URL,
    ANTIGRAVITY_LOAD_CODE_ASSIST_URL,
    ANTIGRAVITY_TOKEN_URL,
    codeAssistRequestUrl,
} from "/providers/antigravity";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import { createPanelApi } from "./app";

const API_TOKEN = "api-secret";
const API_TOKEN_HASH = "014c243ff960e87afc8482648f41e2084dce765aa062dcdcbf4e0e43c4db8a41";
const PANEL_TOKEN = "panel-secret";
const PANEL_TOKEN_HASH = "14c660b289912fe36e4caca8eb372106f19a9ed9d37adfd03c504648faa4b5e4";
const AES_KEY = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const ORIGIN = "https://gate.test";

const bindings: Env = {
    API_KEY_HASH: API_TOKEN_HASH,
    CREDENTIALS_AES_KEY: AES_KEY,
    DATABASE: env.DATABASE,
    PANEL_TOKEN_HASH: PANEL_TOKEN_HASH,
};

interface Route {
    match: (url: string) => boolean;
    respond: (url: string, init: RequestInit | undefined) => Response | Promise<Response>;
}

function router(routes: readonly Route[]): typeof fetch {
    return async (input, init) => {
        const url = String(input);
        for (const route of routes) {
            if (route.match(url)) return await route.respond(url, init);
        }
        throw new Error(`Unexpected upstream call: ${url}`);
    };
}

async function panel(
    app: ReturnType<typeof createPanelApi>,
    path: string,
    body?: Record<string, unknown>,
): Promise<Response> {
    return app.request(
        // The panel sub-app is mounted at /panel-api in index.ts; its own routes carry no prefix.
        `${ORIGIN}/${path}`,
        {
            ...(body === undefined
                ? {}
                : {
                      body: JSON.stringify(body),
                  }),
            headers: {
                ...(body === undefined
                    ? {}
                    : {
                          "Content-Type": "application/json",
                      }),
                Authorization: `Bearer ${PANEL_TOKEN}`,
                Origin: ORIGIN,
            },
            ...(body === undefined
                ? {}
                : {
                      method: "POST",
                  }),
        },
        bindings,
    );
}

function gatewayRequest(body: unknown): Request {
    return new Request(`${ORIGIN}/api/openai-v1/responses`, {
        body: JSON.stringify(body),
        headers: {
            Authorization: `Bearer ${API_TOKEN}`,
            "Content-Type": "application/json",
        },
        method: "POST",
    });
}

describe("Antigravity OAuth end-to-end: paste-back to Responses API", () => {
    beforeAll(async () => initializeTestDatabase(env.DATABASE));
    beforeEach(async () => clearTestDatabase(env.DATABASE));

    it("connects an account, discovers models, and serves Responses requests across token expiry", async () => {
        const issuedAccessToken = "access-1";
        const upstream = router([
            {
                match: (url) => url === ANTIGRAVITY_LOAD_CODE_ASSIST_URL,
                respond: () =>
                    Response.json({
                        allowedTiers: [
                            {
                                id: "free-tier",
                                isDefault: true,
                            },
                        ],
                        cloudaicompanionProject: "managed-project-1",
                    }),
            },
            {
                match: (url) => url === ANTIGRAVITY_TOKEN_URL && issuedAccessToken === "access-1",
                respond: (_url, init) => {
                    const body = new URLSearchParams(String(init?.body));
                    expect(body.get("grant_type")).toBe("authorization_code");
                    expect(body.get("code")).toBe("google-code");
                    return Response.json({
                        access_token: issuedAccessToken,
                        expires_in: 3600,
                        refresh_token: "google-refresh-token",
                    });
                },
            },
            {
                match: (url) => url === ANTIGRAVITY_FETCH_MODELS_URL,
                respond: (_url, init) => {
                    expect(new Headers(init?.headers).get("Authorization")).toBe(
                        `Bearer ${issuedAccessToken}`,
                    );
                    return Response.json({
                        models: {
                            "gemini-3.7-flash": {},
                            "claude-opus-4-6": {},
                            "some-internal-model": {
                                isInternal: true,
                            },
                        },
                    });
                },
            },
            {
                match: (url) => url === codeAssistRequestUrl(false),
                respond: (_url, init) => {
                    const sent = JSON.parse(String(init?.body)) as Record<string, unknown>;
                    expect(sent).toMatchObject({
                        model: "gemini-3.7-flash-tiered",
                        project: "managed-project-1",
                        requestType: "agent",
                        userAgent: "antigravity",
                    });
                    return Response.json({
                        response: {
                            candidates: [
                                {
                                    content: {
                                        parts: [
                                            {
                                                text: "hello from antigravity",
                                            },
                                        ],
                                        role: "model",
                                    },
                                    finishReason: "STOP",
                                },
                            ],
                            usageMetadata: {
                                promptTokenCount: 4,
                                candidatesTokenCount: 6,
                                totalTokenCount: 10,
                            },
                        },
                    });
                },
            },
        ]);
        const panelApi = createPanelApi({
            fetchUpstream: upstream,
        });

        // Step 1 — start the paste-back flow.
        const startResponse = await panel(panelApi, "start-provider-oauth", {
            type: "antigravity",
        });
        expect(startResponse.status).toBe(200);
        const { authorizeUrl } = (await startResponse.json()) as {
            authorizeUrl: string;
        };
        const authorize = new URL(authorizeUrl);
        expect(authorize.origin + authorize.pathname).toBe(
            "https://accounts.google.com/o/oauth2/v2/auth",
        );
        expect(authorize.searchParams.get("access_type")).toBe("offline");
        expect(authorize.searchParams.get("prompt")).toBe("consent");
        const state = authorize.searchParams.get("state");
        if (state === null) throw new Error("state missing");

        // Step 2 — complete the paste-back (token exchange + project resolution).
        const completeResponse = await panel(panelApi, "complete-provider-oauth", {
            modelPrefix: "g/",
            name: "my-google",
            redirectUrl: `http://localhost:51121/oauth-callback?code=google-code&state=${state}`,
        });
        expect(completeResponse.status).toBe(200);

        // Step 3 — discovery runs automatically from the SPA; then publish one model.
        const discoverResponse = await panel(panelApi, "discover-models", {
            providerName: "my-google",
        });
        expect(discoverResponse.status).toBe(200);
        expect(await discoverResponse.json()).toEqual({
            created: 2,
            removed: 0,
            skipped: 0,
            total: 2,
        });
        const listModels = await panel(panelApi, "list-models");
        const { models } = (await listModels.json()) as {
            models: Array<{
                id: number;
                name: string;
                enabled: boolean;
                providerName: string;
                upstreamName: string;
            }>;
        };
        const geminiModel = models.find((model) => model.upstreamName === "gemini-3.7-flash");
        if (geminiModel === undefined) throw new Error("discovered gemini model missing");
        expect(geminiModel.enabled).toBe(false);
        expect(geminiModel.name).toBe("g/gemini-3.7-flash");
        const saveResponse = await panel(panelApi, "save-model", {
            id: geminiModel.id,
            enabled: true,
            providerName: "my-google",
            upstreamName: geminiModel.upstreamName,
        });
        expect(saveResponse.status).toBe(200);
        expect(await saveResponse.json()).toEqual({
            id: geminiModel.id,
            name: "g/gemini-3.7-flash",
        });

        // Step 4 — serve a Responses request with the initial access token.
        const firstCall = await handleGatewayRequest(
            gatewayRequest({
                input: "Hi",
                model: "g/gemini-3.7-flash",
            }),
            bindings,
            "openai_response",
            upstream,
        );
        expect(firstCall.status).toBe(200);
        const firstBody = (await firstCall.json()) as {
            output: Array<{
                content: Array<{
                    text: string;
                }>;
            }>;
        };
        expect(JSON.stringify(firstBody.output)).toContain("hello from antigravity");

        // Step 5 — the stored access token expires; the next request must transparently rotate
        // it at the IdP endpoint and keep serving without reauthentication.
        await env.DATABASE.prepare(
            "UPDATE providers SET expires_at = strftime('%s','now') - 10 WHERE name = 'my-google'",
        ).run();
        const cipher = await CredentialsCipher.create(AES_KEY);
        const readCredentials = async (): Promise<AntigravityOAuthCredentials> => {
            const row = await env.DATABASE.prepare(
                "SELECT encrypted_credentials FROM providers WHERE name = 'my-google'",
            ).first<{
                encrypted_credentials: ArrayBuffer;
            }>();
            if (row === null) throw new Error("provider missing");
            return parseAntigravityOAuthCredentials(
                await cipher.decrypt(row.encrypted_credentials),
            );
        };

        const refreshUpstream = router([
            {
                match: (url) => url === ANTIGRAVITY_TOKEN_URL,
                respond: (_url, init) => {
                    const body = new URLSearchParams(String(init?.body));
                    expect(body.get("grant_type")).toBe("refresh_token");
                    expect(body.get("refresh_token")).toBe("google-refresh-token");
                    return Response.json({
                        access_token: "access-2-fresh",
                        expires_in: 3600,
                    });
                },
            },
            {
                match: (url) => url === codeAssistRequestUrl(false),
                respond: (_url, init) => {
                    expect(new Headers(init?.headers).get("Authorization")).toBe(
                        "Bearer access-2-fresh",
                    );
                    return Response.json({
                        response: {
                            candidates: [
                                {
                                    content: {
                                        parts: [
                                            {
                                                text: "served after automatic refresh",
                                            },
                                        ],
                                        role: "model",
                                    },
                                    finishReason: "STOP",
                                },
                            ],
                        },
                    });
                },
            },
        ]);

        const secondCall = await handleGatewayRequest(
            gatewayRequest({
                input: "Hi again",
                model: "g/gemini-3.7-flash",
            }),
            bindings,
            "openai_response",
            refreshUpstream,
        );
        expect(secondCall.status).toBe(200);
        const secondBody = (await secondCall.json()) as {
            output: Array<{
                content: Array<{
                    text: string;
                }>;
            }>;
        };
        expect(JSON.stringify(secondBody.output)).toContain("served after automatic refresh");
        const credentials = await readCredentials();
        expect(credentials.accessToken).toBe("access-2-fresh");
        // Google omitted refresh_token on refresh; the original grant must survive.
        expect(credentials.refreshToken).toBe("google-refresh-token");
        expect(credentials.projectId).toBe("managed-project-1");
    });
});
