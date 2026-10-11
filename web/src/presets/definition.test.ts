import { describe, expect, test } from "vitest";
import type { DefinitionResponse } from "/src/management-api/generated/v0";
import { PROVIDER_PRESETS } from "./catalog";
import { matchStoredDefinition, presetDefinitionJson } from "./definition";

/**
 * The preset catalog's presentation contracts: the definition JSON the
 * editor prefills serializing and re-parsing losslessly, and the stored
 * definition version picked by the client's own MAX(version) derivation —
 * independent of the listing's order.
 */

/** One stored definition row, as the listings parser rebuilds it. */
function stored(name: string, version: number): DefinitionResponse {
    const preset = PROVIDER_PRESETS[0];
    if (preset === undefined) throw new Error("the catalog is empty");

    return {
        name,
        version,
        created_at: 1_700_000_000_000_000 + version,
        ...preset.payload,
    };
}

describe("presetDefinitionJson", () => {
    test("serializes the payload pretty-printed, re-parsing to the same payload", () => {
        const preset = PROVIDER_PRESETS[0];
        if (preset === undefined) throw new Error("the catalog is empty");

        const json = presetDefinitionJson(preset);

        expect(json).toContain("\n  ");
        expect(JSON.parse(json)).toEqual(preset.payload);
    });
});

describe("matchStoredDefinition", () => {
    test("answers null when the gateway holds none of the preset's name", () => {
        const preset = PROVIDER_PRESETS[0];
        if (preset === undefined) throw new Error("the catalog is empty");

        expect(
            matchStoredDefinition(preset, [
                stored("other-preset", 1),
            ]),
        ).toBeNull();
        expect(matchStoredDefinition(preset, [])).toBeNull();
    });

    test("answers the highest version, whatever order the listing carried", () => {
        const preset = PROVIDER_PRESETS[0];
        if (preset === undefined) throw new Error("the catalog is empty");

        const shuffled = [
            stored(preset.id, 3),
            stored(preset.id, 1),
            stored(preset.id, 2),
        ];

        expect(matchStoredDefinition(preset, shuffled)?.version).toBe(3);
    });

    test("answers the one stored version alone", () => {
        const preset = PROVIDER_PRESETS[0];
        if (preset === undefined) throw new Error("the catalog is empty");

        expect(
            matchStoredDefinition(preset, [
                stored(preset.id, 1),
            ])?.version,
        ).toBe(1);
    });
});
