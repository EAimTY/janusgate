import { assertCredentialsMatchProvider, type CredentialsCipher } from "/common/credentials";
import {
    mapProviderRow,
    PROVIDER_COLUMNS,
    type ProviderRow,
    type StoredProvider,
} from "/common/database";
import type { AntigravityOAuthCredentials, OpenAiOAuthCredentials } from "/common/domain";
import { GatewayError } from "/common/errors";
import { requireJsonObject, requireStringField, unixSeconds } from "/common/validation";
import {
    ANTIGRAVITY_CLIENT_ID,
    ANTIGRAVITY_CLIENT_SECRET,
    ANTIGRAVITY_TOKEN_URL,
} from "./adapters/antigravity";
import { OPENAI_OAUTH_CLIENT_ID, OPENAI_TOKEN_URL } from "./adapters/openai";
import { REFRESH_LEASE_SECONDS, TOKEN_REFRESH_MARGIN_SECONDS } from "./config";

export type OAuthCredentials = AntigravityOAuthCredentials | OpenAiOAuthCredentials;

export interface ProviderAccess {
    provider: StoredProvider;
    credentials: OAuthCredentials;
}

interface RefreshedToken {
    accessToken: string;
    refreshToken: string;
    expiresAt: number;
}

async function loadProvider(database: D1Database, id: number): Promise<StoredProvider> {
    const row = await database
        .prepare(`SELECT ${PROVIDER_COLUMNS} FROM providers WHERE id = ?`)
        .bind(id)
        .first<ProviderRow>();
    if (row === null) throw new Error("Provider disappeared during credential refresh");
    return mapProviderRow(row);
}

async function decryptOAuthCredentials(
    provider: StoredProvider,
    cipher: CredentialsCipher,
): Promise<OAuthCredentials> {
    const decrypted = await cipher.decrypt(provider.encryptedCredentials);
    if (provider.type === "openai_oauth") {
        return assertCredentialsMatchProvider("openai_oauth", decrypted);
    }
    if (provider.type === "antigravity_oauth") {
        return assertCredentialsMatchProvider("antigravity_oauth", decrypted);
    }
    throw new Error("Static-key providers do not have OAuth access tokens");
}

function parseRefreshResponse(
    value: unknown,
    previousRefreshToken: string,
    now: number,
): RefreshedToken {
    const object = requireJsonObject(value, "token refresh response");
    const accessToken = requireStringField(object, "access_token");
    const expiresIn = object["expires_in"];
    if (typeof expiresIn !== "number") {
        throw new Error("Token refresh response expires_in must be a number");
    }
    const refreshTokenValue = object["refresh_token"];
    if (refreshTokenValue !== undefined && typeof refreshTokenValue !== "string") {
        throw new Error("Token refresh response refresh_token must be a string");
    }
    return {
        accessToken,
        expiresAt: now + expiresIn,
        refreshToken: refreshTokenValue ?? previousRefreshToken,
    };
}

async function requestRefreshedToken(
    provider: StoredProvider,
    credentials: OAuthCredentials,
    now: number,
    fetchUpstream: typeof fetch,
): Promise<RefreshedToken> {
    const parameters = new URLSearchParams({
        client_id:
            provider.type === "openai_oauth" ? OPENAI_OAUTH_CLIENT_ID : ANTIGRAVITY_CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token: credentials.refreshToken,
    });
    const tokenUrl = provider.type === "openai_oauth" ? OPENAI_TOKEN_URL : ANTIGRAVITY_TOKEN_URL;
    if (provider.type === "antigravity_oauth") {
        parameters.set("client_secret", ANTIGRAVITY_CLIENT_SECRET);
    }
    const response = await fetchUpstream(tokenUrl, {
        body: parameters,
        headers: {
            "Content-Type": "application/x-www-form-urlencoded",
        },
        method: "POST",
    });
    if (!response.ok) throw new Error("Identity provider rejected the token refresh");
    return parseRefreshResponse(await response.json(), credentials.refreshToken, now);
}

