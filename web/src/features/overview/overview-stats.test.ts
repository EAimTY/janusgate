import { describe, expect, test } from "vitest";
import { DAY_MICROS } from "/src/format";
import type { UsageBucket } from "/src/management-api/generated/v0";
import {
    buildOverviewDailySeries,
    overviewDailyWindow,
    overviewRecentFrom,
    overviewRecentTotals,
} from "./overview-stats";

/**
 * The overview's rolling-window figures: the 14-UTC-day window today
 * included, the daily series placing every day of the window on the axis
 * (a gap is a quiet day, not a missing bar), and the two rolling figures
 * counting the failures under the shared outcome definition.
 */

/** The reading instant: 2026-10-10T12:00:00Z, mid-UTC-day. */
const NOW = Date.UTC(2026, 9, 10, 12, 0, 0) * 1000;

function bucket(
    day: string,
    requests: number,
    outcome: UsageBucket["key"]["outcome"],
): UsageBucket {
    return {
        key:
            outcome === undefined
                ? {
                      day,
                  }
                : {
                      day,
                      outcome,
                  },
        requests,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

describe("overviewDailyWindow", () => {
    test("covers the last 14 UTC days, today included, ending at tomorrow's start", () => {
        const window = overviewDailyWindow(NOW);

        // From: today's start minus 13 days. To: tomorrow's start.
        expect(window.to - window.from).toBe(14 * DAY_MICROS);
        expect(NOW - window.from).toBeLessThan(14 * DAY_MICROS);
        expect(window.to).toBeGreaterThan(NOW);
        // The lower bound is a UTC midnight.
        expect(window.from % DAY_MICROS).toBe(0);
    });
});

describe("overviewRecentFrom", () => {
    test("reaches one rolling day back from the reading instant", () => {
        expect(overviewRecentFrom(NOW)).toBe(NOW - DAY_MICROS);
    });
});

describe("buildOverviewDailySeries", () => {
    test("places every window day in order, quiet days reading zero", () => {
        const series = buildOverviewDailySeries(
            [
                bucket("2026-10-10", 5, undefined),
            ],
            NOW,
        );

        expect(series.points).toHaveLength(14);
        const today = series.points.at(-1);
        expect(today?.day).toBe("2026-10-10");
        expect(today?.requests).toBe(5);
        expect(series.points[0]?.requests).toBe(0);
        expect(series.total).toBe(5);
        expect(series.peak).toEqual({
            day: "2026-10-10",
            requests: 5,
        });
    });

    test("sums buckets that share a day and picks the busiest day as the peak", () => {
        const series = buildOverviewDailySeries(
            [
                bucket("2026-10-09", 3, undefined),
                bucket("2026-10-09", 4, undefined),
                bucket("2026-10-05", 10, undefined),
            ],
            NOW,
        );

        expect(series.total).toBe(17);
        expect(series.peak).toEqual({
            day: "2026-10-05",
            requests: 10,
        });
    });

    test("answers no peak and a zero total for a quiet window", () => {
        const series = buildOverviewDailySeries([], NOW);

        expect(series.total).toBe(0);
        expect(series.peak).toBeNull();
    });

    test("never invents days beyond the window", () => {
        const series = buildOverviewDailySeries(
            [
                bucket("2026-09-01", 100, undefined),
                bucket("2026-11-01", 100, undefined),
            ],
            NOW,
        );

        expect(series.total).toBe(0);
    });
});

describe("overviewRecentTotals", () => {
    test("counts every request and the failures under the listing's failure set", () => {
        const totals = overviewRecentTotals([
            bucket("2026-10-10", 3, "completed"),
            bucket("2026-10-10", 2, "upstream_error"),
            bucket("2026-10-10", 1, "internal"),
            bucket("2026-10-10", 4, "client_aborted"),
        ]);

        expect(totals.requests).toBe(10);
        expect(totals.errors).toBe(3);
    });

    test("answers zeros for an empty window", () => {
        expect(overviewRecentTotals([])).toEqual({
            requests: 0,
            errors: 0,
        });
    });
});
