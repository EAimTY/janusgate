import { describe, expect, test } from "vitest";
import { ContractViolationError } from "../errors";
import { ErrorCode } from "../generated/v0";
import { parseApiKeyEnvelope, parseListApiKeysResponse } from "./api-keys";
import { parseEmptyResponse, parseErrorResponse } from "./common";

/**
 * The common error-body and empty-body readers, and the serving-key
 * response parsers: the closed errno set read from the bindings' own enum,
 * the envelope/list context grammar, and the access policy's owner-default
 * discrimination, which tolerates extra fields (the bodies are never
 * rebuilt) but rejects foreign decisions and owner kinds.
 */

describe("parseEmptyResponse", () => {
    test("answers the empty record for an empty object", () => {
        expect(parseEmptyResponse({})).toEqual({});
    });

    test("rejects a foreign body — an array is not the empty record", () => {
        expect(() => parseEmptyResponse([])).toThrowError("response must be an object");
    });
});

describe("parseErrorResponse", () => {
    test("reads an errno-shaped body into its pair", () => {
        expect(
            parseErrorResponse({
                errno: ErrorCode.Conflict,
                message: "taken",
            }),
        ).toEqual({
            errno: ErrorCode.Conflict,
            message: "taken",
        });
    });

    test("answers null for shapes outside the error domain", () => {
        expect(parseErrorResponse("nope")).toBeNull();
        expect(parseErrorResponse(null)).toBeNull();
        expect(
            parseErrorResponse([
                "errno",
            ]),
        ).toBeNull();
        expect(
            parseErrorResponse({
                errno: 9999,
                message: "foreign",
            }),
        ).toBeNull();
        expect(
            parseErrorResponse({
                errno: ErrorCode.Internal,
            }),
        ).toBeNull();
        expect(
            parseErrorResponse({
                errno: "4",
                message: "taken",
            }),
        ).toBeNull();
    });
});

/** One complete access policy fixture. */
const POLICY = {
    default_access: "deny",
    owner_defaults: [
        {
            type: "provider",
            provider_id: 3,
            access: "allow",
        },
        {
            type: "pool",
            pool_id: 8,
            access: "deny",
        },
    ],
    route_overrides: [
        {
            model_route_id: 12,
            access: "allow",
        },
    ],
};

/** One serving-key row fixture. */
const KEY_ROW = {
    id: 5,
    name: "batch",
    key: "jkc-key-1",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
    access_control: POLICY,
};

describe("serving-key response parsers", () => {
    test("parse the listing with each row's complete readable state", () => {
        const listing = parseListApiKeysResponse({
            api_keys: [
                KEY_ROW,
            ],
        });

        expect(listing.api_keys).toHaveLength(1);
        const row = listing.api_keys[0];
        if (row === undefined) throw new Error("the fixture listed no row");
        expect(row.id).toBe(5);
        expect(row.access_control.default_access).toBe("deny");
        expect(row.access_control.owner_defaults[0]).toEqual({
            type: "provider",
            provider_id: 3,
            access: "allow",
        });
    });

    test("parse the creation envelope", () => {
        const envelope = parseApiKeyEnvelope({
            api_key: KEY_ROW,
        });

        expect(envelope.api_key.name).toBe("batch");
    });

    test("name the failing member and index in the context path", () => {
        expect(() =>
            parseListApiKeysResponse({
                api_keys: "nope",
            }),
        ).toThrowError("response.api_keys must be an array");
        expect(() =>
            parseListApiKeysResponse({
                api_keys: [
                    {
                        ...KEY_ROW,
                        id: "five",
                    },
                ],
            }),
        ).toThrowError("response.api_keys[0].id must be an integer");
    });

    test("reject a foreign access decision and a foreign owner kind", () => {
        expect(() =>
            parseApiKeyEnvelope({
                api_key: {
                    ...KEY_ROW,
                    access_control: {
                        ...POLICY,
                        default_access: "maybe",
                    },
                },
            }),
        ).toThrowError("response.api_key.access_control.default_access is not an access decision");

        expect(() =>
            parseApiKeyEnvelope({
                api_key: {
                    ...KEY_ROW,
                    access_control: {
                        ...POLICY,
                        owner_defaults: [
                            {
                                type: "workspace",
                                workspace_id: 1,
                                access: "allow",
                            },
                        ],
                    },
                },
            }),
        ).toThrowError(ContractViolationError);
    });

    test("tolerate response fields the bindings do not declare", () => {
        const listing = parseListApiKeysResponse({
            api_keys: [
                {
                    ...KEY_ROW,
                    generated_later: true,
                },
            ],
        });

        expect(listing.api_keys[0]?.name).toBe("batch");
    });
});
