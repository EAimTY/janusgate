import { describe, expect, it } from "vitest";
import {
    credentialExpiryLine,
    dateTimeInputValue,
    formatClock,
    formatDuration,
    formatRefreshStamp,
    formatTokenCount,
    isCredentialExpired,
    nowMicros,
    parseDateTimeInputToMicros,
} from "./format";

describe("credentialExpiryLine", () => {
    // A real-world microsecond magnitude: late 2027 at microsecond scale.
    const now = 1_800_000_000_000_000;
    const seconds = 1_000_000;

    it("reads a distant expiry as valid, in days and hours beside the instant", () => {
        const line = credentialExpiryLine(now + 3 * 86_400 * seconds + 4 * 3_600 * seconds, now);

        expect(line).toMatch(/^Valid for 3d 4h — until \d{4}-\d{2}-\d{2} \d{2}:\d{2}\.$/);
    });

    it("reads a past expiry as expired with the same units beside the instant", () => {
        const line = credentialExpiryLine(now - 2 * 3_600 * seconds - 5 * 60 * seconds, now);

        expect(line).toMatch(/^Expired 2h 5m ago — on \d{4}-\d{2}-\d{2} \d{2}:\d{2}\.$/);
    });

    it("drops a zero remainder rather than printing it", () => {
        expect(credentialExpiryLine(now + 2 * 86_400 * seconds, now)).toMatch(
            /^Valid for 2d — until/,
        );
        expect(credentialExpiryLine(now + 5 * 3_600 * seconds, now)).toMatch(
            /^Valid for 5h — until/,
        );
    });

    it("reads hours and minutes inside a day", () => {
        expect(credentialExpiryLine(now + 4 * 3_600 * seconds + 12 * 60 * seconds, now)).toMatch(
            /^Valid for 4h 12m — until/,
        );
        expect(credentialExpiryLine(now + 42 * 60 * seconds, now)).toMatch(
            /^Valid for 42m — until/,
        );
    });

    it("floors anything under a minute to the smallest unit", () => {
        expect(credentialExpiryLine(now + 30 * seconds, now)).toMatch(/^Valid for <1m — until/);
        expect(credentialExpiryLine(now - 30 * seconds, now)).toMatch(/^Expired <1m ago — on/);
    });

    it("reads the exact boundary as expired", () => {
        expect(credentialExpiryLine(now, now)).toMatch(/^Expired <1m ago — on/);
    });

    it("states the expiry's own instant in this browser's zone at minute precision", () => {
        // The date-time bounds' own spelling, with the input separator traded
        // for a space, is the oracle: the instant the line names must be the
        // expiry's, never the reading's, and the shape must match it exactly.
        const expiresAt = now + 3 * 3_600 * seconds;
        const expectedInstant = dateTimeInputValue(new Date(expiresAt / 1000)).replace("T", " ");

        expect(credentialExpiryLine(expiresAt, now)).toBe(
            `Valid for 3h — until ${expectedInstant}.`,
        );
    });
});

describe("isCredentialExpired", () => {
    const now = 1_800_000_000_000_000;

    it("reads a future expiry as unexpired", () => {
        expect(isCredentialExpired(now + 60 * 1_000_000, now)).toBe(false);
    });

    it("reads a past expiry and the exact boundary as expired", () => {
        expect(isCredentialExpired(now - 60 * 1_000_000, now)).toBe(true);
        expect(isCredentialExpired(now, now)).toBe(true);
    });
});

describe("nowMicros", () => {
    it("reads the current moment at microsecond scale, not seconds", () => {
        expect(nowMicros()).toBeGreaterThan(1_000_000_000_000_000);
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

describe("formatDuration", () => {
    it("prints a duration under a minute as one decimal of seconds", () => {
        expect(formatDuration(8_100)).toBe("8.1s");
        expect(formatDuration(120)).toBe("0.1s");
        expect(formatDuration(0)).toBe("0.0s");
    });

    it("floors the sub-minute tenth rather than rolling over to the boundary", () => {
        // 59_999 ms rounds up to a full minute's worth of tenths; the
        // sub-minute shape must not print "60.0s" where the next shape calls
        // the same duration "1m 00s".
        expect(formatDuration(59_999)).toBe("59.9s");
    });

    it("prints a minute and above as minutes and zero-padded seconds", () => {
        expect(formatDuration(60_000)).toBe("1m 00s");
        expect(formatDuration(123_000)).toBe("2m 03s");
    });

    it("keeps counting in minutes past the hour", () => {
        expect(formatDuration(3_725_000)).toBe("62m 05s");
    });
});

describe("date-time input bounds", () => {
    it("reads a well-formed stamp as a bound at microsecond scale", () => {
        expect(parseDateTimeInputToMicros("2026-09-22T10:30")).not.toBeNull();
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

describe("formatRefreshStamp", () => {
    const now = new Date(2026, 8, 27, 14, 0, 0);

    it("prints the bare wall clock for a stamp on today's calendar day", () => {
        const stamp = formatRefreshStamp(new Date(2026, 8, 27, 13, 59, 30), now);

        expect(stamp).toMatch(/^\d{2}:\d{2}:\d{2}$/);
    });

    it("carries the date beside the clock for a stamp from an earlier day", () => {
        const stamp = formatRefreshStamp(new Date(2026, 8, 26, 14, 0, 0), now);

        expect(stamp).toContain("2026");
        expect(stamp).toMatch(/\d{2}:\d{2}:\d{2}$/);
    });

    it("dates a stamp the calendar day has left, even seconds past midnight", () => {
        const midnight = new Date(2026, 8, 28, 0, 0, 30);

        expect(formatRefreshStamp(new Date(2026, 8, 27, 23, 59, 59), midnight)).toContain("2026");
    });
});
