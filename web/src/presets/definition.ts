import type { DefinitionResponse } from "../management-api/generated/v0";
import type { ProviderPreset } from "./types";

/**
 * The definition JSON the editor prefills for one preset: the preset's
 * payload serialized pretty-printed with a two-space indent.
 */
export function presetDefinitionJson(preset: ProviderPreset): string {
    return JSON.stringify(preset.payload, null, 2);
}

/**
 * Reads one preset's stored definition version, when the gateway holds one:
 * the client's own `MAX(version)` derivation over the name's rows (web README
 * §4.5), independent of the listing's order — the contract fixes the ordering
 * keys, never their direction.
 */
export function matchStoredDefinition(
    preset: ProviderPreset,
    definitions: ReadonlyArray<DefinitionResponse>,
): DefinitionResponse | null {
    const stored = definitions.filter((definition) => definition.name === preset.id);
    return stored.reduce<DefinitionResponse | null>(
        (latest, definition) =>
            latest === null || definition.version > latest.version ? definition : latest,
        null,
    );
}
