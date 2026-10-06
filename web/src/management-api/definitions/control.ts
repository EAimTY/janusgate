import { ContractViolationError } from "../errors";
import type {
    ControlBody,
    ControlRequest,
    ExtractionShape,
    ResponseDescriptor,
    ResponseEncoding,
    ResponsePath,
    ResponseTarget,
} from "../generated/v0";
import {
    expectArray,
    expectBoolean,
    expectDeclaredKeys,
    expectOneOf,
    expectRecord,
    expectString,
    isPresentValue,
} from "../validation-primitives";
import { expectParamRows, expectStringExpr, expectTemplateNode } from "./expressions";

/** One control body: a form-encoded field set or a typed JSON tree (README §5.1). */
function expectControlBody(value: unknown, context: string): ControlBody {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "kind"), `${context}.kind`);

    if (kind === "form") {
        expectDeclaredKeys(
            record,
            [
                "kind",
                "fields",
            ],
            context,
        );

        return {
            kind: "form",
            fields: expectParamRows(Reflect.get(record, "fields"), `${context}.fields`),
        };
    }

    if (kind === "json") {
        expectDeclaredKeys(
            record,
            [
                "kind",
                "node",
            ],
            context,
        );

        return {
            kind: "json",
            node: expectTemplateNode(Reflect.get(record, "node"), `${context}.node`),
        };
    }

    throw new ContractViolationError(`${context}.kind is not a control body: ${kind}`);
}

/** One control request: method, URL, headers, query, and body (README §5.1). */
export function expectControlRequest(value: unknown, context: string): ControlRequest {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "method",
            "url",
            "headers",
            "query",
            "body",
        ],
        context,
    );

    const method = expectOneOf(
        expectString(Reflect.get(record, "method"), `${context}.method`),
        [
            "GET",
            "POST",
        ],
        `${context}.method`,
        "a control method",
    );

    const request: ControlRequest = {
        method,
        url: expectStringExpr(Reflect.get(record, "url"), `${context}.url`),
    };

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        request.headers = expectParamRows(headers, `${context}.headers`);
    }

    const query = Reflect.get(record, "query");

    if (query !== undefined) {
        request.query = expectParamRows(query, `${context}.query`);
    }

    const body = Reflect.get(record, "body");

    if (isPresentValue(body)) {
        request.body = expectControlBody(body, `${context}.body`);
    }

    return request;
}

/** One response path: the ordered member chain (README §5.1). */
export function expectResponsePath(value: unknown, context: string): ResponsePath {
    return expectArray(value, context).map((member, index) =>
        expectString(member, `${context}[${index}]`),
    );
}

/** One merged working-state target, rebuilt from its tag (README §5.3, §5.4). */
export function expectResponseTarget(value: unknown, context: string): ResponseTarget {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "slot") {
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
    }

    switch (type) {
        case "access_token":
        case "refresh_token":
        case "expires_in_seconds":
        case "id_token":
        case "base_url":
        case "device_code":
        case "user_code":
        case "verification_uri":
        case "verification_uri_complete":
        case "interval_seconds":
        case "expires_at_unix_seconds":
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
        default:
            throw new ContractViolationError(`${context}.type is not a response target: ${type}`);
    }
}

/** One response mapping field: the path, its target, and the required flag. */
function expectResponseField(
    value: unknown,
    context: string,
): {
    path: ResponsePath;
    target: ResponseTarget;
    required: boolean;
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "path",
            "target",
            "required",
        ],
        context,
    );

    return {
        path: expectResponsePath(Reflect.get(record, "path"), `${context}.path`),
        target: expectResponseTarget(Reflect.get(record, "target"), `${context}.target`),
        required: expectBoolean(Reflect.get(record, "required"), `${context}.required`),
    };
}

/** One response descriptor: the encoding and its field mappings (README §5.1). */
export function expectResponseDescriptor(value: unknown, context: string): ResponseDescriptor {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "encoding",
            "fields",
        ],
        context,
    );

    return {
        encoding: expectResponseEncoding(Reflect.get(record, "encoding"), `${context}.encoding`),
        fields: expectArray(Reflect.get(record, "fields"), `${context}.fields`).map(
            (field, index) => expectResponseField(field, `${context}.fields[${index}]`),
        ),
    };
}

/** The declared shape of one extracted value (README §5.4). */
export function expectExtractionShape(value: unknown, context: string): ExtractionShape {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "string" || type === "integer") {
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

    if (type === "string_or_field") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "field",
            ],
            context,
        );

        return {
            type: "string_or_field",
            field: expectString(Reflect.get(record, "field"), `${context}.field`),
        };
    }

    throw new ContractViolationError(`${context}.type is not an extraction shape: ${type}`);
}

/** The wire encoding a control response arrives in (README §5.1). */
export function expectResponseEncoding(value: unknown, context: string): ResponseEncoding {
    return expectOneOf(
        value,
        [
            "json",
            "form",
        ],
        context,
        "a response encoding",
    );
}
