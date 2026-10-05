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
    QuotaScopeSource,
    QuotaSource,
    QuotaTextSource,
    QuotaWindowSource,
    StringExpr,
    TemplateNode,
} from "../generated/v0";
import { window } from "../quota";
import { choice, invalid, modelIdentifier, parseQuotaPath, shape, text } from "../quota-validation";
import { expectArray, expectBoolean, expectRecord } from "../validation-primitives";
import { expectControlRequest } from "./control";

/** Authored omission defaults to monitoring; unresolved optional model fields are forbidden. */
function routingSource(value: unknown, context: string, isMap: boolean): QuotaRoutingSource {
    if (value === undefined)
        return {
            type: "monitoring_only",
        };
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
    ).map((value, index) => {
        const sourceContext = `${context}.upstream_ids[${index}]`;
        const source = textSource(value, sourceContext, isMap, true);
        if (source.type === "field" && !source.field.is_required) invalid(sourceContext);
        if (source.type === "literal") modelIdentifier(source.value, sourceContext);
        return source;
    });
    if (upstream_ids.length === 0) invalid(context);
    return {
        type,
        upstream_ids,
    };
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
        path: parseQuotaPath(Reflect.get(record, "path"), `${context}.path`),
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
                field: field(Reflect.get(record, "field"), `${context}.field`, canUseEntry),
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
function extraction(value: unknown, context: string, isMap: boolean): QuotaEntryExtraction {
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
            "routing",
        ],
        context,
    );
    const entry: QuotaEntryExtraction = {
        key: text(Reflect.get(record, "key"), `${context}.key`),
        label: text(Reflect.get(record, "label"), `${context}.label`),
        at: parseQuotaPath(Reflect.get(record, "at"), `${context}.at`),
        is_required: expectBoolean(Reflect.get(record, "is_required"), `${context}.is_required`),
        window: windowSource(Reflect.get(record, "window"), `${context}.window`),
        measurement: measurementSource(
            Reflect.get(record, "measurement"),
            `${context}.measurement`,
        ),
        reset: resetSource(Reflect.get(record, "reset"), `${context}.reset`),
        routing: routingSource(Reflect.get(record, "routing"), `${context}.routing`, isMap),
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
export function parseQuotaDescriptor(value: unknown, context: string): QuotaDescriptor {
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
                at: parseQuotaPath(Reflect.get(record, "at"), `${contextPath}.at`),
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
