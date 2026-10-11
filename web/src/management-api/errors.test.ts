import { describe, expect, test } from "vitest";
import {
    ContractViolationError,
    describeManagementFailure,
    isAbortFailure,
    ManagementApiError,
    managementFailureBaseUrl,
} from "./errors";
import { ErrorCode } from "./generated/v0";

/**
 * The failure vocabulary every management call ends in: the error object's
 * defaulted details, the base-URL fallback a diagnosis reads off a cleared
 * connection, the abort predicate that treats control flow as no failure,
 * and the message rendering of any thrown value.
 */

describe("ManagementApiError", () => {
    test("defaults every detail to null", () => {
        const error = new ManagementApiError("transport", "HTTP 502");

        expect(error.category).toBe("transport");
        expect(error.message).toBe("HTTP 502");
        expect(error.httpStatus).toBeNull();
        expect(error.errno).toBeNull();
        expect(error.baseUrl).toBeNull();
    });

    test("carries the details it is given and the cause behind the message", () => {
        const cause = new Error("root");
        const error = new ManagementApiError("gateway", "no such row", {
            httpStatus: 404,
            errno: ErrorCode.NotFound,
            baseUrl: "http://gateway.test",
            cause,
        });

        expect(error.httpStatus).toBe(404);
        expect(error.errno).toBe(ErrorCode.NotFound);
        expect(error.baseUrl).toBe("http://gateway.test");
        expect(error.cause).toBe(cause);
        expect(error.name).toBe("ManagementApiError");
    });
});

describe("managementFailureBaseUrl", () => {
    test("reads the retained base URL off a management failure", () => {
        const error = new ManagementApiError("transport", "Network request failed", {
            baseUrl: "http://gateway.test:8787",
        });

        expect(managementFailureBaseUrl(error)).toBe("http://gateway.test:8787");
    });

    test("answers null for a non-management error or any other value", () => {
        expect(managementFailureBaseUrl(new Error("plain"))).toBeNull();
        expect(managementFailureBaseUrl("transport")).toBeNull();
        expect(managementFailureBaseUrl(undefined)).toBeNull();
    });
});

describe("isAbortFailure", () => {
    test("recognizes a DOMException-named abort", () => {
        expect(isAbortFailure(new DOMException("aborted", "AbortError"))).toBe(true);
    });

    test("recognizes any object carrying the AbortError name", () => {
        expect(
            isAbortFailure({
                name: "AbortError",
            }),
        ).toBe(true);
    });

    test("rejects ordinary errors, strings, and nullish values", () => {
        expect(isAbortFailure(new Error("nope"))).toBe(false);
        expect(isAbortFailure("AbortError")).toBe(false);
        expect(isAbortFailure(null)).toBe(false);
        expect(isAbortFailure(undefined)).toBe(false);
    });
});

describe("describeManagementFailure", () => {
    test("reads an Error's own message", () => {
        expect(describeManagementFailure(new Error("the words"))).toBe("the words");
    });

    test("renders a non-error value as its string form", () => {
        expect(describeManagementFailure(42)).toBe("42");
        expect(describeManagementFailure(null)).toBe("null");
    });
});

describe("ContractViolationError", () => {
    test("names itself for the client's contract classification to recognize", () => {
        const error = new ContractViolationError("response.x must be a string");

        expect(error.name).toBe("ContractViolationError");
        expect(error.message).toBe("response.x must be a string");
    });
});
