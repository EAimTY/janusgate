import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CredentialsCipher } from "/common/credentials";
import type { AntigravityOAuthCredentials } from "/common/domain";
import {
    ANTIGRAVITY_CLIENT_SECRET,
    ANTIGRAVITY_TOKEN_URL,
    codeAssistRequestUrl,
} from "/providers/antigravity";
import { OPENAI_TOKEN_URL } from "/providers/openai";
import {
    clearTestDatabase,
    createTestBindings,
    initializeTestDatabase,
    seedProvider,
    TEST_API_TOKEN,
    TEST_CREDENTIALS_AES_KEY,
} from "/tests/database";
import { handleGatewayRequest } from "./gateway";

const bindings = createTestBindings(env.DATABASE);

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

async function seedAntigravityProvider(expiresAt: number): Promise<number> {
    const provider = await seedProvider(env.DATABASE, {
        credentials: {
            accessToken: "stale-access",
            projectId: "google-project",
            refreshToken: "google-refresh",
        },
        expiresAt,
        name: "google",
        type: "antigravity_oauth",
    });
    await env.DATABASE.prepare(
        `INSERT INTO models (
            provider_id, upstream_name, name, name_override, enabled_override,
            thinking_levels_override
        ) VALUES (?, 'gemini-3.7-flash', 'antigravity-model', NULL, NULL, NULL)`,
    )
        .bind(provider.id)
        .run();
    return provider.id;
}

function gatewayRequest(body: unknown): Request {
    return new Request("https://gateway.test/api/openai-v1/responses", {
        body: JSON.stringify(body),
        headers: {
            Authorization: `Bearer ${TEST_API_TOKEN}`,
            "Content-Type": "application/json",
        },
        method: "POST",
    });
}

async function storedCredentials(
    providerId: number,
    cipher: CredentialsCipher,
): Promise<
    AntigravityOAuthCredentials & {
        expiresAt: number | null;
        leaseCleared: boolean;
    }
> {
    const row = await env.DATABASE.prepare(
        `SELECT encrypted_credentials, expires_at, refresh_started_at
        FROM providers WHERE id = ?`,
    )
        .bind(providerId)
        .first<{
            encrypted_credentials: ArrayBuffer;
            expires_at: number | null;
            refresh_started_at: number | null;
        }>();
    const credentials = (await cipher.decrypt(
        row?.encrypted_credentials ?? new ArrayBuffer(0),
    )) as AntigravityOAuthCredentials;
    return {
        ...credentials,
        expiresAt: row?.expires_at ?? null,
        leaseCleared: row?.refresh_started_at === null,
    };
}

