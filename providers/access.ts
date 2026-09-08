import { assertCredentialsMatchProvider, type CredentialsCipher } from "/common/credentials";
import { findProviderRow, type StoredProvider } from "/common/database";
import type {
    AntigravityOAuthCredentials,
    OAuthProviderType,
    OpenAiOAuthCredentials,
} from "/common/domain";
import { CredentialDecryptionError, ProviderAccessError } from "/common/errors";
import { boundedUpstreamSignal } from "/common/upstream-signal";
import {
    isJsonObject,
    requireJsonObject,
    requireStringField,
    unixSeconds,
} from "/common/validation";
import { REFRESH_LEASE_SECONDS, TOKEN_REFRESH_MARGIN_SECONDS } from "/config";
import {
    ANTIGRAVITY_CLIENT_ID,
    ANTIGRAVITY_CLIENT_SECRET,
    ANTIGRAVITY_REDIRECT_URI,
    ANTIGRAVITY_TOKEN_URL,
} from "./antigravity";
import { OPENAI_OAUTH_CLIENT_ID, OPENAI_REDIRECT_URI, OPENAI_TOKEN_URL } from "./openai";

export type OAuthCredentials = AntigravityOAuthCredentials | OpenAiOAuthCredentials;

interface ProviderAccess {
    provider: StoredProvider;
    credentials: OAuthCredentials;
}

interface RefreshedToken {
    accessToken: string;
    refreshToken: string;
    expiresAt: number;
}

