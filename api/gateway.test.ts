import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OPENAI_CODEX_RESPONSES_URL, OPENAI_TOKEN_URL } from "/api/adapters/openai";
import { CredentialsCipher } from "/common/credentials";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import { handleGatewayRequest } from "./gateway";

const API_TOKEN = "api-secret";
const API_TOKEN_HASH = "014c243ff960e87afc8482648f41e2084dce765aa062dcdcbf4e0e43c4db8a41";
const AES_KEY = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const bindings: Env = {
    API_KEY_HASH: API_TOKEN_HASH,
    CREDENTIALS_AES_KEY: AES_KEY,
    DATABASE: env.DATABASE,
    PANEL_TOKEN_HASH: "",
};

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

async function seedCompatibleProvider(
    protocol: "chat_completions" | "responses",
    isModelEnabled = true,
): Promise<void> {
    const cipher = await CredentialsCipher.create(AES_KEY);
    const encrypted = await cipher.encrypt({
        apiKey: "upstream-key",
    });
    const provider = await env.DATABASE.prepare(
        `INSERT INTO providers (
            name, model_prefix, type, protocol, base_url, encrypted_credentials,
            expires_at, refresh_started_at, enabled
        ) VALUES ('custom', '', 'openai_compatible', ?, 'https://upstream.test/v1', ?, NULL, NULL, 1)`,
    )
        .bind(protocol, encrypted)
        .run();
    await env.DATABASE.prepare(
        `INSERT INTO models (
            name, provider_id, upstream_name, thinking_levels, enabled
        ) VALUES ('public-model', ?, 'wire-model', NULL, ?)`,
    )
        .bind(provider.meta.last_row_id, isModelEnabled ? 1 : 0)
        .run();
}

async function seedOpenAiProvider(): Promise<void> {
    const cipher = await CredentialsCipher.create(AES_KEY);
    const encrypted = await cipher.encrypt({
        accessToken: "old-access",
        accountId: "account",
        refreshToken: "old-refresh",
    });
    const provider = await env.DATABASE.prepare(
        `INSERT INTO providers (
            name, model_prefix, type, protocol, base_url, encrypted_credentials,
            expires_at, refresh_started_at, enabled
        ) VALUES ('openai', '', 'openai_oauth', NULL, NULL, ?, 2000000000, NULL, 1)`,
    )
        .bind(encrypted)
        .run();
    await env.DATABASE.prepare(
        `INSERT INTO models (
            name, provider_id, upstream_name, thinking_levels, enabled
        ) VALUES ('codex', ?, 'gpt-wire', NULL, 1)`,
    )
        .bind(provider.meta.last_row_id)
        .run();
}

function gatewayRequest(path: string, body: unknown): Request {
    return new Request(`https://gateway.test/api/openai-v1/${path}`, {
        body: JSON.stringify(body),
        headers: {
            Authorization: `Bearer ${API_TOKEN}`,
            "Content-Type": "application/json",
        },
        method: "POST",
    });
}

