import type { DefinitionPayload } from "../management-api/definitions/types";

/**
 * The preset catalog (`docs/design/providers.md`): hardcoded, versioned definition documents
 * — one per researched product — that prefill the provider-definition form.
 * The catalog is the only home for presets: the gateway ships no
 * provider-specific anything, and an operator without this frontend authors
 * definition JSON directly against the management API.
 *
 * Every preset carries its reference-implementation provenance and its
 * verification status. `live` means the shape was verified against the real
 * upstream from this codebase; `unverified` means the preset ships on
 * research alone, and flipping it to `live` requires a real upstream run.
 */

/** The verification status one preset carries. */
type PresetVerification = "live" | "unverified";

/** One preset: a full definition payload plus its presentation facts. */
export type ProviderPreset = {
    /** Stable catalog identifier. */
    readonly id: string;
    /** The product the preset connects. */
    readonly title: string;
    /** One line on what the operator needs beside the preset. */
    readonly description: string;
    /**
     * The grant an OAuth preset runs; `null` for a static-key preset, whose
     * payload credential is an `api_key` — the payload carries the kind.
     */
    readonly grant: "authorization_code" | "device" | null;
    /** Suggested provider name base. */
    readonly suggestedName: string;
    /** The reference-implementation provenance: repo + rev or doc URL. */
    readonly provenance: string;
    /** The verification status: live run or research only. */
    readonly verification: PresetVerification;
    /** The definition payload the form prefills, exactly as the wire carries it. */
    readonly payload: DefinitionPayload;
};
