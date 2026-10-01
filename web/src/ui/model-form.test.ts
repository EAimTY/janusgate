import { describe, expect, it } from "vitest";
import {
    composedPublicId,
    isSameLevelSet,
    normalizedPublicIdOverride,
    normalizedThinkingLevelsOverride,
    normalizedTokenCapOverride,
    tokenCapOverrideError,
} from "./model-form";

describe("normalizedPublicIdOverride", () => {
    it("carries the trimmed field only while the switch is on and names something", () => {
        expect(normalizedPublicIdOverride(true, " custom-gpt ")).toBe("custom-gpt");
    });

    it("reads a switched-off or empty override as none, so the composed ID stands", () => {
        expect(normalizedPublicIdOverride(false, "custom-gpt")).toBeNull();
        expect(normalizedPublicIdOverride(true, "   ")).toBeNull();
    });
});

describe("composedPublicId", () => {
    it("joins the provider's prefix and the upstream ID when no override stands", () => {
        expect(composedPublicId("alpha-", "gpt-x", null)).toBe("alpha-gpt-x");
    });

    it("lets a standing override take the public ID's place verbatim", () => {
        expect(composedPublicId("alpha-", "gpt-x", "custom-gpt")).toBe("custom-gpt");
    });
});

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
        expect(tokenCapOverrideError(false, "junk", "Max input tokens override")).toBeNull();
    });

    it("accepts a positive integer while on", () => {
        expect(tokenCapOverrideError(true, "200000", "Max input tokens override")).toBeNull();
    });

    it("names the field for an empty or non-positive entry while on", () => {
        for (const entry of [
            "",
            "0",
            "-5",
            "1.5",
            "many",
        ]) {
            expect(tokenCapOverrideError(true, entry, "Max input tokens override")).toBe(
                "Max input tokens override must be a positive integer.",
            );
        }
    });
});
