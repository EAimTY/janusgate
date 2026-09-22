import { describe, expect, it } from "vitest";
import {
    credentialExpiryLine,
    formatClock,
    formatTokenCount,
    isCredentialExpired,
    nowMicros,
    parseDateTimeInputToMicros,
} from "./format";

describe("credentialExpiryLine", () => {
    // A real-world microsecond magnitude: late 2027 at microsecond scale.
    const now = 1_800_000_000_000_000;
    const seconds = 1_000_000;

    it("reads a distant expiry as valid, in days and hours", () => {
        const line = credentialExpiryLine(now + 3 * 86_400 * seconds + 4 * 3_600 * seconds, now);

        expect(line).toBe("Valid for 3d 4h.");
    });

    it("reads a past expiry as expired with the same units", () => {
        const line = credentialExpiryLine(now - 2 * 3_600 * seconds - 5 * 60 * seconds, now);

        expect(line).toBe("Expired 2h 5m ago.");
    });

    it("drops a zero remainder rather than printing it", () => {
        expect(credentialExpiryLine(now + 2 * 86_400 * seconds, now)).toBe("Valid for 2d.");
        expect(credentialExpiryLine(now + 5 * 3_600 * seconds, now)).toBe("Valid for 5h.");
    });

    it("reads hours and minutes inside a day", () => {
        expect(credentialExpiryLine(now + 4 * 3_600 * seconds + 12 * 60 * seconds, now)).toBe(
            "Valid for 4h 12m.",
        );
        expect(credentialExpiryLine(now + 42 * 60 * seconds, now)).toBe("Valid for 42m.");
    });

    it("floors anything under a minute to the smallest unit", () => {
        expect(credentialExpiryLine(now + 30 * seconds, now)).toBe("Valid for <1m.");
        expect(credentialExpiryLine(now - 30 * seconds, now)).toBe("Expired <1m ago.");
    });

    it("reads the exact boundary as expired", () => {
        expect(credentialExpiryLine(now, now)).toBe("Expired <1m ago.");
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
