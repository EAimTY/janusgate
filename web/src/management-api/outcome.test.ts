import { describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { performManagementCall } from "./outcome";

describe("performManagementCall", () => {
    it("carries a successful result", async () => {
        const outcome = await performManagementCall(async () => 21);

        expect(outcome).toEqual({
            status: "ok",
            result: 21,
        });
    });

    it("keeps the gateway's message and errno for a rendered failure", async () => {
        const outcome = await performManagementCall(async () => {
            throw new ManagementApiError("gateway", "model row not found", {
                httpStatus: 404,
                errno: ErrorCode.NotFound,
            });
        });

        expect(outcome).toEqual({
            status: "failed",
            message: "model row not found",
            errno: ErrorCode.NotFound,
        });
    });

    it("reports a failure outside the gateway's error domain without an errno", async () => {
        const outcome = await performManagementCall(async () => {
            throw new Error("Failed to fetch");
        });

        expect(outcome).toEqual({
            status: "failed",
            message: "Failed to fetch",
            errno: null,
        });
    });

    it("treats an abort as no outcome at all", async () => {
        const outcome = await performManagementCall(async () => {
            throw new DOMException("The operation was aborted.", "AbortError");
        });

        expect(outcome).toEqual({
            status: "aborted",
        });
    });
});
