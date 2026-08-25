import { mapProviderRow, type ProviderRow, type StoredProvider } from "/common/database";
import { parseStoredThinkingLevels } from "/common/validation";
import type { ThinkingLevel } from "/config";
import { MODEL_OWNER_BY_PROVIDER_TYPE } from "./config";

export interface ResolvedModelRoute {
    id: number;
    name: string;
    upstreamName: string;
    thinkingLevels: readonly ThinkingLevel[] | null;
    provider: StoredProvider;
}

interface ModelRouteRow extends ProviderRow {
    model_id: number;
    model_name: string;
    upstream_name: string;
    thinking_levels: string | null;
}

interface PublicModelRow {
    id: number;
    name: string;
    provider_type: StoredProvider["type"];
}

export async function resolveEnabledModel(
    database: D1Database,
    modelName: string,
): Promise<ResolvedModelRoute | null> {
    const row = await database
        .prepare(
            `SELECT
                models.id AS model_id,
                models.name AS model_name,
                models.upstream_name,
                models.thinking_levels,
                providers.id,
                providers.name,
                providers.model_prefix,
                providers.type,
                providers.protocol,
                providers.base_url,
                providers.encrypted_credentials,
                providers.expires_at,
                providers.refresh_started_at,
                providers.enabled
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            WHERE models.name = ? AND models.enabled = 1 AND providers.enabled = 1`,
        )
        .bind(modelName)
        .first<ModelRouteRow>();
    if (row === null) return null;
    return {
        id: row.model_id,
        name: row.model_name,
        provider: mapProviderRow(row),
        thinkingLevels: parseStoredThinkingLevels(row.thinking_levels),
        upstreamName: row.upstream_name,
    };
}

export async function listEnabledPublicModels(database: D1Database): Promise<{
    object: "list";
    data: Array<{
        id: string;
        object: "model";
        created: 0;
        owned_by: string;
    }>;
}> {
    const result = await database
        .prepare(
            `SELECT models.id, models.name, providers.type AS provider_type
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            WHERE models.enabled = 1 AND providers.enabled = 1
            ORDER BY models.id`,
        )
        .all<PublicModelRow>();
    return {
        data: result.results.map((model) => ({
            created: 0,
            id: model.name,
            object: "model",
            owned_by: MODEL_OWNER_BY_PROVIDER_TYPE[model.provider_type],
        })),
        object: "list",
    };
}
