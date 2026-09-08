import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { assertCredentialsMatchProvider, CredentialsCipher } from "/common/credentials";
import {
    mapProviderRow,
    PROVIDER_COLUMNS,
    type ProviderRow,
    type StoredProvider,
} from "/common/database";
import { ProviderAccessError } from "/common/errors";
import {
    clearTestDatabase,
    initializeTestDatabase,
    seedProvider,
    TEST_CREDENTIALS_AES_KEY,
} from "/tests/database";
import { ensureFreshProviderAccess } from "./access";

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

async function seedExpiredOpenAiProvider(now: number): Promise<{
    cipher: CredentialsCipher;
    provider: StoredProvider;
}> {
    const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
    const provider = await seedProvider(env.DATABASE, {
        credentials: {
            accessToken: "old-access",
            accountId: "account",
            refreshToken: "old-refresh",
        },
        expiresAt: now - 1,
        name: "openai",
        type: "openai_oauth",
    });
    return {
        cipher,
        provider,
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
            assertCredentialsMatchProvider(
                "openai_oauth",
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
        const forced = ensureFreshProviderAccess(env.DATABASE, provider, cipher, {
            isForced: true,
            now,
        });
        await expect(forced).rejects.toBeInstanceOf(ProviderAccessError);
        await expect(forced).rejects.toMatchObject({
            isAuthenticationRejected: true,
            message: "The upstream rejected authentication",
        });
    });

    it("skips a second identity-provider call when a concurrent worker already rotated the credentials", async () => {
        const now = 1_800_000_000;
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        const provider = await seedProvider(env.DATABASE, {
            credentials: {
                accessToken: "rejected-access",
                accountId: "account",
                refreshToken: "old-refresh",
            },
            expiresAt: now + 120,
            name: "openai",
            type: "openai_oauth",
        });
        const rowBefore = async (): Promise<StoredProvider> => {
            const row = await env.DATABASE.prepare(
                `SELECT ${PROVIDER_COLUMNS} FROM providers WHERE id = ?`,
            )
                .bind(provider.id)
                .first<ProviderRow>();
            if (row === null) throw new Error("provider missing");
            return mapProviderRow(row);
        };

        // Simulate another isolate having refreshed while this request saw a 401: it wrote new
        // credentials together with a fresh expiry in its single guarded update.
        await env.DATABASE.prepare(
            "UPDATE providers SET encrypted_credentials = ?, expires_at = ? WHERE id = ?",
        )
            .bind(
                await cipher.encrypt({
                    accessToken: "rotated-by-other-worker",
                    accountId: "account",
                    refreshToken: "rotated-refresh",
                }),
                now + 3600,
                provider.id,
            )
            .run();

        const fetchUpstream = vi.fn<typeof fetch>();
        const access = await ensureFreshProviderAccess(env.DATABASE, await rowBefore(), cipher, {
            fetchUpstream,
            isForced: true,
            rejectedAccessToken: "rejected-access",
            now,
        });
        expect(access.credentials.accessToken).toBe("rotated-by-other-worker");
        expect(fetchUpstream).not.toHaveBeenCalled();
        const stored = await env.DATABASE.prepare(
            "SELECT refresh_started_at FROM providers WHERE id = ?",
        )
            .bind(provider.id)
            .first<{
                refresh_started_at: number | null;
            }>();
        expect(stored?.refresh_started_at).toBeNull();

        // Same token as the rejected one: the forced refresh must still contact the IdP.
        await env.DATABASE.prepare(
            "UPDATE providers SET refresh_started_at = NULL, expires_at = ? WHERE id = ?",
        )
            .bind(now + 120, provider.id)
            .run();
        const stillFetched = vi.fn<typeof fetch>(async () =>
            Response.json({
                access_token: "brand-new-access",
                expires_in: 3600,
                refresh_token: "brand-new-refresh",
            }),
        );
        const second = await ensureFreshProviderAccess(env.DATABASE, await rowBefore(), cipher, {
            fetchUpstream: stillFetched,
            isForced: true,
            rejectedAccessToken: "rotated-by-other-worker",
            now,
        });
        expect(second.credentials.accessToken).toBe("brand-new-access");
        expect(stillFetched).toHaveBeenCalledOnce();
    });

    it("clears its lease after an identity-provider failure", async () => {
        const now = 1_800_000_000;
        const { cipher, provider } = await seedExpiredOpenAiProvider(now);
        await expect(
            ensureFreshProviderAccess(env.DATABASE, provider, cipher, {
                fetchUpstream: async () =>
                    Response.json(
                        {
                            error: "invalid_grant",
                            error_description: "Token has been expired or revoked.",
                        },
                        {
                            status: 400,
                        },
                    ),
                now,
            }),
        ).rejects.toMatchObject({
            isAuthenticationRejected: false,
            message: "Identity provider rejected the token refresh (invalid_grant)",
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
