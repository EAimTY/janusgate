import { describe, expect, test } from "vitest";
import { ContractViolationError } from "./errors";
import {
    expectQuotaPath,
    parseQuotaExhaustion,
    parseQuotaMeasurement,
    parseQuotaReset,
    parseQuotaRouting,
    parseQuotaScope,
    parseQuotaWindow,
} from "./quota";

/**
 * The quota observation parsers: the closed variant sets the wire carries
 * (scope, window, measurement, reset, routing, exhaustion), the declared-keys
 * strictness that names an unknown field instead of dropping it, the amount
 * rule (finite, non-negative; positive where demanded; negative zero
 * normalized), and the exact calendar-date rule for a `date` reset.
 */

describe("expectQuotaPath", () => {
    test("narrows an array of non-empty member strings", () => {
        expect(
            expectQuotaPath(
                [
                    "usage",
                    "tokens",
                ],
                "response.key",
            ),
        ).toEqual([
            "usage",
            "tokens",
        ]);
    });

    test("names the offending index for an empty member", () => {
        expect(() =>
            expectQuotaPath(
                [
                    "usage",
                    "",
                ],
                "response.key",
            ),
        ).toThrowError("response.key[1] must not be empty");
    });

    test("rejects a non-array path", () => {
        expect(() => expectQuotaPath("usage", "response.key")).toThrowError(ContractViolationError);
    });
});

describe("parseQuotaRouting", () => {
    test("answers the monitoring-only and all-models tags alone", () => {
        expect(
            parseQuotaRouting(
                {
                    type: "monitoring_only",
                },
                "q.routing",
            ),
        ).toEqual({
            type: "monitoring_only",
        });
        expect(
            parseQuotaRouting(
                {
                    type: "all_models",
                },
                "q.routing",
            ),
        ).toEqual({
            type: "all_models",
        });
    });

    test("answers the models tag with its non-empty unique upstream id set", () => {
        expect(
            parseQuotaRouting(
                {
                    type: "models",
                    upstream_ids: [
                        "gpt-x",
                        "gpt-y",
                    ],
                },
                "q.routing",
            ),
        ).toEqual({
            type: "models",
            upstream_ids: [
                "gpt-x",
                "gpt-y",
            ],
        });
    });

    test("rejects an empty or duplicated upstream id set", () => {
        expect(() =>
            parseQuotaRouting(
                {
                    type: "models",
                    upstream_ids: [],
                },
                "q.routing",
            ),
        ).toThrowError("nonempty set of unique model identifiers");
        expect(() =>
            parseQuotaRouting(
                {
                    type: "models",
                    upstream_ids: [
                        "gpt-x",
                        "gpt-x",
                    ],
                },
                "q.routing",
            ),
        ).toThrowError(ContractViolationError);
    });

    test("rejects an unknown tag and an undeclared member", () => {
        expect(() =>
            parseQuotaRouting(
                {
                    type: "everything",
                },
                "q.routing",
            ),
        ).toThrowError("q.routing.type is not a routing applicability: everything");
        expect(() =>
            parseQuotaRouting(
                {
                    type: "all_models",
                    upstream_ids: [
                        "gpt-x",
                    ],
                },
                "q.routing",
            ),
        ).toThrowError("q.routing.upstream_ids is not a declared field");
    });
});

describe("parseQuotaScope", () => {
    test("answers the unknown and named variants", () => {
        expect(
            parseQuotaScope(
                {
                    type: "unknown",
                },
                "q.scope",
            ),
        ).toEqual({
            type: "unknown",
        });
        expect(
            parseQuotaScope(
                {
                    type: "named",
                    name: "weekly",
                },
                "q.scope",
            ),
        ).toEqual({
            type: "named",
            name: "weekly",
        });
    });

    test("rejects a foreign tag, an empty name, and an undeclared member", () => {
        expect(() =>
            parseQuotaScope(
                {
                    type: "team",
                },
                "q.scope",
            ),
        ).toThrowError("q.scope.type is not a quota scope: team");
        expect(() =>
            parseQuotaScope(
                {
                    type: "named",
                    name: "",
                },
                "q.scope",
            ),
        ).toThrowError("q.scope.name must not be empty");
        expect(() =>
            parseQuotaScope(
                {
                    type: "named",
                    name: "w",
                    extra: 1,
                },
                "q.scope",
            ),
        ).toThrowError("q.scope.extra is not a declared field");
    });
});

