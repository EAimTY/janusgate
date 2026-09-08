import type { CredentialsCipher } from "/common/credentials";
import {
    findProviderRow,
    mapProviderRow,
    PROVIDER_COLUMNS,
    type ProviderRow,
    type StoredProvider,
} from "/common/database";
import {
    assembleProviderSummary,
    type OAuthProviderType,
    type ProviderSummary,
    type ProviderType,
    type UpstreamProtocol,
} from "/common/domain";
import { AUTO_PROVIDER_NAME_PREFIX } from "./config";
import { isDatabaseConstraintError, PanelApiError } from "./errors";
import type { parseSaveProviderInput } from "./input";

// The validated save-provider request (create or update intent); derived from the parser so the
// wire contract stays single-sourced in ./input.
type SaveProviderInput = ReturnType<typeof parseSaveProviderInput>;

interface CreateProviderRecord {
    name: string;
    modelPrefix: string;
    type: ProviderType;
    protocol: UpstreamProtocol | null;
    baseUrl: string | null;
    encryptedCredentials: Uint8Array;
    expiresAt: number | null;
    isEnabled: boolean;
}

interface UpdateProviderRecord {
    id: number;
    modelPrefix: string;
    protocol: UpstreamProtocol | null;
    baseUrl: string | null;
    encryptedCredentials?: Uint8Array;
    expiresAt: number | null;
    isEnabled: boolean;
}

export async function listProviderSummaries(database: D1Database): Promise<ProviderSummary[]> {
    const result = await database
        .prepare(`SELECT ${PROVIDER_COLUMNS} FROM providers ORDER BY id`)
        .all<ProviderRow>();
    // StoredProvider structurally satisfies ProviderSummaryFields, so stored rows pass through
    // the same single invariant check that validates the panel wire shape.
    return result.results.map((row) => assembleProviderSummary(mapProviderRow(row)));
}

export function findProviderByName(
    database: D1Database,
    name: string,
): Promise<StoredProvider | null> {
    return findProviderRow(database, "name", name);
}

export function findProviderById(database: D1Database, id: number): Promise<StoredProvider | null> {
    return findProviderRow(database, "id", id);
}

export async function requireProviderByName(
    database: D1Database,
    name: string,
): Promise<StoredProvider> {
    const provider = await findProviderByName(database, name);
    if (provider === null) throw new PanelApiError(404, "Provider not found");
    return provider;
}

async function createProvider(
    database: D1Database,
    provider: CreateProviderRecord,
): Promise<StoredProvider> {
    try {
        // RETURNING keeps creation to a single roundtrip and routes the stored row through
        // mapProviderRow, so it is validated in exactly one place like every other read.
        const row = await database
            .prepare(
                `INSERT INTO providers (
                    name, model_prefix, type, protocol, base_url,
                    encrypted_credentials, expires_at, enabled
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                RETURNING ${PROVIDER_COLUMNS}`,
            )
            .bind(
                provider.name,
                provider.modelPrefix,
                provider.type,
                provider.protocol,
                provider.baseUrl,
                provider.encryptedCredentials,
                provider.expiresAt,
                provider.isEnabled ? 1 : 0,
            )
            .first<ProviderRow>();
        if (row === null) throw new Error("Created provider is missing");
        return mapProviderRow(row);
    } catch (error) {
        if (isDatabaseConstraintError(error, "providers.name")) {
            throw new PanelApiError(409, "Provider name already exists");
        }
        throw error;
    }
}

async function updateProvider(database: D1Database, provider: UpdateProviderRecord): Promise<void> {
    const credentialsAssignment =
        provider.encryptedCredentials === undefined ? "" : ", encrypted_credentials = ?";
    const values: unknown[] = [
        provider.modelPrefix,
        provider.protocol,
        provider.baseUrl,
        provider.expiresAt,
        provider.isEnabled ? 1 : 0,
    ];
    if (provider.encryptedCredentials !== undefined) values.push(provider.encryptedCredentials);
    values.push(provider.id);
    const result = await database
        .prepare(
            `UPDATE providers SET
                model_prefix = ?, protocol = ?, base_url = ?, expires_at = ?, enabled = ?
                ${credentialsAssignment}
            WHERE id = ?`,
        )
        .bind(...values)
        .run();
    if (result.meta.changes !== 1) throw new Error("Updated provider is missing");
}

