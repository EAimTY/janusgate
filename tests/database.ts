import { CredentialsCipher } from "/common/credentials";
import type { StoredProvider } from "/common/database";
import type { ProviderCredentials, ProviderType, UpstreamProtocol } from "/common/domain";
import databaseSchema from "/D1.sql?raw";
import { TEST_CREDENTIALS_AES_KEY } from "./test-secrets";

export const TEST_API_TOKEN = "api-secret";
export const TEST_PANEL_TOKEN = "panel-secret";
// Single-homed in ./test-secrets (shared with the vitest miniflare binding stub); re-exported for suites.
export { TEST_CREDENTIALS_AES_KEY };

const TEST_API_TOKEN_HASH = "014c243ff960e87afc8482648f41e2084dce765aa062dcdcbf4e0e43c4db8a41";
export const TEST_PANEL_TOKEN_HASH =
    "14c660b289912fe36e4caca8eb372106f19a9ed9d37adfd03c504648faa4b5e4";

// A far-future Unix timestamp standing in for "the OAuth token is not expired".
const DEFAULT_OAUTH_EXPIRES_AT = 2_000_000_000;

export interface TestBindingsOverrides {
    readonly panelTokenHash?: string;
}

/** The standard Env shared by suites; only the panel token hash ever differs between suites. */
export function createTestBindings(
    database: D1Database,
    overrides: TestBindingsOverrides = {},
): Env {
    return {
        API_KEY_HASH: TEST_API_TOKEN_HASH,
        CREDENTIALS_AES_KEY: TEST_CREDENTIALS_AES_KEY,
        DATABASE: database,
        PANEL_TOKEN_HASH: overrides.panelTokenHash ?? "",
    };
}

export interface SeedProviderOverrides {
    readonly name: string;
    readonly type: ProviderType;
    /** Plaintext credentials to encrypt, or raw bytes (e.g. undecryptable garbage) to store as-is. */
    readonly credentials: ProviderCredentials | Uint8Array;
    readonly modelPrefix?: string;
    readonly protocol?: UpstreamProtocol;
    readonly baseUrl?: string;
    readonly expiresAt?: number;
    readonly isEnabled?: boolean;
}

/**
 * Inserts one provider row and returns it as the stored representation. A row is consistent by
 * construction: OAuth providers carry a token expiry and no routing fields, an OpenAI-compatible
 * provider the exact converse — the same invariant `assembleProviderSummary` enforces on read.
 */
export async function seedProvider(
    database: D1Database,
    overrides: SeedProviderOverrides,
): Promise<StoredProvider> {
    const name = overrides.name;
    const type = overrides.type;
    const modelPrefix = overrides.modelPrefix ?? "";
    const isEnabled = overrides.isEnabled ?? true;
    const isOAuthType = type !== "openai_compatible";
    const protocol = overrides.protocol ?? (isOAuthType ? null : "openai_response");
    const baseUrl = overrides.baseUrl ?? (isOAuthType ? null : "https://upstream.test/v1");
    const expiresAt = overrides.expiresAt ?? (isOAuthType ? DEFAULT_OAUTH_EXPIRES_AT : null);
    const encrypted =
        overrides.credentials instanceof Uint8Array
            ? overrides.credentials
            : await (await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY)).encrypt(
                  overrides.credentials,
              );

    const result = await database
        .prepare(
            `INSERT INTO providers (
                name, model_prefix, type, protocol, base_url, encrypted_credentials,
                expires_at, refresh_started_at, enabled
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)`,
        )
        .bind(name, modelPrefix, type, protocol, baseUrl, encrypted, expiresAt, isEnabled ? 1 : 0)
        .run();
    const encryptedCredentials = new ArrayBuffer(encrypted.byteLength);
    new Uint8Array(encryptedCredentials).set(encrypted);
    return {
        baseUrl,
        encryptedCredentials,
        expiresAt,
        id: result.meta.last_row_id,
        isEnabled,
        modelPrefix,
        name,
        protocol,
        refreshStartedAt: null,
        type,
    };
}

export async function initializeTestDatabase(database: D1Database): Promise<void> {
    const statements = databaseSchema
        .split(/;\s*(?:\n|$)/)
        .map((statement) => statement.trim())
        .filter((statement) => statement.length > 0)
        .map((statement) => database.prepare(statement));
    await database.batch(statements);
}

export async function clearTestDatabase(database: D1Database): Promise<void> {
    await database.prepare("DELETE FROM oauth_states").run();
    // The models FK cascades; leftover model rows would therefore fail the next test loudly.
    await database.prepare("DELETE FROM providers").run();
}
