import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
    clearTestDatabase,
    createTestBindings,
    initializeTestDatabase,
    seedProvider,
    TEST_API_TOKEN,
    TEST_PANEL_TOKEN,
    TEST_PANEL_TOKEN_HASH,
} from "/tests/database";
import app from "./index";

const bindings = createTestBindings(env.DATABASE, {
    panelTokenHash: TEST_PANEL_TOKEN_HASH,
});

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

describe("Worker route aggregation", () => {
    it("mounts the authenticated OpenAI-compatible model list at its normative path", async () => {
        const provider = await seedProvider(env.DATABASE, {
            credentials: new Uint8Array([
                1,
            ]),
            name: "google",
            type: "antigravity_oauth",
        });
        await env.DATABASE.prepare(
            `INSERT INTO models (
                provider_id, upstream_name, name, name_override, enabled_override,
                thinking_levels_override
            ) VALUES (?, 'gemini-wire', 'gemini', NULL, NULL, NULL)`,
        )
            .bind(provider.id)
            .run();

        const response = await app.request(
            "https://gateway.test/api/openai-v1/models",
            {
                headers: {
                    Authorization: `Bearer ${TEST_API_TOKEN}`,
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
                    Authorization: `Bearer ${TEST_API_TOKEN}`,
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
                    Authorization: `Bearer ${TEST_PANEL_TOKEN}`,
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
