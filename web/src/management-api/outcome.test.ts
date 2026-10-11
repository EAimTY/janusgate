import { describe, expect, test } from "vitest";
import { ManagementApiError } from "./errors";
import { performManagementCall } from "./outcome";

/**
 * The outcome wrapper screens consume: a value, a failure carried whole as
 * its cause, or nothing to do because the call was aborted — where an abort
 * is recognized by its `AbortError` name alone, whatever object carried it.
 */

describe("performManagementCall", () => {
    test("answers a resolved operation as ok with its result", async () => {
        const outcome = await performManagementCall(async () => 42);

        expect(outcome).toEqual({
            status: "ok",
            result: 42,
        });
    });

    test("answers a rejection that is not an abort as failed, carrying the cause whole", async () => {
        const cause = new ManagementApiError("gateway", "no such row", {
            errno: 3,
        });

        const outcome = await performManagementCall<never>(async () => {
            throw cause;
        });

        expect(outcome).toEqual({
            status: "failed",
            cause,
        });
    });

    test("answers an AbortError-named rejection as aborted, whatever object carried it", async () => {
        const outcome = await performManagementCall<never>(async () => {
            throw new DOMException("The operation was aborted.", "AbortError");
        });

        expect(outcome).toEqual({
            status: "aborted",
        });
    });

    test("answers a plain object named AbortError as aborted", async () => {
        const outcome = await performManagementCall<never>(async () => {
            throw {
                name: "AbortError",
            };
        });

        expect(outcome).toEqual({
            status: "aborted",
        });
    });

    test("keeps a non-abort Error whose message mentions abort as failed", async () => {
        const outcome = await performManagementCall<never>(async () => {
            throw new Error("aborted for another reason");
        });

        expect(outcome).toEqual({
            status: "failed",
            cause: new Error("aborted for another reason"),
        });
    });
});