async function clearLease(
    database: D1Database,
    providerId: number,
    leaseTimestamp: number,
): Promise<void> {
    await database
        .prepare(
            `UPDATE providers SET refresh_started_at = NULL
            WHERE id = ? AND refresh_started_at = ?`,
        )
        .bind(providerId, leaseTimestamp)
        .run();
}

async function acquireLease(
    database: D1Database,
    providerId: number,
    now: number,
): Promise<boolean> {
    const result = await database
        .prepare(
            `UPDATE providers SET refresh_started_at = ?
            WHERE id = ?
                AND (refresh_started_at IS NULL OR refresh_started_at < ?)`,
        )
        .bind(now, providerId, now - REFRESH_LEASE_SECONDS)
        .run();
    return result.meta.changes === 1;
}

function isFresh(provider: StoredProvider, now: number): boolean {
    return provider.expiresAt !== null && provider.expiresAt - now > TOKEN_REFRESH_MARGIN_SECONDS;
}

export async function ensureFreshProviderAccess(
    database: D1Database,
    provider: StoredProvider,
    cipher: CredentialsCipher,
    options: {
        isForced?: boolean;
        now?: number;
        fetchUpstream?: typeof fetch;
    } = {},
): Promise<ProviderAccess> {
    const now = options.now ?? unixSeconds();
    if (!options.isForced && isFresh(provider, now)) {
        return {
            credentials: await decryptOAuthCredentials(provider, cipher),
            provider,
        };
    }

    const hasLease = await acquireLease(database, provider.id, now);
    if (!hasLease) {
        if (options.isForced) {
            throw new GatewayError(
                502,
                "upstream_error",
                "The upstream rejected authentication",
                "upstream_auth_rejected",
            );
        }
        return {
            credentials: await decryptOAuthCredentials(provider, cipher),
            provider,
        };
    }

    let current = await loadProvider(database, provider.id);
    try {
        if (!options.isForced && isFresh(current, now)) {
            await clearLease(database, provider.id, now);
            return {
                credentials: await decryptOAuthCredentials(current, cipher),
                provider: current,
            };
        }
        const credentials = await decryptOAuthCredentials(current, cipher);
        const refreshed = await requestRefreshedToken(
            current,
            credentials,
            now,
            options.fetchUpstream ?? fetch,
        );
        let nextCredentials: OAuthCredentials;
        if (current.type === "openai_oauth" && "accountId" in credentials) {
            nextCredentials = {
                accessToken: refreshed.accessToken,
                accountId: credentials.accountId,
                refreshToken: refreshed.refreshToken,
            };
        } else if (current.type === "antigravity_oauth" && "projectId" in credentials) {
            nextCredentials = {
                accessToken: refreshed.accessToken,
                projectId: credentials.projectId,
                refreshToken: refreshed.refreshToken,
            };
        } else {
            throw new Error("Stored credentials do not match the provider type");
        }
        const encrypted = await cipher.encrypt(nextCredentials);
        const update = await database
            .prepare(
                `UPDATE providers SET
                    encrypted_credentials = ?, expires_at = ?, refresh_started_at = NULL
                WHERE id = ? AND refresh_started_at = ?`,
            )
            .bind(encrypted, refreshed.expiresAt, provider.id, now)
            .run();
        if (update.meta.changes !== 1) {
            current = await loadProvider(database, provider.id);
            return {
                credentials: await decryptOAuthCredentials(current, cipher),
                provider: current,
            };
        }
        current = await loadProvider(database, provider.id);
        return {
            credentials: nextCredentials,
            provider: current,
        };
    } catch {
        await clearLease(database, provider.id, now);
        throw new GatewayError(
            502,
            "upstream_error",
            "The upstream credential refresh failed",
            "upstream_failure",
        );
    }
}
