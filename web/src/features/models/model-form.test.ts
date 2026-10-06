import { describe, expect, it } from "vitest";
import { drainWrites, modelRow } from "/src/test-support/management-api";
import {
    compactionOverrideSelection,
    createCapabilityOverrideDraft,
    isSameCompactionOverride,
    isSameLevelSet,
    normalizedCompactionOverride,
    normalizedThinkingLevelsOverride,
    normalizedTokenCapOverride,
    tokenCapOverrideError,
} from "./model-form";

describe("normalizedThinkingLevelsOverride", () => {
    it("reads a switched-off override as none, so the discovered set stands", () => {
        expect(
            normalizedThinkingLevelsOverride(false, [
                "low",
            ]),
        ).toBeNull();
    });

    it("carries the selection while on, including the empty no-levels declaration", () => {
        expect(
            normalizedThinkingLevelsOverride(true, [
                "low",
                "high",
            ]),
        ).toEqual([
            "low",
            "high",
        ]);
        expect(normalizedThinkingLevelsOverride(true, [])).toEqual([]);
    });
});

describe("isSameLevelSet", () => {
    it("treats null against null as the same undeclared state", () => {
        expect(isSameLevelSet(null, null)).toBe(true);
    });

    it("treats a standing set against none as changed", () => {
        expect(
            isSameLevelSet(
                [
                    "low",
                ],
                null,
            ),
        ).toBe(false);
        expect(isSameLevelSet(null, [])).toBe(false);
    });

    it("ignores the picking order, since the gateway canonicalizes on write", () => {
        expect(
            isSameLevelSet(
                [
                    "low",
                    "high",
                ],
                [
                    "high",
                    "low",
                ],
            ),
        ).toBe(true);
        expect(
            isSameLevelSet(
                [
                    "low",
                ],
                [
                    "low",
                    "high",
                ],
            ),
        ).toBe(false);
    });
});

describe("normalizedTokenCapOverride", () => {
    it("reads a switched-off override as none, so the discovered value stands", () => {
        expect(normalizedTokenCapOverride(false, "200000")).toBeNull();
    });

    it("parses a positive integer while on", () => {
        expect(normalizedTokenCapOverride(true, " 200000 ")).toBe(200_000);
    });

    it("reads an empty or non-positive entry as none, leaving the error to hold the save", () => {
        expect(normalizedTokenCapOverride(true, "")).toBeNull();
        expect(normalizedTokenCapOverride(true, "0")).toBeNull();
        expect(normalizedTokenCapOverride(true, "-5")).toBeNull();
        expect(normalizedTokenCapOverride(true, "1.5")).toBeNull();
        expect(normalizedTokenCapOverride(true, "many")).toBeNull();
    });
});

describe("tokenCapOverrideError", () => {
    it("accepts a switched-off override without reading the field", () => {
        expect(tokenCapOverrideError(false, "junk", "maxInputOverride")).toBeNull();
    });

    it("accepts a positive integer while on", () => {
        expect(tokenCapOverrideError(true, "200000", "maxInputOverride")).toBeNull();
    });

    it("names the field for an empty or non-positive entry while on", () => {
        for (const entry of [
            "",
            "0",
            "-5",
            "1.5",
            "many",
        ]) {
            expect(tokenCapOverrideError(true, entry, "maxInputOverride")).toBe(
                "Max input tokens override must be a positive integer.",
            );
        }
    });
});

describe("compactionOverrideSelection", () => {
    it("opens a null map entirely unset", () => {
        expect(compactionOverrideSelection(null)).toEqual({
            responses_standalone_compact: "unset",
            responses_explicit_trigger: "unset",
            responses_automatic_compaction: "unset",
            messages_on_demand_compaction: "unset",
            messages_threshold_compaction: "unset",
        });
    });

    it("carries a stored map's members and leaves the rest unset", () => {
        expect(
            compactionOverrideSelection({
                messages_on_demand_compaction: "unknown",
            }),
        ).toEqual({
            responses_standalone_compact: "unset",
            responses_explicit_trigger: "unset",
            responses_automatic_compaction: "unset",
            messages_on_demand_compaction: "unknown",
            messages_threshold_compaction: "unset",
        });
    });
});

describe("normalizedCompactionOverride", () => {
    it("reads a switched-off override as none, so the discovered facts stand", () => {
        expect(normalizedCompactionOverride(false, compactionOverrideSelection(null))).toBeNull();
    });

    it("carries only the stated mechanisms while on, the unset ones left out", () => {
        const selected = compactionOverrideSelection(null);

        expect(
            normalizedCompactionOverride(true, {
                ...selected,
                responses_standalone_compact: "supported",
                messages_threshold_compaction: "unknown",
            }),
        ).toEqual({
            responses_standalone_compact: "supported",
            messages_threshold_compaction: "unknown",
        });
    });
});

