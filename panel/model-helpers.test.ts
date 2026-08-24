import { describe, expect, it } from "vitest";
import {
    composeModelName,
    isThinkingLevel,
    previewModelName,
    toggleThinkingLevel,
} from "./model-helpers";

describe("model helpers", () => {
    it("composes model names with or without a prefix", () => {
        expect(composeModelName("openrouter/", "gpt-4")).toBe("openrouter/gpt-4");
        expect(composeModelName("", "gpt-4")).toBe("gpt-4");
    });

    it("uses the upstream name when the optional base is omitted", () => {
        expect(
            previewModelName({
                baseName: "",
                existingName: null,
                modelPrefix: "provider/",
                upstreamName: "upstream",
            }),
        ).toBe("provider/upstream");
    });

    it("keeps an existing name until an explicit destructive rename", () => {
        expect(
            previewModelName({
                baseName: "",
                existingName: "old/model",
                modelPrefix: "new/",
                upstreamName: "upstream",
            }),
        ).toBe("old/model");

        expect(
            previewModelName({
                baseName: "public",
                existingName: "old/model",
                modelPrefix: "provider/",
                upstreamName: "upstream",
            }),
        ).toBe("provider/public");
    });

    it("recognizes only canonical thinking levels", () => {
        expect(isThinkingLevel("xhigh")).toBe(true);
        expect(isThinkingLevel("off")).toBe(false);
    });

    it("toggles levels in canonical order", () => {
        expect(
            toggleThinkingLevel(
                [
                    "high",
                    "low",
                ],
                "medium",
                true,
            ),
        ).toEqual([
            "low",
            "medium",
            "high",
        ]);
        expect(
            toggleThinkingLevel(
                [
                    "minimal",
                    "high",
                ],
                "minimal",
                false,
            ),
        ).toEqual([
            "high",
        ]);
    });
});
