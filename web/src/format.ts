/** A unix timestamp (seconds) in the browser's locale. */
export function formatTimestamp(unixSeconds: number): string {
    return new Date(unixSeconds * 1000).toLocaleString();
}

/** A gateway-stamped creation time (UTC microseconds) in the browser's locale. */
export function formatCreatedAt(createdAtMicros: number): string {
    return new Date(createdAtMicros / 1000).toLocaleString();
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
 * A token count as one phrase: unreported usage (`NULL` — the upstream sent
 * no usage) reads as a dash, never as zero, so an absent breakdown cannot be
 * mistaken for a measured one.
 */
export function formatTokenCount(value: number | null): string {
    return value === null ? "—" : value.toLocaleString();
}

/**
 * A UTC-microsecond instant as a `datetime-local` value in this browser's
 * zone, at the input's minute precision.
 */
export function formatMicrosForDateTimeInput(micros: number): string {
    const date = new Date(micros / 1000);
    const pad = (part: number): string => String(part).padStart(2, "0");

    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * A `datetime-local` value as UTC microseconds. Blank or malformed input
 * reads as `null` — no bound — so a half-typed filter never narrows the
 * listing to nothing.
 */
export function parseDateTimeInputToMicros(value: string): number | null {
    const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value.trim());

    if (match === null) {
        return null;
    }

    const year = Number(match[1]);
    const month = Number(match[2]);
    const day = Number(match[3]);
    const hour = Number(match[4]);
    const minute = Number(match[5]);
    const date = new Date(year, month - 1, day, hour, minute);

    if (
        Number.isNaN(date.getTime()) ||
        date.getFullYear() !== year ||
        date.getMonth() !== month - 1 ||
        date.getDate() !== day ||
        date.getHours() !== hour ||
        date.getMinutes() !== minute
    ) {
        return null;
    }

    return date.getTime() * 1000;
}