async function loadProvider(database: D1Database, id: number): Promise<StoredProvider> {
    const provider = await findProviderRow(database, "id", id);
    if (provider === null) throw new Error("Provider disappeared during credential refresh");
    return provider;
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

/** The RFC 6749 grant a token-endpoint POST requests (README §8.3–8.4). */
export type IdentityProviderGrant =
    | {
          readonly code: string;
          readonly codeVerifier: string;
          readonly grantType: "authorization_code";
      }
    | {
          readonly grantType: "refresh_token";
          readonly refreshToken: string;
      };

/**
 * One token-endpoint request shared by both IdP flows: the OAuth code exchange (§8) and the
 * credential refresh (§7). The endpoint, client identity, and secret handling are pinned per
 * provider type by the adapter modules — OpenAI's Codex CLI client is a public PKCE client with
 * no secret, while Antigravity's installed-app client adds its public-by-design `client_secret`.
 */
export interface IdentityProviderTokenRequest {
    readonly body: URLSearchParams;
    readonly headers: Readonly<Record<string, string>>;
    readonly method: "POST";
    readonly tokenUrl: string;
}

/**
 * Builds the per-provider-type token POST for the stored provider kinds that hold an
 * identity-provider token grant (`OAuthProviderType`; README §3.1) — a static API-key provider
 * never touches a token endpoint. Field order is fixed (base fields in the literal below, the
 * Antigravity secret appended last) so every consumer emits byte-identical bodies.
 */
export function identityProviderTokenRequest(
    providerType: OAuthProviderType,
    grant: IdentityProviderGrant,
): IdentityProviderTokenRequest {
    const isOpenAi = providerType === "openai_oauth";
    const parameters =
        grant.grantType === "authorization_code"
            ? new URLSearchParams({
                  client_id: isOpenAi ? OPENAI_OAUTH_CLIENT_ID : ANTIGRAVITY_CLIENT_ID,
                  code: grant.code,
                  code_verifier: grant.codeVerifier,
                  grant_type: "authorization_code",
                  redirect_uri: isOpenAi ? OPENAI_REDIRECT_URI : ANTIGRAVITY_REDIRECT_URI,
              })
            : new URLSearchParams({
                  client_id: isOpenAi ? OPENAI_OAUTH_CLIENT_ID : ANTIGRAVITY_CLIENT_ID,
                  grant_type: "refresh_token",
                  refresh_token: grant.refreshToken,
              });
    if (!isOpenAi) parameters.set("client_secret", ANTIGRAVITY_CLIENT_SECRET);
    return {
        body: parameters,
        headers: {
            "Content-Type": "application/x-www-form-urlencoded",
        },
        method: "POST",
        tokenUrl: isOpenAi ? OPENAI_TOKEN_URL : ANTIGRAVITY_TOKEN_URL,
    };
}

/**
 * RFC 6749 §5.2 token-endpoint failures carry a bounded `error` code (`invalid_grant`,
 * `invalid_client`, …). Surfacing that code is the only safe diagnostic: it tells the admin
 * whether reauthentication is required (`invalid_grant`) or the failure was transient — without
 * exposing token material.
 */
export async function identityProviderErrorCode(response: Response): Promise<string | undefined> {
    try {
        const value: unknown = await response.json();
        if (!isJsonObject(value)) return undefined;
        const error = value["error"];
        if (typeof error === "string" && /^[a-z_]{1,32}$/.test(error)) return error;
    } catch {
        // The body is diagnostics only; an unreadable one leaves the generic message.
    }
    return undefined;
}

async function requestRefreshedToken(
    provider: StoredProvider,
    credentials: OAuthCredentials,
    now: number,
    fetchUpstream: typeof fetch,
    signal?: AbortSignal,
): Promise<RefreshedToken> {
    // Contract violation: decryptOAuthCredentials already rejects static-key providers before a
    // refresh is attempted.
    if (provider.type === "openai_compatible") {
        throw new Error("Static-key providers do not have OAuth access tokens");
    }
    const request = identityProviderTokenRequest(provider.type, {
        grantType: "refresh_token",
        refreshToken: credentials.refreshToken,
    });
    let response: Response;
    try {
        response = await fetchUpstream(request.tokenUrl, {
            body: request.body,
            headers: request.headers,
            method: request.method,
            signal: boundedUpstreamSignal(signal),
        });
    } catch {
        throw new ProviderAccessError("Identity provider token refresh request failed");
    }
    if (!response.ok) {
        const errorCode = await identityProviderErrorCode(response);
        throw new ProviderAccessError(
            errorCode === undefined
                ? "Identity provider rejected the token refresh"
                : `Identity provider rejected the token refresh (${errorCode})`,
            errorCode === undefined
                ? {}
                : {
                      idpErrorCode: errorCode,
                  },
        );
    }
    return parseRefreshResponse(await response.json(), credentials.refreshToken, now);
}

/**
 * The §7 credential-mutation lease: at most one in-flight credential write per provider.
 * Both token refresh and OAuth reauthentication acquire it through the same compare-and-set,
 * so an in-flight refresh and a concurrent reauth are strictly ordered.
 */
export async function acquireCredentialLease(
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

export async function clearCredentialLease(
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

function isFresh(provider: StoredProvider, now: number): boolean {
    return provider.expiresAt !== null && provider.expiresAt - now > TOKEN_REFRESH_MARGIN_SECONDS;
}

export async function ensureFreshProviderAccess(
    database: D1Database,
    provider: StoredProvider,
    cipher: CredentialsCipher,
    options: {
        isForced?: boolean;
        /** Access token the upstream just rejected with 401; enables skip-if-already-rotated. */
        rejectedAccessToken?: string;
        now?: number;
        fetchUpstream?: typeof fetch;
        signal?: AbortSignal | undefined;
    } = {},
): Promise<ProviderAccess> {
    const now = options.now ?? unixSeconds();
    if (!options.isForced && isFresh(provider, now)) {
        return {
            credentials: await decryptOAuthCredentials(provider, cipher),
            provider,
        };
    }

    const hasLease = await acquireCredentialLease(database, provider.id, now);
    if (!hasLease) {
        if (options.isForced) {
            // The lease holder's rotation is the only recovery; the rejected token stands (§7).
            throw new ProviderAccessError("The upstream rejected authentication", {
                isAuthenticationRejected: true,
            });
        }
        return {
            credentials: await decryptOAuthCredentials(provider, cipher),
            provider,
        };
    }

    let current = await loadProvider(database, provider.id);
    try {
        const currentCredentials = await decryptOAuthCredentials(current, cipher);
        if (isFresh(current, now)) {
            // A forced refresh still defers to credentials another worker rotated while we
            // raced for the lease: retrying against them is the correct 401 recovery, and a
            // second IdP call would needlessly consume rotation grace.
            const alreadyRotated =
                options.rejectedAccessToken !== undefined &&
                currentCredentials.accessToken !== options.rejectedAccessToken;
            if (!options.isForced || alreadyRotated) {
                await clearCredentialLease(database, provider.id, now);
                return {
                    credentials: currentCredentials,
                    provider: current,
                };
            }
        }
        const refreshed = await requestRefreshedToken(
            current,
            currentCredentials,
            now,
            options.fetchUpstream ?? fetch,
            options.signal,
        );
        let nextCredentials: OAuthCredentials;
        if (current.type === "openai_oauth" && "accountId" in currentCredentials) {
            nextCredentials = {
                accessToken: refreshed.accessToken,
                accountId: currentCredentials.accountId,
                refreshToken: refreshed.refreshToken,
            };
        } else if (current.type === "antigravity_oauth" && "projectId" in currentCredentials) {
            nextCredentials = {
                accessToken: refreshed.accessToken,
                projectId: currentCredentials.projectId,
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
    } catch (error: unknown) {
        await clearCredentialLease(database, provider.id, now);
        if (error instanceof ProviderAccessError) throw error;
        // A decryption failure is a stored-secret problem, not an access failure: it keeps its
        // own identity so each entry point can give it a distinct surface.
        if (error instanceof CredentialDecryptionError) throw error;
        throw new ProviderAccessError(
            error instanceof Error && error.message.length > 0 && error.message.length <= 200
                ? error.message
                : "The upstream credential refresh failed",
        );
    }
}
