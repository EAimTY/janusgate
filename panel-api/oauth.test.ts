import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
    ANTIGRAVITY_CLIENT_METADATA,
    ANTIGRAVITY_LOAD_CODE_ASSIST_URL,
    ANTIGRAVITY_ONBOARD_USER_URL,
    ANTIGRAVITY_TOKEN_URL,
} from "/api/adapters/antigravity";
import { OPENAI_AUTHORIZE_URL, OPENAI_TOKEN_URL } from "/api/adapters/openai";
import {
    CredentialsCipher,
    parseAntigravityOAuthCredentials,
    parseOpenAiOAuthCredentials,
} from "/common/credentials";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import { OAUTH_STATE_TTL_SECONDS } from "./config";
import { completeProviderOAuth, startProviderOAuth } from "./oauth";
import { findProviderByName } from "./provider-repository";

const CREDENTIAL_KEY = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const NOW = 1_700_000_000;

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

function encodeBase64Url(value: string): string {
    return btoa(value).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function openAiIdToken(accountId: string): string {
    return `${encodeBase64Url(
        JSON.stringify({
            alg: "none",
        }),
    )}.${encodeBase64Url(
        JSON.stringify({
            chatgpt_account_id: accountId,
        }),
    )}.signature`;
}

describe("OAuth orchestration", () => {
    it("persists PKCE state and completes OpenAI creation atomically", async () => {
        const authorizeUrl = await startProviderOAuth(
            env.DATABASE,
            {
                type: "openai",
            },
            {
                now: NOW,
            },
        );
        const authorize = new URL(authorizeUrl);
        expect(authorize.origin + authorize.pathname).toBe(OPENAI_AUTHORIZE_URL);
        expect(authorize.searchParams.get("code_challenge_method")).toBe("S256");
        const state = authorize.searchParams.get("state");
        expect(state).not.toBeNull();
        if (state === null) throw new Error("state missing");
        const storedState = await env.DATABASE.prepare(
            "SELECT verifier, expires_at FROM oauth_states WHERE state = ?",
        )
            .bind(state)
            .first<{
                verifier: string;
                expires_at: number;
            }>();
        expect(storedState?.expires_at).toBe(NOW + OAUTH_STATE_TTL_SECONDS);
        const verifierDigest = new Uint8Array(
            await crypto.subtle.digest("SHA-256", new TextEncoder().encode(storedState?.verifier)),
        );
        const challenge = btoa(String.fromCharCode(...verifierDigest))
            .replaceAll("+", "-")
            .replaceAll("/", "_")
            .replace(/=+$/, "");
        expect(authorize.searchParams.get("code_challenge")).toBe(challenge);

        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            expect(String(input)).toBe(OPENAI_TOKEN_URL);
            expect(init?.method).toBe("POST");
            const body = new URLSearchParams(String(init?.body));
            expect(body.get("code")).toBe("authorization-code");
            expect(body.get("code_verifier")).toBe(storedState?.verifier);
            return Response.json({
                access_token: "access",
                expires_in: 3600,
                id_token: openAiIdToken("account-1"),
                refresh_token: "refresh",
            });
        });
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await expect(
            completeProviderOAuth(
                env.DATABASE,
                cipher,
                {
                    modelPrefix: "codex/",
                    name: "work",
                    redirectUrl: `http://localhost:1455/auth/callback?code=authorization-code&state=${state}`,
                },
                {
                    fetchUpstream,
                    now: NOW,
                },
            ),
        ).resolves.toBeUndefined();

        const provider = await findProviderByName(env.DATABASE, "work");
        expect(provider).toMatchObject({
            expiresAt: NOW + 3600,
            modelPrefix: "codex/",
            type: "openai_oauth",
        });
        if (provider === null) throw new Error("provider missing");
        expect(
            parseOpenAiOAuthCredentials(await cipher.decrypt(provider.encryptedCredentials)),
        ).toEqual({
            accessToken: "access",
            accountId: "account-1",
            refreshToken: "refresh",
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM oauth_states").first("count"),
        ).resolves.toBe(0);
    });

    it("consumes state but leaves no provider when token exchange fails", async () => {
        const authorizeUrl = await startProviderOAuth(
            env.DATABASE,
            {
                type: "openai",
            },
            {
                now: NOW,
            },
        );
        const state = new URL(authorizeUrl).searchParams.get("state");
        if (state === null) throw new Error("state missing");
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);

        await expect(
            completeProviderOAuth(
                env.DATABASE,
                cipher,
                {
                    name: "failed",
                    redirectUrl: `http://localhost:1455/auth/callback?code=bad&state=${state}`,
                },
                {
                    fetchUpstream: async () =>
                        new Response("denied", {
                            status: 400,
                        }),
                    now: NOW,
                },
            ),
        ).rejects.toMatchObject({
            status: 502,
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM providers").first("count"),
        ).resolves.toBe(0);
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM oauth_states").first("count"),
        ).resolves.toBe(0);
    });

    it("shares the credential lease with reauthentication and preserves old credentials when busy", async () => {
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        const original = await cipher.encrypt({
            accessToken: "old-access",
            accountId: "old-account",
            refreshToken: "old-refresh",
        });
        await env.DATABASE.prepare(
            `INSERT INTO providers (
                name, model_prefix, type, protocol, base_url, encrypted_credentials,
                expires_at, refresh_started_at, enabled
            ) VALUES ('work', '', 'openai_oauth', NULL, NULL, ?, ?, NULL, 1)`,
        )
            .bind(original, NOW + 100)
            .run();
        const authorizeUrl = await startProviderOAuth(
            env.DATABASE,
            {
                providerName: "work",
                type: "openai",
            },
            {
                now: NOW,
            },
        );
        const state = new URL(authorizeUrl).searchParams.get("state");
        if (state === null) throw new Error("state missing");
        await env.DATABASE.prepare(
            "UPDATE providers SET refresh_started_at = ? WHERE name = 'work'",
        )
            .bind(NOW)
            .run();

        await expect(
            completeProviderOAuth(
                env.DATABASE,
                cipher,
                {
                    redirectUrl: `http://localhost:1455/auth/callback?code=new&state=${state}`,
                },
                {
                    fetchUpstream: async () =>
                        Response.json({
                            access_token: "new-access",
                            expires_in: 3600,
                            id_token: openAiIdToken("new-account"),
                            refresh_token: "new-refresh",
                        }),
                    now: NOW,
                },
            ),
        ).rejects.toMatchObject({
            status: 409,
        });
        const provider = await findProviderByName(env.DATABASE, "work");
        if (provider === null) throw new Error("provider missing");
        expect(
            parseOpenAiOAuthCredentials(await cipher.decrypt(provider.encryptedCredentials)),
        ).toEqual({
            accessToken: "old-access",
            accountId: "old-account",
            refreshToken: "old-refresh",
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM oauth_states").first("count"),
        ).resolves.toBe(0);
    });

    it("onboards an unprovisioned Antigravity account and stores its resolved project", async () => {
        const authorizeUrl = await startProviderOAuth(
            env.DATABASE,
            {
                type: "antigravity",
            },
            {
                now: NOW,
            },
        );
        const state = new URL(authorizeUrl).searchParams.get("state");
        if (state === null) throw new Error("state missing");
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            const url = String(input);
            if (url === ANTIGRAVITY_TOKEN_URL) {
                return Response.json({
                    access_token: "google-access",
                    expires_in: 1800,
                    refresh_token: "google-refresh",
                });
            }
            if (url === ANTIGRAVITY_LOAD_CODE_ASSIST_URL) {
                const expectedMetadata = {
                    ideType: "ANTIGRAVITY",
                    platform: "PLATFORM_UNSPECIFIED",
                    pluginType: "GEMINI",
                };
                expect(JSON.parse(String(init?.body))).toEqual({
                    metadata: expectedMetadata,
                });
                expect(new Headers(init?.headers).get("Client-Metadata")).toBe(
                    ANTIGRAVITY_CLIENT_METADATA,
                );
                expect(JSON.parse(ANTIGRAVITY_CLIENT_METADATA)).toEqual(expectedMetadata);
                return Response.json({
                    allowedTiers: [
                        {
                            id: "free-tier",
                            isDefault: true,
                        },
                    ],
                });
            }
            if (url === ANTIGRAVITY_ONBOARD_USER_URL) {
                expect(JSON.parse(String(init?.body))).toMatchObject({
                    tierId: "free-tier",
                });
                return Response.json({
                    done: true,
                    response: {
                        cloudaicompanionProject: {
                            id: "project-1",
                        },
                    },
                });
            }
            throw new Error(`Unexpected fetch: ${url}`);
        });
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await completeProviderOAuth(
            env.DATABASE,
            cipher,
            {
                redirectUrl: `http://localhost:51121/oauth-callback?code=code&state=${state}`,
            },
            {
                fetchUpstream,
                now: NOW,
            },
        );

        const provider = await findProviderByName(env.DATABASE, "antigravity-1");
        expect(provider).toMatchObject({
            modelPrefix: "antigravity-1",
            type: "antigravity_oauth",
        });
        if (provider === null) throw new Error("provider missing");
        expect(
            parseAntigravityOAuthCredentials(await cipher.decrypt(provider.encryptedCredentials)),
        ).toEqual({
            accessToken: "google-access",
            projectId: "project-1",
            refreshToken: "google-refresh",
        });
    });

    it("allows only one provider write when the same state completes concurrently", async () => {
        const authorizeUrl = await startProviderOAuth(
            env.DATABASE,
            {
                type: "openai",
            },
            {
                now: NOW,
            },
        );
        const state = new URL(authorizeUrl).searchParams.get("state");
        if (state === null) throw new Error("state missing");
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        let exchangeCount = 0;
        let releaseExchanges: (() => void) | undefined;
        const exchangesReady = new Promise<void>((resolve) => {
            releaseExchanges = resolve;
        });
        const fetchUpstream: typeof fetch = async () => {
            exchangeCount += 1;
            if (exchangeCount === 2) releaseExchanges?.();
            await exchangesReady;
            return Response.json({
                access_token: "access",
                expires_in: 3600,
                id_token: openAiIdToken("account"),
                refresh_token: "refresh",
            });
        };
        const complete = (name: string): Promise<void> =>
            completeProviderOAuth(
                env.DATABASE,
                cipher,
                {
                    name,
                    redirectUrl: `http://localhost:1455/auth/callback?code=code&state=${state}`,
                },
                {
                    fetchUpstream,
                    now: NOW,
                },
            );

        const results = await Promise.allSettled([
            complete("first"),
            complete("second"),
        ]);
        expect(results.filter((result) => result.status === "fulfilled")).toHaveLength(1);
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM providers").first("count"),
        ).resolves.toBe(1);
    });
});
