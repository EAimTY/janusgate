import { ContractViolationError } from "/src/management-api/errors";
import type {
    ControlRequest,
    CredentialAcquisition,
    GetProviderQuotaResponse,
    ProvisioningStep,
    QuotaDescriptor,
    QuotaEntry,
    QuotaEntryExtraction,
    QuotaField,
    QuotaMeasurement,
    QuotaMeasurementSource,
    QuotaNumberField,
    QuotaReset,
    QuotaResetSource,
    QuotaScope,
    QuotaScopeSource,
    QuotaSource,
    QuotaTextSource,
    QuotaWindow,
    QuotaWindowSource,
    StringExpr,
    TemplateNode,
} from "/src/management-api/generated/v0";
import {
    expectArray,
    expectBoolean,
    expectRecord,
    expectString,
} from "/src/management-api/validation";

/** Rejects a contract violation at a safe, known JSON path. */
function invalid(context: string): never {
    throw new ContractViolationError(`${context} violates the quota contract`);
}

/** Closed quota shape; unknown fields cannot be discarded during editing. */
function shape(value: unknown, keys: readonly string[], context: string): object {
    const record = expectRecord(value, context);
    for (const key of Object.keys(record)) {
        if (!keys.includes(key)) invalid(`${context}.${key}`);
    }
    return record;
}

/** Narrows a generated closed vocabulary without unchecked assertions. */
function choice<T extends string>(value: unknown, choices: readonly T[], context: string): T {
    const selected = choices.find((candidate) => candidate === value);
    return selected ?? invalid(context);
}

/** Nonempty text is retained verbatim, including meaningful spaces. */
function text(value: unknown, context: string): string {
    const result = expectString(value, context);
    return result.length > 0 ? result : invalid(context);
}

/** A frontend-safe integer, optionally positive. */
function integer(value: unknown, context: string, isPositive = false): number {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || (isPositive && value <= 0)) {
        invalid(context);
    }
    return value;
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
        invalid(context);
    }
    return value === 0 ? 0 : value;
}

