import { describe, expect, test } from "vitest";
import { DAY_MICROS } from "/src/format";
import type { UsageFilterDraft } from "./filter";
import {
    buildUsageFilter,
    draftUsageFilter,
    EMPTY_USAGE_FILTER_DRAFT,
    EMPTY_USAGE_FILTER_FIELDS,
    hasUsageFilter,
    hasUsageFilterFields,
    hasUsageFilterFieldsExcept,
    namedSubjects,
    parseUsageFilterDraft,
    serializeUsageFilterDraft,
    soleSubject,
    statusError,
    streamAggregatesSubject,
    streamDraftOf,
    subjectFilterFields,
    USAGE_OUTCOME_FAILURES,
    usageQuerySubjectOfFilter,
    windowBoundMicros,
} from "./filter";

/**
 * The usage filter's route vocabulary and wire shape: the draft read
 * strictly from the route (a value outside its vocabulary reading as
 * absent), the route written with only the choices that differ from the
 * default, the wire filter built absent-keeps with the window resolved
 * against the reading moment, and the aggregates-subject rule — a subject is
 * exactly one entity dimension.
 */

/** The empty fields, spelled out for the assertions below. */
const EMPTY_FIELDS = EMPTY_USAGE_FILTER_DRAFT;

describe("parseUsageFilterDraft", () => {
    test("reads the empty route as the unfiltered draft", () => {
        expect(parseUsageFilterDraft("")).toEqual(EMPTY_FIELDS);
    });

    test("reads every field back from a fully-addressed route", () => {
        const draft = parseUsageFilterDraft(
            "?window=7d&provider=3&model=11&route=5&key=8&endpoint=openai_chat_completions&outcome=upstream_error&stream=streamed&status=429,500",
        );

        expect(draft).toEqual({
            window: "7d",
            providerId: "3",
            modelId: "11",
            modelRouteId: "5",
            apiKeyId: "8",
            endpointType: "openai_chat_completions",
            outcome: "upstream_error",
            streamChoice: "streamed",
            statusInput: "429,500",
        });
    });

    test("reads the failure pair as the failures choice in any order", () => {
        expect(parseUsageFilterDraft("?outcome=upstream_error,internal").outcome).toBe("failures");
        expect(parseUsageFilterDraft("?outcome=internal, upstream_error").outcome).toBe("failures");
    });

    test("reads a hand-written outcome list the control cannot present as absent", () => {
        expect(parseUsageFilterDraft("?outcome=completed,failed").outcome).toBe("");
        expect(parseUsageFilterDraft("?outcome=upstream_error,upstream_error").outcome).toBe("");
        expect(parseUsageFilterDraft("?outcome=nonsense").outcome).toBe("");
    });

    test("reads a value outside its vocabulary as absent", () => {
        expect(parseUsageFilterDraft("?window=year").window).toBe("all");
        expect(parseUsageFilterDraft("?stream=maybe").streamChoice).toBe("");
        expect(parseUsageFilterDraft("?endpoint=graphql").endpointType).toBe("");
    });

    test("reads an entity value that is not a row identifier as absent", () => {
        expect(parseUsageFilterDraft("?provider=abc").providerId).toBe("");
        expect(parseUsageFilterDraft("?provider=1e3").providerId).toBe("");
        expect(parseUsageFilterDraft(`?provider=${Number.MAX_SAFE_INTEGER + 1}`).providerId).toBe(
            "",
        );
        expect(parseUsageFilterDraft("?provider=-1").providerId).toBe("");
        expect(parseUsageFilterDraft("?provider=3").providerId).toBe("3");
    });

    test("carries the status text verbatim, validity being the form's concern", () => {
        expect(parseUsageFilterDraft("?status=4xx,stuff").statusInput).toBe("4xx,stuff");
    });
});

describe("serializeUsageFilterDraft", () => {
    test("writes the unfiltered draft as the empty query", () => {
        expect(serializeUsageFilterDraft(EMPTY_FIELDS)).toBe("");
    });

    test("round-trips a draft through the route reader", () => {
        const draft: UsageFilterDraft = {
            ...EMPTY_FIELDS,
            window: "30d",
            providerId: "3",
            outcome: "failures",
            streamChoice: "non-streamed",
            statusInput: "429, 500",
        };

        expect(parseUsageFilterDraft(serializeUsageFilterDraft(draft))).toEqual(draft);
    });

    test("keeps the failures list's commas literal in the address", () => {
        const query = serializeUsageFilterDraft({
            ...EMPTY_FIELDS,
            outcome: "failures",
        });

        expect(query).toBe("outcome=upstream_error,internal");
    });
});

describe("windowBoundMicros", () => {
    test("resolves a rolling window as a duration before the reading moment", () => {
        const now = 1_800_000_000_000_000;

        expect(windowBoundMicros("all", now)).toBeNull();
        expect(windowBoundMicros("24h", now)).toBe(now - DAY_MICROS);
        expect(windowBoundMicros("7d", now)).toBe(now - 7 * DAY_MICROS);
        expect(windowBoundMicros("30d", now)).toBe(now - 30 * DAY_MICROS);
    });
});

describe("hasUsageFilter", () => {
    test("counts a window alone as a filter", () => {
        expect(
            hasUsageFilter({
                ...EMPTY_FIELDS,
                window: "24h",
            }),
        ).toBe(true);
        expect(hasUsageFilter(EMPTY_FIELDS)).toBe(false);
    });

    test("counts each field, the aggregates page's except-clause included", () => {
        expect(
            hasUsageFilterFields({
                ...EMPTY_FIELDS,
                statusInput: "429",
            }),
        ).toBe(true);
        expect(hasUsageFilterFields(EMPTY_FIELDS)).toBe(false);

        const narrow = {
            ...EMPTY_FIELDS,
            providerId: "3",
        };
        expect(hasUsageFilterFieldsExcept(narrow, "providerId")).toBe(false);
        expect(hasUsageFilterFieldsExcept(narrow, "modelId")).toBe(true);
    });
});

