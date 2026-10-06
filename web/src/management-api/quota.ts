import { ContractViolationError } from "./errors";
import type {
    QuotaExhaustion,
    QuotaExhaustionRule,
    QuotaMeasurement,
    QuotaReset,
    QuotaRouting,
    QuotaScope,
    QuotaWindow,
    RestrictionKeySegment,
} from "./generated/v0";
import { expectModelIdentifier } from "./identifiers";
import {
    expectArray,
    expectDeclaredKeys,
    expectDeclaredRecord,
    expectNonEmptyString,
    expectOneOf,
    expectRecord,
    expectSafeInteger,
    expectString,
} from "./validation-primitives";

/**
 * Object-member paths share the existing descriptor language (README §5.1),
 * with every member named and none empty: a quota identity segment is never
 * the empty string.
 */
export function expectQuotaPath(value: unknown, context: string): string[] {
    return expectArray(value, context).map((member, index) =>
        expectNonEmptyString(member, `${context}[${index}]`),
    );
}

/** Finite informational amounts keep decimals, over-limit use, and normalize negative zero. */
function amount(value: unknown, context: string, isPositive = false): number {
    if (
        typeof value !== "number" ||
        !Number.isFinite(value) ||
        value < 0 ||
        (isPositive && value === 0) ||
        (Number.isInteger(value) && !Number.isSafeInteger(value))
    ) {
        throw new ContractViolationError(
            `${context} must be a finite ${isPositive ? "positive" : "non-negative"} amount`,
        );
    }

    return value === 0 ? 0 : value;
}

/** Validates an exact real calendar date without constructing a timezone-dependent instant. */
function isQuotaDate(value: string): boolean {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
    const year = Number(value.slice(0, 4));
    const month = Number(value.slice(5, 7));
    const day = Number(value.slice(8, 10));
    const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
    const days = [
        31,
        leap ? 29 : 28,
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    const maximum = days[month - 1];
    return maximum !== undefined && day >= 1 && day <= maximum;
}

/** Concrete routing applicability is required on every complete observation. */
export function parseQuotaRouting(value: unknown, context: string): QuotaRouting {
    const record = expectRecord(value, context);
    const type = expectOneOf(
        Reflect.get(record, "type"),
        [
            "monitoring_only",
            "all_models",
            "models",
        ],
        `${context}.type`,
        "a routing applicability",
    );
    expectDeclaredKeys(
        record,
        type === "models"
            ? [
                  "type",
                  "upstream_ids",
              ]
            : [
                  "type",
              ],
        context,
    );
    if (type !== "models")
        return {
            type,
        };
    const upstream_ids = expectArray(
        Reflect.get(record, "upstream_ids"),
        `${context}.upstream_ids`,
    ).map((value, index) => expectModelIdentifier(value, `${context}.upstream_ids[${index}]`));
    if (upstream_ids.length === 0 || new Set(upstream_ids).size !== upstream_ids.length) {
        throw new ContractViolationError(
            `${context}.upstream_ids must be a nonempty set of unique model identifiers`,
        );
    }
    return {
        type,
        upstream_ids,
    };
}

/** Exact rejection policies share quota identities while retaining authored rule order. */
export function parseQuotaExhaustion(value: unknown, context: string): QuotaExhaustion {
    const record = expectDeclaredRecord(
        value,
        [
            "rules",
        ],
        context,
    );
    const names = new Set<string>();
    const rules = expectArray(Reflect.get(record, "rules"), `${context}.rules`).map(
        (value, index): QuotaExhaustionRule => {
            const ruleContext = `${context}.rules[${index}]`;
            const rule = expectDeclaredRecord(
                value,
                [
                    "name",
                    "statuses",
                    "code",
                    "restriction_key",
                    "applies_to",
                ],
                ruleContext,
            );
            const name = expectNonEmptyString(Reflect.get(rule, "name"), `${ruleContext}.name`);
            if (names.has(name)) {
                throw new ContractViolationError(
                    `${ruleContext}.name must not repeat an earlier rule name`,
                );
            }
            names.add(name);
            const statuses = Array.from(
                new Set(
                    expectArray(Reflect.get(rule, "statuses"), `${ruleContext}.statuses`).map(
                        (value, index) => {
                            const statusContext = `${ruleContext}.statuses[${index}]`;
                            const status = expectSafeInteger(value, statusContext);
                            if (status < 100 || status > 999 || (status >= 200 && status < 300)) {
                                throw new ContractViolationError(
                                    `${statusContext} is not a rejected status: ${status}`,
                                );
                            }
                            return status;
                        },
                    ),
                ),
            ).sort((left, right) => left - right);
            if (statuses.length === 0) {
                throw new ContractViolationError(`${ruleContext}.statuses must not be empty`);
            }
            const code = expectDeclaredRecord(
                Reflect.get(rule, "code"),
                [
                    "at",
                    "value",
                ],
                `${ruleContext}.code`,
            );
            const at = expectQuotaPath(Reflect.get(code, "at"), `${ruleContext}.code.at`);
            if (at.length === 0) {
                throw new ContractViolationError(`${ruleContext}.code.at must not be empty`);
            }
            const restriction_key = expectArray(
                Reflect.get(rule, "restriction_key"),
                `${ruleContext}.restriction_key`,
            ).map((value, index): RestrictionKeySegment => {
                const segmentContext = `${ruleContext}.restriction_key[${index}]`;
                const segment = expectRecord(value, segmentContext);
                const type = expectOneOf(
                    Reflect.get(segment, "type"),
                    [
                        "literal",
                        "requested_model",
                    ],
                    `${segmentContext}.type`,
                    "a restriction key segment",
                );
                expectDeclaredKeys(
                    segment,
                    type === "literal"
                        ? [
                              "type",
                              "value",
                          ]
                        : [
                              "type",
                          ],
                    segmentContext,
                );
                return type === "literal"
                    ? {
                          type,
                          value: expectNonEmptyString(
                              Reflect.get(segment, "value"),
                              `${segmentContext}.value`,
                          ),
                      }
                    : {
                          type,
                      };
            });
            if (restriction_key.length === 0) {
                throw new ContractViolationError(
                    `${ruleContext}.restriction_key must not be empty`,
                );
            }
            const applies = expectDeclaredRecord(
                Reflect.get(rule, "applies_to"),
                [
                    "type",
                ],
                `${ruleContext}.applies_to`,
            );
            return {
                name,
                statuses,
                code: {
                    at,
                    value: expectString(Reflect.get(code, "value"), `${ruleContext}.code.value`),
                },
                restriction_key,
                applies_to: {
                    type: expectOneOf(
                        Reflect.get(applies, "type"),
                        [
                            "all_models",
                            "requested_model",
                        ],
                        `${ruleContext}.applies_to.type`,
                        "a rejection applicability",
                    ),
                },
            };
        },
    );
    if (rules.length === 0) {
        throw new ContractViolationError(`${context}.rules must not be empty`);
    }
    return {
        rules,
    };
}

/** A normalized scope variant. */
export function parseQuotaScope(value: unknown, context: string): QuotaScope {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    switch (type) {
        case "unknown":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type,
            };
        case "named":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "name",
                ],
                context,
            );
            return {
                type,
                name: expectNonEmptyString(Reflect.get(record, "name"), `${context}.name`),
            };
        default:
            throw new ContractViolationError(`${context}.type is not a quota scope: ${type}`);
    }
}

