import type { CredentialsCipher } from "/common/credentials";
import { assertCredentialsMatchProvider } from "/common/credentials";
import {
    hasModelOverrides,
    MODEL_COLUMN_NAMES,
    type ModelRow,
    mapModelRow,
    type StoredProvider,
} from "/common/database";
import type { AntigravityOAuthCredentials, OpenAiCompatibleCredentials } from "/common/domain";
import { CredentialDecryptionError, ProviderAccessError, ValidationError } from "/common/errors";
import { oauthUpstreamSignal } from "/common/upstream-signal";
import { composeModelName, isJsonObject } from "/common/validation";
import { ensureFreshProviderAccess } from "/providers/access";
import { ANTIGRAVITY_FETCH_MODELS_URL, antigravityRequestHeaders } from "/providers/antigravity";
import { OPENAI_CODEX_MODELS } from "/providers/openai";
import type { DiscoveryResult } from "./domain";
import { isDatabaseConstraintError, PanelApiError } from "./errors";
import { readJsonObjectResponse } from "./upstream-json";

/**
 * Columns for whole-model reads, derived from the canonical tuple so the read cannot drift from
 * the models schema; kept beside the discovery reconcile, its only consumer.
 */
const MODEL_COLUMNS = MODEL_COLUMN_NAMES.join(",\n    ");

function upstreamDiscoveryError(message: string): PanelApiError {
    return new PanelApiError(502, message);
}

/** Antigravity credentials plus the possibly-refreshed provider row backing them. */
interface AntigravityProviderAccess {
    readonly credentials: AntigravityOAuthCredentials;
    readonly provider: StoredProvider;
}

/**
 * Refreshes credentials ahead of Antigravity discovery and returns them decrypted, mapping
 * credential failures onto the panel 502. The access layer's message is the sanitized, bounded
 * §7 diagnostic — including the RFC 6749 error code — and is forwarded verbatim instead of a
 * fixed string. Client-disconnect aborts and contract violations are rethrown: nobody is left
 * to answer a fabricated 502, and an internal error must stay internal instead of masquerading
 * as an upstream failure. Returning the decrypted credentials keeps every later use on this
 * mapped path, so no call site decrypts stored secrets bare.
 */
async function ensureDiscoveryProviderAccess(
    database: D1Database,
    provider: StoredProvider,
    cipher: CredentialsCipher,
    fetchUpstream: typeof fetch,
    signal: AbortSignal | undefined,
    options: {
        isForced?: boolean;
        rejectedAccessToken?: string;
    } = {},
): Promise<AntigravityProviderAccess> {
    try {
        const access = await ensureFreshProviderAccess(database, provider, cipher, {
            fetchUpstream,
            ...options,
            // `signal?: AbortSignal` in the access layer rejects an explicit `undefined` under
            // exactOptionalPropertyTypes, so the key is only present for a real signal.
            ...(signal === undefined
                ? {}
                : {
                      signal,
                  }),
        });
        if (!("projectId" in access.credentials)) {
            // Contract violation: discovery only invokes this for Antigravity providers, and the
            // access layer always returns credentials matching the stored provider type.
            throw new Error("Antigravity discovery received non-Antigravity credentials");
        }
        return {
            credentials: access.credentials,
            provider: access.provider,
        };
    } catch (error) {
        if (signal?.aborted) throw error;
        if (error instanceof CredentialDecryptionError) {
            throw upstreamDiscoveryError("Antigravity credentials could not be decrypted");
        }
        if (error instanceof ProviderAccessError) {
            throw upstreamDiscoveryError(error.message);
        }
        throw error;
    }
}

function parseCompatibleModelList(value: Record<string, unknown>): string[] {
    if (!Array.isArray(value["data"])) {
        throw upstreamDiscoveryError("The upstream returned an invalid model list");
    }
    const modelNames: string[] = [];
    for (const entry of value["data"]) {
        // An empty ID would compose a public name equal to the bare prefix (§6.1), so it is
        // rejected like any other malformed entry; discovery owns the non-empty upstream name.
        if (!isJsonObject(entry) || typeof entry["id"] !== "string" || entry["id"].length === 0) {
            throw upstreamDiscoveryError("The upstream returned an invalid model list");
        }
        modelNames.push(entry["id"]);
    }
    return modelNames;
}

