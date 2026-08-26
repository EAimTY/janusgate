import {
    mapProviderRow,
    PROVIDER_COLUMNS,
    type ProviderRow,
    type StoredProvider,
} from "/common/database";
import type { ProviderSummary, ProviderType, UpstreamProtocol } from "/common/domain";
import { AUTO_PROVIDER_NAME_PREFIX } from "./config";
import { isDatabaseConstraintError, PanelApiError } from "./errors";

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

function providerSummary(provider: StoredProvider): ProviderSummary {
    const shared = {
        enabled: provider.isEnabled,
        modelPrefix: provider.modelPrefix,
        name: provider.name,
    };
    if (provider.type === "openai_compatible") {
        if (
            provider.protocol === null ||
            provider.baseUrl === null ||
            provider.expiresAt !== null
        ) {
            throw new Error("Invalid stored OpenAI-compatible provider");
        }
        return {
            ...shared,
            baseUrl: provider.baseUrl,
            expiresAt: null,
            protocol: provider.protocol,
            type: provider.type,
        };
    }
    if (provider.protocol !== null || provider.baseUrl !== null || provider.expiresAt === null) {
        throw new Error("Invalid stored OAuth provider");
    }
    return {
        ...shared,
        baseUrl: null,
        expiresAt: provider.expiresAt,
        protocol: null,
        type: provider.type,
    };
}

export async function listProviderSummaries(database: D1Database): Promise<ProviderSummary[]> {
    const result = await database
        .prepare(`SELECT ${PROVIDER_COLUMNS} FROM providers ORDER BY id`)
        .all<ProviderRow>();
    return result.results.map((row) => providerSummary(mapProviderRow(row)));
}

export async function findProviderByName(
    database: D1Database,
    name: string,
): Promise<StoredProvider | null> {
    const row = await database
        .prepare(`SELECT ${PROVIDER_COLUMNS} FROM providers WHERE name = ?`)
        .bind(name)
        .first<ProviderRow>();
    return row === null ? null : mapProviderRow(row);
}

export async function findProviderById(
    database: D1Database,
    id: number,
): Promise<StoredProvider | null> {
    const row = await database
        .prepare(`SELECT ${PROVIDER_COLUMNS} FROM providers WHERE id = ?`)
        .bind(id)
        .first<ProviderRow>();
    return row === null ? null : mapProviderRow(row);
}

export async function requireProviderByName(
    database: D1Database,
    name: string,
): Promise<StoredProvider> {
    const provider = await findProviderByName(database, name);
    if (provider === null) throw new PanelApiError(404, "Provider not found");
    return provider;
}

export async function createProvider(
    database: D1Database,
    provider: CreateProviderRecord,
): Promise<StoredProvider> {
    try {
        const result = await database
            .prepare(
                `INSERT INTO providers (
                    name, model_prefix, type, protocol, base_url,
                    encrypted_credentials, expires_at, enabled
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
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
            .run();
        const created = await findProviderById(database, result.meta.last_row_id);
        if (created === null) throw new Error("Created provider is missing");
        return created;
    } catch (error) {
        if (isDatabaseConstraintError(error, "providers.name")) {
            throw new PanelApiError(409, "Provider name already exists");
        }
        throw error;
    }
}

export async function updateProvider(
    database: D1Database,
    provider: UpdateProviderRecord,
): Promise<void> {
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

export async function deleteProviderByName(database: D1Database, name: string): Promise<void> {
    const results = await database.batch([
        database
            .prepare(
                `DELETE FROM models
                WHERE provider_id = (SELECT id FROM providers WHERE name = ?)`,
            )
            .bind(name),
        database.prepare("DELETE FROM providers WHERE name = ?").bind(name),
    ]);
    if (results[1]?.meta.changes !== 1) throw new PanelApiError(404, "Provider not found");
}

export async function generateAvailableProviderName(
    database: D1Database,
    type: "antigravity_oauth" | "openai_oauth",
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
