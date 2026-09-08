import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeBase64Url } from "/common/base64";
import { isJsonObject } from "/common/validation";
import { OPENAI_TOKEN_URL } from "/providers/openai";
import {
    clearTestDatabase,
    createTestBindings,
    initializeTestDatabase,
    TEST_PANEL_TOKEN,
    TEST_PANEL_TOKEN_HASH,
} from "/tests/database";
import { createPanelApi } from "./app";

const ORIGIN = "https://panel.test";
// API auth is disabled: this suite never exercises the gateway key.
const bindings = createTestBindings(env.DATABASE, {
    apiTokenHash: "",
    panelTokenHash: TEST_PANEL_TOKEN_HASH,
});

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

function authorizedHeaders(isPost = false): Headers {
    const headers = new Headers({
        Authorization: `Bearer ${TEST_PANEL_TOKEN}`,
    });
    if (isPost) {
        headers.set("Content-Type", "application/json");
        headers.set("Origin", ORIGIN);
    }
    return headers;
}

function oauthIdToken(): string {
    // The fixture payloads are pure ASCII, so UTF-8 encoding reproduces the exact bytes the
    // JWT segments carry.
    const encode = (value: string): string => encodeBase64Url(new TextEncoder().encode(value));
    return `${encode("{}")}.${encode(
        JSON.stringify({
            "https://api.openai.com/auth": {
                chatgpt_account_id: "account",
            },
        }),
    )}.x`;
}

async function request(
    app: ReturnType<typeof createPanelApi>,
    path: string,
    method: "GET" | "POST",
    body?: unknown,
    headers = authorizedHeaders(method === "POST"),
): Promise<Response> {
    return app.request(
        `${ORIGIN}${path}`,
        {
            ...(body === undefined
                ? {}
                : {
                      body: JSON.stringify(body),
                  }),
            headers,
            method,
        },
        bindings,
    );
}