describe("buildUsageFilter", () => {
    test("omits every field with nothing chosen — an empty array is a rejection, never a wildcard", () => {
        expect(buildUsageFilter(EMPTY_FIELDS)).toEqual({});
    });

    test("names each chosen field as the wire's single-value array", () => {
        const filter = buildUsageFilter({
            ...EMPTY_FIELDS,
            providerId: "3",
            modelId: "11",
            modelRouteId: "5",
            apiKeyId: "8",
            endpointType: "openai_chat_completions",
            streamChoice: "streamed",
            statusInput: "429, 500, , 503",
        });

        expect(filter).toEqual({
            provider_id: [
                3,
            ],
            model_id: [
                11,
            ],
            model_route_id: [
                5,
            ],
            api_key_id: [
                8,
            ],
            endpoint_type: [
                "openai_chat_completions",
            ],
            is_stream: true,
            http_status: [
                429,
                500,
                503,
            ],
        });
    });

    test("carries the failures choice as both failure outcomes", () => {
        expect(
            buildUsageFilter({
                ...EMPTY_FIELDS,
                outcome: "failures",
            }).outcome,
        ).toEqual(USAGE_OUTCOME_FAILURES);
        expect(
            buildUsageFilter({
                ...EMPTY_FIELDS,
                outcome: "completed",
            }).outcome,
        ).toEqual([
            "completed",
        ]);
    });

    test("carries the bounds the screen resolved, absent-keeps otherwise", () => {
        expect(
            buildUsageFilter(EMPTY_FIELDS, {
                from: 100,
                to: 200,
            }),
        ).toEqual({
            from: 100,
            to: 200,
        });
    });

    test("resolves the draft's rolling window against the reading moment", () => {
        const now = 1_800_000_000_000_000;
        const filter = draftUsageFilter(
            {
                ...EMPTY_FIELDS,
                window: "24h",
            },
            now,
        );

        expect(filter.from).toBe(now - DAY_MICROS);
        expect(filter.to).toBeUndefined();
        expect(filter.provider_id).toBeUndefined();
    });
});

describe("statusError", () => {
    test("answers null for blank, for whole entries, and for surrounding blanks", () => {
        expect(statusError("")).toBeNull();
        expect(statusError("429, 500")).toBeNull();
        expect(statusError("429,,500")).toBeNull();
    });

    test("names the mistake for an entry no integer reading accepts", () => {
        expect(statusError("2xx")).not.toBeNull();
        expect(statusError("429,oops")).not.toBeNull();
        expect(statusError("+200")).not.toBeNull();
        expect(statusError(`1${"0".repeat(19)}`)).not.toBeNull();
    });
});

describe("subject derivations", () => {
    test("prefills exactly the subject dimension's own draft field", () => {
        expect(
            subjectFilterFields({
                dimension: "provider",
                id: 3,
            }),
        ).toEqual({
            ...EMPTY_USAGE_FILTER_FIELDS,
            providerId: "3",
        });
        expect(
            subjectFilterFields({
                dimension: "api_key",
                id: 8,
            }),
        ).toEqual({
            ...EMPTY_USAGE_FILTER_FIELDS,
            apiKeyId: "8",
        });
        expect(subjectFilterFields(null)).toEqual(EMPTY_USAGE_FILTER_FIELDS);
    });

    test("crosses the stream with the fields verbatim and the window reset to all", () => {
        const draft = streamDraftOf({
            ...EMPTY_FIELDS,
            providerId: "3",
            statusInput: "429",
        });

        expect(draft).toEqual({
            ...EMPTY_FIELDS,
            providerId: "3",
            statusInput: "429",
            window: "all",
        });
    });

    test("collects one candidate per named dimension and answers exactly one as the subject", () => {
        const named = namedSubjects((dimension) =>
            dimension === "model" ? 11 : dimension === "api_key" ? 8 : null,
        );

        expect(named).toEqual([
            {
                dimension: "model",
                id: 11,
            },
            {
                dimension: "api_key",
                id: 8,
            },
        ]);
        expect(soleSubject(named)).toBeNull();
        expect(
            soleSubject([
                {
                    dimension: "model",
                    id: 11,
                },
            ]),
        ).toEqual({
            dimension: "model",
            id: 11,
        });
        expect(soleSubject([])).toBeNull();
    });

    test("carries a stream's single entity narrow to the aggregates page", () => {
        expect(
            streamAggregatesSubject({
                ...EMPTY_FIELDS,
                providerId: "3",
            }),
        ).toEqual({
            dimension: "provider",
            id: 3,
        });
        expect(
            streamAggregatesSubject({
                ...EMPTY_FIELDS,
                modelRouteId: "5",
            }),
        ).toEqual({
            dimension: "model_route",
            id: 5,
        });
    });

    test("carries no subject for none named or several named", () => {
        expect(streamAggregatesSubject(EMPTY_FIELDS)).toBeUndefined();
        expect(
            streamAggregatesSubject({
                ...EMPTY_FIELDS,
                providerId: "3",
                modelId: "11",
            }),
        ).toBeUndefined();
    });

    test("reads the aggregates subject back off a subject filter, a shape naming none being fatal", () => {
        expect(
            usageQuerySubjectOfFilter({
                model_id: 11,
            }),
        ).toEqual({
            dimension: "model",
            id: 11,
        });
        expect(() => usageQuerySubjectOfFilter({} as never)).toThrowError(
            "the usage subject filter names no subject dimension",
        );
    });
});
