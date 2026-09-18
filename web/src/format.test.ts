import { describe, expect, it } from "vitest";
import { describeLevels, describeValidity, formatClock } from "./format";

describe("describeValidity", () => {
    const now = 1_700_000_000;

    it("reads a distant expiry in days and hours", () => {
        const validity = describeValidity(now + 3 * 86_400 + 4 * 3_600, now);

        expect(validity.isExpired).toBe(false);
        expect(validity.text).toBe("in 3d 4h");
    });

    it("drops a zero remainder rather than printing it", () => {
        expect(describeValidity(now + 2 * 86_400, now).text).toBe("in 2d");
        expect(describeValidity(now + 5 * 3_600, now).text).toBe("in 5h");
    });

    it("reads hours and minutes inside a day", () => {
        expect(describeValidity(now + 4 * 3_600 + 12 * 60, now).text).toBe("in 4h 12m");
        expect(describeValidity(now + 42 * 60, now).text).toBe("in 42m");
    });

    it("floors anything under a minute to the smallest unit", () => {
        expect(describeValidity(now + 30, now).text).toBe("in <1m");
        expect(describeValidity(now - 30, now).text).toBe("expired <1m ago");
    });

    it("reads a past expiry as expired with the same units", () => {
        const validity = describeValidity(now - 2 * 3_600 - 5 * 60, now);

        expect(validity.isExpired).toBe(true);
        expect(validity.text).toBe("expired 2h 5m ago");
    });

    it("reads the exact boundary as expired", () => {
        expect(describeValidity(now, now).isExpired).toBe(true);
    });
});

describe("describeLevels", () => {
    it("reads no override as passthrough", () => {
        expect(describeLevels(null)).toBe("passes through");
    });

    it("reads the whole ladder as all four", () => {
        expect(
            describeLevels([
                "off",
                "low",
                "medium",
                "high",
            ]),
        ).toBe("all four");
    });

    it("reads a single allowed level as only that level", () => {
        expect(
            describeLevels([
                "medium",
            ]),
        ).toBe("only medium");
    });

    it("reads a contiguous run as its span", () => {
        expect(
            describeLevels([
                "low",
                "medium",
                "high",
            ]),
        ).toBe("low–high");
        expect(
            describeLevels([
                "low",
                "medium",
            ]),
        ).toBe("low–medium");
    });

    it("lists a gapped set in canonical order, not input order", () => {
        expect(
            describeLevels([
                "high",
                "off",
            ]),
        ).toBe("off, high");
    });
});

describe("formatClock", () => {
    it("prints a wall clock of hours, minutes and seconds", () => {
        expect(formatClock(new Date(1_700_000_000 * 1000))).toMatch(/^\d{2}:\d{2}:\d{2}$/);
    });
});
