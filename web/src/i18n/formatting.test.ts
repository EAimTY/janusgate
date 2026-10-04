import { afterEach, describe, expect, it } from "vitest";
import {
    credentialExpiryLine,
    formatDuration,
    formatRefreshStamp,
    formatTokenCount,
    parseDateTimeInputToMicros,
} from "/src/format";
import { formatDate, formatNumber, formatRelative } from "./formatters";
import { setLanguagePreference } from "./index";

afterEach(async () => {
    await setLanguagePreference("en");
});
describe("locale display with invariant time arithmetic", () => {
    it("keeps duration thresholds and machine input unchanged while phrasing Chinese quantities", async () => {
        const machine = "2026-09-23T10:00";
        const original = parseDateTimeInputToMicros(machine);
        await setLanguagePreference("zh-CN");
        expect(formatDuration(59_999)).toBe("59.9 秒");
        expect(formatDuration(60_000)).toBe("1 分 00 秒");
        expect(formatDuration(3_725_000)).toBe("62 分 05 秒");
        expect(formatTokenCount(null)).toBe("—");
        expect(formatTokenCount(1_234_567)).toBe("1,234,567");
        const now = 1_800_000_000_000_000;
        expect(credentialExpiryLine(now, now)).toContain("过期");
        expect(credentialExpiryLine(now + 59_999_999, now)).toContain("不到一分钟");
        expect(credentialExpiryLine(now + 60_000_000, now)).toContain("1 分钟");
        expect(parseDateTimeInputToMicros(machine)).toBe(original);
        expect(formatRelative(-12, "second")).toBe("12秒钟前");
    });
    it("prints browser-local Gregorian instants and a 24-hour freshness clock at midnight", async () => {
        const stamp = new Date(2026, 8, 27, 23, 59, 59);
        const nextDay = new Date(2026, 8, 28, 0, 0, 1);
        for (const locale of [
            "en",
            "zh-CN",
        ] as const) {
            await setLanguagePreference(locale);
            expect(formatRefreshStamp(stamp, stamp)).toBe("23:59:59");
            expect(formatRefreshStamp(stamp, nextDay)).toBe(`${formatDate(stamp, "day")} 23:59:59`);
            expect(formatDate(nextDay, "clock")).toBe("00:00:01");
            expect(formatNumber(Number.MAX_VALUE, "percentage")).not.toMatch(/Infinity|NaN/);
        }
    });
});
