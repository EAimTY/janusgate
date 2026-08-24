import { ANTIGRAVITY_MODELS } from "/api/adapters/antigravity";
import { OPENAI_CODEX_MODELS } from "/api/adapters/openai";
import type { CredentialsCipher } from "/common/credentials";
import { assertCredentialsMatchProvider } from "/common/credentials";
import type { StoredProvider } from "/common/database";
import type { DiscoveryResult } from "/common/domain";
import { isJsonObject } from "/common/validation";
import { isDatabaseConstraintError, PanelApiError } from "./errors";

interface ExistingModelNameRow {
    name: string;
}

function upstreamDiscoveryError(message: string): PanelApiError {
    return new PanelApiError(502, message);
}

function parseCompatibleModelList(value: unknown): string[] {
    if (!isJsonObject(value) || !Array.isArray(value["data"])) {
        throw upstreamDiscoveryError("The upstream returned an invalid model list");
    }
    const modelNames: string[] = [];
    for (const entry of value["data"]) {
        if (!isJsonObject(entry) || typeof entry["id"] !== "string") {
            throw upstreamDiscoveryError("The upstream returned an invalid model list");
        }
        modelNames.push(entry["id"]);
    }
    return modelNames;
}

async function fetchCompatibleModelNames(
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch,
): Promise<string[]> {
    if (provider.baseUrl === null) {
        throw new Error("OpenAI-compatible provider is missing its base URL");
    }
    const credentials = assertCredentialsMatchProvider(
        "openai_compatible",
        await cipher.decrypt(provider.encryptedCredentials),
    );

    let response: Response;
    try {
        response = await fetchUpstream(`${provider.baseUrl}/models`, {
            headers: {
                Authorization: `Bearer ${credentials.apiKey}`,
            },
            method: "GET",
        });
    } catch {
        throw upstreamDiscoveryError("The upstream model discovery request failed");
    }
    if (!response.ok) {
        throw upstreamDiscoveryError(
            `The upstream model discovery request failed with HTTP ${response.status}`,
        );
    }
    let value: unknown;
    try {
        value = await response.json();
    } catch {
        throw upstreamDiscoveryError("The upstream returned an invalid model list");
    }
    return parseCompatibleModelList(value);
}

async function discoveredUpstreamNames(
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch,
): Promise<readonly string[]> {
    switch (provider.type) {
        case "openai_oauth":
            return OPENAI_CODEX_MODELS;
        case "antigravity_oauth":
            return ANTIGRAVITY_MODELS;
        case "openai_compatible":
            return fetchCompatibleModelNames(provider, cipher, fetchUpstream);
        default:
            throw new Error("Unsupported stored provider type");
    }
}

function assertNoDuplicateNames(upstreamNames: readonly string[]): void {
    if (new Set(upstreamNames).size !== upstreamNames.length) {
        throw upstreamDiscoveryError("The discovered model list contains a duplicate ID");
    }
}

export async function discoverProviderModels(
    database: D1Database,
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch = fetch,
): Promise<DiscoveryResult> {
    const upstreamNames = await discoveredUpstreamNames(provider, cipher, fetchUpstream);
    assertNoDuplicateNames(upstreamNames);

    const existingRows = await database
        .prepare("SELECT name FROM models")
        .all<ExistingModelNameRow>();
    const existingNames = new Set(existingRows.results.map((row) => row.name));
    const modelsToCreate = upstreamNames
        .map((upstreamName) => ({
            name: `${provider.modelPrefix}${upstreamName}`,
            upstreamName,
        }))
        .filter((model) => !existingNames.has(model.name));

    if (modelsToCreate.length > 0) {
        const statements = modelsToCreate.map((model) =>
            database
                .prepare(
                    `INSERT INTO models (
                        name, provider_id, upstream_name, thinking_levels, enabled
                    ) VALUES (?, ?, ?, NULL, 0)`,
                )
                .bind(model.name, provider.id, model.upstreamName),
        );
        try {
            await database.batch(statements);
        } catch (error: unknown) {
            if (isDatabaseConstraintError(error, "models.name")) {
                throw new PanelApiError(409, "A discovered model name already exists");
            }
            throw error;
        }
    }

    return {
        created: modelsToCreate.length,
        skipped: upstreamNames.length - modelsToCreate.length,
        total: upstreamNames.length,
    };
}