/** A normalized period, independent of its reset metadata. */
export function parseQuotaWindow(value: unknown, context: string): QuotaWindow {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    switch (type) {
        case "unknown":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type,
            };
        case "duration":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "seconds",
                ],
                context,
            );
            return {
                type,
                seconds: expectSafeInteger(Reflect.get(record, "seconds"), `${context}.seconds`, 1),
            };
        case "calendar":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "unit",
                ],
                context,
            );
            return {
                type,
                unit: expectOneOf(
                    Reflect.get(record, "unit"),
                    [
                        "month",
                    ],
                    `${context}.unit`,
                    "a calendar unit",
                ),
            };
        default:
            throw new ContractViolationError(`${context}.type is not a quota window: ${type}`);
    }
}

/** A normalized measurement; no totals or remaining percentages are invented. */
export function parseQuotaMeasurement(value: unknown, context: string): QuotaMeasurement {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    switch (type) {
        case "unknown":
        case "unlimited":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type,
            };
        case "ratio":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "used_fraction",
                ],
                context,
            );
            return {
                type,
                used_fraction: amount(
                    Reflect.get(record, "used_fraction"),
                    `${context}.used_fraction`,
                ),
            };
        case "counter":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "used",
                    "limit",
                    "unit",
                ],
                context,
            );
            return {
                type,
                used: amount(Reflect.get(record, "used"), `${context}.used`),
                limit: amount(Reflect.get(record, "limit"), `${context}.limit`, true),
                unit: expectNonEmptyString(Reflect.get(record, "unit"), `${context}.unit`),
            };
        case "remaining":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "remaining",
                    "unit",
                ],
                context,
            );
            return {
                type,
                remaining: amount(Reflect.get(record, "remaining"), `${context}.remaining`),
                unit: expectNonEmptyString(Reflect.get(record, "unit"), `${context}.unit`),
            };
        default:
            throw new ContractViolationError(`${context}.type is not a quota measurement: ${type}`);
    }
}

/** An exact instant or calendar date, never a guessed midnight. */
export function parseQuotaReset(value: unknown, context: string): QuotaReset {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    switch (type) {
        case "unknown":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type,
            };
        case "at":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "at",
                ],
                context,
            );
            return {
                type,
                at: expectSafeInteger(Reflect.get(record, "at"), `${context}.at`),
            };
        case "date": {
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "date",
                ],
                context,
            );
            const date = expectNonEmptyString(Reflect.get(record, "date"), `${context}.date`);
            if (!isQuotaDate(date)) {
                throw new ContractViolationError(`${context}.date is not a calendar date: ${date}`);
            }
            return {
                type,
                date,
            };
        }
        default:
            throw new ContractViolationError(`${context}.type is not a quota reset: ${type}`);
    }
}