describe("parseQuotaWindow", () => {
    test("answers unknown, duration, and calendar-month windows", () => {
        expect(
            parseQuotaWindow(
                {
                    type: "unknown",
                },
                "q.window",
            ),
        ).toEqual({
            type: "unknown",
        });
        expect(
            parseQuotaWindow(
                {
                    type: "duration",
                    seconds: 3600,
                },
                "q.window",
            ),
        ).toEqual({
            type: "duration",
            seconds: 3600,
        });
        expect(
            parseQuotaWindow(
                {
                    type: "calendar",
                    unit: "month",
                },
                "q.window",
            ),
        ).toEqual({
            type: "calendar",
            unit: "month",
        });
    });

    test("rejects a non-positive duration and a foreign calendar unit", () => {
        expect(() =>
            parseQuotaWindow(
                {
                    type: "duration",
                    seconds: 0,
                },
                "q.window",
            ),
        ).toThrowError("q.window.seconds must be a safe integer of at least 1");
        expect(() =>
            parseQuotaWindow(
                {
                    type: "calendar",
                    unit: "week",
                },
                "q.window",
            ),
        ).toThrowError("q.window.unit is not a calendar unit: week");
    });
});

describe("parseQuotaMeasurement", () => {
    test("answers the four measurement variants with their declared members", () => {
        expect(
            parseQuotaMeasurement(
                {
                    type: "unknown",
                },
                "q.measurement",
            ),
        ).toEqual({
            type: "unknown",
        });
        expect(
            parseQuotaMeasurement(
                {
                    type: "unlimited",
                },
                "q.measurement",
            ),
        ).toEqual({
            type: "unlimited",
        });
        expect(
            parseQuotaMeasurement(
                {
                    type: "ratio",
                    used_fraction: 0.25,
                },
                "q.measurement",
            ),
        ).toEqual({
            type: "ratio",
            used_fraction: 0.25,
        });
        expect(
            parseQuotaMeasurement(
                {
                    type: "counter",
                    used: 120.5,
                    limit: 200,
                    unit: "tokens",
                },
                "q.measurement",
            ),
        ).toEqual({
            type: "counter",
            used: 120.5,
            limit: 200,
            unit: "tokens",
        });
        expect(
            parseQuotaMeasurement(
                {
                    type: "remaining",
                    remaining: 5,
                    unit: "requests",
                },
                "q.measurement",
            ),
        ).toEqual({
            type: "remaining",
            remaining: 5,
            unit: "requests",
        });
    });

    test("keeps finite decimal amounts but normalizes negative zero", () => {
        const ratio = parseQuotaMeasurement(
            {
                type: "ratio",
                used_fraction: -0,
            },
            "q.measurement",
        );

        expect(ratio).toEqual({
            type: "ratio",
            used_fraction: 0,
        });
        expect(Object.is(ratio.type === "ratio" ? ratio.used_fraction : null, 0)).toBe(true);
    });

    test("rejects negative, non-finite, and huge integer amounts", () => {
        expect(() =>
            parseQuotaMeasurement(
                {
                    type: "ratio",
                    used_fraction: -1,
                },
                "q.measurement",
            ),
        ).toThrowError("finite non-negative amount");
        expect(() =>
            parseQuotaMeasurement(
                {
                    type: "counter",
                    used: Number.POSITIVE_INFINITY,
                    limit: 1,
                    unit: "t",
                },
                "q.measurement",
            ),
        ).toThrowError(ContractViolationError);
        expect(() =>
            parseQuotaMeasurement(
                {
                    type: "counter",
                    used: 2 ** 53,
                    limit: 1,
                    unit: "t",
                },
                "q.measurement",
            ),
        ).toThrowError(ContractViolationError);
    });

    test("rejects a zero limit — a counter's ceiling is positive", () => {
        expect(() =>
            parseQuotaMeasurement(
                {
                    type: "counter",
                    used: 0,
                    limit: 0,
                    unit: "t",
                },
                "q.measurement",
            ),
        ).toThrowError("finite positive amount");
    });

    test("rejects a foreign tag and an undeclared member", () => {
        expect(() =>
            parseQuotaMeasurement(
                {
                    type: "fraction",
                },
                "q.measurement",
            ),
        ).toThrowError("q.measurement.type is not a quota measurement: fraction");
        expect(() =>
            parseQuotaMeasurement(
                {
                    type: "unlimited",
                    remaining: 0,
                },
                "q.measurement",
            ),
        ).toThrowError("q.measurement.remaining is not a declared field");
    });
});