describe("panel API", () => {
    it("enforces bearer auth and exact POST Origin before parsing a body", async () => {
        const app = createPanelApi();
        const unauthorized = await app.request(`${ORIGIN}/list-providers`, {}, bindings);
        expect(unauthorized.status).toBe(401);
        expect(await unauthorized.json()).toEqual({
            error: "Unauthorized",
        });

        const missingOrigin = await request(
            app,
            "/save-provider",
            "POST",
            {
                name: "x",
            },
            new Headers({
                Authorization: `Bearer ${TEST_PANEL_TOKEN}`,
                "Content-Type": "application/json",
            }),
        );
        expect(missingOrigin.status).toBe(403);
        const wrongOriginHeaders = authorizedHeaders(true);
        wrongOriginHeaders.set("Origin", "https://attacker.test");
        expect(
            (
                await request(
                    app,
                    "/save-provider",
                    "POST",
                    {
                        name: "x",
                    },
                    wrongOriginHeaders,
                )
            ).status,
        ).toBe(403);

        const invalidBody = await app.request(
            `${ORIGIN}/delete-model`,
            {
                body: "not-json",
                headers: authorizedHeaders(true),
                method: "POST",
            },
            bindings,
        );
        expect(invalidBody.status).toBe(400);
        expect(await invalidBody.json()).toEqual({
            error: "Request body must be a JSON object",
        });
    });

    it("orchestrates provider and model save/list/discover/delete routes", async () => {
        const fetchUpstream = vi.fn<typeof fetch>(async (input) => {
            if (String(input) === "https://upstream.test/v1/models") {
                return Response.json({
                    data: [
                        {
                            id: "wire-a",
                        },
                        {
                            id: "wire-b",
                        },
                    ],
                });
            }
            throw new Error(`Unexpected fetch: ${String(input)}`);
        });
        const app = createPanelApi({
            fetchUpstream,
        });

        const savedProvider = await request(app, "/save-provider", "POST", {
            apiKey: "secret",
            baseUrl: "https://upstream.test/v1/",
            enabled: true,
            modelPrefix: "custom/",
            name: "custom",
            protocol: "openai_response",
            type: "openai_compatible",
        });
        expect(savedProvider.status).toBe(200);
        expect(await savedProvider.json()).toEqual({
            name: "custom",
        });
        expect(
            (
                await request(app, "/save-provider", "POST", {
                    apiKey: "replacement",
                    baseUrl: "https://other.test/v1",
                    name: "custom",
                    protocol: "openai_chat_completion",
                    type: "openai_compatible",
                })
            ).status,
        ).toBe(409);

        const providers = await request(app, "/list-providers", "GET");
        expect(await providers.json()).toEqual({
            providers: [
                {
                    baseUrl: "https://upstream.test/v1",
                    enabled: true,
                    expiresAt: null,
                    modelPrefix: "custom/",
                    name: "custom",
                    protocol: "openai_response",
                    type: "openai_compatible",
                },
            ],
        });

        const discovery = await request(app, "/discover-models", "POST", {
            providerName: "custom",
        });
        expect(await discovery.json()).toEqual({
            created: 2,
            removed: 0,
            skipped: 0,
            total: 2,
        });

        const savedModel = await request(app, "/save-model", "POST", {
            enabledOverride: true,
            nameOverride: "manual",
            providerName: "custom",
            thinkingLevelsOverride: [
                "high",
                "low",
            ],
            upstreamName: "wire-manual",
        });
        expect(savedModel.status).toBe(200);
        expect(await savedModel.json()).toEqual({
            name: "custom/manual",
        });

        const models = await request(app, "/list-models", "GET");
        const modelBody: unknown = await models.json();
        if (!isJsonObject(modelBody) || !Array.isArray(modelBody["models"])) {
            throw new Error("invalid model list response");
        }
        const listedModels = modelBody["models"];
        expect(listedModels).toHaveLength(3);
        expect(listedModels[0]).toEqual({
            enabled: true,
            name: "custom/manual",
            nameOverride: "manual",
            providerName: "custom",
            providerType: "openai_compatible",
            thinkingLevels: [
                "low",
                "high",
            ],
            upstreamName: "wire-manual",
        });

        expect(
            (
                await request(app, "/save-model", "POST", {
                    nameOverride: "wire-a",
                    providerName: "custom",
                    upstreamName: "wire-manual",
                })
            ).status,
        ).toBe(409);

        // save-model is a full-replace upsert on (providerName, upstreamName): a save that omits
        // an override clears it, and a disabled model clears to enabled once more.
        await request(app, "/save-model", "POST", {
            enabledOverride: false,
            providerName: "custom",
            upstreamName: "wire-manual",
        });
        await request(app, "/save-model", "POST", {
            providerName: "custom",
            upstreamName: "wire-manual",
        });
        const persisted = await env.DATABASE.prepare(
            `SELECT name, name_override, enabled_override FROM models
            WHERE provider_id = (SELECT id FROM providers WHERE name = 'custom')
                AND upstream_name = 'wire-manual'`,
        ).first<{
            name: string;
            name_override: string | null;
            enabled_override: number | null;
        }>();
        expect(persisted).toEqual({
            enabled_override: null,
            name: "custom/wire-manual",
            name_override: null,
        });

        expect(
            (
                await request(app, "/delete-model", "POST", {
                    providerName: "custom",
                    upstreamName: "wire-manual",
                })
            ).status,
        ).toBe(200);
        expect(
            (
                await request(app, "/delete-model", "POST", {
                    providerName: "custom",
                    upstreamName: "wire-manual",
                })
            ).status,
        ).toBe(404);

        expect(
            (
                await request(app, "/delete-provider", "POST", {
                    providerName: "custom",
                })
            ).status,
        ).toBe(200);
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM models").first("count"),
        ).resolves.toBe(0);
    });

    it("exposes both OAuth routes and completes a paste-back flow", async () => {
        const fetchUpstream = vi.fn<typeof fetch>(async (input) => {
            if (String(input) !== OPENAI_TOKEN_URL) throw new Error("unexpected OAuth request");
            return Response.json({
                access_token: "access",
                expires_in: 3600,
                id_token: oauthIdToken(),
                refresh_token: "refresh",
            });
        });
        const app = createPanelApi({
            fetchUpstream,
        });
        const start = await request(app, "/start-provider-oauth", "POST", {
            type: "openai",
        });
        expect(start.status).toBe(200);
        const authorizeUrl: unknown = await start.json();
        if (!isJsonObject(authorizeUrl) || typeof authorizeUrl["authorizeUrl"] !== "string") {
            throw new Error("invalid OAuth start response");
        }
        const state = new URL(authorizeUrl["authorizeUrl"]).searchParams.get("state");
        expect(state).not.toBeNull();

        const complete = await request(app, "/complete-provider-oauth", "POST", {
            name: "oauth",
            redirectUrl: `http://localhost:1455/auth/callback?code=code&state=${state}`,
        });
        expect(complete.status).toBe(200);
        expect(await complete.json()).toEqual({
            ok: true,
        });
    });
});