describe("isSameCompactionOverride", () => {
    it("treats null against null as the same undeclared state", () => {
        expect(isSameCompactionOverride(null, null)).toBe(true);
    });

    it("treats a standing map against none as changed, whatever it states", () => {
        expect(isSameCompactionOverride({}, null)).toBe(false);
        expect(
            isSameCompactionOverride(null, {
                messages_on_demand_compaction: "supported",
            }),
        ).toBe(false);
    });

    it("compares the stated members, whatever order the keys were built in", () => {
        expect(
            isSameCompactionOverride(
                {
                    messages_on_demand_compaction: "supported",
                    responses_standalone_compact: "unknown",
                },
                {
                    responses_standalone_compact: "unknown",
                    messages_on_demand_compaction: "supported",
                },
            ),
        ).toBe(true);
        expect(
            isSameCompactionOverride(
                {
                    messages_on_demand_compaction: "supported",
                },
                {
                    messages_on_demand_compaction: "unknown",
                },
            ),
        ).toBe(false);
        expect(
            isSameCompactionOverride(
                {
                    messages_on_demand_compaction: "supported",
                },
                {
                    messages_on_demand_compaction: "supported",
                    responses_standalone_compact: "supported",
                },
            ),
        ).toBe(false);
    });
});

describe("createCapabilityOverrideDraft", () => {
    it("opens empty for a creation — every override off, every field clear", () => {
        const draft = createCapabilityOverrideDraft(null);

        expect(draft.hasLevelsOverride()).toBe(false);
        expect(draft.selectedLevels()).toEqual([]);
        expect(draft.hasMaxInputOverride()).toBe(false);
        expect(draft.maxInputText()).toBe("");
        expect(draft.hasMaxOutputOverride()).toBe(false);
        expect(draft.maxOutputText()).toBe("");
        expect(draft.hasCompactionOverride()).toBe(false);
        expect(draft.selectedCompaction()).toEqual(compactionOverrideSelection(null));
        expect(draft.normalizedLevels()).toBeNull();
        expect(draft.normalizedMaxInput()).toBeNull();
        expect(draft.normalizedMaxOutput()).toBeNull();
        expect(draft.normalizedCompaction()).toBeNull();
        expect(draft.maxInputError()).toBeNull();
        expect(draft.maxOutputError()).toBeNull();
    });

    it("seeds from a row's stored overrides and derives the normalized values and errors from them", async () => {
        const draft = createCapabilityOverrideDraft(
            modelRow({
                thinking_levels_override: [
                    "high",
                ],
                max_input_tokens_override: 4096,
                max_output_tokens_override: 16384,
                compaction_support_override: {
                    messages_on_demand_compaction: "supported",
                },
            }),
        );

        expect(draft.hasLevelsOverride()).toBe(true);
        expect(draft.selectedLevels()).toEqual([
            "high",
        ]);
        expect(draft.normalizedLevels()).toEqual([
            "high",
        ]);
        expect(draft.maxInputText()).toBe("4096");
        expect(draft.normalizedMaxInput()).toBe(4096);
        expect(draft.normalizedMaxOutput()).toBe(16384);
        expect(draft.selectedCompaction()).toEqual(
            compactionOverrideSelection({
                messages_on_demand_compaction: "supported",
            }),
        );
        expect(draft.normalizedCompaction()).toEqual({
            messages_on_demand_compaction: "supported",
        });

        // An invalid entry derives its inline error while the normalized
        // value reads none — the error holds the save, not the normalization.
        draft.setMaxInputText("nope");
        await drainWrites();
        expect(draft.maxInputError()).not.toBeNull();
        expect(draft.normalizedMaxInput()).toBeNull();
        draft.setMaxInputText("512");
        await drainWrites();
        expect(draft.maxInputError()).toBeNull();
        expect(draft.normalizedMaxInput()).toBe(512);
    });

    it("re-seeds the whole bundle from a row on apply", async () => {
        const draft = createCapabilityOverrideDraft(null);
        draft.setHasLevelsOverride(true);
        draft.setSelectedLevels([
            "high",
        ]);
        draft.setHasMaxInputOverride(true);
        draft.setMaxInputText("4096");
        await drainWrites();
        expect(draft.hasLevelsOverride()).toBe(true);
        expect(draft.maxInputText()).toBe("4096");

        draft.apply(
            modelRow({
                max_output_tokens_override: 2048,
            }),
        );
        await drainWrites();

        expect(draft.hasLevelsOverride()).toBe(false);
        expect(draft.selectedLevels()).toEqual([]);
        expect(draft.hasMaxInputOverride()).toBe(false);
        expect(draft.maxInputText()).toBe("");
        expect(draft.hasMaxOutputOverride()).toBe(true);
        expect(draft.maxOutputText()).toBe("2048");
        expect(draft.normalizedMaxOutput()).toBe(2048);
        expect(draft.normalizedCompaction()).toBeNull();
    });
});