describe("parseQuotaReset", () => {
    test("answers unknown, at-instant, and calendar-date resets", () => {
        expect(
            parseQuotaReset(
                {
                    type: "unknown",
                },
                "q.reset",
            ),
        ).toEqual({
            type: "unknown",
        });
        expect(
            parseQuotaReset(
                {
                    type: "at",
                    at: 1_700_000_000_000_000,
                },
                "q.reset",
            ),
        ).toEqual({
            type: "at",
            at: 1_700_000_000_000_000,
        });
        expect(
            parseQuotaReset(
                {
                    type: "date",
                    date: "2026-02-28",
                },
                "q.reset",
            ),
        ).toEqual({
            type: "date",
            date: "2026-02-28",
        });
    });

    test("accepts the leap day of a leap year and rejects impossible dates", () => {
        expect(
            parseQuotaReset(
                {
                    type: "date",
                    date: "2024-02-29",
                },
                "q.reset",
            ).type,
        ).toBe("date");
        expect(() =>
            parseQuotaReset(
                {
                    type: "date",
                    date: "2023-02-29",
                },
                "q.reset",
            ),
        ).toThrowError("q.reset.date is not a calendar date: 2023-02-29");
        expect(() =>
            parseQuotaReset(
                {
                    type: "date",
                    date: "2026-13-01",
                },
                "q.reset",
            ),
        ).toThrowError(ContractViolationError);
        expect(() =>
            parseQuotaReset(
                {
                    type: "date",
                    date: "2026-04-31",
                },
                "q.reset",
            ),
        ).toThrowError(ContractViolationError);
    });

    test("rejects a foreign tag", () => {
        expect(() =>
            parseQuotaReset(
                {
                    type: "soon",
                },
                "q.reset",
            ),
        ).toThrowError("q.reset.type is not a quota reset: soon");
    });
});

describe("parseQuotaExhaustion", () => {
    /** One complete valid rule, the fixture every mutation below starts from. */
    const RULE = {
        name: "hard cap",
        statuses: [
            429,
            503,
        ],
        code: {
            at: [
                "error",
                "code",
            ],
            value: "rate_limited",
        },
        restriction_key: [
            {
                type: "requested_model",
            },
        ],
        applies_to: {
            type: "all_models",
        },
    };

    test("answers the rules with deduplicated sorted statuses", () => {
        const exhaustion = parseQuotaExhaustion(
            {
                rules: [
                    {
                        ...RULE,
                        statuses: [
                            503,
                            429,
                            429,
                        ],
                    },
                ],
            },
            "definition.quota_exhaustion",
        );

        expect(exhaustion.rules).toHaveLength(1);
        expect(exhaustion.rules[0]?.statuses).toEqual([
            429,
            503,
        ]);
    });

    test("narrows a literal restriction-key segment with its non-empty value", () => {
        const exhaustion = parseQuotaExhaustion(
            {
                rules: [
                    {
                        ...RULE,
                        restriction_key: [
                            {
                                type: "literal",
                                value: "gpt-x",
                            },
                        ],
                    },
                ],
            },
            "definition.quota_exhaustion",
        );

        expect(exhaustion.rules[0]?.restriction_key).toEqual([
            {
                type: "literal",
                value: "gpt-x",
            },
        ]);
    });

    test("rejects a repeated rule name, an empty rule list, and empty statuses", () => {
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        RULE,
                        {
                            ...RULE,
                            statuses: [
                                500,
                            ],
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError("must not repeat an earlier rule name");
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [],
                },
                "x",
            ),
        ).toThrowError("x.rules must not be empty");
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        {
                            ...RULE,
                            statuses: [],
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError("statuses must not be empty");
    });

    test("rejects a 2xx status — a rejection policy states non-success statuses", () => {
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        {
                            ...RULE,
                            statuses: [
                                200,
                                429,
                            ],
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError("is not a rejected status: 200");
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        {
                            ...RULE,
                            statuses: [
                                99,
                            ],
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError(ContractViolationError);
    });

    test("rejects an empty code path and an empty restriction key", () => {
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        {
                            ...RULE,
                            code: {
                                at: [],
                                value: "x",
                            },
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError("code.at must not be empty");
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        {
                            ...RULE,
                            restriction_key: [],
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError("restriction_key must not be empty");
    });

    test("rejects an undeclared member anywhere in the walk", () => {
        expect(() =>
            parseQuotaExhaustion(
                {
                    rules: [
                        {
                            ...RULE,
                            surprise: 1,
                        },
                    ],
                },
                "x",
            ),
        ).toThrowError("x.rules[0].surprise is not a declared field");
    });
});
