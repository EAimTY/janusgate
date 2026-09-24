import { describe, expect, it } from "vitest";
import {
    defaultModelPrefix,
    defaultProviderBaseName,
    suggestProviderName,
} from "./provider-defaults";

describe("defaultProviderBaseName", () => {
    it("names each format after the product family it connects", () => {
        expect(defaultProviderBaseName("openai_response")).toBe("openai");
        expect(defaultProviderBaseName("openai_chat_completion")).toBe("openai");
        expect(defaultProviderBaseName("gemini_generate_content")).toBe("gemini");
        expect(defaultProviderBaseName("gemini_interaction")).toBe("gemini");
        expect(defaultProviderBaseName("anthropic_messages")).toBe("anthropic");
    });
});

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

describe("defaultModelPrefix", () => {
    it("derives from the name with one trailing dash", () => {
        expect(defaultModelPrefix("codex")).toBe("codex-");
    });

    it("stays empty while there is no name", () => {
        expect(defaultModelPrefix("")).toBe("");
    });
});