describe("gateway request lifecycle", () => {
    it("preserves native response bytes while substituting only request routing fields", async () => {
        await seedCompatibleProvider("responses");
        const exactBody = '{  "id":"upstream", "unknown": true }\n';
        const fetchUpstream = vi.fn<typeof fetch>(async (_input, init) => {
            const sent: unknown = JSON.parse(String(init?.body));
            expect(sent).toEqual({
                custom_extension: {
                    keep: true,
                },
                input: "hello",
                model: "wire-model",
                reasoning: {
                    effort: "high",
                },
                stream: false,
            });
            expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer upstream-key");
            return new Response(exactBody, {
                headers: {
                    "Content-Type": "application/json",
                },
                status: 201,
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                custom_extension: {
                    keep: true,
                },
                input: "hello",
                model: "public-model",
                reasoning: {
                    effort: "xhigh",
                },
                stream: false,
            }),
            bindings,
            "responses",
            fetchUpstream,
        );

        expect(response.status).toBe(201);
        expect(await response.text()).toBe(exactBody);
        expect(fetchUpstream).toHaveBeenCalledOnce();
    });

    it("translates a non-streaming Responses upstream into Chat Completions", async () => {
        await seedCompatibleProvider("responses");
        const response = await handleGatewayRequest(
            gatewayRequest("chat/completions", {
                messages: [
                    {
                        content: "hello",
                        role: "user",
                    },
                ],
                model: "public-model",
            }),
            bindings,
            "chat_completions",
            async () =>
                Response.json({
                    id: "upstream-id",
                    model: "wire-model",
                    output: [
                        {
                            content: [
                                {
                                    text: "answer",
                                    type: "output_text",
                                },
                            ],
                            role: "assistant",
                            type: "message",
                        },
                    ],
                    status: "completed",
                    usage: {
                        input_tokens: 2,
                        output_tokens: 1,
                        total_tokens: 3,
                    },
                }),
        );

        expect(response.status).toBe(200);
        const result = await response.json<{
            model: string;
            choices: Array<{
                message: {
                    content: string;
                };
            }>;
            usage: {
                prompt_tokens: number;
            };
        }>();
        expect(result.model).toBe("public-model");
        expect(result.choices[0]?.message.content).toBe("answer");
        expect(result.usage.prompt_tokens).toBe(2);
    });

    it("translates streaming events without changing streaming mode", async () => {
        await seedCompatibleProvider("responses");
        const upstreamFrames = [
            'event: response.created\ndata: {"type":"response.created","response":{"id":"upstream"}}\n\n',
            'event: response.output_text.delta\ndata: {"type":"response.output_text.delta","delta":"answer"}\n\n',
            'event: response.completed\ndata: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}}\n\n',
        ].join("");
        const response = await handleGatewayRequest(
            gatewayRequest("chat/completions", {
                messages: [
                    {
                        content: "hello",
                        role: "user",
                    },
                ],
                model: "public-model",
                stream: true,
                stream_options: {
                    include_usage: true,
                },
            }),
            bindings,
            "chat_completions",
            async (_input, init) => {
                expect(JSON.parse(String(init?.body))).toMatchObject({
                    stream: true,
                });
                return new Response(upstreamFrames, {
                    headers: {
                        "Content-Type": "text/event-stream",
                    },
                });
            },
        );
        const output = await response.text();
        expect(output).toContain('"content":"answer"');
        expect(output).toContain('"prompt_tokens":2');
        expect(output.endsWith("data: [DONE]\n\n")).toBe(true);
    });

    it("performs one forced OAuth refresh and exactly one retry after an upstream 401", async () => {
        await seedOpenAiProvider();
        let codexRequestCount = 0;
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            const url = String(input);
            if (url === OPENAI_TOKEN_URL) {
                expect(String(init?.body)).toContain("refresh_token=old-refresh");
                return Response.json({
                    access_token: "new-access",
                    expires_in: 3600,
                    refresh_token: "new-refresh",
                });
            }
            expect(url).toBe(OPENAI_CODEX_RESPONSES_URL);
            codexRequestCount += 1;
            const authorization = new Headers(init?.headers).get("Authorization");
            if (codexRequestCount === 1) {
                expect(authorization).toBe("Bearer old-access");
                return new Response("rejected", {
                    status: 401,
                });
            }
            expect(authorization).toBe("Bearer new-access");
            return new Response('{"id":"success"}', {
                headers: {
                    "Content-Type": "application/json",
                },
            });
        });
        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "hello",
                model: "codex",
            }),
            bindings,
            "responses",
            fetchUpstream,
        );
        expect(response.status).toBe(200);
        expect(await response.text()).toBe('{"id":"success"}');
        expect(codexRequestCount).toBe(2);
        expect(fetchUpstream).toHaveBeenCalledTimes(3);
    });

    it("makes disabled models indistinguishable from missing models", async () => {
        await seedCompatibleProvider("responses", false);
        const fetchUpstream = vi.fn<typeof fetch>();
        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "hello",
                model: "public-model",
            }),
            bindings,
            "responses",
            fetchUpstream,
        );
        expect(response.status).toBe(404);
        expect(await response.json()).toEqual({
            error: {
                code: "model_not_found",
                message: "The requested model was not found",
                type: "not_found_error",
            },
        });
        expect(fetchUpstream).not.toHaveBeenCalled();
    });
});
