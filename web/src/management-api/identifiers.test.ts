import { describe, expect, it } from "vitest";
import {
    baseUrlMessage,
    bearerSecretError,
    compositionMessage,
    isBaseUrl,
    isBearerSecret,
    isModelPrefix,
    isRoutingToken,
    modelPrefixError,
    routingTokenError,
    routingTokenMessage,
} from "./identifiers";

describe("isModelPrefix", () => {
    it("accepts empty as no prefix and the usual prefix shapes", () => {
        expect(isModelPrefix("")).toBe(true);
        expect(isModelPrefix("codex-")).toBe(true);
        expect(isModelPrefix("agy-")).toBe(true);
    });

    it("rejects whitespace, non-ASCII text, slashes, and colons in a set prefix", () => {
        expect(isModelPrefix("codex /")).toBe(false);
        expect(isModelPrefix("前缀-")).toBe(false);
        expect(isModelPrefix("codex/")).toBe(false);
        expect(isModelPrefix("agy/")).toBe(false);
        expect(isModelPrefix("qwen:32b")).toBe(false);
    });
});

describe("isRoutingToken", () => {
    it("accepts plain routing tokens", () => {
        expect(isRoutingToken("gpt-4o")).toBe(true);
        expect(isRoutingToken("custom-model_1.2")).toBe(true);
    });

    it("rejects slashes and colons beside the generic composition", () => {
        for (const invalid of [
            "qwen3:32b",
            "meta-llama/Llama-3.1-8B",
            "codex/",
            "a:b",
            "a/b",
            "has space",
            "模型",
            "",
        ]) {
            expect(isRoutingToken(invalid), invalid).toBe(false);
        }
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
        expect(routingTokenMessage("Upstream ID")).toContain("Upstream ID");
        expect(baseUrlMessage()).toContain("http");
    });
});

describe("bearerSecretError", () => {
    it("reads an empty secret as the kept stored one", () => {
        expect(bearerSecretError("", "Key")).toBeNull();
    });

    it("accepts a composed secret and rejects the rest, naming the field", () => {
        expect(bearerSecretError("sk-live-123_~-abcXYZ019", "Key")).toBeNull();
        expect(bearerSecretError("has space", "Key")).toBe(compositionMessage("Key"));
    });
});

describe("routingTokenError", () => {
    it("reads an absent or empty value as nothing to reject", () => {
        expect(routingTokenError(null, "Upstream ID")).toBeNull();
        expect(routingTokenError("", "Upstream ID")).toBeNull();
    });

    it("accepts a routing token and rejects the rest, naming the field", () => {
        expect(routingTokenError("gpt-4o", "Upstream ID")).toBeNull();
        expect(routingTokenError("meta-llama/Llama-3.1-8B", "Upstream ID")).toBe(
            routingTokenMessage("Upstream ID"),
        );
    });
});

describe("modelPrefixError", () => {
    it("reads an empty prefix as no prefix", () => {
        expect(modelPrefixError("")).toBeNull();
    });

    it("accepts a prefix and rejects the rest", () => {
        expect(modelPrefixError("codex-")).toBeNull();
        expect(modelPrefixError("qwen:32b")).toBe(routingTokenMessage("Model prefix"));
    });
});