/** Gemini ListModels entries carry a `models/`-prefixed name; only chat-capable models apply. */
function parseGeminiModelList(value: Record<string, unknown>): string[] {
    if (!Array.isArray(value["models"])) {
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
    let access = await ensureDiscoveryProviderAccess(
        database,
        provider,
        cipher,
        fetchUpstream,
        signal,
    );
    let response: Response | undefined;
    for (let attempt = 0; attempt < 2; attempt += 1) {
        try {
            response = await fetchUpstream(ANTIGRAVITY_FETCH_MODELS_URL, {
                body: JSON.stringify({
                    project: access.credentials.projectId,
                }),
                headers: antigravityRequestHeaders(
                    access.credentials.accessToken,
                    "application/json",
                ),
                method: "POST",
                signal: oauthUpstreamSignal(signal),
            });
        } catch {
            throw upstreamDiscoveryError("The Antigravity model discovery request failed");
        }
        if (response.status !== 401 || attempt === 1) break;
        access = await ensureDiscoveryProviderAccess(
            database,
            access.provider,
            cipher,
            fetchUpstream,
            signal,
            {
                isForced: true,
                rejectedAccessToken: access.credentials.accessToken,
            },
        );
    }
    if (response === undefined) throw new Error("Antigravity discovery made no request");
    const value = await readJsonObjectResponse(response, "The Antigravity model discovery request");
    if (!isJsonObject(value["models"])) {
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

/**
 * Decrypts static-key credentials, mapping stored-credential problems onto the same panel 502
 * credential diagnostic as OAuth credential-access failures: an undecryptable ciphertext and a
 * shape that no longer matches the provider type are both stored-secret problems the admin must
 * see as such — not a request-validation 400 or an internal 500.
 */
async function decryptCompatibleCredentials(
    provider: StoredProvider,
    cipher: CredentialsCipher,
): Promise<OpenAiCompatibleCredentials> {
    let credentials: unknown;
    try {
        credentials = await cipher.decrypt(provider.encryptedCredentials);
        return assertCredentialsMatchProvider("openai_compatible", credentials);
    } catch (error) {
        if (error instanceof CredentialDecryptionError) {
            throw upstreamDiscoveryError("OpenAI-compatible credentials could not be decrypted");
        }
        if (error instanceof ValidationError) {
            throw upstreamDiscoveryError("Stored credentials do not match the provider type");
        }
        throw error;
    }
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
    const credentials = await decryptCompatibleCredentials(provider, cipher);

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
    const value = await readJsonObjectResponse(response, "The upstream model discovery request");
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
    const staleUpstreamNames = new Set(staleModels.map((model) => model.upstreamName));
    const occupiedNames = new Set(
        existingModels.filter((model) => !deletedNames.has(model.name)).map((model) => model.name),
    );
    const existingUpstreamNames = new Set(providerModels.map((model) => model.upstreamName));

    // Unoverridden public names re-derive from the current prefix on every run, so a prefix
    // change propagates at the next discovery. Stale rows are exempt: this same batch deletes
    // them ahead of the renames, so re-deriving one would target a deleted row and reserve its
    // derived name against a brand-new upstream model in this very run. Overridden rows are
    // never stale and keep re-deriving. An occupied derived name keeps the previous name
    // resolvable instead.
    const renameStatements: D1PreparedStatement[] = [];
    for (const model of providerModels) {
        if (model.nameOverride !== null) continue;
        if (staleUpstreamNames.has(model.upstreamName)) continue;
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
                    // The VALUES list keeps the NULL tail in lockstep with MODEL_COLUMN_NAMES:
                    // every discovery-derived field starts fully unoverridden (§6.3).
                    `INSERT INTO models (${MODEL_COLUMNS}) VALUES (?, ?, ?, NULL, NULL, NULL)`,
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
