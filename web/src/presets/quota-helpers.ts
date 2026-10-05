import type {
    QuotaEntryExtraction,
    QuotaField,
    QuotaNumberEncoding,
    QuotaNumberField,
    QuotaWindowSource,
} from "../management-api/generated/v0";

/** Explicit scalar context; requiredness belongs to the mapping, never a fallback. */
export function field(
    path: string[],
    isRequired = false,
    base: QuotaField["base"] = "entry",
): QuotaField {
    return {
        base,
        path,
        is_required: isRequired,
    };
}

/** The numeric union is chosen from reviewed wire-shape evidence. */
export function numberField(
    path: string[],
    encoding: QuotaNumberEncoding = "number",
    isRequired = false,
): QuotaNumberField {
    return {
        field: field(path, isRequired),
        encoding,
    };
}

/** One optional reported ratio window; an absent entry supplies no observation. */
export function ratioEntry(
    key: string,
    label: string,
    at: string[],
    valuePath: string[],
    window: QuotaWindowSource,
    resetPath: string[],
    resetEncoding: "unix_seconds" | "rfc3339",
    encoding: QuotaNumberEncoding = "number",
): QuotaEntryExtraction {
    return {
        key,
        label,
        at,
        is_required: false,
        routing: {
            type: "monitoring_only",
        },
        window,
        measurement: {
            type: "ratio",
            value: numberField(valuePath, encoding),
            scale: "percent",
            direction: "used",
        },
        reset: {
            type: "field",
            field: field(resetPath),
            encoding: resetEncoding,
        },
    };
}
