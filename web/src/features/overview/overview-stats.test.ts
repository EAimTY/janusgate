import { describe, expect, it } from "vitest";
import type { UsageBucket } from "/src/management-api/generated/v0";
import {
    buildOverviewDailySeries,
    OVERVIEW_DAILY_DAYS,
    overviewDailyWindow,
    overviewRecentFrom,
    overviewRecentTotals,
} from "./overview-stats";

/** An arbitrary instant late in a UTC day, so the window's day boundaries are visible. */
const NOW = Date.UTC(2026, 8, 24, 10, 30, 0) * 1000;

const DAY_MICROS = 86_400_000_000;

/** One bucket of the `usage:query` shape, carrying only what the overview reads. */
function bucket(key: Partial<UsageBucket["key"]>, requests: number, tokens = 0): UsageBucket {
    return {
        key,
        requests,
        input_tokens: tokens,
        output_tokens: tokens,
        cached_tokens: tokens,
        cache_write_tokens: tokens,
        reasoning_tokens: tokens,
    };
}

describe("overviewDailyWindow", () => {
    it("covers the last fourteen UTC days, today included", () => {
        const window = overviewDailyWindow(NOW);

        expect(window.to).toBe(Date.UTC(2026, 8, 25) * 1000);
        expect(window.from).toBe(Date.UTC(2026, 8, 11) * 1000);
        expect((window.to - window.from) / DAY_MICROS).toBe(OVERVIEW_DAILY_DAYS);
    });
});

describe("buildOverviewDailySeries", () => {
    it("places every day of the window on the axis, zero-filling the quiet ones", () => {
        const series = buildOverviewDailySeries(
            [
                bucket(
                    {
                        day: "2026-09-24",
                    },
                    5,
                ),
                bucket(
                    {
                        day: "2026-09-11",
                    },
                    2,
                ),
            ],
            NOW,
        );

        expect(series.points).toHaveLength(OVERVIEW_DAILY_DAYS);
        expect(series.points[0]).toEqual({
            day: "2026-09-11",
            requests: 2,
        });
        expect(series.points.at(-1)).toEqual({
            day: "2026-09-24",
            requests: 5,
        });
        expect(series.points[1]).toEqual({
            day: "2026-09-12",
            requests: 0,
        });
        expect(series.total).toBe(7);
        expect(series.peak).toEqual({
            day: "2026-09-24",
            requests: 5,
        });
    });

    it("answers no peak and a zero total for a window that recorded nothing", () => {
        const series = buildOverviewDailySeries([], NOW);

        expect(series.total).toBe(0);
        expect(series.peak).toBeNull();
        expect(series.points.every((point) => point.requests === 0)).toBe(true);
    });

    it("keeps the first day of the window when two rows answer for it", () => {
        const series = buildOverviewDailySeries(
            [
                bucket(
                    {
                        day: "2026-09-11",
                    },
                    2,
                ),
                bucket(
                    {
                        day: "2026-09-11",
                    },
                    3,
                ),
            ],
            NOW,
        );

        expect(series.points[0]?.requests).toBe(5);
        expect(series.total).toBe(5);
    });
});

describe("overviewRecentFrom", () => {
    it("reaches exactly one rolling day back", () => {
        expect(overviewRecentFrom(NOW)).toBe(NOW - DAY_MICROS);
    });
});

describe("overviewRecentTotals", () => {
    it("counts every recorded request and the failures among them", () => {
        const totals = overviewRecentTotals([
            bucket(
                {
                    outcome: "completed",
                },
                40,
            ),
            bucket(
                {
                    outcome: "client_aborted",
                },
                3,
            ),
            bucket(
                {
                    outcome: "upstream_error",
                },
                5,
            ),
            bucket(
                {
                    outcome: "internal",
                },
                1,
            ),
        ]);

        expect(totals).toEqual({
            requests: 49,
            errors: 6,
        });
    });

    it("counts a window that recorded nothing as zeroes", () => {
        expect(overviewRecentTotals([])).toEqual({
            requests: 0,
            errors: 0,
        });
    });
});
