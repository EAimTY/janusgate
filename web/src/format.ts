/** A unix timestamp (seconds) in the browser's locale. */
export function formatTimestamp(unixSeconds: number): string {
    return new Date(unixSeconds * 1000).toLocaleString();
}

/** A wall clock of hours, minutes and seconds, as the panel's validity stamp. */
export function formatClock(date: Date): string {
    return date.toLocaleTimeString(undefined, {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hour12: false,
    });
}

import { THINKING_LEVEL_ORDER, type ThinkingLevel } from "/src/management-api/generated/v0";

/**
 * How long a stored credential still holds, in the panel's words: a distance
 * in the largest units that say something, and whether it has already run out.
 */
export function describeValidity(
    expiresAt: number,
    nowSeconds: number,
): {
    readonly isExpired: boolean;
    readonly text: string;
} {
    const delta = expiresAt - nowSeconds;
    const isExpired = delta <= 0;
    const magnitude = Math.abs(delta);

    if (magnitude < 60) {
        return {
            isExpired,
            text: isExpired ? "expired <1m ago" : "in <1m",
        };
    }

    const minutes = Math.floor(magnitude / 60);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);

    const distance =
        days > 0
            ? hours % 24 === 0
                ? `${days}d`
                : `${days}d ${hours % 24}h`
            : hours > 0
              ? minutes % 60 === 0
                  ? `${hours}h`
                  : `${hours}h ${minutes % 60}m`
              : `${minutes}m`;

    return {
        isExpired,
        text: isExpired ? `expired ${distance} ago` : `in ${distance}`,
    };
}

/**
 * The allowed thinking levels of one model as one phrase: the canonical order
 * is the ladder, so a run of allowed levels reads as its span and anything
 * else is listed in that order.
 */
export function describeLevels(levels: ReadonlyArray<ThinkingLevel> | null): string {
    if (levels === null) {
        return "passes through";
    }

    const allowed = THINKING_LEVEL_ORDER.filter((level) => levels.includes(level));

    if (allowed.length === THINKING_LEVEL_ORDER.length) {
        return "all four";
    }

    if (allowed.length === 1) {
        return `only ${allowed[0]}`;
    }

    const firstIndex = THINKING_LEVEL_ORDER.indexOf(allowed[0] as ThinkingLevel);
    const isContiguous = allowed.every(
        (level, offset) => THINKING_LEVEL_ORDER.indexOf(level) === firstIndex + offset,
    );

    return isContiguous ? `${allowed[0]}–${allowed[allowed.length - 1]}` : allowed.join(", ");
}
