import { ContractViolationError } from "../errors";
import type {
    CredentialAcquisition,
    ProvisioningStep,
    QuotaDescriptor,
    QuotaEntryExtraction,
    QuotaField,
    QuotaMeasurementSource,
    QuotaNumberField,
    QuotaResetSource,
    QuotaRoutingSource,
    QuotaSource,
    QuotaTextSource,
    QuotaWindowSource,
    StringExpr,
    TemplateNode,
} from "../generated/v0";
import { expectModelIdentifier } from "../identifiers";
import { expectQuotaPath, parseQuotaWindow } from "../quota";
import {
    expectArray,
    expectBoolean,
    expectDeclaredKeys,
    expectDeclaredRecord,
    expectNonEmptyString,
    expectOneOf,
    expectRecord,
    expectString,
    isPresentValue,
} from "../validation-primitives";
import { expectControlRequest } from "./control";

/** Authored omission defaults to monitoring; unresolved optional model fields are forbidden. */
function routingSource(value: unknown, context: string, isMap: boolean): QuotaRoutingSource {
    if (value === undefined)
        return {
            type: "monitoring_only",
        };
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
    ).map((value, index) => {
        const sourceContext = `${context}.upstream_ids[${index}]`;
        const source = textSource(value, sourceContext, isMap, true);
        if (source.type === "field" && !source.field.is_required) {
            throw new ContractViolationError(`${sourceContext} must be a required field`);
        }
        if (source.type === "literal") expectModelIdentifier(source.value, sourceContext);
        return source;
    });
    if (upstream_ids.length === 0) {
        throw new ContractViolationError(`${context}.upstream_ids must not be empty`);
    }
    return {
        type,
        upstream_ids,
    };
}

/** A scalar selection with a stage-available context. */
function field(value: unknown, context: string, canUseEntry = true): QuotaField {
    const record = expectDeclaredRecord(
        value,
        [
            "base",
            "path",
            "is_required",
        ],
        context,
    );
    const base = expectOneOf(
        Reflect.get(record, "base"),
        [
            "root",
            "item",
            "entry",
        ],
        `${context}.base`,
        "a field base",
    );
    if (!canUseEntry && base === "entry") {
        throw new ContractViolationError(`${context}.base must not be \`entry\` here`);
    }
    return {
        base,
        path: expectQuotaPath(Reflect.get(record, "path"), `${context}.path`),
        is_required: expectBoolean(Reflect.get(record, "is_required"), `${context}.is_required`),
    };
}

/** A text mapping; map keys exist only in map collections. */
function textSource(
    value: unknown,
    context: string,
    isMap: boolean,
    canUseEntry = false,
): QuotaTextSource {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    switch (type) {
        case "literal":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type,
                value: expectNonEmptyString(Reflect.get(record, "value"), `${context}.value`),
            };
        case "field":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "field",
                    "encoding",
                ],
                context,
            );
            return {
                type,
                field: field(Reflect.get(record, "field"), `${context}.field`, canUseEntry),
                encoding: expectOneOf(
                    Reflect.get(record, "encoding"),
                    [
                        "string",
                        "integer",
                    ],
                    `${context}.encoding`,
                    "a text encoding",
                ),
            };
        case "map_key":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            if (!isMap) {
                throw new ContractViolationError(
                    `${context}.type must not be \`map_key\` outside a map`,
                );
            }
            return {
                type,
            };
        default:
            throw new ContractViolationError(`${context}.type is not a text source: ${type}`);
    }
}

/**
 * The optional named scope shares the text-source vocabulary; an omitted
 * scope is the unknown scope, exactly as the gateway's serde default parses
 * it.
 */
function optionalScope(
    value: unknown,
    context: string,
    isMap: boolean,
): QuotaTextSource | undefined {
    if (!isPresentValue(value)) {
        return undefined;
    }

    return textSource(value, context, isMap);
}

/** The numeric encoding is explicit rather than guessed from a foreign value. */
function numberField(value: unknown, context: string): QuotaNumberField {
    const record = expectDeclaredRecord(
        value,
        [
            "field",
            "encoding",
        ],
        context,
    );
    return {
        field: field(Reflect.get(record, "field"), `${context}.field`),
        encoding: expectOneOf(
            Reflect.get(record, "encoding"),
            [
                "number",
                "numeric_string",
                "number_or_numeric_string",
            ],
            `${context}.encoding`,
            "a number encoding",
        ),
    };
}

/** The authored period mapping. */
function windowSource(value: unknown, context: string): QuotaWindowSource {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    if (type === "duration_field") {
        expectDeclaredKeys(
            record,
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
            unit: expectOneOf(
                Reflect.get(record, "unit"),
                [
                    "seconds",
                    "minutes",
                    "hours",
                    "days",
                    "weeks",
                ],
                `${context}.unit`,
                "a duration unit",
            ),
        };
    }
    return parseQuotaWindow(value, context);
}

/** The authored measurement mapping, whose optional facts stay unknown together. */
function measurementSource(value: unknown, context: string): QuotaMeasurementSource {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    if (type === "unknown") {
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
    }
    if (type === "ratio") {
        expectDeclaredKeys(
            record,
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
            scale: expectOneOf(
                Reflect.get(record, "scale"),
                [
                    "fraction",
                    "percent",
                ],
                `${context}.scale`,
                "a ratio scale",
            ),
            direction: expectOneOf(
                Reflect.get(record, "direction"),
                [
                    "used",
                    "remaining",
                ],
                `${context}.direction`,
                "a direction",
            ),
        };
    }
    if (type === "counter") {
        expectDeclaredKeys(
            record,
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
            direction: expectOneOf(
                Reflect.get(record, "direction"),
                [
                    "used",
                    "remaining",
                ],
                `${context}.direction`,
                "a direction",
            ),
            unit: expectNonEmptyString(Reflect.get(record, "unit"), `${context}.unit`),
        };
    }
    if (type === "remaining") {
        expectDeclaredKeys(
            record,
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
            unit: expectNonEmptyString(Reflect.get(record, "unit"), `${context}.unit`),
        };
    }
    throw new ContractViolationError(`${context}.type is not a measurement source: ${type}`);
}

