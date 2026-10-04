import { activeLocale } from "./index";

const numberKinds = {
    integer: {
        maximumFractionDigits: 0,
    },
    tenth: {
        minimumFractionDigits: 1,
        maximumFractionDigits: 1,
        useGrouping: false,
    },
    quantity: {
        maximumFractionDigits: 20,
    },
    percentage: {
        style: "percent",
        minimumFractionDigits: 1,
        maximumFractionDigits: 1,
    },
    paddedSecond: {
        minimumIntegerDigits: 2,
        maximumFractionDigits: 0,
        useGrouping: false,
    },
} as const satisfies Record<string, Intl.NumberFormatOptions>;
const dateKinds = {
    timestamp: {
        year: "numeric",
        month: "numeric",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
        second: "2-digit",
    },
    minute: {
        year: "numeric",
        month: "numeric",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
    },
    day: {
        year: "numeric",
        month: "numeric",
        day: "numeric",
    },
    clock: {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hourCycle: "h23",
    },
} as const satisfies Record<string, Intl.DateTimeFormatOptions>;
const numbers = new Map<string, Intl.NumberFormat>();
const dates = new Map<string, Intl.DateTimeFormat>();
const relatives = new Map<string, Intl.RelativeTimeFormat>();

/** Named cached formats avoid per-cell allocation and arbitrary option identities. */
export function formatNumber(value: number, kind: keyof typeof numberKinds = "integer"): string {
    const locale = activeLocale();
    const key = `${locale}:${kind}`;
    let formatter = numbers.get(key);
    if (!formatter) {
        formatter = new Intl.NumberFormat(locale, numberKinds[kind]);
        numbers.set(key, formatter);
    }
    return formatter.format(value);
}
export function formatDate(value: Date, kind: keyof typeof dateKinds): string {
    const locale = activeLocale();
    const key = `${locale}:${kind}`;
    let formatter = dates.get(key);
    if (!formatter) {
        formatter = new Intl.DateTimeFormat(locale, {
            ...dateKinds[kind],
            calendar: "gregory",
        });
        dates.set(key, formatter);
    }
    return formatter.format(value);
}
/** The caller chooses the unit and rounding; this facade never changes arithmetic. */
export function formatRelative(value: number, unit: Intl.RelativeTimeFormatUnit): string {
    const locale = activeLocale();
    let formatter = relatives.get(locale);
    if (!formatter) {
        formatter = new Intl.RelativeTimeFormat(locale, {
            numeric: "always",
        });
        relatives.set(locale, formatter);
    }
    return formatter.format(value, unit);
}
