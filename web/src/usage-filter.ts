import { ContractViolationError } from "/src/management-api/errors";
import type {
    ApiLogEndpointType,
    ApiLogOutcome,
    UsageFilter,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { API_LOG_ENDPOINT_TYPE, API_LOG_OUTCOME } from "/src/management-api/vocabulary";
import { DAY_MICROS } from "./format";
import { parseRowIdentifier } from "./row-id";

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
 * Reads one route value as a row identifier, keeping its text when
 * [`parseRowIdentifier`] reads a row out of it and answering absent
 * otherwise: a digit string beyond the safe-integer range names no row the
 * gateway could hold, and its `Number` would only distort the value put on
 * the wire.
 */
function routeRowId(parameters: URLSearchParams, name: string): string {
    const raw = parameters.get(name);

    return raw !== null && parseRowIdentifier(raw) !== null ? raw : "";
}

/** Reads one route value as a window, answering all time when it names no window. */
function routeWindow(raw: string | null): UsageWindowChoice {
    return raw === "24h" || raw === "7d" || raw === "30d" ? raw : "all";
}

/**
 * The entries one comma-separated list field holds: the raw value split on
 * commas, each entry trimmed, the blanks dropped. The one reading behind the
 * route's outcome list, [`parseStatusList`], and [`statusError`], so a status
 * list's validity and its reading agree entry-for-entry — an entry
 * [`statusError`] accepts is exactly an entry [`parseStatusList`] reads.
 */
function splitListEntries(input: string): string[] {
    return input
        .split(",")
        .map((entry) => entry.trim())
        .filter((entry) => entry !== "");
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

    const values = splitListEntries(raw);

    if (values.length === 1) {
        return API_LOG_OUTCOME.find(values[0]) ?? "";
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
        endpointType: API_LOG_ENDPOINT_TYPE.find(parameters.get("endpoint")) ?? "",
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

/**
 * The names of [`UsageFilterFields`], in the order the forms hold them: the
 * one enumeration the fields-half predicates walk, so a field added to the
 * controls is answered by [`hasUsageFilterFields`] and
 * [`hasUsageFilterFieldsExcept`] alike with one entry here.
 */
const USAGE_FILTER_FIELD_NAMES: ReadonlyArray<keyof UsageFilterFields> = [
    "providerId",
    "modelId",
    "apiKeyId",
    "endpointType",
    "outcome",
    "streamChoice",
    "statusInput",
];

/**
 * Whether any filter field is chosen — the fields half of [`hasUsageFilter`],
 * shared with the aggregates page, whose form holds the fields without a
 * window: its narrow question is these fields alone.
 */
export function hasUsageFilterFields(fields: UsageFilterFields): boolean {
    return hasUsageFilterFieldsExcept(fields, null);
}

/**
 * Whether any filter field except the one carried field is chosen — the
 * fields half of the aggregates door's stated trade: the stream's crossing to
 * the aggregates page carries a single-entity narrow's field alone, and the
 * window and every other field stay the aggregates page's own, so the door
 * states its trade exactly while this predicate over the standing draft
 * stands. `carriedField` is that one field's name, or `null` when the
 * crossing carries no field and every set field counts. The walk is
 * [`USAGE_FILTER_FIELD_NAMES`], the same enumeration
 * [`hasUsageFilterFields`] answers through, so a field added to the controls
 * can never leave the trade unstated.
 */
export function hasUsageFilterFieldsExcept(
    fields: UsageFilterFields,
    carriedField: keyof UsageFilterFields | null,
): boolean {
    return USAGE_FILTER_FIELD_NAMES.some((name) => name !== carriedField && fields[name] !== "");
}

/** Whether anything is chosen — a window alone counts as a filter. */
export function hasUsageFilter(draft: UsageFilterDraft): boolean {
    return draft.window !== "all" || hasUsageFilterFields(draft);
}

/**
 * Parses the operator's comma-separated status list: blank entries drop out,
 * and every remaining entry is read by [`parseRowIdentifier`] — the one
 * row-identifier rule, whose plain-decimal safe-integer shape is the one the
 * gateway's i64 status column carries — so spellings `Number` would accept
 * (`1e3`, `0x10`, `+200`, a 20-digit entry) narrow to nothing. The forms hold
 * their actions while [`statusError`] stands, so a hand-typed list reaches
 * here only whole; a route-carried one still narrows to the statuses it does
 * name.
 */
export function parseStatusList(input: string): number[] {
    return splitListEntries(input)
        .map((entry) => parseRowIdentifier(entry))
        .filter((entry): entry is number => entry !== null);
}

/**
 * The inline error for the hand-typed status list: every comma-separated
 * entry must name an integer — the exact shape [`parseStatusList`] turns
 * into a filter value — so a typo or a glob like `2xx` names its mistake
 * instead of silently dropping the status narrow. Blank stays valid: an
 * empty field sets no status filter.
 */
export function statusError(input: string): string | null {
    const entries = splitListEntries(input);

    if (entries.every((entry) => parseRowIdentifier(entry) !== null)) {
        return null;
    }

    return "HTTP statuses must be plain integers, comma-separated — like 200, 429 — or blank for no status filter.";
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
type UsageFilterBounds = {
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

/**
 * The entity dimensions an aggregates subject can name: the `usage:query`
 * grouping dimensions that have a filter field of their own.
 */
export type UsageQuerySubjectDimension = Extract<UsageGroupBy, "provider" | "model" | "api_key">;

/** The subject an aggregates link carries: one entity dimension and the row to preselect there. */
export type UsageQuerySubject = {
    readonly dimension: UsageQuerySubjectDimension;
    readonly id: number;
};

/** The subject dimensions, in the order the subject derivations walk them. */
const USAGE_QUERY_SUBJECT_DIMENSIONS: ReadonlyArray<UsageQuerySubjectDimension> = [
    "provider",
    "model",
    "api_key",
];

/**
 * The names one subject dimension's row rides under: the draft field the usage
 * forms hold it as, the route parameter the usage routes address it under —
 * the stream's own name, so both usage routes address an entity alike — and
 * the wire filter field the usage reads narrow by.
 */
type UsageQuerySubjectFieldNames = {
    readonly draftField: keyof UsageFilterFields;
    readonly routeParameter: string;
    readonly wireField: keyof UsageFilter & string;
};

/**
 * The one dimension↔field pairing behind every aggregates-subject shape. Each
 * derivation that reads a subject — the detail card's wire filter and
 * aggregates link, the stream's subject narrow, the aggregates route's
 * parameters — takes its field names from this table, so one dimension's
 * three spellings can never drift apart.
 */
export const USAGE_QUERY_SUBJECT_FIELDS = {
    provider: {
        draftField: "providerId",
        routeParameter: "provider",
        wireField: "provider_id",
    },
    model: {
        draftField: "modelId",
        routeParameter: "model",
        wireField: "model_id",
    },
    api_key: {
        draftField: "apiKeyId",
        routeParameter: "key",
        wireField: "api_key_id",
    },
} as const satisfies Record<UsageQuerySubjectDimension, UsageQuerySubjectFieldNames>;

/**
 * The filter fields one subject prefills: its id, spelled as the draft string
 * it rides as, filled into its dimension's own draft field over the empty
 * fields — or nothing prefilled, when no subject arrives. The field name is
 * read from [`USAGE_QUERY_SUBJECT_FIELDS`], the one dimension↔field pairing,
 * so every derivation that opens a usage page with a subject preselected
 * fills the same field, and a dimension added to the table is answered here
 * with no edit.
 */
export function subjectFilterFields(subject: UsageQuerySubject | null): UsageFilterFields {
    if (subject === null) {
        return EMPTY_USAGE_FILTER_FIELDS;
    }

    const field = USAGE_QUERY_SUBJECT_FIELDS[subject.dimension].draftField;
    const narrow: Partial<Record<typeof field, string>> = {
        [field]: String(subject.id),
    };

    return {
        ...EMPTY_USAGE_FILTER_FIELDS,
        ...narrow,
    };
}

/**
 * The stream draft the aggregates page's filter fields become — the crossing
 * rule both usage doors state: the fields cross verbatim (both pages hold the
 * shared filter in the one [`UsageFilterFields`] vocabulary, single values on
 * each side), while the From and To bounds stay behind with no stream
 * spelling to become, and the stream's own window row governs time there,
 * opening at all time.
 */
export function streamDraftOf(fields: UsageFilterFields): UsageFilterDraft {
    return {
        ...fields,
        window: "all",
    };
}

/**
 * The subject candidates one walk over [`USAGE_QUERY_SUBJECT_DIMENSIONS`]
 * collects, in the walk's order: one entry per dimension whose read names an
 * id. The callback answers the one id `dimension` names — its wire field's
 * single value, its route parameter's row identifier — or `null` when it
 * names none. One id per dimension is this walk's contract, stated here once
 * instead of proven at each caller; [`soleSubject`] answers the exactly-one
 * rule over the list.
 */
export function namedSubjects(
    readId: (dimension: UsageQuerySubjectDimension) => number | null,
): UsageQuerySubject[] {
    const named: UsageQuerySubject[] = [];

    for (const dimension of USAGE_QUERY_SUBJECT_DIMENSIONS) {
        const id = readId(dimension);

        if (id !== null) {
            named.push({
                dimension,
                id,
            });
        }
    }

    return named;
}

/**
 * The sole subject among the candidates one subject walk collected: the one
 * entry when exactly one entity dimension was named, `null` otherwise — none
 * named and several named alike answer no subject, because a subject is
 * exactly one entity dimension. Every derivation that gathers subject
 * candidates ends in this one rule, so the exactly-one reading can never
 * drift between the derivations that share it.
 */
export function soleSubject(named: ReadonlyArray<UsageQuerySubject>): UsageQuerySubject | null {
    const [subject] = named;

    return named.length === 1 ? (subject ?? null) : null;
}

/**
 * Which recorded rows the stats count: exactly one subject's, named by its
 * wire filter field. Derived from [`USAGE_QUERY_SUBJECT_FIELDS`], so the
 * shape stays the wire spelling of the dimension table rather than a parallel
 * vocabulary.
 */
export type UsageSubjectFilter = {
    readonly [TDimension in UsageQuerySubjectDimension]: {
        readonly [TWireField in (typeof USAGE_QUERY_SUBJECT_FIELDS)[TDimension]["wireField"]]: number;
    };
}[UsageQuerySubjectDimension];

/** The wire fields a subject filter can name: one per dimension, from the table. */
type UsageSubjectWireField =
    (typeof USAGE_QUERY_SUBJECT_FIELDS)[UsageQuerySubjectDimension]["wireField"];

/**
 * The subject filter as one lookup over the table's wire fields, every entry
 * optional: the shape the dimension walk reads a [`UsageSubjectFilter`] as,
 * so one indexed read per dimension needs no per-member narrowing.
 */
type UsageSubjectFilterValues = Readonly<Partial<Record<UsageSubjectWireField, number>>>;

/**
 * The aggregates subject one [`UsageSubjectFilter`] names: the dimension
 * whose wire field it carries, and the id under it. The walk is
 * [`namedSubjects`] over [`USAGE_QUERY_SUBJECT_DIMENSIONS`] through
 * [`USAGE_QUERY_SUBJECT_FIELDS`] — the one walk the aggregates route's
 * parameters and the stream's subject narrow share, ending in the same
 * [`soleSubject`] rule — so a dimension added to the table is answered here
 * with no edit. The filter's type holds exactly one dimension's field; one
 * naming none cannot exist, and reads as the contract violation it is.
 */
export function usageQuerySubjectOfFilter(filter: UsageSubjectFilter): UsageQuerySubject {
    const values: UsageSubjectFilterValues = filter;
    const subject = soleSubject(
        namedSubjects(
            (dimension) => values[USAGE_QUERY_SUBJECT_FIELDS[dimension].wireField] ?? null,
        ),
    );

    if (subject === null) {
        throw new ContractViolationError("the usage subject filter names no subject dimension");
    }

    return subject;
}
