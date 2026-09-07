import { describe, expect, it } from "vitest";
import { composeModelName, isThinkingLevel } from "/common/validation";
import { previewModelName, toggleThinkingLevel } from "./model-helpers";

describe("model helpers", () => {
    it("composes model names with or without a prefix", () => {
        expect(composeModelName("openrouter/", "gpt-4")).toBe("openrouter/gpt-4");
        expect(composeModelName("", "gpt-4")).toBe("gpt-4");
        // The shared composition is pure: an empty base yields the bare prefix instead of
        // throwing, so live previews keep working while the user types.
        expect(composeModelName("openrouter/", "")).toBe("openrouter/");
    });

    it("uses the upstream name when the optional base is omitted", () => {
        expect(
            previewModelName({
                baseName: "",
                modelPrefix: "provider/",
                upstreamName: "upstream",
            }),
        ).toBe("provider/upstream");
    });

    it("prefers the override base over the upstream name", () => {
        expect(
            previewModelName({
                baseName: "public",
                modelPrefix: "provider/",
                upstreamName: "upstream",
            }),
        ).toBe("provider/public");

        expect(
            previewModelName({
                baseName: "public",
                modelPrefix: "",
                upstreamName: "upstream",
            }),
        ).toBe("public");
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
