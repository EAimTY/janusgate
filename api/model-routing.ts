import {
    MODEL_COLUMN_NAMES,
    type ModelRow,
    mapProviderRow,
    PROVIDER_COLUMN_NAMES,
    type ProviderRow,
    parseStoredProviderType,
    type StoredProvider,
} from "/common/database";
import { assembleProviderSummary, type ProviderSummary } from "/common/domain";
import { parseStoredThinkingLevels } from "/common/validation";
import type { ThinkingLevel } from "/config";
import { MODEL_OWNER_BY_PROVIDER_TYPE } from "./config";

export interface ResolvedModelRoute {
    name: string;
    upstreamName: string;
    thinkingLevels: readonly ThinkingLevel[] | null;
    provider: StoredProvider;
    /**
     * The invariant-checked view of `provider` (§12.3); upstream request preparation switches on
     * this so protocol/base-url null guards become type-impossible. It always describes the same
     * row as `provider` and is re-derived whenever the provider is refreshed.
     */
    providerSummary: ProviderSummary;
}

interface ModelRouteRow extends ProviderRow, ModelRow {
    model_name: string;
}

/** The public models list reads the provider type raw and parses it before use. */
interface PublicModelRow {
    name: string;
    provider_type: string;
}

/** Canonical provider columns qualified for the join alias, so routing cannot drift from the schema. */
const QUALIFIED_PROVIDER_COLUMNS = PROVIDER_COLUMN_NAMES.map(
    (column) => `providers.${column}`,
).join(", ");

/**
 * Canonical model columns qualified for the join, with the public name aliased because the joined
 * provider row also carries a `name`; deriving keeps routing tracking the models schema too.
 */
const QUALIFIED_MODEL_COLUMNS = MODEL_COLUMN_NAMES.map((column) =>
    column === "name" ? "models.name AS model_name" : `models.${column}`,
).join(", ");

export async function resolveEnabledModel(
    database: D1Database,
    modelName: string,
): Promise<ResolvedModelRoute | null> {
    const row = await database
        .prepare(
            `SELECT
                ${QUALIFIED_MODEL_COLUMNS},
                ${QUALIFIED_PROVIDER_COLUMNS}
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            WHERE models.name = ?
                AND COALESCE(models.enabled_override, 1) = 1 AND providers.enabled = 1`,
        )
        .bind(modelName)
        .first<ModelRouteRow>();
    if (row === null) return null;
    const provider = mapProviderRow(row);
    return {
        name: row.model_name,
        provider,
        providerSummary: assembleProviderSummary(provider),
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
            // A corrupted type must fail loudly (→ 500 envelope), never emit `owned_by: undefined`.
            owned_by: MODEL_OWNER_BY_PROVIDER_TYPE[parseStoredProviderType(model.provider_type)],
        })),
        object: "list",
    };
}
