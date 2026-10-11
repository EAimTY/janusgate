import { describe, expect, test } from "vitest";
import { ContractViolationError } from "./errors";
import {
    expectArray,
    expectBoolean,
    expectDeclaredKeys,
    expectDeclaredRecord,
    expectInteger,
    expectNonEmptyString,
    expectNullableInteger,
    expectNullableSafeInteger,
    expectNullableString,
    expectOneOf,
    expectPositiveInteger,
    expectRecord,
    expectRowId,
    expectSafeInteger,
    expectString,
    isPresentValue,
    parseEnvelopeResponse,
    parseListResponse,
} from "./validation-primitives";

/**
 * The scalar narrowing and list/envelope rebuilding shared by every response
 * parser: each failure names the offending JSON path, safe integers bound
 * the wire's exactness guarantee, the declared-keys walk names an unknown
 * field instead of dropping it, and the two list/envelope builders own the
 * `response.<member>[i]` context grammar.
 */

describe("scalar narrowing", () => {
    test("rejects a non-record, arrays included", () => {
        expect(() => expectRecord([], "response")).toThrowError("response must be an object");
        expect(() => expectRecord("x", "response")).toThrowError("response must be an object");
        expect(
            expectRecord(
                {
                    a: 1,
                },
                "response",
            ),
        ).toEqual({
            a: 1,
        });
    });

    test("rejects a non-array", () => {
        expect(() => expectArray({}, "response.items")).toThrowError(
            "response.items must be an array",
        );
        expect(
            expectArray(
                [
                    1,
                ],
                "x",
            ),
        ).toEqual([
            1,
        ]);
    });

    test("rejects an empty string where the contract stores identity", () => {
        expect(() => expectNonEmptyString("", "row.name")).toThrowError(
            "row.name must not be empty",
        );
        expect(() => expectString(5, "row.name")).toThrowError("row.name must be a string");
    });

    test("narrows a closed vocabulary with its noun", () => {
        expect(
            expectOneOf(
                "b",
                [
                    "a",
                    "b",
                ],
                "row.type",
                "a kind",
            ),
        ).toBe("b");
        expect(() =>
            expectOneOf(
                "c",
                [
                    "a",
                    "b",
                ],
                "row.type",
                "a kind",
            ),
        ).toThrowError("row.type is not a kind: c");
    });

    test("binds booleans and integers", () => {
        expect(() => expectBoolean(1, "row.is_enabled")).toThrowError(
            "row.is_enabled must be a boolean",
        );
        expect(() => expectInteger(1.5, "row.duration")).toThrowError(
            "row.duration must be an integer",
        );
    });

    test("binds safe integers with the optional minimum", () => {
        expect(expectSafeInteger(2 ** 53 - 1, "row.stamp")).toBe(Number.MAX_SAFE_INTEGER);
        expect(() => expectSafeInteger(2 ** 53, "row.stamp")).toThrowError(
            "row.stamp must be a safe integer",
        );
        expect(expectSafeInteger(0, "row.stamp")).toBe(0);
        expect(expectRowId(1, "row.id")).toBe(1);
    });

    test("binds positive integers as the wire mirror of a NonZero count", () => {
        expect(expectPositiveInteger(1, "budget")).toBe(1);
        expect(() => expectPositiveInteger(0, "budget")).toThrowError(
            "budget must be a positive integer",
        );
        expect(() => expectPositiveInteger(-2, "budget")).toThrowError(
            "budget must be a positive integer",
        );
    });

    test("binds nullable scalars with null passing through untouched", () => {
        expect(expectNullableString(null, "row.base_url")).toBeNull();
        expect(expectNullableString("x", "row.base_url")).toBe("x");
        expect(() => expectNullableString(5, "row.base_url")).toThrowError(
            "row.base_url must be a string or null",
        );

        expect(expectNullableInteger(null, "row.tokens")).toBeNull();
        expect(() => expectNullableInteger(1.5, "row.tokens")).toThrowError(
            "row.tokens must be an integer or null",
        );

        expect(expectNullableSafeInteger(null, "row.expires_at")).toBeNull();
        expect(() => expectNullableSafeInteger(2 ** 53, "row.expires_at")).toThrowError(
            "row.expires_at must be a safe integer or null",
        );
    });

    test("states whether an optional-and-nullable value carries content", () => {
        expect(isPresentValue(undefined)).toBe(false);
        expect(isPresentValue(null)).toBe(false);
        expect(isPresentValue(0)).toBe(true);
        expect(isPresentValue("")).toBe(true);
    });
});

describe("declared-keys strictness", () => {
    test("names an unknown field at its path instead of dropping it", () => {
        expect(() =>
            expectDeclaredKeys(
                {
                    type: "unknown",
                    extra: 1,
                },
                [
                    "type",
                ],
                "quota.scope",
            ),
        ).toThrowError("quota.scope.extra is not a declared field");
        expect(
            expectDeclaredKeys(
                {
                    type: "unknown",
                },
                [
                    "type",
                ],
                "quota.scope",
            ),
        ).toBeUndefined();
    });

    test("composes the record check with the declared-keys walk", () => {
        expect(
            expectDeclaredRecord(
                {
                    type: "unknown",
                },
                [
                    "type",
                ],
                "scope",
            ),
        ).toEqual({
            type: "unknown",
        });
        expect(() =>
            expectDeclaredRecord(
                [],
                [
                    "type",
                ],
                "scope",
            ),
        ).toThrowError("scope must be an object");
    });
});

describe("list and envelope rebuilding", () => {
    test("owns the response.<member>[i] context grammar", () => {
        const listing = parseListResponse(
            {
                rows: [
                    {
                        id: 1,
                    },
                    {
                        id: 2,
                    },
                ],
            },
            "rows",
            (value, context) => {
                expect(context).toMatch(/^response\.rows\[\d\]$/);
                const record = expectRecord(value, context);
                return {
                    id: expectRowId(Reflect.get(record, "id"), `${context}.id`),
                };
            },
        );

        expect(listing.rows).toEqual([
            {
                id: 1,
            },
            {
                id: 2,
            },
        ]);
        expect(() =>
            parseListResponse(
                {
                    rows: 4,
                },
                "rows",
                (value) => value,
            ),
        ).toThrowError("response.rows must be an array");
        expect(() =>
            parseListResponse(
                {
                    rows: [
                        {
                            id: 0,
                        },
                    ],
                },
                "rows",
                (value, context) => {
                    const record = expectRecord(value, context);
                    return expectRowId(Reflect.get(record, "id"), `${context}.id`);
                },
            ),
        ).toThrowError("response.rows[0].id must be a safe integer of at least 1");
    });

    test("owns the response.<member> context grammar for envelopes", () => {
        const envelope = parseEnvelopeResponse(
            {
                row: {
                    id: 3,
                },
            },
            "row",
            (value, context) => {
                expect(context).toBe("response.row");
                const record = expectRecord(value, context);
                return {
                    id: expectRowId(Reflect.get(record, "id"), `${context}.id`),
                };
            },
        );

        expect(envelope.row).toEqual({
            id: 3,
        });
        expect(() => parseEnvelopeResponse(null, "row", (value) => value)).toThrowError(
            "response must be an object",
        );
    });

    test("lets a ContractViolationError carry its subclass identity through", () => {
        expect(() => expectString(Symbol("x"), "x")).toThrowError(ContractViolationError);
    });
});
