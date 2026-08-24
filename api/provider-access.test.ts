import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CredentialsCipher, parseOpenAiOAuthCredentials } from "/common/credentials";
import type { StoredProvider } from "/common/database";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import { ensureFreshProviderAccess } from "./provider-access";

const AES_KEY = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

async function seedExpiredOpenAiProvider(now: number): Promise<{
    cipher: CredentialsCipher;
    provider: StoredProvider;
}> {
    const cipher = await CredentialsCipher.create(AES_KEY);
    const encrypted = await cipher.encrypt({
        accessToken: "old-access",
        accountId: "account",
        refreshToken: "old-refresh",
    });
    const result = await env.DATABASE.prepare(
        `INSERT INTO providers (
            name, model_prefix, type, protocol, base_url, encrypted_credentials,
            expires_at, refresh_started_at, enabled
        ) VALUES ('openai', '', 'openai_oauth', NULL, NULL, ?, ?, NULL, 1)`,
    )
        .bind(encrypted, now - 1)
        .run();
    const encryptedBuffer = new ArrayBuffer(encrypted.byteLength);
    new Uint8Array(encryptedBuffer).set(encrypted);
    return {
        cipher,
        provider: {
            baseUrl: null,
            encryptedCredentials: encryptedBuffer,
            expiresAt: now - 1,
            id: result.meta.last_row_id,
            isEnabled: true,
            modelPrefix: "",
            name: "openai",
            protocol: null,
            refreshStartedAt: null,
            type: "openai_oauth",
        },
    };
}

describe("OAuth credential refresh leasing", () => {
    it("allows one refresh while a lease-busy reader uses the current-best token", async () => {
        const now = 1_800_000_000;
        const { cipher, provider } = await seedExpiredOpenAiProvider(now);
        let releaseRefresh: (() => void) | undefined;
        let announceRefresh: (() => void) | undefined;
        const refreshStarted = new Promise<void>((resolve) => {
            announceRefresh = resolve;
        });
        const refreshMayFinish = new Promise<void>((resolve) => {
            releaseRefresh = resolve;
        });
        const fetchUpstream = vi.fn<typeof fetch>(async () => {
            announceRefresh?.();
            await refreshMayFinish;
            return Response.json({
                access_token: "new-access",
                expires_in: 3600,
                refresh_token: "new-refresh",
            });
        });

        const owner = ensureFreshProviderAccess(env.DATABASE, provider, cipher, {
            fetchUpstream,
            now,
        });
        await refreshStarted;
        const busyReader = await ensureFreshProviderAccess(env.DATABASE, provider, cipher, {
            fetchUpstream,
            now,
        });
        expect(busyReader.credentials.accessToken).toBe("old-access");
        expect(fetchUpstream).toHaveBeenCalledOnce();

        releaseRefresh?.();
        const refreshed = await owner;
        expect(refreshed.credentials.accessToken).toBe("new-access");
        const row = await env.DATABASE.prepare(
            "SELECT encrypted_credentials, expires_at, refresh_started_at FROM providers WHERE id = ?",
        )
            .bind(provider.id)
            .first<{
                encrypted_credentials: ArrayBuffer;
                expires_at: number;
                refresh_started_at: number | null;
            }>();
        expect(row?.expires_at).toBe(now + 3600);
        expect(row?.refresh_started_at).toBeNull();
        expect(
            parseOpenAiOAuthCredentials(
                await cipher.decrypt(row?.encrypted_credentials ?? new ArrayBuffer(0)),
            ),
        ).toMatchObject({
            accessToken: "new-access",
            refreshToken: "new-refresh",
        });
    });

    it("makes a forced refresh fail immediately when another mutation owns the lease", async () => {
        const now = 1_800_000_000;
        const { cipher, provider } = await seedExpiredOpenAiProvider(now);
        await env.DATABASE.prepare("UPDATE providers SET refresh_started_at = ? WHERE id = ?")
            .bind(now, provider.id)
            .run();
        await expect(
            ensureFreshProviderAccess(env.DATABASE, provider, cipher, {
                isForced: true,
                now,
            }),
        ).rejects.toMatchObject({
            code: "upstream_auth_rejected",
            status: 502,
        });
    });

    it("clears its lease after an identity-provider failure", async () => {
        const now = 1_800_000_000;
        const { cipher, provider } = await seedExpiredOpenAiProvider(now);
        await expect(
            ensureFreshProviderAccess(env.DATABASE, provider, cipher, {
                fetchUpstream: async () =>
                    new Response("rejected", {
                        status: 400,
                    }),
                now,
            }),
        ).rejects.toMatchObject({
            code: "upstream_failure",
            status: 502,
        });
        const lease = await env.DATABASE.prepare(
            "SELECT refresh_started_at FROM providers WHERE id = ?",
        )
            .bind(provider.id)
            .first<{
                refresh_started_at: number | null;
            }>();
        expect(lease?.refresh_started_at).toBeNull();
    });
});
