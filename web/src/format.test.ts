import { describe, expect, test } from "vitest";
import {
    credentialExpiryLine,
    DAY_MICROS,
    dateTimeInputValue,
    formatDuration,
    formatRefreshStamp,
    formatTokenCount,
    isCredentialExpired,
    nowMicros,
    parseDateTimeInputToMicros,
} from "./format";

/**
 * The time and count formatting the screens share: the freshness stamp's
 * today-versus-older shapes, the credential distance and expiry line's two
 * directions, the unknown-not-zero token count, the duration's tenth-floor
 * rule, and the datetime-local bounds' one spelling with its strict
 * round-trip parse.
 */

describe("clock primitives", () => {
    test("states the day and the current microsecond stamp", () => {
        expect(DAY_MICROS).toBe(86_400_000_000);
        // The stamp's contract is microsecond scale on the wall clock; a
        // millisecond-scale answer is off by three orders of magnitude, so a
        // one-second tolerance pins the scale while staying immune to the
        // scheduler jitter between the two clock reads.
        expect(Math.abs(nowMicros() - Date.now() * 1000)).toBeLessThan(1_000_000);
    });
});

describe("formatRefreshStamp", () => {
    test("answers the bare wall clock while the stamp sits on today's calendar day", () => {
        const now = new Date(2026, 9, 10, 12, 0, 0);
        const stamp = new Date(2026, 9, 10, 8, 30, 0);

        const rendered = formatRefreshStamp(stamp, now);

        // The same-day stamp carries no date part; the older one does.
        const older = formatRefreshStamp(new Date(2026, 9, 9, 8, 30, 0), now);
        expect(older.length).toBeGreaterThan(rendered.length);
        expect(older).toContain(rendered.slice(-8));
    });
});

describe("isCredentialExpired", () => {
    test("counts an expiry exactly at the reading moment as expired", () => {
        expect(isCredentialExpired(1000, 1001)).toBe(true);
        expect(isCredentialExpired(1001, 1000)).toBe(false);
        expect(isCredentialExpired(1000, 1000)).toBe(true);
    });
});

describe("credentialExpiryLine", () => {
    test("phrases a still-valid expiry with its distance and absolute instant", () => {
        const now = 1_800_000_000_000_000;
        const line = credentialExpiryLine(now + 2 * DAY_MICROS + 3_600_000_000, now);

        // The English catalog's own wording, the distance beside the instant.
        expect(line).toContain("Valid for");
        expect(line).toContain("2d 1h");
        expect(line).toContain("until");
    });

    test("phrases a spent expiry in the past tense", () => {
        const now = 1_800_000_000_000_000;
        const line = credentialExpiryLine(now - 90_000_000, now);

        expect(line).toContain("Expired");
        expect(line).toContain("ago");
    });

    test("drops the zero remainder and states the sub-minute distance", () => {
        const now = 1_800_000_000_000_000;

        expect(credentialExpiryLine(now + 30_000_000, now)).toContain("less than one minute");
        expect(credentialExpiryLine(now + 3 * DAY_MICROS, now)).not.toContain("0h");
        expect(credentialExpiryLine(now + 5 * 3_600_000_000, now)).toContain("5h");
    });
});

describe("formatTokenCount", () => {
    test("answers a dash for unreported usage, never a zero", () => {
        expect(formatTokenCount(null)).toBe("—");
        expect(formatTokenCount(0)).toBe("0");
        expect(formatTokenCount(1234)).toMatch(/1/);
    });
});

describe("formatDuration", () => {
    test("answers one tenth-floor second under a minute", () => {
        expect(formatDuration(0)).toContain("0s");
        expect(formatDuration(8_100)).toContain("8.1s");
        // Floored, never rounded up to a 60.0s the composite shape would rename.
        expect(formatDuration(59_999)).not.toContain("60");
        expect(formatDuration(59_999)).toContain("59.9s");
    });

    test("answers whole minutes and zero-padded seconds at and above the minute", () => {
        expect(formatDuration(60_000)).toContain("1m");
        expect(formatDuration(123_000)).toContain("2m");
        expect(formatDuration(123_000)).toContain("03s");
        // Minutes keep counting past the hour; no third unit appears.
        expect(formatDuration(3_600_000)).toContain("60m");
    });
});

describe("datetime-local bounds", () => {
    test("round-trips a local wall-clock value through the parser", () => {
        const date = new Date(2026, 8, 10, 14, 5);
        const value = dateTimeInputValue(date);

        expect(value).toBe("2026-09-10T14:05");
        expect(parseDateTimeInputToMicros(value)).toBe(date.getTime() * 1000);
    });

    test("answers null for blank, malformed, and impossible input", () => {
        expect(parseDateTimeInputToMicros("")).toBeNull();
        expect(parseDateTimeInputToMicros("2026-09-10")).toBeNull();
        expect(parseDateTimeInputToMicros("2026-09-10T14:05:00")).toBeNull();
        expect(parseDateTimeInputToMicros("2026-13-01T00:00")).toBeNull();
        expect(parseDateTimeInputToMicros("2026-02-30T00:00")).toBeNull();
    });

    test("keeps the value in this browser's zone", () => {
        const date = new Date(2026, 0, 1, 0, 0);

        expect(parseDateTimeInputToMicros(dateTimeInputValue(date))).toBe(date.getTime() * 1000);
    });
});
