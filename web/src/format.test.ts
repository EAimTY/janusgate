import { describe, expect, it } from "vitest";
import {
    describeValidity,
    formatClock,
    formatMicrosForDateTimeInput,
    formatTokenCount,
    parseDateTimeInputToMicros,
} from "./format";

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

describe("formatTokenCount", () => {
    it("reads unreported usage as a dash, never zero", () => {
        expect(formatTokenCount(null)).toBe("—");
    });

    it("reads a reported zero as zero", () => {
        expect(formatTokenCount(0)).toBe("0");
    });

    it("groups large counts", () => {
        expect(formatTokenCount(1_234_567)).toBe("1,234,567");
    });
});

describe("date-time input bounds", () => {
    it("round-trips an instant through the input's minute precision", () => {
        // 2026-09-22T10:30 local, seconds truncated by the input.
        const micros = parseDateTimeInputToMicros("2026-09-22T10:30");

        expect(micros).not.toBeNull();

        const rendered = formatMicrosForDateTimeInput(micros ?? 0);
        expect(rendered).toBe("2026-09-22T10:30");
    });

    it("reads blank as no bound", () => {
        expect(parseDateTimeInputToMicros("")).toBeNull();
        expect(parseDateTimeInputToMicros("   ")).toBeNull();
    });

    it("reads malformed input as no bound", () => {
        expect(parseDateTimeInputToMicros("yesterday")).toBeNull();
        expect(parseDateTimeInputToMicros("2026-13-40T25:70")).toBeNull();
    });
});

describe("formatClock", () => {
    it("prints a wall clock of hours, minutes and seconds", () => {
        expect(formatClock(new Date(1_700_000_000 * 1000))).toMatch(/^\d{2}:\d{2}:\d{2}$/);
    });
});
