import { DAY_MICROS } from "/src/format";
import type { UsageBucket } from "/src/management-api/generated/v0";
import { bucketKeyField } from "/src/management-api/usage-buckets";
import { USAGE_OUTCOME_FAILURES } from "../usage/filter";

/** The number of UTC days the overview's daily chart covers, today included. */
const OVERVIEW_DAILY_DAYS = 14;

/** One day's bar: the UTC day and the requests recorded inside it. */
type OverviewDailyPoint = {
    readonly day: string;
    readonly requests: number;
};

/** The overview's daily series: every day of the window, in order, quiet days included. */
export type OverviewDailySeries = {
    readonly points: ReadonlyArray<OverviewDailyPoint>;
    readonly total: number;
    /** The busiest day of the window, or `null` when the window recorded nothing. */
    readonly peak: OverviewDailyPoint | null;
};

/** The two figures the overview's rolling window states. */
export type OverviewRecentTotals = {
    readonly requests: number;
    readonly errors: number;
};

/**
 * The window the daily series covers: the last `OVERVIEW_DAILY_DAYS` UTC days,
 * today included. The day is the gateway's own — its `day` dimension is a UTC
 * day — so the axis matches the per-day figures every detail page prints.
 */
export function overviewDailyWindow(nowMicros: number): {
    readonly from: number;
    readonly to: number;
} {
    const todayStart = Math.floor(nowMicros / DAY_MICROS) * DAY_MICROS;

    return {
        from: todayStart - (OVERVIEW_DAILY_DAYS - 1) * DAY_MICROS,
        to: todayStart + DAY_MICROS,
    };
}

/** How far back the overview's recent figures reach: one rolling day. */
const OVERVIEW_RECENT_WINDOW_MICROS = DAY_MICROS;

/** The inclusive lower bound of the overview's rolling figures. */
export function overviewRecentFrom(nowMicros: number): number {
    return nowMicros - OVERVIEW_RECENT_WINDOW_MICROS;
}

/**
 * The 14-day series a `usage:query` by day answers: the gateway lists only the
 * days that recorded rows, so every day of the window is placed on the axis and
 * the missing ones read as zero — a gap is a quiet day, not a missing bar.
 *
 * The buckets arrive through `readUsageBuckets`, which proved inside the read
 * cycle that every key carries the `day` its request asked for; the
 * `bucketKeyField` read below is the fatal assert on that proven invariant,
 * never the guard that would discover a gateway/bindings skew.
 */
export function buildOverviewDailySeries(
    buckets: ReadonlyArray<UsageBucket>,
    nowMicros: number,
): OverviewDailySeries {
    const { from } = overviewDailyWindow(nowMicros);
    const requestsByDay = new Map<string, number>();

    for (const bucket of buckets) {
        const day = bucketKeyField(bucket.key.day, "day");
        requestsByDay.set(day, (requestsByDay.get(day) ?? 0) + bucket.requests);
    }

    const points = Array.from(
        {
            length: OVERVIEW_DAILY_DAYS,
        },
        (_, index) => {
            const day = new Date((from + index * DAY_MICROS) / 1000).toISOString().slice(0, 10);

            return {
                day,
                requests: requestsByDay.get(day) ?? 0,
            };
        },
    );

    let total = 0;
    let peak: OverviewDailyPoint | null = null;

    for (const point of points) {
        total += point.requests;

        if (point.requests > 0 && (peak === null || point.requests > peak.requests)) {
            peak = point;
        }
    }

    return {
        points,
        total,
        peak,
    };
}

/**
 * The overview's two rolling figures out of one `usage:query` by outcome: every
 * request recorded in the window, and the failures among them under the shared
 * definition the listing's "Failures" filter names. As in
 * `buildOverviewDailySeries`, the keys' `outcome` parts were proven present by
 * `readUsageBuckets`; the `bucketKeyField` read asserts that invariant.
 */
export function overviewRecentTotals(buckets: ReadonlyArray<UsageBucket>): OverviewRecentTotals {
    let requests = 0;
    let errors = 0;

    for (const bucket of buckets) {
        requests += bucket.requests;

        if (USAGE_OUTCOME_FAILURES.includes(bucketKeyField(bucket.key.outcome, "outcome"))) {
            errors += bucket.requests;
        }
    }

    return {
        requests,
        errors,
    };
}
