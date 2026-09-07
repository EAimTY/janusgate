import {
    mapProviderRow,
    PROVIDER_COLUMN_NAMES,
    type ProviderRow,
    type StoredProvider,
} from "/common/database";
import { parseStoredThinkingLevels } from "/common/validation";
import type { ThinkingLevel } from "/config";
import { MODEL_OWNER_BY_PROVIDER_TYPE } from "./config";

export interface ResolvedModelRoute {
    name: string;
    upstreamName: string;
    thinkingLevels: readonly ThinkingLevel[] | null;
    provider: StoredProvider;
}

interface ModelRouteRow extends ProviderRow {
    model_name: string;
    upstream_name: string;
    thinking_levels_override: string | null;
}

interface PublicModelRow {
    name: string;
    provider_type: StoredProvider["type"];
}

/** Canonical provider columns qualified for the join alias, so routing cannot drift from the schema. */
const QUALIFIED_PROVIDER_COLUMNS = PROVIDER_COLUMN_NAMES.map(
    (column) => `providers.${column}`,
).join(", ");

export async function resolveEnabledModel(
    database: D1Database,
    modelName: string,
): Promise<ResolvedModelRoute | null> {
    const row = await database
        .prepare(
            `SELECT
                models.name AS model_name,
                models.upstream_name,
                models.thinking_levels_override,
                ${QUALIFIED_PROVIDER_COLUMNS}
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            WHERE models.name = ?
                AND COALESCE(models.enabled_override, 1) = 1 AND providers.enabled = 1`,
        )
        .bind(modelName)
        .first<ModelRouteRow>();
    if (row === null) return null;
    return {
        name: row.model_name,
        provider: mapProviderRow(row),
        thinkingLevels: parseStoredThinkingLevels(row.thinking_levels_override),
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
            `SELECT models.name, providers.type AS provider_type
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            WHERE COALESCE(models.enabled_override, 1) = 1 AND providers.enabled = 1
            ORDER BY models.name`,
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
