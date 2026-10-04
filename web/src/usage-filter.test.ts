import { describe, expect, it } from "vitest";
import {
    buildUsageFilter,
    draftUsageFilter,
    EMPTY_USAGE_FILTER_DRAFT,
    EMPTY_USAGE_FILTER_FIELDS,
    hasUsageFilter,
    parseStatusList,
    parseUsageFilterDraft,
    serializeUsageFilterDraft,
    statusError,
    subjectFilterFields,
    USAGE_QUERY_SUBJECT_FIELDS,
    type UsageQuerySubjectDimension,
    type UsageSubjectFilter,
    usageQuerySubjectOfFilter,
    windowBoundMicros,
} from "./usage-filter";

/** An arbitrary instant; every window bound below is stated relative to it. */
const NOW = Date.UTC(2026, 8, 24, 10, 30, 0) * 1000;

const HOUR_MICROS = 3_600_000_000;

describe("parseUsageFilterDraft", () => {
    it("reads an absent or empty search as the default draft", () => {
        expect(parseUsageFilterDraft("")).toEqual(EMPTY_USAGE_FILTER_DRAFT);
        expect(parseUsageFilterDraft("?")).toEqual(EMPTY_USAGE_FILTER_DRAFT);
    });

    it("round-trips every choice through the route", () => {
        const draft = {
            window: "7d",
            providerId: "3",
            modelId: "9",
            modelRouteId: "17",
            apiKeyId: "7",
            endpointType: "anthropic_messages",
            outcome: "failures",
            streamChoice: "non-streamed",
            statusInput: "200,429",
        } as const;

        expect(parseUsageFilterDraft(`?${serializeUsageFilterDraft(draft)}`)).toEqual(draft);
    });

    it("writes only the choices that differ from the default", () => {
        expect(serializeUsageFilterDraft(EMPTY_USAGE_FILTER_DRAFT)).toBe("");
        expect(
            serializeUsageFilterDraft({
                ...EMPTY_USAGE_FILTER_DRAFT,
                window: "24h",
            }),
        ).toBe("window=24h");
    });

    it("reads a value outside its vocabulary as absent", () => {
        expect(
            parseUsageFilterDraft(
                "?window=99d&provider=abc&model=-1&route=-1&endpoint=openai_embeddings&stream=maybe&key=7",
            ),
        ).toEqual({
            ...EMPTY_USAGE_FILTER_DRAFT,
            apiKeyId: "7",
        });
    });

    it("reads a row identifier beyond the safe-integer range as absent", () => {
        // A digit string past `Number.isSafeInteger` names no row the gateway
        // could hold, and its `Number` would distort the value put on the wire.
        expect(
            parseUsageFilterDraft(
                "?provider=99999999999999999999999&model=9007199254740992&route=9007199254740992",
            ),
        ).toEqual(EMPTY_USAGE_FILTER_DRAFT);

        // The largest safe integer is still an addressable row identifier.
        expect(parseUsageFilterDraft("?key=9007199254740991").apiKeyId).toBe("9007199254740991");
    });

    it("keeps one outcome and the failure pair, reading any other set as absent", () => {
        expect(parseUsageFilterDraft("?outcome=upstream_error").outcome).toBe("upstream_error");
        expect(parseUsageFilterDraft("?outcome=internal,upstream_error").outcome).toBe("failures");
        expect(parseUsageFilterDraft("?outcome=completed,upstream_error").outcome).toBe("");
        expect(parseUsageFilterDraft("?outcome=completed").outcome).toBe("completed");
    });
});

describe("windowBoundMicros", () => {
    it("bounds a rolling window at the moment it is read", () => {
        expect(windowBoundMicros("24h", NOW)).toBe(NOW - 24 * HOUR_MICROS);
        expect(windowBoundMicros("7d", NOW)).toBe(NOW - 7 * 24 * HOUR_MICROS);
        expect(windowBoundMicros("30d", NOW)).toBe(NOW - 30 * 24 * HOUR_MICROS);
    });

    it("sets no bound for all time", () => {
        expect(windowBoundMicros("all", NOW)).toBeNull();
    });
});

describe("buildUsageFilter", () => {
    it("states no filter at all for the default fields", () => {
        expect(buildUsageFilter(EMPTY_USAGE_FILTER_FIELDS)).toEqual({});
    });

    it("lists every chosen value, expanding the failure pair", () => {
        expect(
            buildUsageFilter({
                providerId: "3",
                modelId: "9",
                modelRouteId: "17",
                apiKeyId: "7",
                endpointType: "gemini_interactions",
                outcome: "failures",
                streamChoice: "streamed",
                statusInput: "200, abc, 429",
            }),
        ).toEqual({
            provider_id: [
                3,
            ],
            model_route_id: [
                17,
            ],
            model_id: [
                9,
            ],
            api_key_id: [
                7,
            ],
            endpoint_type: [
                "gemini_interactions",
            ],
            outcome: [
                "upstream_error",
                "internal",
            ],
            is_stream: true,
            http_status: [
                200,
                429,
            ],
        });
    });

    it("carries the bounds the screen resolved", () => {
        expect(
            buildUsageFilter(EMPTY_USAGE_FILTER_FIELDS, {
                from: 5,
                to: null,
            }),
        ).toEqual({
            from: 5,
        });
    });
});

