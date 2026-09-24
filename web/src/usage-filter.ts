import type {
    ApiLogEndpointType,
    ApiLogOutcome,
    UsageFilter,
} from "/src/management-api/generated/v0";
import { findApiLogEndpointType, findApiLogOutcome } from "/src/management-api/vocabulary";

/**
 * The gateway's one usage filter, as the screens' forms hold it: every field
 * is a string, exactly as its control reads it, and the wire shape is built
 * only at read time ([`buildUsageFilter`]). Both usage screens present the
 * shared filter type in this one vocabulary, so the listing and the aggregates
 * page can never name the same filter differently.
 */
export type UsageFilterFields = {
    readonly providerId: string;
    readonly modelId: string;
    readonly apiKeyId: string;
    readonly endpointType: "" | ApiLogEndpointType;
    readonly outcome: UsageOutcomeChoice;
    readonly streamChoice: UsageStreamChoice;
    readonly statusInput: string;
};

/** The outcome control's value: nothing, the two failure outcomes together, or one outcome. */
export type UsageOutcomeChoice = "" | "failures" | ApiLogOutcome;

/** Whether the client streamed, as the stream control spells it. */
export type UsageStreamChoice = "" | "streamed" | "non-streamed";

/**
 * The outcomes the app calls failures — the set the "Failures" choice filters
 * by and the overview's error figure counts, so the figure and the listing it
 * links to can never disagree about what an error is.
 */
export const USAGE_OUTCOME_FAILURES: ReadonlyArray<ApiLogOutcome> = [
    "upstream_error",
    "internal",
];

/** The filter with nothing chosen. */
export const EMPTY_USAGE_FILTER_FIELDS: UsageFilterFields = {
    providerId: "",
    modelId: "",
    apiKeyId: "",
    endpointType: "",
    outcome: "",
    streamChoice: "",
    statusInput: "",
};

/** The rolling windows a usage route can name. */
export type UsageWindowChoice = "all" | "24h" | "7d" | "30d";

/** The window control's options, in the order the control lists them. */
export const USAGE_WINDOW_OPTIONS: ReadonlyArray<{
    readonly value: UsageWindowChoice;
    readonly label: string;
}> = [
    {
        value: "all",
        label: "All time",
    },
    {
        value: "24h",
        label: "Last 24 hours",
    },
    {
        value: "7d",
        label: "Last 7 days",
    },
    {
        value: "30d",
        label: "Last 30 days",
    },
];

/** One usage screen's whole filter as its route carries it: the fields plus the window. */
export type UsageFilterDraft = UsageFilterFields & {
    readonly window: UsageWindowChoice;
};

/** The draft with nothing chosen: the unfiltered listing. */
export const EMPTY_USAGE_FILTER_DRAFT: UsageFilterDraft = {
    ...EMPTY_USAGE_FILTER_FIELDS,
    window: "all",
};

const DAY_MICROS = 86_400_000_000;

const WINDOW_MICROS: Record<Exclude<UsageWindowChoice, "all">, number> = {
    "24h": DAY_MICROS,
    "7d": 7 * DAY_MICROS,
    "30d": 30 * DAY_MICROS,
};

/**
 * The inclusive lower bound one window names, resolved against the moment it
 * is read: a rolling window is a duration before now, never a fixed instant,
 * so a shared link keeps meaning the same span however long it sits. `all`
 * sets no bound.
 */
export function windowBoundMicros(window: UsageWindowChoice, nowMicros: number): number | null {
    return window === "all" ? null : nowMicros - WINDOW_MICROS[window];
}

/**
 * Reads one route value as a row identifier, answering absent when it is not
 * one. The safe-integer bound is the same one `parseRowIdentifier` applies to
 * path ids: a digit string beyond it names no row the gateway could hold, and
 * its `Number` would only distort the value put on the wire.
 */
function routeRowId(parameters: URLSearchParams, name: string): string {
    const raw = parameters.get(name);

    return raw !== null && /^\d+$/.test(raw) && Number.isSafeInteger(Number(raw)) ? raw : "";
}

/** Reads one route value as a window, answering all time when it names no window. */
function routeWindow(raw: string | null): UsageWindowChoice {
    return raw === "24h" || raw === "7d" || raw === "30d" ? raw : "all";
}

/**
 * Reads the outcome choice out of the route's comma-separated value list: one
 * known outcome, or exactly the failure pair. Any other set — including a
 * hand-written list the control cannot present — reads as absent, so the
 * control never shows a choice that is not the filter in force.
 */
function routeOutcome(raw: string | null): UsageOutcomeChoice {
    if (raw === null) {
        return "";
    }

    const values = raw
        .split(",")
        .map((entry) => entry.trim())
        .filter((entry) => entry !== "");

    if (values.length === 1) {
        return findApiLogOutcome(values[0]) ?? "";
    }

    if (
        values.length === USAGE_OUTCOME_FAILURES.length &&
        USAGE_OUTCOME_FAILURES.every((failure) => values.includes(failure))
    ) {
        return "failures";
    }

    return "";
}

/** Reads the stream choice, answering absent when the value names no mode. */
function routeStream(raw: string | null): UsageStreamChoice {
    return raw === "streamed" || raw === "non-streamed" ? raw : "";
}

