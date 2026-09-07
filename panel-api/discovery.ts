import type { CredentialsCipher } from "/common/credentials";
import { assertCredentialsMatchProvider } from "/common/credentials";
import {
    hasModelOverrides,
    MODEL_COLUMNS,
    type ModelRow,
    mapModelRow,
    type StoredProvider,
} from "/common/database";
import type { DiscoveryResult } from "/common/domain";
import { GatewayError } from "/common/errors";
import { oauthUpstreamSignal } from "/common/upstream-signal";
import { composeModelName, isJsonObject } from "/common/validation";
import { ensureFreshProviderAccess } from "/providers/access";
import { ANTIGRAVITY_FETCH_MODELS_URL, antigravityRequestHeaders } from "/providers/antigravity";
import { OPENAI_CODEX_MODELS } from "/providers/openai";
import { isDatabaseConstraintError, PanelApiError } from "./errors";

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

/** Gemini ListModels entries carry a `models/`-prefixed name; only chat-capable models apply. */
function parseGeminiModelList(value: unknown): string[] {
    if (!isJsonObject(value) || !Array.isArray(value["models"])) {
        throw upstreamDiscoveryError("The upstream returned an invalid model list");
    }
    const modelNames: string[] = [];
    for (const entry of value["models"]) {
        if (!isJsonObject(entry)) {
            throw upstreamDiscoveryError("The upstream returned an invalid model list");
        }
        const methods = entry["supportedGenerationMethods"];
        if (!Array.isArray(methods) || !methods.every((method) => typeof method === "string")) {
            throw upstreamDiscoveryError("The upstream returned an invalid model list");
        }
        if (!methods.includes("generateContent")) continue;
        const name = entry["name"];
        if (typeof name !== "string" || !name.startsWith("models/") || name.length <= 7) {
            throw upstreamDiscoveryError("The upstream returned an invalid model list");
        }
        modelNames.push(name.slice("models/".length));
    }
    return modelNames;
}

async function fetchAntigravityModelNames(
    database: D1Database,
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch,
    signal?: AbortSignal,
): Promise<string[]> {
    // Margin-refresh before the first call so an idle-but-expired token does not waste an
    // upstream roundtrip; a mid-flight rejection still recovers through the forced loop below.
    let effectiveProvider: StoredProvider;
    try {
        effectiveProvider = (
            await ensureFreshProviderAccess(database, provider, cipher, {
                fetchUpstream,
                ...(signal === undefined
                    ? {}
                    : {
                          signal,
                      }),
            })
        ).provider;
    } catch (error) {
        if (error instanceof GatewayError) {
            throw upstreamDiscoveryError("Antigravity credential refresh failed");
        }
        throw error;
    }
    let response: Response | undefined;
    for (let attempt = 0; attempt < 2; attempt += 1) {
        const credentials = assertCredentialsMatchProvider(
            "antigravity_oauth",
            await cipher.decrypt(effectiveProvider.encryptedCredentials),
        );
        try {
            response = await fetchUpstream(ANTIGRAVITY_FETCH_MODELS_URL, {
                body: JSON.stringify({
                    project: credentials.projectId,
                }),
                headers: antigravityRequestHeaders(credentials.accessToken, "application/json"),
                method: "POST",
                signal: oauthUpstreamSignal(signal),
            });
        } catch {
            throw upstreamDiscoveryError("The Antigravity model discovery request failed");
        }
        if (response.status !== 401 || attempt === 1) break;
        try {
            effectiveProvider = (
                await ensureFreshProviderAccess(database, effectiveProvider, cipher, {
                    fetchUpstream,
                    isForced: true,
                    rejectedAccessToken: credentials.accessToken,
                    ...(signal === undefined
                        ? {}
                        : {
                              signal,
                          }),
                })
            ).provider;
        } catch {
            throw upstreamDiscoveryError("Antigravity credential refresh failed");
        }
    }
    if (response === undefined) throw new Error("Antigravity discovery made no request");
    if (!response.ok) {
        throw upstreamDiscoveryError(
            `The Antigravity model discovery request failed with HTTP ${response.status}`,
        );
    }
    let value: unknown;
    try {
        value = await response.json();
    } catch {
        throw upstreamDiscoveryError("Antigravity returned an invalid model list");
    }
    if (!isJsonObject(value) || !isJsonObject(value["models"])) {
        throw upstreamDiscoveryError("Antigravity returned an invalid model list");
    }
    const modelNames = Object.entries(value["models"])
        .filter(
            ([name, descriptor]) =>
                /^(?:gemini-|claude-|gpt-oss-)/i.test(name) &&
                !/\s/.test(name) &&
                (!isJsonObject(descriptor) || descriptor["isInternal"] !== true),
        )
        .map(([name]) => name);
    if (modelNames.length === 0) {
        throw upstreamDiscoveryError("Antigravity returned an empty model list");
    }
    return modelNames;
}

