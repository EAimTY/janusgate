import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CredentialsCipher } from "/common/credentials";
import {
    ANTIGRAVITY_API_CLIENT,
    ANTIGRAVITY_HEADER_CLIENT_METADATA,
    ANTIGRAVITY_USER_AGENT,
    codeAssistRequestUrl,
} from "/providers/antigravity";
import { OPENAI_CODEX_RESPONSES_URL, OPENAI_TOKEN_URL } from "/providers/openai";
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
    protocol: "openai_chat_completion" | "openai_response" | "gemini_generate_content",
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

async function seedAntigravityProvider(upstreamName = "gemini-wire"): Promise<void> {
    const cipher = await CredentialsCipher.create(AES_KEY);
    const encrypted = await cipher.encrypt({
        accessToken: "google-access",
        projectId: "google-project",
        refreshToken: "google-refresh",
    });
    const provider = await env.DATABASE.prepare(
        `INSERT INTO providers (
            name, model_prefix, type, protocol, base_url, encrypted_credentials,
            expires_at, refresh_started_at, enabled
        ) VALUES ('google', '', 'antigravity_oauth', NULL, NULL, ?, 2000000000, NULL, 1)`,
    )
        .bind(encrypted)
        .run();
    await env.DATABASE.prepare(
        `INSERT INTO models (
            name, provider_id, upstream_name, thinking_levels, enabled
        ) VALUES ('antigravity-model', ?, ?, NULL, 1)`,
    )
        .bind(provider.meta.last_row_id, upstreamName)
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
        await seedCompatibleProvider("openai_response");
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
                    effort: "xhigh",
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
            "openai_response",
            fetchUpstream,
        );

        expect(response.status).toBe(201);
        expect(await response.text()).toBe(exactBody);
        expect(fetchUpstream).toHaveBeenCalledOnce();
    });

    it("translates a non-streaming Responses upstream into Chat Completions", async () => {
        await seedCompatibleProvider("openai_response");
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
            "openai_chat_completion",
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
        await seedCompatibleProvider("openai_response");
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
            "openai_chat_completion",
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

    it("speaks Gemini GenerateContent to API-key upstreams and maps effort onto thinkingConfig", async () => {
        await seedCompatibleProvider("gemini_generate_content");
        const fetchUpstream = vi.fn<typeof fetch>(async (_input, init) => {
            expect(String(_input)).toBe(
                "https://upstream.test/v1/models/wire-model:generateContent",
            );
            const headers = new Headers(init?.headers);
            expect(headers.get("x-goog-api-key")).toBe("upstream-key");
            expect(headers.get("Authorization")).toBeNull();
            expect(JSON.parse(String(init?.body))).toMatchObject({
                contents: [
                    {
                        role: "user",
                        parts: [
                            {
                                text: "hello",
                            },
                        ],
                    },
                ],
                generationConfig: {
                    thinkingConfig: {
                        includeThoughts: true,
                        thinkingLevel: "high",
                    },
                },
            });
            return Response.json({
                candidates: [
                    {
                        content: {
                            parts: [
                                {
                                    text: "answer",
                                },
                            ],
                        },
                        finishReason: "STOP",
                    },
                ],
                usageMetadata: {
                    promptTokenCount: 2,
                    candidatesTokenCount: 1,
                    totalTokenCount: 3,
                },
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest("chat/completions", {
                messages: [
                    {
                        content: "hello",
                        role: "user",
                    },
                ],
                model: "public-model",
                reasoning_effort: "xhigh",
            }),
            bindings,
            "openai_chat_completion",
            fetchUpstream,
        );

        expect(response.status).toBe(200);
        const result = await response.json<{
            choices: Array<{
                message: {
                    content: string;
                };
            }>;
            usage: {
                total_tokens: number;
            };
        }>();
        expect(result.choices[0]?.message.content).toBe("answer");
        expect(result.usage.total_tokens).toBe(3);
    });

    it("uses the Antigravity generation endpoint, runtime model, and agent envelope", async () => {
        await seedAntigravityProvider("gemini-3.7-flash");
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            expect(String(input)).toBe(codeAssistRequestUrl(false));
            const headers = new Headers(init?.headers);
            expect(headers.get("Authorization")).toBe("Bearer google-access");
            expect(headers.get("User-Agent")).toBe(ANTIGRAVITY_USER_AGENT);
            expect(headers.get("X-Goog-Api-Client")).toBe(ANTIGRAVITY_API_CLIENT);
            expect(headers.get("Client-Metadata")).toBe(ANTIGRAVITY_HEADER_CLIENT_METADATA);
            expect(headers.get("Accept")).toBe("application/json");
            const sent = JSON.parse(String(init?.body)) as Record<string, unknown>;
            expect(sent).toMatchObject({
                model: "gemini-3.7-flash-tiered",
                project: "google-project",
                request: {
                    contents: [
                        {
                            parts: [
                                {
                                    text: "Hello",
                                },
                            ],
                            role: "user",
                        },
                    ],
                    generationConfig: {
                        maxOutputTokens: 65536,
                        thinkingConfig: {
                            thinkingLevel: "LOW",
                        },
                    },
                    systemInstruction: {
                        parts: [
                            {
                                text: "Follow the application rules.",
                            },
                        ],
                        role: "user",
                    },
                },
                requestType: "agent",
                userAgent: "antigravity",
            });
            expect(sent["requestId"]).toEqual(expect.stringMatching(/^agent-/));
            return Response.json({
                response: {
                    candidates: [
                        {
                            content: {
                                parts: [
                                    {
                                        text: "answer",
                                    },
                                ],
                                role: "model",
                            },
                            finishReason: "STOP",
                        },
                    ],
                },
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: [
                    {
                        content: "Follow the application rules.",
                        role: "developer",
                        type: "message",
                    },
                    {
                        content: "Hello",
                        role: "user",
                        type: "message",
                    },
                ],
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );

        expect(response.status).toBe(200);
        expect(await response.json()).toMatchObject({
            model: "antigravity-model",
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
        });
        expect(fetchUpstream).toHaveBeenCalledOnce();
    });

    it("sends the Claude interleaved-thinking header through Antigravity", async () => {
        await seedAntigravityProvider("claude-sonnet-4-6");
        const fetchUpstream = vi.fn<typeof fetch>(async (_input, init) => {
            expect(new Headers(init?.headers).get("anthropic-beta")).toBe(
                "interleaved-thinking-2025-05-14",
            );
            expect(JSON.parse(String(init?.body))).toMatchObject({
                model: "claude-sonnet-4-6",
                request: {
                    generationConfig: {
                        maxOutputTokens: 64000,
                    },
                },
            });
            return Response.json({
                response: {
                    candidates: [
                        {
                            content: {
                                parts: [
                                    {
                                        text: "answer",
                                    },
                                ],
                            },
                            finishReason: "STOP",
                        },
                    ],
                },
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "Hello",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );

        expect(response.status).toBe(200);
        expect(fetchUpstream).toHaveBeenCalledOnce();
    });

    it("reports a sanitized OAuth upstream failure category without exposing its body", async () => {
        await seedAntigravityProvider();
        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "Hello",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            async () =>
                new Response('{"error":{"message":"sensitive project detail"}}', {
                    status: 403,
                }),
        );

        expect(response.status).toBe(502);
        expect(await response.json()).toEqual({
            error: {
                code: "upstream_failure",
                message: "The upstream denied access to this OAuth account",
                type: "upstream_error",
            },
        });
    });

    it("surfaces only bounded safe fields from OAuth upstream errors", async () => {
        await seedAntigravityProvider();
        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "Hello",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            async () =>
                Response.json(
                    {
                        error: {
                            message:
                                'Invalid JSON payload. Unknown name "badField" at project-secret',
                            status: "INVALID_ARGUMENT",
                        },
                    },
                    {
                        status: 400,
                    },
                ),
        );

        expect(await response.json()).toEqual({
            error: {
                code: "upstream_failure",
                message:
                    "The upstream rejected the translated request (INVALID_ARGUMENT, unsupported field badField)",
                type: "upstream_error",
            },
        });
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
            "openai_response",
            fetchUpstream,
        );
        expect(response.status).toBe(200);
        expect(await response.text()).toBe('{"id":"success"}');
        expect(codexRequestCount).toBe(2);
        expect(fetchUpstream).toHaveBeenCalledTimes(3);
    });

    it("translates Chat Completions requests into the Antigravity agent envelope and back", async () => {
        await seedAntigravityProvider("gemini-3.7-flash");
        const fetchUpstream = vi.fn<typeof fetch>(async (_input, init) => {
            const headers = new Headers(init?.headers);
            expect(headers.get("Accept")).toBe("application/json");
            const sent = JSON.parse(String(init?.body)) as Record<string, unknown>;
            expect(sent).toMatchObject({
                model: "gemini-3.7-flash-tiered",
                project: "google-project",
                requestType: "agent",
                request: {
                    contents: [
                        {
                            parts: [
                                {
                                    text: "Hello",
                                },
                            ],
                            role: "user",
                        },
                    ],
                    generationConfig: {
                        thinkingConfig: {
                            thinkingLevel: "LOW",
                        },
                    },
                },
            });
            return Response.json({
                response: {
                    candidates: [
                        {
                            content: {
                                parts: [
                                    {
                                        text: "cc answer",
                                    },
                                ],
                                role: "model",
                            },
                            finishReason: "STOP",
                        },
                    ],
                    usageMetadata: {
                        promptTokenCount: 3,
                        candidatesTokenCount: 2,
                        totalTokenCount: 5,
                    },
                },
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest("chat/completions", {
                messages: [
                    {
                        content: "Hello",
                        role: "user",
                    },
                ],
                model: "antigravity-model",
            }),
            bindings,
            "openai_chat_completion",
            fetchUpstream,
        );

        expect(response.status).toBe(200);
        expect(await response.json()).toMatchObject({
            choices: [
                {
                    finish_reason: "stop",
                    message: {
                        content: "cc answer",
                        role: "assistant",
                    },
                },
            ],
            model: "antigravity-model",
            object: "chat.completion",
            usage: {
                total_tokens: 5,
            },
        });
    });

    it("maps an unreachable upstream to a 502 upstream_error instead of an internal error", async () => {
        await seedAntigravityProvider();
        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "Hello",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            async () => {
                throw new TypeError("fetch failed: connection reset");
            },
        );
        expect(response.status).toBe(502);
        expect(await response.json()).toMatchObject({
            error: {
                code: "upstream_failure",
                type: "upstream_error",
            },
        });
    });

    it("makes disabled models indistinguishable from missing models", async () => {
        await seedCompatibleProvider("openai_response", false);
        const fetchUpstream = vi.fn<typeof fetch>();
        const response = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "hello",
                model: "public-model",
            }),
            bindings,
            "openai_response",
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

    it("rejects n greater than one before any upstream call", async () => {
        await seedCompatibleProvider("openai_chat_completion");
        const fetchUpstream = vi.fn<typeof fetch>();
        const response = await handleGatewayRequest(
            gatewayRequest("chat/completions", {
                messages: [
                    {
                        content: "hello",
                        role: "user",
                    },
                ],
                model: "public-model",
                n: 2,
            }),
            bindings,
            "openai_chat_completion",
            fetchUpstream,
        );
        expect(response.status).toBe(400);
        expect(await response.json()).toMatchObject({
            error: {
                code: "unsupported_parameter",
                type: "invalid_request_error",
            },
        });
        expect(fetchUpstream).not.toHaveBeenCalled();
    });

    it("rejects malformed reasoning effort before any upstream call", async () => {
        await seedCompatibleProvider("openai_response");
        const fetchUpstream = vi.fn<typeof fetch>();
        const responsesReject = await handleGatewayRequest(
            gatewayRequest("responses", {
                input: "hello",
                model: "public-model",
                reasoning: {
                    effort: 3,
                },
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );
        expect(responsesReject.status).toBe(400);
        expect(await responsesReject.json()).toMatchObject({
            error: {
                code: "unsupported_parameter",
                type: "invalid_request_error",
            },
        });
        const chatReject = await handleGatewayRequest(
            gatewayRequest("chat/completions", {
                messages: [
                    {
                        content: "hello",
                        role: "user",
                    },
                ],
                model: "public-model",
                reasoning_effort: "",
            }),
            bindings,
            "openai_chat_completion",
            fetchUpstream,
        );
        expect(chatReject.status).toBe(400);
        expect(await chatReject.json()).toMatchObject({
            error: {
                code: "unsupported_parameter",
                type: "invalid_request_error",
            },
        });
        expect(fetchUpstream).not.toHaveBeenCalled();
    });
});
