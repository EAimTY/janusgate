import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import app from "./index";

const API_TOKEN = "api-secret";
const API_TOKEN_HASH = "014c243ff960e87afc8482648f41e2084dce765aa062dcdcbf4e0e43c4db8a41";
const PANEL_TOKEN = "panel-secret";
const PANEL_TOKEN_HASH = "14c660b289912fe36e4caca8eb372106f19a9ed9d37adfd03c504648faa4b5e4";
const bindings: Env = {
    API_KEY_HASH: API_TOKEN_HASH,
    CREDENTIALS_AES_KEY: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    DATABASE: env.DATABASE,
    PANEL_TOKEN_HASH: PANEL_TOKEN_HASH,
};

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

describe("Worker route aggregation", () => {
    it("mounts the authenticated OpenAI-compatible model list at its normative path", async () => {
        const provider = await env.DATABASE.prepare(
            `INSERT INTO providers (
                name, model_prefix, type, protocol, base_url, encrypted_credentials,
                expires_at, refresh_started_at, enabled
            ) VALUES ('google', '', 'antigravity_oauth', NULL, NULL, ?, 2000000000, NULL, 1)`,
        )
            .bind(
                new Uint8Array([
                    1,
                ]),
            )
            .run();
        await env.DATABASE.prepare(
            `INSERT INTO models (
                name, provider_id, upstream_name, thinking_levels, enabled
            ) VALUES ('gemini', ?, 'gemini-wire', NULL, 1)`,
        )
            .bind(provider.meta.last_row_id)
            .run();

        const response = await app.request(
            "https://gateway.test/api/openai-v1/models",
            {
                headers: {
                    Authorization: `Bearer ${API_TOKEN}`,
                },
            },
            bindings,
        );
        expect(response.status).toBe(200);
        expect(await response.json()).toEqual({
            data: [
                {
                    created: 0,
                    id: "gemini",
                    object: "model",
                    owned_by: "antigravity",
                },
            ],
            object: "list",
        });
    });

    it("authenticates API-prefixed unknown routes and keeps them out of the SPA", async () => {
        const unauthorized = await app.request("https://gateway.test/api/unknown", {}, bindings);
        expect(unauthorized.status).toBe(401);

        const response = await app.request(
            "https://gateway.test/api/unknown",
            {
                headers: {
                    Authorization: `Bearer ${API_TOKEN}`,
                },
            },
            bindings,
        );
        expect(response.status).toBe(404);
        expect(await response.json()).toEqual({
            error: {
                code: "not_found",
                message: "The requested API route was not found",
                type: "not_found_error",
            },
        });
    });

    it("answers unknown panel routes through the panel middleware's auth and CSRF gates", async () => {
        const unauthorized = await app.request(
            "https://gateway.test/panel-api/unknown",
            {},
            bindings,
        );
        expect(unauthorized.status).toBe(401);

        const authorized = await app.request(
            "https://gateway.test/panel-api/unknown",
            {
                headers: {
                    Authorization: `Bearer ${PANEL_TOKEN}`,
                    Origin: "https://gateway.test",
                },
            },
            bindings,
        );
        expect(authorized.status).toBe(404);
        expect(await authorized.json()).toEqual({
            error: "Not found",
        });
    });
});
