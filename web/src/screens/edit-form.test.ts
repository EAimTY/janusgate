import { createRoot } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { createDeleteAction, createEditForm } from "./edit-form";
import { createScreenRead, type ScreenRead } from "./read";
import { settle } from "./screen-test-support";

const mountedDisposes: Array<() => void> = [];

afterEach(() => {
    for (const dispose of mountedDisposes.reverse()) {
        dispose();
    }
    mountedDisposes.length = 0;
});

/**
 * Mounts one read harness outside a component, registered for disposal — a
 * kept-notice mount, the way the three edit screens mount theirs, so a
 * save's or delete's reported outcome stays stated beside its reload.
 */
function mountedRead(load: () => Promise<void>): ScreenRead {
    const box: {
        read?: ScreenRead;
    } = {};
    const dispose = createRoot((teardown) => {
        box.read = createScreenRead({
            load,
            isNoticeKeptOnSuccess: true,
        });
        return teardown;
    });

    if (box.read === undefined) {
        throw new Error("The harness did not mount");
    }

    mountedDisposes.push(dispose);

    return box.read;
}

describe("createEditForm", () => {
    it("runs a write only through the save gate, one at a time", async () => {
        const read = mountedRead(async () => {});
        await settle();

        let writeCount = 0;
        let releaseWrite: (() => void) | undefined;
        let isGateOpen = false;
        const form = createEditForm({
            read,
            action: "update-api-key",
            isDirty: () => true,
            canSave: () => isGateOpen,
            perform: () => {
                writeCount += 1;
                return new Promise<void>((resolve) => {
                    releaseWrite = resolve;
                });
            },
            onSucceeded: () => ({
                tone: "ok",
                title: "Saved.",
            }),
            onDirtyChange: () => {},
            onSaved: () => {},
        });

        // A closed gate runs nothing, and nothing takes flight.
        form.submit(new SubmitEvent("submit"));
        await settle();
        expect(writeCount).toBe(0);
        expect(form.isSaving()).toBe(false);

        // An open gate runs one write; a submit racing it in flight runs none.
        // The one-microtask wait is the gap between two real events, across
        // which the flight signal has landed.
        isGateOpen = true;
        form.submit(new SubmitEvent("submit"));
        await Promise.resolve();
        form.submit(new SubmitEvent("submit"));
        expect(writeCount).toBe(1);
        expect(form.isSaving()).toBe(true);

        if (releaseWrite === undefined) {
            throw new Error("The write did not start");
        }
        releaseWrite();
        await settle();

        expect(form.isSaving()).toBe(false);
        expect(writeCount).toBe(1);
    });

    it("re-baselines, latches the hold, states the notice, and reloads — in that order", async () => {
        const events: string[] = [];
        const read = mountedRead(async () => {
            events.push("reloaded");
        });
        await settle();
        events.length = 0;

        const form = createEditForm({
            read,
            action: "update-model",
            isDirty: () => true,
            canSave: () => true,
            perform: () =>
                Promise.resolve({
                    publicId: "alpha-gpt-x",
                }),
            onSucceeded: (saved) => {
                events.push(`re-baselined:${saved.publicId}`);

                return {
                    tone: "ok",
                    title: "Model saved.",
                    detail: `Clients now call it ${saved.publicId}.`,
                };
            },
            onDirtyChange: () => {},
            onSaved: () => {
                events.push("latched");
            },
        });

        form.submit(new SubmitEvent("submit"));
        await settle();

        expect(events).toEqual([
            "re-baselined:alpha-gpt-x",
            "latched",
            "reloaded",
        ]);
        expect(read.notice()).toEqual({
            tone: "ok",
            title: "Model saved.",
            detail: "Clients now call it alpha-gpt-x.",
        });
    });

    it("stops at the report gate when the save fails, leaving the success tail unrun", async () => {
        let loadCount = 0;
        const read = mountedRead(async () => {
            loadCount += 1;
        });
        await settle();
        const loadsAfterMount = loadCount;

        let succeeded = false;
        let latched = false;
        const form = createEditForm({
            read,
            action: "update-api-key",
            isDirty: () => true,
            canSave: () => true,
            perform: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "the name is taken", {
                        errno: ErrorCode.Conflict,
                        httpStatus: 409,
                    }),
                ),
            onSucceeded: () => {
                succeeded = true;

                return {
                    tone: "ok",
                    title: "Saved.",
                };
            },
            onDirtyChange: () => {},
            onSaved: () => {
                latched = true;
            },
        });

        form.submit(new SubmitEvent("submit"));
        await settle();

        expect(succeeded).toBe(false);
        expect(latched).toBe(false);
        expect(loadCount).toBe(loadsAfterMount);
        expect(read.notice()?.tone).toBe("bad");
    });
});

describe("createDeleteAction", () => {
    it("answers the row is gone and navigates when the confirmed delete answers", async () => {
        const read = mountedRead(async () => {});
        await settle();

        let navigations = 0;
        const confirmDelete = createDeleteAction({
            read,
            action: "delete-provider",
            perform: () => Promise.resolve(),
            onDeleted: () => {
                navigations += 1;
            },
        });

        await expect(confirmDelete()).resolves.toBe(true);
        expect(navigations).toBe(1);
    });

    it("answers the row survived, states the failure, and reloads a gone answer's listing", async () => {
        let loadCount = 0;
        const read = mountedRead(async () => {
            loadCount += 1;
        });
        await settle();
        const loadsAfterMount = loadCount;

        let navigations = 0;
        const confirmDelete = createDeleteAction({
            read,
            action: "delete-model",
            perform: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "no model 11", {
                        errno: ErrorCode.NotFound,
                        httpStatus: 404,
                    }),
                ),
            onDeleted: () => {
                navigations += 1;
            },
        });

        await expect(confirmDelete()).resolves.toBe(false);
        expect(navigations).toBe(0);
        await settle();

        // The gone diagnosis reloaded the listing first (§6.4), so the
        // not-found state takes over instead of a ghost form.
        expect(loadCount).toBe(loadsAfterMount + 1);
        expect(read.notice()?.tone).toBe("bad");
    });
});
