import { describe, expect, it } from "vitest";
import { composedPublicId, normalizedPublicIdOverride } from "./model-form";

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
