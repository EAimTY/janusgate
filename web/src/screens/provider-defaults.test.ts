import { describe, expect, it } from "vitest";
import { suggestProviderName } from "./provider-defaults";

describe("suggestProviderName", () => {
    it("uses the base name while it is free", () => {
        expect(suggestProviderName("codex", new Set())).toBe("codex");
    });

    it("counts up from the base name while taken names collide", () => {
        expect(
            suggestProviderName(
                "codex",
                new Set([
                    "codex",
                ]),
            ),
        ).toBe("codex-1");
    });

    it("reuses the lowest free suffix", () => {
        expect(
            suggestProviderName(
                "codex",
                new Set([
                    "codex",
                    "codex-1",
                    "codex-3",
                ]),
            ),
        ).toBe("codex-2");
    });
});
