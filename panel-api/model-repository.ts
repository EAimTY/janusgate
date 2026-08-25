import { MODEL_COLUMNS, type ModelRow, mapModelRow, type StoredModel } from "/common/database";
import type { ModelSummary } from "/common/domain";
import { parseStoredThinkingLevels } from "/common/validation";
import type { ThinkingLevel } from "/config";
import { isDatabaseConstraintError, PanelApiError } from "./errors";

export type PanelModel = ModelSummary;

interface PanelModelRow extends ModelRow {
    provider_name: string;
    provider_type: PanelModel["providerType"];
}

export interface SaveModelRecord {
    id?: number;
    name: string;
    providerId: number;
    upstreamName: string;
    thinkingLevels: readonly ThinkingLevel[] | null;
    isEnabled: boolean;
}

export async function listPanelModels(database: D1Database): Promise<PanelModel[]> {
    const result = await database
        .prepare(
            `SELECT
                models.id,
                models.name,
                models.provider_id,
                models.upstream_name,
                models.thinking_levels,
                models.enabled,
                models.is_manually_edited,
                providers.name AS provider_name,
                providers.type AS provider_type
            FROM models
            INNER JOIN providers ON providers.id = models.provider_id
            ORDER BY models.id`,
        )
        .all<PanelModelRow>();
    return result.results.map((row) => {
        const model = mapModelRow(row);
        return {
            id: model.id,
            enabled: model.isEnabled,
            name: model.name,
            providerName: row.provider_name,
            providerType: row.provider_type,
            thinkingLevels: parseStoredThinkingLevels(model.thinkingLevelsJson),
            upstreamName: model.upstreamName,
        };
    });
}

export async function findModelById(database: D1Database, id: number): Promise<StoredModel | null> {
    const row = await database
        .prepare(`SELECT ${MODEL_COLUMNS} FROM models WHERE id = ?`)
        .bind(id)
        .first<ModelRow>();
    return row === null ? null : mapModelRow(row);
}

export async function saveModel(
    database: D1Database,
    model: SaveModelRecord,
): Promise<{
    id: number;
    name: string;
}> {
    const thinkingLevelsJson =
        model.thinkingLevels === null ? null : JSON.stringify(model.thinkingLevels);
    try {
        if (model.id === undefined) {
            const result = await database
                .prepare(
                    `INSERT INTO models (
                        name, provider_id, upstream_name, thinking_levels, enabled,
                        is_manually_edited
                    ) VALUES (?, ?, ?, ?, ?, 1)`,
                )
                .bind(
                    model.name,
                    model.providerId,
                    model.upstreamName,
                    thinkingLevelsJson,
                    model.isEnabled ? 1 : 0,
                )
                .run();
            return {
                id: result.meta.last_row_id,
                name: model.name,
            };
        }

        const result = await database
            .prepare(
                `UPDATE models SET
                    name = ?, provider_id = ?, upstream_name = ?, thinking_levels = ?, enabled = ?,
                    is_manually_edited = 1
                WHERE id = ?`,
            )
            .bind(
                model.name,
                model.providerId,
                model.upstreamName,
                thinkingLevelsJson,
                model.isEnabled ? 1 : 0,
                model.id,
            )
            .run();
        if (result.meta.changes !== 1) throw new PanelApiError(404, "Model not found");
        return {
            id: model.id,
            name: model.name,
        };
    } catch (error) {
        if (isDatabaseConstraintError(error, "models.name")) {
            throw new PanelApiError(409, "Model name already exists");
        }
        throw error;
    }
}

export async function deleteModel(database: D1Database, id: number): Promise<void> {
    const result = await database.prepare("DELETE FROM models WHERE id = ?").bind(id).run();
    if (result.meta.changes !== 1) throw new PanelApiError(404, "Model not found");
}