describe("lazy OAuth token auto-refresh through the gateway", () => {
    it("refreshes an Antigravity access token inside the margin and serves a Responses request without reauthentication", async () => {
        const now = Math.floor(Date.now() / 1000);
        const providerId = await seedAntigravityProvider(now + 60);
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        let codeAssistCalls = 0;
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            const url = String(input);
            if (url === ANTIGRAVITY_TOKEN_URL) {
                const body = String(init?.body);
                expect(body).toContain("grant_type=refresh_token");
                expect(body).toContain("refresh_token=google-refresh");
                expect(body).toContain(encodeURIComponent(ANTIGRAVITY_CLIENT_SECRET));
                return Response.json({
                    access_token: "fresh-access",
                    expires_in: 3600,
                });
            }
            expect(url).toBe(codeAssistRequestUrl(false));
            codeAssistCalls += 1;
            expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer fresh-access");
            return Response.json({
                response: {
                    candidates: [
                        {
                            content: {
                                parts: [
                                    {
                                        text: "auto-refreshed answer",
                                    },
                                ],
                                role: "model",
                            },
                            finishReason: "STOP",
                        },
                    ],
                    usageMetadata: {
                        promptTokenCount: 5,
                        candidatesTokenCount: 3,
                        totalTokenCount: 8,
                    },
                },
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest({
                input: "Hello",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );

        expect(response.status).toBe(200);
        expect(codeAssistCalls).toBe(1);
        await response.body?.cancel();
        const credentials = await storedCredentials(providerId, cipher);
        expect(credentials.accessToken).toBe("fresh-access");
        // Google refresh responses omit refresh_token; the previous one must survive.
        expect(credentials.refreshToken).toBe("google-refresh");
        expect(credentials.projectId).toBe("google-project");
        expect(credentials.expiresAt).toBeGreaterThan(now + 3000);
        expect(credentials.leaseCleared).toBe(true);

        // The next request inside the new freshness window must skip the token endpoint.
        fetchUpstream.mockClear();
        codeAssistCalls = 0;
        const followUp = await handleGatewayRequest(
            gatewayRequest({
                input: "Again",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );
        expect(followUp.status).toBe(200);
        expect(codeAssistCalls).toBe(1);
        for (const call of fetchUpstream.mock.calls)
            expect(String(call[0])).not.toBe(ANTIGRAVITY_TOKEN_URL);
        await followUp.body?.cancel();
    });

    it("streams an Antigravity Responses answer through a mid-flight token refresh", async () => {
        const now = Math.floor(Date.now() / 1000);
        await seedAntigravityProvider(now + 10);
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            if (String(input) === ANTIGRAVITY_TOKEN_URL) {
                return Response.json({
                    access_token: "stream-fresh-access",
                    expires_in: 3600,
                });
            }
            expect(String(init?.body)).toContain('"sessionId":"session-42"');
            const headers = new Headers(init?.headers);
            expect(headers.get("Accept")).toBe("text/event-stream");
            expect(headers.get("Authorization")).toBe("Bearer stream-fresh-access");
            const frames = [
                `data: {"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"hel"}]}}]}}\n\n`,
                `data: {"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"lo"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":2,"totalTokenCount":4}}}\n\n`,
            ];
            const body = new ReadableStream<Uint8Array>({
                start(controller) {
                    for (const frame of frames) controller.enqueue(new TextEncoder().encode(frame));
                    controller.close();
                },
            });
            return new Response(body, {
                headers: {
                    "Content-Type": "text/event-stream",
                },
            });
        });

        const request = gatewayRequest({
            input: "Hello",
            model: "antigravity-model",
            stream: true,
        });
        request.headers.set("Session-Id", "session-42");
        const response = await handleGatewayRequest(
            request,
            bindings,
            "openai_response",
            fetchUpstream,
        );

        expect(response.status).toBe(200);
        const text = await response.text();
        expect(text).toContain("response.output_text.delta");
        expect(text).toContain('"delta":"hel"');
        expect(text).toContain('"delta":"lo"');
        expect(text).toContain("response.completed");
        expect(text).toContain('"total_tokens":4');
        // The terminal completed frame is the last event in the stream.
        expect(text.trimEnd().endsWith("}")).toBe(true);
        expect(text.lastIndexOf("response.completed")).toBeGreaterThan(
            text.lastIndexOf("response.output_text.delta"),
        );
        expect(text.endsWith("\n\n")).toBe(true);
    });

    it("surfaces a permanent refresh rejection as upstream_error and clears the lease", async () => {
        const now = Math.floor(Date.now() / 1000);
        const providerId = await seedAntigravityProvider(now - 1);
        const fetchUpstream = vi.fn<typeof fetch>(async () =>
            Response.json(
                {
                    error: "invalid_grant",
                    error_description: "Token has been expired or revoked.",
                },
                {
                    status: 400,
                },
            ),
        );

        const response = await handleGatewayRequest(
            gatewayRequest({
                input: "Hello",
                model: "antigravity-model",
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );

        expect(response.status).toBe(502);
        expect(await response.json()).toMatchObject({
            error: {
                code: "upstream_failure",
                type: "upstream_error",
            },
        });
        const row = await env.DATABASE.prepare(
            "SELECT expires_at, refresh_started_at FROM providers WHERE id = ?",
        )
            .bind(providerId)
            .first<{
                expires_at: number | null;
                refresh_started_at: number | null;
            }>();
        expect(row?.expires_at).toBe(now - 1);
        expect(row?.refresh_started_at).toBeNull();
    });

    it("lazily refreshes an OpenAI account inside the margin preserving rotation", async () => {
        const now = Math.floor(Date.now() / 1000);
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        const provider = await seedProvider(env.DATABASE, {
            credentials: {
                accessToken: "old-openai-access",
                accountId: "account-1",
                refreshToken: "old-openai-refresh",
            },
            expiresAt: now + 120,
            name: "openai",
            type: "openai_oauth",
        });
        await env.DATABASE.prepare(
            `INSERT INTO models (
                provider_id, upstream_name, name, name_override, enabled_override,
                thinking_levels_override
            ) VALUES (?, 'gpt-5.2', 'codex-model', NULL, NULL, NULL)`,
        )
            .bind(provider.id)
            .run();

        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            const url = String(input);
            if (url === OPENAI_TOKEN_URL) {
                const body = String(init?.body);
                expect(body).toContain("grant_type=refresh_token");
                expect(body).toContain("refresh_token=old-openai-refresh");
                return Response.json({
                    access_token: "new-openai-access",
                    expires_in: 3600,
                    refresh_token: "new-openai-refresh",
                    id_token: `.${btoa(
                        JSON.stringify({
                            chatgpt_account_id: "account-1",
                        }),
                    )}.`,
                });
            }
            return Response.json({
                id: "resp-native",
            });
        });

        const response = await handleGatewayRequest(
            gatewayRequest({
                input: "hi",
                model: "codex-model",
            }),
            bindings,
            "openai_response",
            fetchUpstream,
        );
        expect(response.status).toBe(200);
        expect(await response.text()).toBe('{"id":"resp-native"}');

        const row = await env.DATABASE.prepare(
            "SELECT encrypted_credentials, expires_at FROM providers WHERE id = ?",
        )
            .bind(provider.id)
            .first<{
                encrypted_credentials: ArrayBuffer;
                expires_at: number | null;
            }>();
        const decrypted = (await cipher.decrypt(
            row?.encrypted_credentials ?? new ArrayBuffer(0),
        )) as {
            accessToken: string;
            refreshToken: string;
            accountId: string;
        };
        expect(decrypted.accessToken).toBe("new-openai-access");
        expect(decrypted.refreshToken).toBe("new-openai-refresh");
        expect(decrypted.accountId).toBe("account-1");
        expect(row?.expires_at).toBeGreaterThan(now + 3000);
    });
});