async function fetchCompatibleModelNames(
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch,
    signal?: AbortSignal,
): Promise<string[]> {
    if (provider.baseUrl === null) {
        throw new Error("OpenAI-compatible provider is missing its base URL");
    }
    if (provider.protocol === null) {
        throw new Error("OpenAI-compatible provider is missing its protocol");
    }
    const credentials = assertCredentialsMatchProvider(
        "openai_compatible",
        await cipher.decrypt(provider.encryptedCredentials),
    );

    // Both protocol families list models at `{baseUrl}/models`; they differ in auth header,
    // response shape, and (for Gemini) the `models/` name prefix.
    const isGemini = provider.protocol === "gemini_generate_content";
    const headers: Record<string, string> = isGemini
        ? {
              "x-goog-api-key": credentials.apiKey,
          }
        : {
              Authorization: `Bearer ${credentials.apiKey}`,
          };

    let response: Response;
    try {
        response = await fetchUpstream(`${provider.baseUrl}/models`, {
            headers,
            method: "GET",
            signal: oauthUpstreamSignal(signal),
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
    return isGemini ? parseGeminiModelList(value) : parseCompatibleModelList(value);
}

async function discoveredUpstreamNames(
    database: D1Database,
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch,
    signal?: AbortSignal,
): Promise<readonly string[]> {
    switch (provider.type) {
        case "openai_oauth":
            return OPENAI_CODEX_MODELS;
        case "antigravity_oauth":
            return fetchAntigravityModelNames(database, provider, cipher, fetchUpstream, signal);
        case "openai_compatible":
            return fetchCompatibleModelNames(provider, cipher, fetchUpstream, signal);
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
    signal?: AbortSignal,
): Promise<DiscoveryResult> {
    const upstreamNames = await discoveredUpstreamNames(
        database,
        provider,
        cipher,
        fetchUpstream,
        signal,
    );
    assertNoDuplicateNames(upstreamNames);

    const existingModels = (
        await database.prepare(`SELECT ${MODEL_COLUMNS} FROM models`).all<ModelRow>()
    ).results.map(mapModelRow);
    const providerModels = existingModels.filter((model) => model.providerId === provider.id);
    const upstreamNameSet = new Set(upstreamNames);

    // Rows without any override mirror discovery, so the ones whose upstream model disappeared
    // are pruned; overridden rows belong to the admin and are never touched.
    const staleModels = providerModels.filter(
        (model) => !upstreamNameSet.has(model.upstreamName) && !hasModelOverrides(model),
    );
    const deletedNames = new Set(staleModels.map((model) => model.name));
    const occupiedNames = new Set(
        existingModels.filter((model) => !deletedNames.has(model.name)).map((model) => model.name),
    );
    const existingUpstreamNames = new Set(providerModels.map((model) => model.upstreamName));

    // Unoverridden public names re-derive from the current prefix on every run, so a prefix
    // change propagates at the next discovery. A derived name that is still occupied keeps the
    // previous name resolvable instead.
    const renameStatements: D1PreparedStatement[] = [];
    for (const model of providerModels) {
        if (model.nameOverride !== null) continue;
        const derivedName = composeModelName(provider.modelPrefix, model.upstreamName);
        if (model.name === derivedName || occupiedNames.has(derivedName)) continue;
        occupiedNames.delete(model.name);
        occupiedNames.add(derivedName);
        renameStatements.push(
            database
                .prepare(
                    `UPDATE models SET name = ?
                    WHERE provider_id = ? AND upstream_name = ? AND name_override IS NULL`,
                )
                .bind(derivedName, provider.id, model.upstreamName),
        );
    }

    const insertStatements: D1PreparedStatement[] = [];
    for (const upstreamName of upstreamNames) {
        if (existingUpstreamNames.has(upstreamName)) continue;
        const name = composeModelName(provider.modelPrefix, upstreamName);
        if (occupiedNames.has(name)) continue;
        occupiedNames.add(name);
        insertStatements.push(
            database
                .prepare(
                    `INSERT INTO models (
                        provider_id, upstream_name, name, name_override, enabled_override,
                        thinking_levels_override
                    ) VALUES (?, ?, ?, NULL, NULL, NULL)`,
                )
                .bind(provider.id, upstreamName, name),
        );
    }

    const statements = [
        ...staleModels.map((model) =>
            database
                .prepare(
                    `DELETE FROM models
                    WHERE provider_id = ? AND upstream_name = ?
                        AND name_override IS NULL AND enabled_override IS NULL
                        AND thinking_levels_override IS NULL`,
                )
                .bind(model.providerId, model.upstreamName),
        ),
        ...renameStatements,
        ...insertStatements,
    ];
    let removedCount = 0;
    if (statements.length > 0) {
        try {
            const results = await database.batch(statements);
            removedCount = results
                .slice(0, staleModels.length)
                .reduce((total, result) => total + result.meta.changes, 0);
        } catch (error: unknown) {
            if (isDatabaseConstraintError(error, "models.name")) {
                throw new PanelApiError(409, "A discovered model name already exists");
            }
            throw error;
        }
    }

    return {
        created: insertStatements.length,
        removed: removedCount,
        skipped: upstreamNames.length - insertStatements.length,
        total: upstreamNames.length,
    };
}
