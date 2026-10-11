import { describe, expect, test } from "vitest";
import { deadlineController } from "./deadline";

/**
 * The probe's deadline plumbing: one controller whose signal carries either
 * the deadline's expiry (reason included) or the caller's own abort,
 * whichever fires first — the forwarding that makes an expiry
 * distinguishable at the transport.
 */

describe("deadlineController", () => {
    test("forwards the deadline's expiry and reason into the controller", async () => {
        const controller = deadlineController(20);

        const outcome = await new Promise<string>((resolve) => {
            controller.signal.addEventListener(
                "abort",
                () => resolve(String(controller.signal.reason)),
                {
                    once: true,
                },
            );
        });

        expect(controller.signal.aborted).toBe(true);
        expect(outcome).toContain("TimeoutError");
    });

    test("lets the caller abort first, ending the request the same way", () => {
        const controller = deadlineController(60_000);

        controller.abort(new Error("operator cancel"));

        expect(controller.signal.aborted).toBe(true);
        expect(controller.signal.reason).toBeInstanceOf(Error);
    });
});