/**
 * The draft one route address carries. Every value is read strictly: a value
 * outside its vocabulary reads as absent — the unfiltered listing — exactly as
 * an unaddressable provider filter reads as absent on the models page.
 */
export function parseUsageFilterDraft(search: string): UsageFilterDraft {
    const parameters = new URLSearchParams(search);

    return {
        window: routeWindow(parameters.get("window")),
        providerId: routeRowId(parameters, "provider"),
        modelId: routeRowId(parameters, "model"),
        apiKeyId: routeRowId(parameters, "key"),
        endpointType: findApiLogEndpointType(parameters.get("endpoint")) ?? "",
        outcome: routeOutcome(parameters.get("outcome")),
        streamChoice: routeStream(parameters.get("stream")),
        statusInput: parameters.get("status") ?? "",
    };
}

/** The route value one outcome choice writes: the wire values it filters by. */
function outcomeRouteValue(choice: Exclude<UsageOutcomeChoice, "">): string {
    return choice === "failures" ? USAGE_OUTCOME_FAILURES.join(",") : choice;
}

/**
 * The route query one draft writes: only the choices that differ from the
 * default appear, so the unfiltered listing's address stays exactly `/usage`.
 * Commas inside a value stay literal — the outcome list and the status list
 * are the only multi-value fields, and a readable address is the point of
 * carrying the filter in the route at all.
 */
export function serializeUsageFilterDraft(draft: UsageFilterDraft): string {
    const parameters = new URLSearchParams();

    if (draft.window !== "all") {
        parameters.set("window", draft.window);
    }

    if (draft.providerId !== "") {
        parameters.set("provider", draft.providerId);
    }

    if (draft.modelId !== "") {
        parameters.set("model", draft.modelId);
    }

    if (draft.apiKeyId !== "") {
        parameters.set("key", draft.apiKeyId);
    }

    if (draft.endpointType !== "") {
        parameters.set("endpoint", draft.endpointType);
    }

    if (draft.outcome !== "") {
        parameters.set("outcome", outcomeRouteValue(draft.outcome));
    }

    if (draft.streamChoice !== "") {
        parameters.set("stream", draft.streamChoice);
    }

    if (draft.statusInput !== "") {
        parameters.set("status", draft.statusInput);
    }

    return parameters.toString().replaceAll("%2C", ",");
}

/** Whether anything is chosen — a window alone counts as a filter. */
export function hasUsageFilter(draft: UsageFilterDraft): boolean {
    return serializeUsageFilterDraft(draft) !== "";
}

/**
 * Parses the operator's comma-separated status list: blank entries drop out,
 * and entries that are not integers never form a filter value, so a half-typed
 * list narrows to the statuses it does name.
 */
export function parseStatusList(input: string): number[] {
    return input
        .split(",")
        .map((entry) => entry.trim())
        .filter((entry) => entry !== "")
        .map((entry) => Number(entry))
        .filter((entry) => Number.isInteger(entry));
}

/** One outcome choice as the values the filter lists; nothing chosen lists nothing. */
function outcomeValues(choice: UsageOutcomeChoice): ReadonlyArray<ApiLogOutcome> {
    if (choice === "") {
        return [];
    }

    return choice === "failures"
        ? USAGE_OUTCOME_FAILURES
        : [
              choice,
          ];
}

/** The bounds a screen resolved for the filter, or none. */
export type UsageFilterBounds = {
    readonly from?: number | null | undefined;
    readonly to?: number | null | undefined;
};

/**
 * The wire filter the fields describe, with the bounds the screen resolved.
 * Every field is absent-keeps, so a field with nothing chosen is omitted
 * rather than sent empty — an empty array is a rejection, never a wildcard.
 */
export function buildUsageFilter(
    fields: UsageFilterFields,
    bounds: UsageFilterBounds = {},
): UsageFilter {
    const from = bounds.from ?? null;
    const to = bounds.to ?? null;
    const statuses = parseStatusList(fields.statusInput);
    const outcomes = outcomeValues(fields.outcome);

    return {
        ...(from === null
            ? {}
            : {
                  from,
              }),
        ...(to === null
            ? {}
            : {
                  to,
              }),
        ...(fields.providerId === ""
            ? {}
            : {
                  provider_id: [
                      Number(fields.providerId),
                  ],
              }),
        ...(fields.modelId === ""
            ? {}
            : {
                  model_id: [
                      Number(fields.modelId),
                  ],
              }),
        ...(fields.apiKeyId === ""
            ? {}
            : {
                  api_key_id: [
                      Number(fields.apiKeyId),
                  ],
              }),
        ...(fields.endpointType === ""
            ? {}
            : {
                  endpoint_type: [
                      fields.endpointType,
                  ],
              }),
        ...(outcomes.length === 0
            ? {}
            : {
                  outcome: [
                      ...outcomes,
                  ],
              }),
        ...(fields.streamChoice === ""
            ? {}
            : {
                  is_stream: fields.streamChoice === "streamed",
              }),
        ...(statuses.length === 0
            ? {}
            : {
                  http_status: statuses,
              }),
    };
}

/** The wire filter one route draft describes, its rolling window resolved against the moment. */
export function draftUsageFilter(draft: UsageFilterDraft, nowMicros: number): UsageFilter {
    return buildUsageFilter(draft, {
        from: windowBoundMicros(draft.window, nowMicros),
    });
}
