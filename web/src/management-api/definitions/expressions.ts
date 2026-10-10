import { ContractViolationError } from "../errors";
import type { JsonValue, ParamRow, StringExpr, TemplateNode, ValueExpr } from "../generated/v0";
import {
    expectArray,
    expectDeclaredKeys,
    expectNonEmptyString,
    expectRecord,
    expectString,
} from "../validation-primitives";

/** One value expression part, rebuilt from its validated kind (`docs/design/providers.md`). */
function expectExpressionPart(part: unknown, context: string): ValueExpr {
    const record = expectRecord(part, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
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
                type: "literal",
                value: expectString(Reflect.get(record, "value"), `${context}.value`),
            };
        case "secret":
        case "client_id":
        case "client_secret":
        case "authorization_code":
        case "code_verifier":
        case "redirect_uri":
        case "oauth_state":
        case "device_code":
        case "refresh_token":
        case "request_uuid":
        case "model":
        case "base":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: kind,
            };
        case "slot":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "name",
                ],
                context,
            );
            return {
                type: "slot",
                name: expectString(Reflect.get(record, "name"), `${context}.name`),
            };
        case "coalesce": {
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "parts",
                ],
                context,
            );
            const parts = expectStringExpr(Reflect.get(record, "parts"), `${context}.parts`);

            // The gateway rejects an argument-less coalesce, which could never
            // resolve, and a coalesce nested in a coalesce's argument list,
            // whose flattening is always representable (`docs/design/providers.md`).
            if (parts.length === 0) {
                throw new ContractViolationError(`${context} carries an empty coalesce`);
            }

            const nested = parts.findIndex((candidate) => candidate.type === "coalesce");

            if (nested !== -1) {
                throw new ContractViolationError(
                    `${context}.parts[${nested}] nests a coalesce inside a coalesce; ` +
                        "coalesce arguments carry direct value parts only",
                );
            }

            return {
                type: "coalesce",
                parts,
            };
        }
        default:
            throw new ContractViolationError(`${context}.type is not an expression kind: ${kind}`);
    }
}

/** Validates one string expression: an ordered array of expression parts. */
export function expectStringExpr(value: unknown, context: string): StringExpr {
    return expectArray(value, context).map((part, index) =>
        expectExpressionPart(part, `${context}[${index}]`),
    );
}

/**
 * The byte set of a valid HTTP header name: the RFC token characters the
 * gateway's HTTP crate accepts for a header row's name (`docs/design/providers.md`).
 */
const HEADER_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/** One row's name: non-empty everywhere, and a header name only token bytes. */
function expectRowName(value: unknown, context: string, isHeader: boolean): string {
    const name = expectNonEmptyString(value, context);

    if (isHeader && !HEADER_NAME.test(name)) {
        throw new ContractViolationError(`${context} is not a valid HTTP header name: ${name}`);
    }

    return name;
}

/** One parsed row list: the name rule is the caller's row kind's contract. */
function expectRows(value: unknown, context: string, isHeader: boolean): Array<ParamRow> {
    return expectArray(value, context).map((row, index) => {
        const rowContext = `${context}[${index}]`;
        const record = expectRecord(row, rowContext);
        expectDeclaredKeys(
            record,
            [
                "name",
                "value",
            ],
            rowContext,
        );

        return {
            name: expectRowName(Reflect.get(record, "name"), `${rowContext}.name`, isHeader),
            value: expectStringExpr(Reflect.get(record, "value"), `${rowContext}.value`),
        };
    });
}

/**
 * One percent-encoded parameter row list — query parameters, form-encoded
 * fields, and extra authorize parameters — whose names must not be empty.
 */
export function expectParamRows(value: unknown, context: string): Array<ParamRow> {
    return expectRows(value, context, false);
}

/**
 * One literal HTTP header row list: the rows reach the wire verbatim, so each
 * name is held to the header-name byte contract on top of the row rule.
 */
export function expectHeaderRows(value: unknown, context: string): Array<ParamRow> {
    return expectRows(value, context, true);
}

/** One JSON value, walked so a non-JSON leaf (a function, a bare `undefined`) is named. */
function expectJsonValue(value: unknown, context: string): JsonValue {
    if (value === null) {
        return null;
    }

    switch (typeof value) {
        case "string":
            return value;
        case "number":
            if (Number.isFinite(value)) {
                return value;
            }

            throw new ContractViolationError(`${context} is not a JSON value`);
        case "boolean":
            return value;
        case "object":
            if (Array.isArray(value)) {
                return value.map((entry, index) => expectJsonValue(entry, `${context}[${index}]`));
            }

            return Object.fromEntries(
                Object.keys(value).map((key) => [
                    key,
                    expectJsonValue(Reflect.get(value, key), `${context}.${key}`),
                ]),
            );
        default:
            throw new ContractViolationError(`${context} is not a JSON value`);
    }
}

/** One template tree node, rebuilt from its node kind (`docs/design/providers.md`). */
export function expectTemplateNode(value: unknown, context: string): TemplateNode {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
        case "json":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type: "json",
                value: expectJsonValue(Reflect.get(record, "value"), `${context}.value`),
            };

        case "string":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "expr",
                ],
                context,
            );
            return {
                type: "string",
                expr: expectStringExpr(Reflect.get(record, "expr"), `${context}.expr`),
            };

        case "body":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "body",
            };

        case "object": {
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "fields",
                ],
                context,
            );
            const fields = expectRecord(Reflect.get(record, "fields"), `${context}.fields`);

            // An empty member name addresses nothing on the wire and is the
            // typo the gateway's write names instead of storing.
            for (const name of Object.keys(fields)) {
                if (name === "") {
                    throw new ContractViolationError(
                        `${context}.fields carries an empty object member name`,
                    );
                }
            }

            return {
                type: "object",
                fields: Object.fromEntries(
                    Object.keys(fields).map((name) => [
                        name,
                        expectTemplateNode(Reflect.get(fields, name), `${context}.fields.${name}`),
                    ]),
                ),
            };
        }

        case "array":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "items",
                ],
                context,
            );
            return {
                type: "array",
                items: expectArray(Reflect.get(record, "items"), `${context}.items`).map(
                    (item, index) => expectTemplateNode(item, `${context}.items[${index}]`),
                ),
            };

        default:
            throw new ContractViolationError(`${context}.type is not a template node: ${kind}`);
    }
}

/**
 * Counts the body placeholders of one parsed template tree — the walk behind
 * the exactly-one-envelope rule and the control-request body rule. The tree
 * is a rebuilt `TemplateNode`, so its member names and expression structure
 * already passed the parse above.
 */
export function countTemplateBodyNodes(node: TemplateNode): number {
    switch (node.type) {
        case "json":
        case "string":
            return 0;
        case "body":
            return 1;
        case "object":
            return Object.values(node.fields).reduce(
                (total, field) => total + countTemplateBodyNodes(field),
                0,
            );
        case "array":
            return node.items.reduce((total, item) => total + countTemplateBodyNodes(item), 0);
    }
}
