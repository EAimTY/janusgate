import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OPENAI_TOKEN_URL } from "/api/adapters/openai";
import { isJsonObject } from "/common/validation";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import { createPanelApi } from "./app";

const PANEL_TOKEN = "panel-secret";
const PANEL_TOKEN_HASH = "14c660b289912fe36e4caca8eb372106f19a9ed9d37adfd03c504648faa4b5e4";
const ORIGIN = "https://panel.test";
const bindings: Env = {
    API_KEY_HASH: "",
    CREDENTIALS_AES_KEY: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    DATABASE: env.DATABASE,
    PANEL_TOKEN_HASH,
};

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

function authorizedHeaders(isPost = false): Headers {
    const headers = new Headers({
        Authorization: `Bearer ${PANEL_TOKEN}`,
    });
    if (isPost) {
        headers.set("Content-Type", "application/json");
        headers.set("Origin", ORIGIN);
    }
    return headers;
}

function oauthIdToken(): string {
    const encode = (value: string): string =>
        btoa(value).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
    return `${encode("{}")}.${encode(
        JSON.stringify({
            chatgpt_account_id: "account",
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
                Authorization: `Bearer ${PANEL_TOKEN}`,
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
            protocol: "responses",
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
                    protocol: "chat_completions",
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
                    protocol: "responses",
                    type: "openai_compatible",
                },
            ],
        });

        const discovery = await request(app, "/discover-models", "POST", {
            providerName: "custom",
        });
        expect(await discovery.json()).toEqual({
            created: 2,
            skipped: 0,
            total: 2,
        });

        const savedModel = await request(app, "/save-model", "POST", {
            enabled: true,
            name: "manual",
            providerName: "custom",
            thinkingLevels: [
                "high",
                "low",
            ],
            upstreamName: "wire-manual",
        });
        expect(savedModel.status).toBe(200);
        const savedModelBody: unknown = await savedModel.json();
        if (!isJsonObject(savedModelBody)) throw new Error("invalid saved model response");
        expect(savedModelBody["name"]).toBe("custom/manual");

        const models = await request(app, "/list-models", "GET");
        const modelBody: unknown = await models.json();
        if (!isJsonObject(modelBody) || !Array.isArray(modelBody["models"])) {
            throw new Error("invalid model list response");
        }
        const listedModels = modelBody["models"];
        expect(listedModels).toHaveLength(3);
        expect(listedModels[2]).toMatchObject({
            enabled: true,
            name: "custom/manual",
            providerName: "custom",
            providerType: "openai_compatible",
            thinkingLevels: [
                "low",
                "high",
            ],
            upstreamName: "wire-manual",
        });

        const modelId = savedModelBody["id"];
        if (typeof modelId !== "number") throw new Error("invalid saved model id");
        await request(app, "/save-model", "POST", {
            enabled: false,
            id: modelId,
            providerName: "custom",
            upstreamName: "wire-manual",
        });
        await request(app, "/save-model", "POST", {
            id: modelId,
            providerName: "custom",
            upstreamName: "wire-manual",
        });
        const persistedEnabled = await env.DATABASE.prepare(
            "SELECT enabled FROM models WHERE id = ?",
        )
            .bind(modelId)
            .first<number>("enabled");
        expect(persistedEnabled).toBe(0);

        expect(
            (
                await request(app, "/delete-provider", "POST", {
                    providerName: "custom",
                })
            ).status,
        ).toBe(409);
        for (const model of listedModels) {
            if (!isJsonObject(model) || typeof model["id"] !== "number") {
                throw new Error("invalid listed model");
            }
            await expect(
                request(app, "/delete-model", "POST", {
                    id: model["id"],
                }),
            ).resolves.toMatchObject({
                status: 200,
            });
        }
        expect(
            (
                await request(app, "/delete-provider", "POST", {
                    providerName: "custom",
                })
            ).status,
        ).toBe(200);
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
