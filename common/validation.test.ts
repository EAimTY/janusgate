import { expect, test } from "vitest";
import { ValidationError } from "/common/errors";
import {
    clampThinkingLevel,
    composeModelName,
    parseBaseUrl,
    parseModelPrefix,
    parseProviderName,
    parseThinkingLevels,
} from "/common/validation";
import {
    CANONICAL_THINKING_LEVELS,
    MODEL_PREFIX_MAX_LENGTH,
    PROVIDER_NAME_MAX_LENGTH,
} from "/config";

test("provider names implement the exact README character and length contract", () => {
    expect(parseProviderName("openai_work-1")).toBe("openai_work-1");
    expect(parseProviderName("a".repeat(PROVIDER_NAME_MAX_LENGTH))).toBe("a".repeat(64));
    for (const invalid of [
        "",
        "Uppercase",
        "has space",
        "a".repeat(65),
    ]) {
        expect(() => parseProviderName(invalid)).toThrow(ValidationError);
    }
});

test("model prefixes preserve caller separators and permit an empty prefix", () => {
    expect(parseModelPrefix("")).toBe("");
    expect(parseModelPrefix("openrouter/v1:")).toBe("openrouter/v1:");
    expect(parseModelPrefix("a".repeat(MODEL_PREFIX_MAX_LENGTH))).toBe("a".repeat(32));
    expect(() => parseModelPrefix("bad prefix")).toThrow(ValidationError);
});

test("base URLs are HTTP(S), canonicalized, and have no trailing slash", () => {
    expect(parseBaseUrl("HTTPS://Example.COM/api/v1///")).toBe("https://example.com/api/v1");
    expect(parseBaseUrl("http://localhost:8787/")).toBe("http://localhost:8787");
    expect(() => parseBaseUrl("ftp://example.com/api")).toThrow(ValidationError);
    expect(() => parseBaseUrl("not a URL")).toThrow(ValidationError);
});

test("thinking levels are canonical, unique, and returned in scale order", () => {
    expect(CANONICAL_THINKING_LEVELS).toEqual([
        "minimal",
        "low",
        "medium",
        "high",
        "xhigh",
    ]);
    expect(
        parseThinkingLevels([
            "high",
            "low",
        ]),
    ).toEqual([
        "low",
        "high",
    ]);
    expect(parseThinkingLevels([])).toEqual([]);
    expect(parseThinkingLevels(null)).toBeNull();
    expect(() =>
        parseThinkingLevels([
            "maximum",
        ]),
    ).toThrow(ValidationError);
    expect(() =>
        parseThinkingLevels([
            "low",
            "low",
        ]),
    ).toThrow(ValidationError);
});

test("reasoning effort clamps down only and empty support strips it", () => {
    expect(
        clampThinkingLevel("xhigh", [
            "low",
            "high",
        ]),
    ).toBe("high");
    expect(
        clampThinkingLevel("medium", [
            "minimal",
            "high",
        ]),
    ).toBe("minimal");
    expect(
        clampThinkingLevel("minimal", [
            "low",
            "medium",
        ]),
    ).toBeUndefined();
    expect(clampThinkingLevel("high", [])).toBeUndefined();
});

test("model composition is literal", () => {
    expect(composeModelName("openrouter/", "gpt-4")).toBe("openrouter/gpt-4");
    expect(composeModelName("", "gpt-4")).toBe("gpt-4");
});
