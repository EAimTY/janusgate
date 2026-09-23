/** A unix timestamp (microseconds) in the browser's locale. */
export function formatTimestamp(unixMicros: number): string {
    return new Date(unixMicros / 1000).toLocaleString();
}

/** The current moment as a unix timestamp in microseconds, matching the gateway's stamps. */
export function nowMicros(): number {
    return Date.now() * 1000;
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
 * How far an instant sits from the current moment, in the largest units that
 * say something: `"<1m"`, `"42m"`, `"4h 12m"`, `"3d 4h"`. Zero remainders
 * are dropped rather than printed.
 */
function validityDistance(expiresAtMicros: number, nowMicros: number): string {
    const magnitudeSeconds = Math.floor(Math.abs(expiresAtMicros - nowMicros) / 1_000_000);

    if (magnitudeSeconds < 60) {
        return "<1m";
    }

    const minutes = Math.floor(magnitudeSeconds / 60);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);

    if (days > 0) {
        return hours % 24 === 0 ? `${days}d` : `${days}d ${hours % 24}h`;
    }

    if (hours > 0) {
        return minutes % 60 === 0 ? `${hours}h` : `${hours}h ${minutes % 60}m`;
    }

    return `${minutes}m`;
}

/**
 * Whether a stored credential's expiry instant has already passed. An expiry
 * exactly now counts as expired: there is nothing left to hold.
 */
export function isCredentialExpired(expiresAtMicros: number, nowMicros: number): boolean {
    return expiresAtMicros <= nowMicros;
}

/**
 * A stored credential's expiry as one sentence: how long it still holds, or
 * how long ago it ran out. An expiry the credential does not carry (`null`
 * on the wire) is the caller's concern — this only phrases a known instant.
 */
export function credentialExpiryLine(expiresAtMicros: number, nowMicros: number): string {
    return isCredentialExpired(expiresAtMicros, nowMicros)
        ? `Expired ${validityDistance(expiresAtMicros, nowMicros)} ago.`
        : `Valid for ${validityDistance(expiresAtMicros, nowMicros)}.`;
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
