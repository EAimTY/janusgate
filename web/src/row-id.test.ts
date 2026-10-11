import { describe, expect, test } from "vitest";
import { parseRowIdentifier } from "./row-id";

/**
 * The row-identifier route-reading rule, stated once for every place a route
 * carries a row: only a plain decimal integer within the safe-integer range
 * addresses a row; anything else reads as absent.
 */

describe("parseRowIdentifier", () => {
    test("reads a plain decimal integer", () => {
        expect(parseRowIdentifier("1")).toBe(1);
        expect(parseRowIdentifier("9007199254740991")).toBe(Number.MAX_SAFE_INTEGER);
        // Leading zeros are still one decimal spelling.
        expect(parseRowIdentifier("007")).toBe(7);
    });

    test("answers null for null, empty, and signed input", () => {
        expect(parseRowIdentifier(null)).toBeNull();
        expect(parseRowIdentifier("")).toBeNull();
        expect(parseRowIdentifier("-1")).toBeNull();
        expect(parseRowIdentifier("+1")).toBeNull();
    });

    test("answers null for spellings Number would accept that name no row", () => {
        expect(parseRowIdentifier("1e3")).toBeNull();
        expect(parseRowIdentifier("0x10")).toBeNull();
        expect(parseRowIdentifier("1.5")).toBeNull();
        expect(parseRowIdentifier(" 1")).toBeNull();
        expect(parseRowIdentifier("1 ")).toBeNull();
    });

    test("answers null beyond the safe-integer range", () => {
        expect(parseRowIdentifier("9007199254740992")).toBeNull();
        expect(parseRowIdentifier(`${"9".repeat(25)}`)).toBeNull();
    });
});
