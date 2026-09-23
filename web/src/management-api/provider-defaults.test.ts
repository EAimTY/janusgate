import { describe, expect, it } from "vitest";
import {
    defaultModelPrefix,
    defaultProviderBaseName,
    suggestProviderName,
} from "./provider-defaults";

describe("defaultProviderBaseName", () => {
    it("names each kind after the account it connects", () => {
        expect(defaultProviderBaseName("codex_oauth")).toBe("codex");
        expect(defaultProviderBaseName("antigravity_oauth")).toBe("antigravity");
        expect(defaultProviderBaseName("openai_response")).toBe("openai");
        expect(defaultProviderBaseName("openai_chat_completion")).toBe("openai");
        expect(defaultProviderBaseName("gemini_generate_content")).toBe("gemini");
    });
});

describe("suggestProviderName", () => {
    it("uses the base name while it is free", () => {
        expect(suggestProviderName("codex_oauth", new Set())).toBe("codex");
    });

    it("counts up from the base name while taken names collide", () => {
        expect(
            suggestProviderName(
                "codex_oauth",
                new Set([
                    "codex",
                ]),
            ),
        ).toBe("codex-1");
        expect(
            suggestProviderName(
                "codex_oauth",
                new Set([
                    "codex",
                    "codex-1",
                ]),
            ),
        ).toBe("codex-2");
    });

    it("skips over gaps instead of reusing a freed suffix", () => {
        expect(
            suggestProviderName(
                "codex_oauth",
                new Set([
                    "codex",
                    "codex-2",
                ]),
            ),
        ).toBe("codex-1");
    });
});

describe("defaultModelPrefix", () => {
    it("is the provider name with one trailing slash", () => {
        expect(defaultModelPrefix("codex")).toBe("codex/");
    });

    it("is empty when there is no name to derive from", () => {
        expect(defaultModelPrefix("")).toBe("");
    });
});
