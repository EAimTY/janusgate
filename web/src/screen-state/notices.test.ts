import { describe, expect, test } from "vitest";

/** Solid 2 schedules signal writes; every write below settles through a tick. */
async function tick(): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

import type { Diagnosis } from "/src/management-api/diagnosis";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { createStandingNotice, noticeOf, screenFailureNotice } from "./notices";

/**
 * The notice layer screens state their failures and outcomes through: a
 * diagnosis carried verbatim as a bad notice, and the origin-paired
 * standing notice under the two-origin rule — set-by-origin, clear-by-origin,
 * with `clear` the one origin-unchecked lift.
 */

describe("noticeOf", () => {
    test("carries a diagnosis's title, detail, and fix as a bad notice", () => {
        const diagnosis: Diagnosis = {
            context: {
                baseUrl: "http://gw.test",
                pageOrigin: "http://page.test",
            },
            kind: "conflict",
            title: "taken",
            detail: "the gateway refused",
            fix: "choose another",
        };

        const notice = noticeOf(diagnosis);

        expect(notice.tone).toBe("bad");
        expect(notice.title).toBe("taken");
        expect(notice.detail).toBe("the gateway refused");
        expect(notice.fix).toBe("choose another");
        expect(notice.rawDetail).toBeUndefined();
    });
});

describe("screenFailureNotice", () => {
    test("diagnoses a failed call's cause into the bad notice it renders", () => {
        const cause = new ManagementApiError("gateway", "no such row", {
            errno: ErrorCode.NotFound,
            httpStatus: 404,
        });

        const notice = screenFailureNotice(cause, {
            action: "delete-provider",
        });

        expect(notice.tone).toBe("bad");
        expect(notice.detail).toBe("no such row");
    });
});

describe("createStandingNotice two-origin rule", () => {
    /** A stable notice factory, the identity each assertion below reads. */
    function stated(title: string): {
        tone: "bad";
        title: string;
    } {
        return {
            tone: "bad",
            title,
        };
    }

    test("starts empty and pairs every notice with its origin", async () => {
        const controller = createStandingNotice();

        expect(controller.notice()).toBeNull();
        expect(controller.origin()).toBeNull();

        controller.state("read", stated("read failed"));
        await tick();

        expect(controller.notice()).toEqual(stated("read failed"));
        expect(controller.origin()).toBe("read");
    });

    test("lifts only the origin that stated the standing notice", async () => {
        const controller = createStandingNotice();
        controller.state("action", stated("saved"));
        await tick();

        controller.clearOfOrigin("read");
        await tick();

        // A read's success cannot clear an action's notice.
        expect(controller.notice()).toEqual(stated("saved"));

        controller.clearOfOrigin("action");
        await tick();

        expect(controller.notice()).toBeNull();
    });

    test("replaces whatever stands when another origin states its own", async () => {
        const controller = createStandingNotice();
        controller.state("read", stated("read failed"));
        controller.state("action", stated("delete done"));
        await tick();

        expect(controller.origin()).toBe("action");
        expect(controller.notice()).toEqual(stated("delete done"));

        // And the new origin's own clear lifts it, the old origin's reaching nothing.
        controller.clearOfOrigin("read");
        await tick();
        expect(controller.notice()).toEqual(stated("delete done"));
        controller.clearOfOrigin("action");
        await tick();
        expect(controller.notice()).toBeNull();
    });

    test("clear is the one origin-unchecked lift", async () => {
        const controller = createStandingNotice();
        controller.state("action", stated("delete done"));
        await tick();

        controller.clear();
        await tick();

        expect(controller.notice()).toBeNull();
        expect(controller.origin()).toBeNull();
        // Clearing an empty controller changes nothing and throws nothing.
        expect(() => controller.clear()).not.toThrow();
    });
});