/** Validates an exact real calendar date without constructing a timezone-dependent instant. */
export function isQuotaDate(value: string): boolean {
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

/** A normalized scope variant. */
function scope(value: unknown, context: string): QuotaScope {
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
function window(value: unknown, context: string): QuotaWindow {
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
function measurement(value: unknown, context: string): QuotaMeasurement {
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
function reset(value: unknown, context: string): QuotaReset {
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

/** Validates the entire observation before any row reaches the UI. */
export function parseGetProviderQuotaResponse(value: unknown): GetProviderQuotaResponse {
    const record = shape(
        value,
        [
            "observed_at",
            "quotas",
        ],
        "response",
    );
    const identities = new Set<string>();
    const quotas = expectArray(Reflect.get(record, "quotas"), "response.quotas").map(
        (value, index): QuotaEntry => {
            const context = `response.quotas[${index}]`;
            const record = shape(
                value,
                [
                    "key",
                    "label",
                    "scope",
                    "window",
                    "measurement",
                    "reset",
                ],
                context,
            );
            const key = expectArray(Reflect.get(record, "key"), `${context}.key`).map(
                (part, index) => text(part, `${context}.key[${index}]`),
            );
            if (key.length === 0 || identities.has(JSON.stringify(key))) invalid(`${context}.key`);
            identities.add(JSON.stringify(key));
            return {
                key,
                label: text(Reflect.get(record, "label"), `${context}.label`),
                scope: scope(Reflect.get(record, "scope"), `${context}.scope`),
                window: window(Reflect.get(record, "window"), `${context}.window`),
                measurement: measurement(
                    Reflect.get(record, "measurement"),
                    `${context}.measurement`,
                ),
                reset: reset(Reflect.get(record, "reset"), `${context}.reset`),
            };
        },
    );
    return {
        observed_at: integer(Reflect.get(record, "observed_at"), "response.observed_at"),
        quotas,
    };
}

/** Object-member paths share the existing descriptor language. */
function path(value: unknown, context: string): string[] {
    return expectArray(value, context).map((part, index) => text(part, `${context}[${index}]`));
}

/** A scalar selection with a stage-available context. */
function field(value: unknown, context: string, canUseEntry = true): QuotaField {
    const record = shape(
        value,
        [
            "base",
            "path",
            "is_required",
        ],
        context,
    );
    const base = choice(
        Reflect.get(record, "base"),
        [
            "root",
            "item",
            "entry",
        ],
        `${context}.base`,
    );
    if (!canUseEntry && base === "entry") invalid(`${context}.base`);
    return {
        base,
        path: path(Reflect.get(record, "path"), `${context}.path`),
        is_required: expectBoolean(Reflect.get(record, "is_required"), `${context}.is_required`),
    };
}

/** A text mapping; map keys exist only in map collections. */
function textSource(value: unknown, context: string, isMap: boolean): QuotaTextSource {
    const record = expectRecord(value, context);
    switch (Reflect.get(record, "type")) {
        case "literal":
            shape(
                value,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type: "literal",
                value: text(Reflect.get(record, "value"), `${context}.value`),
            };
        case "field":
            shape(
                value,
                [
                    "type",
                    "field",
                    "encoding",
                ],
                context,
            );
            return {
                type: "field",
                field: field(Reflect.get(record, "field"), `${context}.field`, false),
                encoding: choice(
                    Reflect.get(record, "encoding"),
                    [
                        "string",
                        "integer",
                    ],
                    `${context}.encoding`,
                ),
            };
        case "map_key":
            shape(
                value,
                [
                    "type",
                ],
                context,
            );
            if (!isMap) invalid(context);
            return {
                type: "map_key",
            };
        default:
            return invalid(context);
    }
}

/** Scope accepts unknown in addition to the shared text vocabulary. */
function scopeSource(value: unknown, context: string, isMap: boolean): QuotaScopeSource {
    const record = expectRecord(value, context);
    if (Reflect.get(record, "type") === "unknown") {
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
    }
    return textSource(value, context, isMap);
}

/** The numeric encoding is explicit rather than guessed from a foreign value. */
function numberField(value: unknown, context: string): QuotaNumberField {
    const record = shape(
        value,
        [
            "field",
            "encoding",
        ],
        context,
    );
    return {
        field: field(Reflect.get(record, "field"), `${context}.field`),
        encoding: choice(
            Reflect.get(record, "encoding"),
            [
                "number",
                "numeric_string",
                "number_or_numeric_string",
            ],
            `${context}.encoding`,
        ),
    };
}

/** The authored period mapping. */
function windowSource(value: unknown, context: string): QuotaWindowSource {
    const record = expectRecord(value, context);
    if (Reflect.get(record, "type") === "duration_field") {
        shape(
            value,
            [
                "type",
                "value",
                "unit",
            ],
            context,
        );
        return {
            type: "duration_field",
            value: numberField(Reflect.get(record, "value"), `${context}.value`),
            unit: choice(
                Reflect.get(record, "unit"),
                [
                    "seconds",
                    "minutes",
                    "hours",
                    "days",
                    "weeks",
                ],
                `${context}.unit`,
            ),
        };
    }
    return window(value, context);
}

/** The authored measurement mapping, whose optional facts stay unknown together. */
function measurementSource(value: unknown, context: string): QuotaMeasurementSource {
    const record = expectRecord(value, context);
    const type = Reflect.get(record, "type");
    if (type === "unknown") {
        shape(
            value,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }
    if (type === "ratio") {
        shape(
            value,
            [
                "type",
                "value",
                "scale",
                "direction",
            ],
            context,
        );
        return {
            type,
            value: numberField(Reflect.get(record, "value"), `${context}.value`),
            scale: choice(
                Reflect.get(record, "scale"),
                [
                    "fraction",
                    "percent",
                ],
                `${context}.scale`,
            ),
            direction: choice(
                Reflect.get(record, "direction"),
                [
                    "used",
                    "remaining",
                ],
                `${context}.direction`,
            ),
        };
    }
    if (type === "counter") {
        shape(
            value,
            [
                "type",
                "value",
                "limit",
                "direction",
                "unit",
            ],
            context,
        );
        return {
            type,
            value: numberField(Reflect.get(record, "value"), `${context}.value`),
            limit: numberField(Reflect.get(record, "limit"), `${context}.limit`),
            direction: choice(
                Reflect.get(record, "direction"),
                [
                    "used",
                    "remaining",
                ],
                `${context}.direction`,
            ),
            unit: text(Reflect.get(record, "unit"), `${context}.unit`),
        };
    }
    if (type === "remaining") {
        shape(
            value,
            [
                "type",
                "value",
                "unit",
            ],
            context,
        );
        return {
            type,
            value: numberField(Reflect.get(record, "value"), `${context}.value`),
            unit: text(Reflect.get(record, "unit"), `${context}.unit`),
        };
    }
    return invalid(context);
}

/** The authored reset encoding. */
function resetSource(value: unknown, context: string): QuotaResetSource {
    const record = expectRecord(value, context);
    if (Reflect.get(record, "type") === "unknown") {
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
    }
    if (Reflect.get(record, "type") !== "field") invalid(context);
    shape(
        value,
        [
            "type",
            "field",
            "encoding",
        ],
        context,
    );
    return {
        type: "field",
        field: field(Reflect.get(record, "field"), `${context}.field`),
        encoding: choice(
            Reflect.get(record, "encoding"),
            [
                "unix_seconds",
                "unix_milliseconds",
                "rfc3339",
                "date",
            ],
            `${context}.encoding`,
        ),
    };
}

/** An entry's complete mapping, including explicit unlimited precedence. */
function extraction(value: unknown, context: string): QuotaEntryExtraction {
    const record = shape(
        value,
        [
            "key",
            "label",
            "at",
            "is_required",
            "window",
            "measurement",
            "reset",
            "is_unlimited",
        ],
        context,
    );
    const entry: QuotaEntryExtraction = {
        key: text(Reflect.get(record, "key"), `${context}.key`),
        label: text(Reflect.get(record, "label"), `${context}.label`),
        at: path(Reflect.get(record, "at"), `${context}.at`),
        is_required: expectBoolean(Reflect.get(record, "is_required"), `${context}.is_required`),
        window: windowSource(Reflect.get(record, "window"), `${context}.window`),
        measurement: measurementSource(
            Reflect.get(record, "measurement"),
            `${context}.measurement`,
        ),
        reset: resetSource(Reflect.get(record, "reset"), `${context}.reset`),
    };
    if (
        Reflect.get(record, "is_unlimited") !== undefined &&
        Reflect.get(record, "is_unlimited") !== null
    )
        entry.is_unlimited = field(Reflect.get(record, "is_unlimited"), `${context}.is_unlimited`);
    return entry;
}

/** Rejects empty lists and repeated configured identities. */
function unique<
    T extends {
        key: string;
    },
>(values: T[], context: string): T[] {
    if (values.length === 0 || new Set(values.map((value) => value.key)).size !== values.length)
        invalid(context);
    return values;
}

/** Rebuilds the closed quota vocabulary through the existing control-request parser. */
export function parseQuotaDescriptor(
    value: unknown,
    context: string,
    parseRequest: (value: unknown, context: string) => ControlRequest,
): QuotaDescriptor {
    const record = shape(
        value,
        [
            "request",
            "sources",
        ],
        context,
    );
    const sources = expectArray(Reflect.get(record, "sources"), `${context}.sources`).map(
        (value, index): QuotaSource => {
            const contextPath = `${context}.sources[${index}]`;
            const record = expectRecord(value, contextPath);
            const type = choice(
                Reflect.get(record, "type"),
                [
                    "object",
                    "array",
                    "map",
                ],
                `${contextPath}.type`,
            );
            shape(
                value,
                type === "array"
                    ? [
                          "type",
                          "key",
                          "at",
                          "is_required",
                          "identity",
                          "scope",
                          "entries",
                      ]
                    : [
                          "type",
                          "key",
                          "at",
                          "is_required",
                          "scope",
                          "entries",
                      ],
                contextPath,
            );
            const common = {
                key: text(Reflect.get(record, "key"), `${contextPath}.key`),
                at: path(Reflect.get(record, "at"), `${contextPath}.at`),
                is_required: expectBoolean(
                    Reflect.get(record, "is_required"),
                    `${contextPath}.is_required`,
                ),
                scope: scopeSource(
                    Reflect.get(record, "scope"),
                    `${contextPath}.scope`,
                    type === "map",
                ),
                entries: unique(
                    expectArray(Reflect.get(record, "entries"), `${contextPath}.entries`).map(
                        (value, index) => extraction(value, `${contextPath}.entries[${index}]`),
                    ),
                    `${contextPath}.entries`,
                ),
            };
            if (type !== "array")
                return {
                    type,
                    ...common,
                };
            const identity = expectArray(
                Reflect.get(record, "identity"),
                `${contextPath}.identity`,
            ).map((value, index) => textSource(value, `${contextPath}.identity[${index}]`, false));
            if (
                identity.length === 0 ||
                !identity.some((part) => part.type === "field" && part.field.base === "item") ||
                identity.some((part) => part.type === "field" && !part.field.is_required)
            )
                invalid(`${contextPath}.identity`);
            return {
                type,
                identity,
                ...common,
            };
        },
    );
    return {
        request: parseRequest(Reflect.get(record, "request"), `${context}.request`),
        sources: unique(sources, `${context}.sources`),
    };
}

/** Checks quota-only stage expressions and the existing OAuth slot namespace once at parsing. */
export function validateQuotaRequest(
    descriptor: QuotaDescriptor,
    credential: CredentialAcquisition,
    provisioning: ProvisioningStep[] | null | undefined,
    context: string,
): void {
    const slots = new Set<string>();
    if (credential.type === "oauth") {
        for (const initializer of credential.initial_slots ?? []) slots.add(initializer.name);
        for (const claim of credential.id_token_claims ?? []) slots.add(claim.slot);
        for (const field of credential.exchange.response.fields ?? [])
            if (field.target.type === "slot") slots.add(field.target.name);
        for (const step of provisioning ?? []) {
            for (const field of [
                ...(step.extract ?? []),
                ...(step.poll?.result_extract ?? []),
            ])
                if (field.target.type === "slot") slots.add(field.target.name);
        }
    }
    const expression = (value: StringExpr): void => {
        for (const part of value) {
            if (part.type === "coalesce") {
                if (part.parts.some((value) => value.type === "coalesce")) invalid(context);
                expression(part.parts);
            } else if (part.type === "slot") {
                if (!slots.has(part.name)) invalid(context);
            } else if (
                ![
                    "literal",
                    "secret",
                    "base",
                ].includes(part.type)
            )
                invalid(context);
        }
    };
    const node = (value: TemplateNode): void => {
        if (value.type === "body") invalid(context);
        if (value.type === "string") expression(value.expr);
        if (value.type === "array") value.items.forEach(node);
        if (value.type === "object") Object.values(value.fields).forEach(node);
    };
    const request = descriptor.request;
    expression(request.url);
    for (const row of [
        ...(request.headers ?? []),
        ...(request.query ?? []),
    ])
        expression(row.value);
    if (request.body?.kind === "form") for (const row of request.body.fields) expression(row.value);
    if (request.body?.kind === "json") node(request.body.node);
    if (request.method === "GET" && request.body != null) invalid(context);
}