interface UpdateOAuthProviderRecord {
    id: number;
    modelPrefix: string;
    isEnabled: boolean;
}

/**
 * OAuth providers only ever change `enabled` and `modelPrefix` (§12.4), so this update writes
 * exactly those columns. Rewriting the routing columns or the token expiry here would silently
 * overwrite a corrupted row's state instead of letting the invariant violation surface as the
 * internal error it is.
 */
async function updateOAuthProvider(
    database: D1Database,
    provider: UpdateOAuthProviderRecord,
): Promise<void> {
    const result = await database
        .prepare(
            `UPDATE providers SET
                model_prefix = ?, enabled = ?
            WHERE id = ?`,
        )
        .bind(provider.modelPrefix, provider.isEnabled ? 1 : 0, provider.id)
        .run();
    if (result.meta.changes !== 1) throw new Error("Updated provider is missing");
}

/**
 * Create/update orchestration for `POST /panel-api/save-provider` (§12.4). Creation only accepts
 * API-key providers; updates preserve the stored type, encrypting a replacement API key for
 * `openai_compatible` providers and rejecting credential/endpoint fields for OAuth providers.
 */
export async function saveProvider(
    database: D1Database,
    cipher: CredentialsCipher,
    input: SaveProviderInput,
): Promise<{
    name: string;
}> {
    const existing = await findProviderByName(database, input.name);

    if (input.kind === "create") {
        if (existing !== null) {
            throw new PanelApiError(409, "Provider name already exists");
        }
        const encryptedCredentials = await cipher.encrypt({
            apiKey: input.apiKey,
        });
        const created = await createProvider(database, {
            baseUrl: input.baseUrl,
            encryptedCredentials,
            expiresAt: null,
            isEnabled: input.enabled ?? true,
            modelPrefix: input.modelPrefix ?? input.name,
            name: input.name,
            protocol: input.protocol,
            type: "openai_compatible",
        });
        return {
            name: created.name,
        };
    }

    if (existing === null) {
        throw new PanelApiError(404, "Provider not found");
    }

    if (existing.type === "openai_compatible") {
        if (existing.baseUrl === null || existing.protocol === null) {
            throw new Error("Invalid stored OpenAI-compatible provider");
        }
        const encryptedCredentials =
            input.apiKey === undefined
                ? undefined
                : await cipher.encrypt({
                      apiKey: input.apiKey,
                  });
        await updateProvider(database, {
            baseUrl: input.baseUrl ?? existing.baseUrl,
            ...(encryptedCredentials === undefined
                ? {}
                : {
                      encryptedCredentials,
                  }),
            expiresAt: null,
            id: existing.id,
            isEnabled: input.enabled ?? existing.isEnabled,
            modelPrefix: input.modelPrefix ?? existing.modelPrefix,
            protocol: input.protocol ?? existing.protocol,
        });
        return {
            name: existing.name,
        };
    }

    if (input.protocol !== undefined || input.baseUrl !== undefined || input.apiKey !== undefined) {
        throw new PanelApiError(400, "OAuth providers only accept enabled and modelPrefix updates");
    }
    await updateOAuthProvider(database, {
        id: existing.id,
        isEnabled: input.enabled ?? existing.isEnabled,
        modelPrefix: input.modelPrefix ?? existing.modelPrefix,
    });
    return {
        name: existing.name,
    };
}

// The models FK cascades, so one statement removes the provider together with all its models.
// Existence is checked via RETURNING because meta.changes also counts cascade-deleted models.
export async function deleteProviderByName(database: D1Database, name: string): Promise<void> {
    const deleted = await database
        .prepare("DELETE FROM providers WHERE name = ? RETURNING name")
        .bind(name)
        .first<{
            name: string;
        }>();
    if (deleted === null) throw new PanelApiError(404, "Provider not found");
}

export async function generateAvailableProviderName(
    database: D1Database,
    type: OAuthProviderType,
): Promise<string> {
    const prefix = AUTO_PROVIDER_NAME_PREFIX[type];
    const result = await database
        .prepare("SELECT name FROM providers WHERE name GLOB ? ORDER BY name")
        .bind(`${prefix}*`)
        .all<{
            name: string;
        }>();
    const occupied = new Set(result.results.map(({ name }) => name));
    for (let suffix = 1; ; suffix += 1) {
        const candidate = `${prefix}${suffix}`;
        if (!occupied.has(candidate)) return candidate;
    }
}
