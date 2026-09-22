import { describe, expect, it } from "vitest";
import {
    baseUrlMessage,
    compositionMessage,
    isBaseUrl,
    isBearerSecret,
    isModelIdentifier,
    isModelPrefix,
} from "./identifiers";

describe("isModelIdentifier", () => {
    it("accepts the punctuation real model IDs carry", () => {
        for (const valid of [
            "gpt-4o",
            "qwen3:32b",
            "meta-llama/Llama-3.1-8B",
            "text-embedding-3-small",
            "codex/",
            "sk-live-123_~",
            "!",
        ]) {
            expect(isModelIdentifier(valid), valid).toBe(true);
        }
    });

    it("rejects emptiness, whitespace, and non-ASCII text", () => {
        for (const invalid of [
            "",
            "gpt 4o",
            " leading",
            "trailing ",
            "\ttab",
            "new\nline",
            "模型",
            "gpt-4o💥",
            "caf\x7fe",
        ]) {
            expect(isModelIdentifier(invalid), JSON.stringify(invalid)).toBe(false);
        }
    });
});

describe("isModelPrefix", () => {
    it("accepts empty as no prefix and the usual prefix shapes", () => {
        expect(isModelPrefix("")).toBe(true);
        expect(isModelPrefix("codex/")).toBe(true);
        expect(isModelPrefix("agy-")).toBe(true);
    });

    it("rejects whitespace and non-ASCII text in a set prefix", () => {
        expect(isModelPrefix("codex /")).toBe(false);
        expect(isModelPrefix("前缀/")).toBe(false);
    });
});

describe("isBearerSecret", () => {
    it("accepts generated and hand-typed secrets", () => {
        expect(isBearerSecret("sk-live-123_~-abcXYZ019")).toBe(true);
    });

    it("rejects emptiness, whitespace, and non-ASCII text", () => {
        expect(isBearerSecret("")).toBe(false);
        expect(isBearerSecret("has space")).toBe(false);
        expect(isBearerSecret("密钥")).toBe(false);
    });
});

describe("isBaseUrl", () => {
    it("accepts http(s) addresses", () => {
        expect(isBaseUrl("https://upstream.example.com/v1")).toBe(true);
        expect(isBaseUrl("http://localhost:8080")).toBe(true);
        expect(isBaseUrl("https://x.example/models?q=1")).toBe(true);
    });

    it("rejects other schemes, bare schemes, and whitespace or non-ASCII anywhere", () => {
        for (const invalid of [
            "",
            "https://",
            "http://",
            "ftp://upstream.example.com",
            "upstream.example.com/v1",
            "https://upstream.example.com/v 1",
            "https://例子.example.com",
        ]) {
            expect(isBaseUrl(invalid), invalid).toBe(false);
        }
    });
});

describe("messages", () => {
    it("names the rejected field", () => {
        expect(compositionMessage("Upstream ID")).toContain("Upstream ID");
        expect(baseUrlMessage()).toContain("http");
    });
});
