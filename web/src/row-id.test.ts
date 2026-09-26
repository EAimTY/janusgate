import { describe, expect, it } from "vitest";
import { parseRowIdentifier } from "./row-id";

describe("parseRowIdentifier", () => {
    it("accepts plain decimal integers", () => {
        expect(parseRowIdentifier("12")).toBe(12);
    });

    it("reads a digit string beyond the safe-integer range as absent", () => {
        // The shape check alone accepts it; the safe-integer bound is what
        // names no row the gateway could hold.
        expect(parseRowIdentifier("99999999999999999999")).toBeNull();
        expect(parseRowIdentifier("9007199254740992")).toBeNull();
        expect(parseRowIdentifier("9007199254740991")).toBe(9007199254740991);
    });

    it("rejects everything else", () => {
        expect(parseRowIdentifier(null)).toBeNull();
        expect(parseRowIdentifier("")).toBeNull();
        expect(parseRowIdentifier("codex")).toBeNull();
        expect(parseRowIdentifier("12x")).toBeNull();
        expect(parseRowIdentifier("-3")).toBeNull();
        expect(parseRowIdentifier("1.5")).toBeNull();
    });
});
