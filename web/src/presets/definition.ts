import type { DefinitionResponse } from "../management-api/generated/v0";
import type { ProviderPreset } from "./types";

/**
 * The definition JSON the editor prefills for one preset: the preset's
 * payload serialized pretty-printed with a two-space indent.
 */
export function presetDefinitionJson(preset: ProviderPreset): string {
    return JSON.stringify(preset.payload, null, 2);
}

/** Reads one preset's stored definition version, when the gateway holds one. */
export function matchStoredDefinition(
    preset: ProviderPreset,
    definitions: ReadonlyArray<DefinitionResponse>,
): DefinitionResponse | null {
    const stored = definitions.filter((definition) => definition.name === preset.id);
    return stored.length === 0 ? null : (stored.at(-1) ?? null);
}