/** The authored reset encoding. */
function resetSource(value: unknown, context: string): QuotaResetSource {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    if (type === "unknown") {
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
    }
    if (type !== "field") {
        throw new ContractViolationError(`${context}.type is not a reset source: ${type}`);
    }
    expectDeclaredKeys(
        record,
        [
            "type",
            "field",
            "encoding",
        ],
        context,
    );
    return {
        type,
        field: field(Reflect.get(record, "field"), `${context}.field`),
        encoding: expectOneOf(
            Reflect.get(record, "encoding"),
            [
                "unix_seconds",
                "unix_milliseconds",
                "rfc3339",
                "date",
            ],
            `${context}.encoding`,
            "a reset encoding",
        ),
    };
}

/** An entry's complete mapping, including explicit unlimited precedence. */
function extraction(value: unknown, context: string, isMap: boolean): QuotaEntryExtraction {
    const record = expectDeclaredRecord(
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
            "routing",
        ],
        context,
    );
    const entry: QuotaEntryExtraction = {
        key: expectNonEmptyString(Reflect.get(record, "key"), `${context}.key`),
        label: expectNonEmptyString(Reflect.get(record, "label"), `${context}.label`),
        at: expectQuotaPath(Reflect.get(record, "at"), `${context}.at`),
        is_required: expectBoolean(Reflect.get(record, "is_required"), `${context}.is_required`),
        window: windowSource(Reflect.get(record, "window"), `${context}.window`),
        measurement: measurementSource(
            Reflect.get(record, "measurement"),
            `${context}.measurement`,
        ),
        reset: resetSource(Reflect.get(record, "reset"), `${context}.reset`),
        routing: routingSource(Reflect.get(record, "routing"), `${context}.routing`, isMap),
    };
    const isUnlimited = Reflect.get(record, "is_unlimited");
    if (isPresentValue(isUnlimited)) {
        entry.is_unlimited = field(isUnlimited, `${context}.is_unlimited`);
    }
    return entry;
}

/** Rejects empty lists and repeated configured identities. */
function unique<
    T extends {
        key: string;
    },
>(values: T[], context: string): T[] {
    if (values.length === 0 || new Set(values.map((value) => value.key)).size !== values.length) {
        throw new ContractViolationError(`${context} must be a nonempty list of unique keys`);
    }
    return values;
}

/** Rebuilds the closed quota vocabulary through the existing control-request parser. */
export function parseQuotaDescriptor(value: unknown, context: string): QuotaDescriptor {
    const record = expectDeclaredRecord(
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
            const type = expectOneOf(
                Reflect.get(record, "type"),
                [
                    "object",
                    "array",
                    "map",
                ],
                `${contextPath}.type`,
                "a source kind",
            );
            expectDeclaredKeys(
                record,
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
            const scope = optionalScope(
                Reflect.get(record, "scope"),
                `${contextPath}.scope`,
                type === "map",
            );
            const common: Pick<
                Extract<
                    QuotaSource,
                    {
                        type: "object";
                    }
                >,
                "key" | "at" | "is_required" | "scope" | "entries"
            > = {
                key: expectNonEmptyString(Reflect.get(record, "key"), `${contextPath}.key`),
                at: expectQuotaPath(Reflect.get(record, "at"), `${contextPath}.at`),
                is_required: expectBoolean(
                    Reflect.get(record, "is_required"),
                    `${contextPath}.is_required`,
                ),
                ...(scope === undefined
                    ? {}
                    : {
                          scope,
                      }),
                entries: unique(
                    expectArray(Reflect.get(record, "entries"), `${contextPath}.entries`).map(
                        (value, index) =>
                            extraction(value, `${contextPath}.entries[${index}]`, type === "map"),
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
            ) {
                throw new ContractViolationError(
                    `${contextPath}.identity must list required fields, at least one item-based`,
                );
            }
            return {
                type,
                identity,
                ...common,
            };
        },
    );
    return {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
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
                if (part.parts.some((value) => value.type === "coalesce")) {
                    throw new ContractViolationError(
                        `${context} must not nest a coalesce expression`,
                    );
                }
                expression(part.parts);
            } else if (part.type === "slot") {
                if (!slots.has(part.name)) {
                    throw new ContractViolationError(
                        `${context} must not reference an undeclared slot: ${part.name}`,
                    );
                }
            } else if (
                ![
                    "literal",
                    "secret",
                    "base",
                ].includes(part.type)
            ) {
                // The quota request is its own stage: the OAuth-flow and
                // inference-stage values an expression may otherwise name are
                // not available to it.
                throw new ContractViolationError(`${context} must not reference \`${part.type}\``);
            }
        }
    };
    const node = (value: TemplateNode): void => {
        if (value.type === "body") {
            throw new ContractViolationError(`${context} must not reference the inference body`);
        }
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
    if (request.method === "GET" && request.body != null) {
        throw new ContractViolationError(`${context} must not carry a body on a GET request`);
    }
}
