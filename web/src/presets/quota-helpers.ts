import type {
    QuotaDirection,
    QuotaEntryExtraction,
    QuotaField,
    QuotaFieldBase,
    QuotaNumberEncoding,
    QuotaNumberField,
    QuotaRatioScale,
    QuotaResetEncoding,
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

/**
 * One optional reported ratio window; an absent entry supplies no observation.
 * The entry rides the monitoring-only routing, the `entry`-relative selections,
 * and the used direction; the object carries each researched delta from those
 * defaults.
 */
export function ratioEntry(input: {
    key: string;
    label: string;
    at: string[];
    valuePath: string[];
    window: QuotaWindowSource;
    resetPath: string[];
    resetEncoding: QuotaResetEncoding;
    /** Wire encoding of the ratio's value field. */
    encoding?: QuotaNumberEncoding;
    /** Magnitude scale; percent unless the upstream reports a fraction. */
    scale?: QuotaRatioScale;
    /** Measurement direction; used unless the upstream reports remaining. */
    direction?: QuotaDirection;
    /** Whether an absent entry fails extraction instead of being omitted. */
    isRequired?: boolean;
    /** Whether an absent value field fails the entry's measurement. */
    isValueRequired?: boolean;
    /** Explicit unlimited bypass field, selected relative to the entry. */
    unlimitedPath?: string[];
    /** Context the reset field resolves from; the entry unless it sits at the item root. */
    resetBase?: QuotaFieldBase;
}): QuotaEntryExtraction {
    return {
        key: input.key,
        label: input.label,
        at: input.at,
        is_required: input.isRequired ?? false,
        routing: {
            type: "monitoring_only",
        },
        window: input.window,
        measurement: {
            type: "ratio",
            value: numberField(
                input.valuePath,
                input.encoding ?? "number",
                input.isValueRequired ?? false,
            ),
            scale: input.scale ?? "percent",
            direction: input.direction ?? "used",
        },
        reset: {
            type: "field",
            field: field(input.resetPath, false, input.resetBase ?? "entry"),
            encoding: input.resetEncoding,
        },
        ...(input.unlimitedPath === undefined
            ? {}
            : {
                  is_unlimited: field(input.unlimitedPath, true),
              }),
    };
}
