import { describe, expect, test } from "vitest";
import {
    baseUrlMessage,
    bearerSecretError,
    expectModelIdentifier,
    isBaseUrl,
    isBearerSecret,
    isRoutingToken,
    routingTokenError,
} from "./identifiers";

/**
 * The composition rules for machine-consumed management fields: printable
 * ASCII without whitespace everywhere, `/` and `:` additionally barred from
 * routing tokens, and the `http(s)` scheme required of a base URL — the
 * app rejecting exactly what the gateway would, before the round trip.
 */

describe("isRoutingToken", () => {
    test("accepts printable ASCII without whitespace, slash, or colon", () => {
        expect(isRoutingToken("gpt-4o_mini.2024")).toBe(true);
        expect(isRoutingToken("claude-3-5-sonnet")).toBe(true);
    });

    test("rejects the empty string and whitespace", () => {
        expect(isRoutingToken("")).toBe(false);
        expect(isRoutingToken("a b")).toBe(false);
        expect(isRoutingToken("\ttab")).toBe(false);
    });

    test("rejects slash and colon, the Gemini-path delimiters", () => {
        expect(isRoutingToken("models/gemini")).toBe(false);
        expect(isRoutingToken("gemini:generateContent")).toBe(false);
    });

    test("rejects non-ASCII and control characters", () => {
        expect(isRoutingToken("模型")).toBe(false);
        expect(isRoutingToken("line\nbreak")).toBe(false);
    });
});

describe("isBearerSecret", () => {
    test("accepts printable ASCII without whitespace", () => {
        expect(isBearerSecret("sk-a1!@#$%")).toBe(true);
    });

    test("rejects whitespace, non-ASCII, and the empty string", () => {
        expect(isBearerSecret("has space")).toBe(false);
        expect(isBearerSecret("ключ")).toBe(false);
        expect(isBearerSecret("")).toBe(false);
    });
});

describe("isBaseUrl", () => {
    test("requires the http or https scheme and a non-empty remainder", () => {
        expect(isBaseUrl("https://api.example.test/v1")).toBe(true);
        expect(isBaseUrl("http://localhost:8787")).toBe(true);
    });

    test("rejects foreign schemes and scheme-less addresses", () => {
        expect(isBaseUrl("ftp://files.example.test")).toBe(false);
        expect(isBaseUrl("api.example.test")).toBe(false);
        expect(isBaseUrl("https://")).toBe(false);
    });

    test("rejects whitespace and non-ASCII anywhere in the address", () => {
        expect(isBaseUrl("https://api.example.test/v1 has space")).toBe(false);
        expect(isBaseUrl("https://exämple.test")).toBe(false);
    });
});

describe("expectModelIdentifier", () => {
    test("answers a routing-safe identifier verbatim", () => {
        expect(expectModelIdentifier("gpt-4o", "response.public_id")).toBe("gpt-4o");
    });

    test("throws a contract violation naming the path for a non-token", () => {
        expect(() => expectModelIdentifier("a/b", "response.public_id")).toThrowError(
            "response.public_id is not a routing token: a/b",
        );
        expect(() => expectModelIdentifier("", "response.public_id")).toThrowError(
            "response.public_id must not be empty",
        );
    });
});

describe("bearerSecretError", () => {
    test("accepts the empty paste — the write-only field's keep-the-secret state", () => {
        expect(bearerSecretError("", "apiKey")).toBeNull();
    });

    test("accepts a well-composed secret", () => {
        expect(bearerSecretError("sk-static-1", "apiKey")).toBeNull();
    });

    test("names the field's composition wording for a broken secret", () => {
        expect(bearerSecretError("bad secret", "apiKey")).not.toBeNull();
    });
});

describe("routingTokenError", () => {
    test("accepts null and empty as the not-yet-typed state the submit gate names", () => {
        expect(routingTokenError(null, "upstreamId")).toBeNull();
        expect(routingTokenError("", "upstreamId")).toBeNull();
    });

    test("accepts a routing-safe value", () => {
        expect(routingTokenError("gemini-2.5-pro", "publicId")).toBeNull();
    });

    test("names the routing wording for a value the gateway would refuse", () => {
        expect(routingTokenError("a:b", "publicId")).not.toBeNull();
        expect(routingTokenError("a b", "upstreamId")).not.toBeNull();
    });
});

describe("baseUrlMessage", () => {
    test("states the composition rule the form shows where the URL was typed", () => {
        expect(baseUrlMessage()).toContain("http");
    });
});
