import { describe, expect, it } from "vitest";
import { performManagementCall } from "./outcome";

describe("performManagementCall", () => {
    it("carries a successful result", async () => {
        const outcome = await performManagementCall(async () => 21);

        expect(outcome).toEqual({
            status: "ok",
            result: 21,
        });
    });

    it("carries a failed call's cause whole for the diagnosis to word", async () => {
        const failure = new Error("Failed to fetch");

        const outcome = await performManagementCall(async () => {
            throw failure;
        });

        expect(outcome).toEqual({
            status: "failed",
            cause: failure,
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
