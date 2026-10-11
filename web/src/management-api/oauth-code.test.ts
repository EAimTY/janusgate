import { describe, expect, test } from "vitest";
import { extractAuthorizationCode } from "./oauth-code";

/**
 * The pasted-callback reader: a bare code, a whole callback URL reduced to
 * its `code` parameter, and every paste mistake answered `null` so the flow
 * survives it — never sending the gateway a value it would burn the flow on.
 */

describe("extractAuthorizationCode", () => {
    test("answers a bare code verbatim", () => {
        expect(extractAuthorizationCode("abc_123-DEF")).toBe("abc_123-DEF");
    });

    test("trims surrounding whitespace around a bare code", () => {
        expect(extractAuthorizationCode("  bare-code\n")).toBe("bare-code");
    });

    test("reads the code parameter out of a pasted callback URL", () => {
        expect(
            extractAuthorizationCode("https://login.example.test/callback?state=s1&code=xyz789"),
        ).toBe("xyz789");
    });

    test("reads the fragment form some providers paste", () => {
        expect(extractAuthorizationCode("https://auth.example.test/cb#code=frag-code")).toBe(
            "frag-code",
        );
    });

    test("percent-decodes the code parameter", () => {
        expect(extractAuthorizationCode("https://cb.test?code=a%2Bb")).toBe("a+b");
    });

    test("answers null for an empty or whitespace-only paste", () => {
        expect(extractAuthorizationCode("")).toBeNull();
        expect(extractAuthorizationCode("   ")).toBeNull();
    });

    test("answers null when the URL carries an empty code parameter", () => {
        expect(extractAuthorizationCode("https://cb.test?code=")).toBeNull();
    });

    test("answers null for URL-structured paste holding no code parameter", () => {
        expect(extractAuthorizationCode("https://cb.test?state=s1&scope=read")).toBeNull();
    });

    test("answers null for a paste carrying whitespace or URL structure", () => {
        expect(extractAuthorizationCode("two codes here")).toBeNull();
        expect(extractAuthorizationCode("code?leftover")).toBeNull();
        expect(extractAuthorizationCode("code#leftover")).toBeNull();
        expect(extractAuthorizationCode("code&leftover")).toBeNull();
    });

    test("answers the raw value when the code parameter is malformed percent text", () => {
        expect(extractAuthorizationCode("https://cb.test?code=%ZZ")).toBe("%ZZ");
    });

    test("reads the `code=` prefix spelling without URL structure", () => {
        expect(extractAuthorizationCode("code=prefixed-value")).toBe("prefixed-value");
        expect(extractAuthorizationCode("code=")).toBeNull();
    });
});
