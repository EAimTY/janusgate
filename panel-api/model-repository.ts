import { isModelEnabled, type ModelRow, mapModelRow } from "/common/database";
import type { ModelSummary } from "/common/domain";
import { composeModelName, parseStoredThinkingLevels } from "/common/validation";
import type { ThinkingLevel } from "/config";
import { isDatabaseConstraintError, PanelApiError } from "./errors";

interface PanelModelRow extends ModelRow {
    provider_name: string;
    provider_type: ModelSummary["providerType"];
}

/** Admin-controlled deviations from the discovery result; `null` keeps following discovery. */
interface ModelOverrideRecord {
    nameOverride: string | null;
    enabledOverride: boolean | null;
    thinkingLevelsOverride: readonly ThinkingLevel[] | null;
}

export interface UpsertModelRecord extends ModelOverrideRecord {
    providerId: number;
    modelPrefix: string;
    upstreamName: string;
}

function composeStoredModelName(
    modelPrefix: string,
    upstreamName: string,
    nameOverride: string | null,
): string {
    return composeModelName(modelPrefix, nameOverride ?? upstreamName);
}

export async function listPanelModels(database: D1Database): Promise<ModelSummary[]> {
    const result = await database
        .prepare(
            `SELECT
                models.provider_id,
                models.upstream_name,
                models.name,
                models.name_override,
                models.enabled_override,
                models.thinking_levels_override,
                providers.name AS provider_name,
                providers.type AS provider_type
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            ORDER BY models.name`,
        )
        .all<PanelModelRow>();
    return result.results.map((row) => {
        const model = mapModelRow(row);
        return {
            enabled: isModelEnabled(model),
            name: model.name,
            nameOverride: model.nameOverride,
            providerName: row.provider_name,
            providerType: row.provider_type,
            thinkingLevels: parseStoredThinkingLevels(model.thinkingLevelsOverrideJson),
            upstreamName: model.upstreamName,
        };
    });
}

/**
 * Writes the complete override set for the model identity `(providerId, upstreamName)`, creating
 * the row when absent. Returns the stored public name.
 */
export async function upsertModel(database: D1Database, model: UpsertModelRecord): Promise<string> {
    const name = composeStoredModelName(model.modelPrefix, model.upstreamName, model.nameOverride);
    const thinkingLevelsOverrideJson =
        model.thinkingLevelsOverride === null ? null : JSON.stringify(model.thinkingLevelsOverride);
    try {
        await database
            .prepare(
                `INSERT INTO models (
                    provider_id, upstream_name, name, name_override, enabled_override,
                    thinking_levels_override
                ) VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT (provider_id, upstream_name) DO UPDATE SET
                    name = excluded.name,
                    name_override = excluded.name_override,
                    enabled_override = excluded.enabled_override,
                    thinking_levels_override = excluded.thinking_levels_override`,
            )
            .bind(
                model.providerId,
                model.upstreamName,
                name,
                model.nameOverride,
                model.enabledOverride === null ? null : model.enabledOverride ? 1 : 0,
                thinkingLevelsOverrideJson,
            )
            .run();
    } catch (error) {
        if (isDatabaseConstraintError(error, "models.name")) {
            throw new PanelApiError(409, "Model name already exists");
        }
        throw error;
    }
    return name;
}

export async function deleteModel(
    database: D1Database,
    providerId: number,
    upstreamName: string,
): Promise<void> {
    const result = await database
        .prepare("DELETE FROM models WHERE provider_id = ? AND upstream_name = ?")
        .bind(providerId, upstreamName)
        .run();
    if (result.meta.changes !== 1) throw new PanelApiError(404, "Model not found");
}
