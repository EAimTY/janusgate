import { ContractViolationError } from "../errors";
import type { JsonValue, ParamRow, StringExpr, TemplateNode, ValueExpr } from "../generated/v0";
import {
    expectArray,
    expectDeclaredKeys,
    expectRecord,
    expectString,
} from "../validation-primitives";

/** One value expression part, rebuilt from its validated kind (README §5.1). */
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
        case "coalesce":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "parts",
                ],
                context,
            );
            return {
                type: "coalesce",
                parts: expectStringExpr(Reflect.get(record, "parts"), `${context}.parts`),
            };
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

/** One param row: a name and its expression, rebuilt (README §5.1). */
export function expectParamRows(value: unknown, context: string): Array<ParamRow> {
    return expectArray(value, context).map((row, index) => {
        const record = expectRecord(row, `${context}[${index}]`);
        expectDeclaredKeys(
            record,
            [
                "name",
                "value",
            ],
            `${context}[${index}]`,
        );

        return {
            name: expectString(Reflect.get(record, "name"), `${context}[${index}].name`),
            value: expectStringExpr(Reflect.get(record, "value"), `${context}[${index}].value`),
        };
    });
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

/** One template tree node, rebuilt from its node kind (README §5.1). */
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
