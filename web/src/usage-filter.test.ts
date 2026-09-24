import { describe, expect, it } from "vitest";
import {
    buildUsageFilter,
    draftUsageFilter,
    EMPTY_USAGE_FILTER_DRAFT,
    EMPTY_USAGE_FILTER_FIELDS,
    hasUsageFilter,
    parseUsageFilterDraft,
    serializeUsageFilterDraft,
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
                "?window=99d&provider=abc&model=-1&endpoint=openai_embeddings&stream=maybe&key=7",
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
            parseUsageFilterDraft("?provider=99999999999999999999999&model=9007199254740992"),
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