describe("statusError", () => {
    it("accepts blank and a list whose every entry parses as an integer", () => {
        expect(statusError("")).toBeNull();
        expect(statusError("200, 429")).toBeNull();
        expect(statusError(" 200 ,429 ")).toBeNull();
    });

    it("names the field's contract when an entry is no integer", () => {
        expect(statusError("2xx")).toBe(
            "HTTP statuses must be plain integers, comma-separated — like 200, 429 — or blank for no status filter.",
        );
        expect(statusError("abc")).not.toBeNull();

        // Spellings `Number` would accept are not plain integers: the gate
        // and the parse narrow one shape, the one the gateway's i64 status
        // column holds.
        expect(statusError("1e3")).not.toBeNull();
        expect(statusError("0x10")).not.toBeNull();
        expect(statusError("+200")).not.toBeNull();
        expect(statusError("99999999999999999999")).not.toBeNull();
    });

    it("holds a partially valid list — the filter fires only when the whole field parses", () => {
        // The pure builder keeps its narrowing for the raw text a route can
        // still carry; the gate is what refuses a form that parses in part.
        expect(parseStatusList("200, 2xx")).toEqual([
            200,
        ]);
        expect(statusError("200, 2xx")).not.toBeNull();

        // An entry outside the plain-decimal shape narrows out the same way.
        expect(parseStatusList("200, 1e3")).toEqual([
            200,
        ]);
    });
});

describe("draftUsageFilter", () => {
    it("resolves the draft's rolling window against the moment", () => {
        expect(
            draftUsageFilter(
                {
                    ...EMPTY_USAGE_FILTER_DRAFT,
                    window: "24h",
                },
                NOW,
            ),
        ).toEqual({
            from: NOW - 24 * HOUR_MICROS,
        });
    });
});

describe("hasUsageFilter", () => {
    it("counts the window alone as a filter", () => {
        expect(hasUsageFilter(EMPTY_USAGE_FILTER_DRAFT)).toBe(false);
        expect(
            hasUsageFilter({
                ...EMPTY_USAGE_FILTER_DRAFT,
                window: "24h",
            }),
        ).toBe(true);
        expect(
            hasUsageFilter({
                ...EMPTY_USAGE_FILTER_DRAFT,
                outcome: "failures",
            }),
        ).toBe(true);
    });
});

/**
 * The dimensions the round-trips below must each cover, read through the
 * public table: its `satisfies Record<UsageQuerySubjectDimension, ...>` bound
 * makes its keys exhaustive over the dimension union, so a dimension added to
 * the module is answered here with no edit — the same auto-coverage the
 * module-private walk enumeration (`USAGE_QUERY_SUBJECT_DIMENSIONS`) gave.
 */
const SUBJECT_DIMENSIONS = Object.keys(USAGE_QUERY_SUBJECT_FIELDS) as UsageQuerySubjectDimension[];

/**
 * The subject filter one table dimension names, built under that dimension's
 * own wire field. The computed key's union spelling reads as an index
 * signature, so the fixture asserts the one table pairing the round-trip
 * below pins.
 */
function subjectFilterNamed(dimension: UsageQuerySubjectDimension, id: number): UsageSubjectFilter {
    return {
        [USAGE_QUERY_SUBJECT_FIELDS[dimension].wireField]: id,
    } as UsageSubjectFilter;
}

describe("usageQuerySubjectOfFilter", () => {
    it("round-trips every dimension through the table: the filter built under its wire field reads back as its subject", () => {
        for (const dimension of SUBJECT_DIMENSIONS) {
            expect(usageQuerySubjectOfFilter(subjectFilterNamed(dimension, 12))).toEqual({
                dimension,
                id: 12,
            });
        }
    });
});

describe("subjectFilterFields", () => {
    it("round-trips every dimension through the table: the id fills its own draft field alone", () => {
        for (const dimension of SUBJECT_DIMENSIONS) {
            expect(
                subjectFilterFields({
                    dimension,
                    id: 12,
                }),
            ).toEqual({
                ...EMPTY_USAGE_FILTER_FIELDS,
                [USAGE_QUERY_SUBJECT_FIELDS[dimension].draftField]: "12",
            });
        }
    });

    it("prefills nothing when no subject arrives", () => {
        expect(subjectFilterFields(null)).toEqual(EMPTY_USAGE_FILTER_FIELDS);
    });
});
