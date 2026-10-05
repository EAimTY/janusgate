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
import {
    amount,
    choice,
    integer,
    invalid,
    isQuotaDate,
    modelIdentifier,
    parseQuotaPath,
    shape,
    text,
} from "./quota-validation";
import { expectArray, expectRecord, expectString } from "./validation-primitives";

/** Concrete routing applicability is required on every complete observation. */
export function parseQuotaRouting(value: unknown, context: string): QuotaRouting {
    const record = expectRecord(value, context);
    const type = choice(
        Reflect.get(record, "type"),
        [
            "monitoring_only",
            "all_models",
            "models",
        ],
        `${context}.type`,
    );
    shape(
        value,
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
    ).map((value, index) => modelIdentifier(value, `${context}.upstream_ids[${index}]`));
    if (upstream_ids.length === 0 || new Set(upstream_ids).size !== upstream_ids.length)
        invalid(context);
    return {
        type,
        upstream_ids,
    };
}

/** Exact rejection policies share quota identities while retaining authored rule order. */
export function parseQuotaExhaustion(value: unknown, context: string): QuotaExhaustion {
    const record = shape(
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
            const rule = shape(
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
            const name = text(Reflect.get(rule, "name"), `${ruleContext}.name`);
            if (names.has(name)) invalid(`${ruleContext}.name`);
            names.add(name);
            const statuses = Array.from(
                new Set(
                    expectArray(Reflect.get(rule, "statuses"), `${ruleContext}.statuses`).map(
                        (value, index) => {
                            const statusContext = `${ruleContext}.statuses[${index}]`;
                            const status = integer(value, statusContext);
                            if (status < 100 || status > 999 || (status >= 200 && status < 300))
                                invalid(statusContext);
                            return status;
                        },
                    ),
                ),
            ).sort((left, right) => left - right);
            if (statuses.length === 0) invalid(`${ruleContext}.statuses`);
            const code = shape(
                Reflect.get(rule, "code"),
                [
                    "at",
                    "value",
                ],
                `${ruleContext}.code`,
            );
            const at = parseQuotaPath(Reflect.get(code, "at"), `${ruleContext}.code.at`);
            if (at.length === 0) invalid(`${ruleContext}.code.at`);
            const restriction_key = expectArray(
                Reflect.get(rule, "restriction_key"),
                `${ruleContext}.restriction_key`,
            ).map((value, index): RestrictionKeySegment => {
                const segmentContext = `${ruleContext}.restriction_key[${index}]`;
                const segment = expectRecord(value, segmentContext);
                const type = choice(
                    Reflect.get(segment, "type"),
                    [
                        "literal",
                        "requested_model",
                    ],
                    `${segmentContext}.type`,
                );
                shape(
                    value,
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
                          value: text(Reflect.get(segment, "value"), `${segmentContext}.value`),
                      }
                    : {
                          type,
                      };
            });
            if (restriction_key.length === 0) invalid(`${ruleContext}.restriction_key`);
            const applies = shape(
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
                    type: choice(
                        Reflect.get(applies, "type"),
                        [
                            "all_models",
                            "requested_model",
                        ],
                        `${ruleContext}.applies_to.type`,
                    ),
                },
            };
        },
    );
    if (rules.length === 0) invalid(`${context}.rules`);
    return {
        rules,
    };
}

/** A normalized scope variant. */
export function scope(value: unknown, context: string): QuotaScope {
    const record = expectRecord(value, context);
    switch (Reflect.get(record, "type")) {
        case "unknown":
            shape(
                value,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "unknown",
            };
        case "named":
            shape(
                value,
                [
                    "type",
                    "name",
                ],
                context,
            );
            return {
                type: "named",
                name: text(Reflect.get(record, "name"), `${context}.name`),
            };
        default:
            return invalid(context);
    }
}

/** A normalized period, independent of its reset metadata. */
export function window(value: unknown, context: string): QuotaWindow {
    const record = expectRecord(value, context);
    switch (Reflect.get(record, "type")) {
        case "unknown":
            shape(
                value,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "unknown",
            };
        case "duration":
            shape(
                value,
                [
                    "type",
                    "seconds",
                ],
                context,
            );
            return {
                type: "duration",
                seconds: integer(Reflect.get(record, "seconds"), `${context}.seconds`, true),
            };
        case "calendar":
            shape(
                value,
                [
                    "type",
                    "unit",
                ],
                context,
            );
            return {
                type: "calendar",
                unit: choice(
                    Reflect.get(record, "unit"),
                    [
                        "month",
                    ],
                    `${context}.unit`,
                ),
            };
        default:
            return invalid(context);
    }
}

/** A normalized measurement; no totals or remaining percentages are invented. */
export function measurement(value: unknown, context: string): QuotaMeasurement {
    const record = expectRecord(value, context);
    switch (Reflect.get(record, "type")) {
        case "unknown":
            shape(
                value,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "unknown",
            };
        case "unlimited":
            shape(
                value,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "unlimited",
            };
        case "ratio":
            shape(
                value,
                [
                    "type",
                    "used_fraction",
                ],
                context,
            );
            return {
                type: "ratio",
                used_fraction: amount(
                    Reflect.get(record, "used_fraction"),
                    `${context}.used_fraction`,
                ),
            };
        case "counter":
            shape(
                value,
                [
                    "type",
                    "used",
                    "limit",
                    "unit",
                ],
                context,
            );
            return {
                type: "counter",
                used: amount(Reflect.get(record, "used"), `${context}.used`),
                limit: amount(Reflect.get(record, "limit"), `${context}.limit`, true),
                unit: text(Reflect.get(record, "unit"), `${context}.unit`),
            };
        case "remaining":
            shape(
                value,
                [
                    "type",
                    "remaining",
                    "unit",
                ],
                context,
            );
            return {
                type: "remaining",
                remaining: amount(Reflect.get(record, "remaining"), `${context}.remaining`),
                unit: text(Reflect.get(record, "unit"), `${context}.unit`),
            };
        default:
            return invalid(context);
    }
}

/** An exact instant or calendar date, never a guessed midnight. */
export function reset(value: unknown, context: string): QuotaReset {
    const record = expectRecord(value, context);
    switch (Reflect.get(record, "type")) {
        case "unknown":
            shape(
                value,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "unknown",
            };
        case "at":
            shape(
                value,
                [
                    "type",
                    "at",
                ],
                context,
            );
            return {
                type: "at",
                at: integer(Reflect.get(record, "at"), `${context}.at`),
            };
        case "date": {
            shape(
                value,
                [
                    "type",
                    "date",
                ],
                context,
            );
            const date = text(Reflect.get(record, "date"), `${context}.date`);
            if (!isQuotaDate(date)) invalid(`${context}.date`);
            return {
                type: "date",
                date,
            };
        }
        default:
            return invalid(context);
    }
}
