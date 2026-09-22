import { describe, expect, it } from "vitest";
import { extractAuthorizationCode } from "./oauth-code";

describe("extractAuthorizationCode", () => {
    it("passes a bare code through, trimming padding whitespace", () => {
        expect(extractAuthorizationCode("ac_clean.code-123_~")).toBe("ac_clean.code-123_~");
        expect(extractAuthorizationCode("  ac_padded  ")).toBe("ac_padded");
    });

    it("reads the code out of a pasted callback URL", () => {
        expect(
            extractAuthorizationCode("http://localhost:1455/auth/callback?code=ac_abc&state=xyz"),
        ).toBe("ac_abc");
        expect(
            extractAuthorizationCode("https://app.example/cb?state=xyz&code=ac_mid&other=1"),
        ).toBe("ac_mid");
        expect(extractAuthorizationCode("https://app.example/cb#code=ac_frag&state=xyz")).toBe(
            "ac_frag",
        );
    });

    it("decodes a percent-encoded code from the URL", () => {
        expect(extractAuthorizationCode("https://app.example/cb?code=ac%2Fabc%3D%3D")).toBe(
            "ac/abc==",
        );
    });

    it("reads a bare code= fragment", () => {
        expect(extractAuthorizationCode("code=ac_bare")).toBe("ac_bare");
    });

    it("rejects an empty paste and a URL carrying no code", () => {
        expect(extractAuthorizationCode("")).toBeNull();
        expect(extractAuthorizationCode("   ")).toBeNull();
        expect(extractAuthorizationCode("https://app.example/cb?state=xyz")).toBeNull();
        expect(extractAuthorizationCode("https://app.example/cb?code=")).toBeNull();
    });

    it("rejects a paste carrying URL structure but no readable code parameter", () => {
        expect(extractAuthorizationCode("ac_abc def")).toBeNull();
        expect(extractAuthorizationCode("ac_abc\ndef")).toBeNull();
        expect(extractAuthorizationCode("https://app.example/cb?state=x y")).toBeNull();
    });
});
