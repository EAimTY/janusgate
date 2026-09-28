import { createRoot, createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { createDeleteAction, createEditForm } from "./edit-form";
import { createScreenRead, type ScreenRead } from "./read";
import { conflictFailure, createDisposals, goneFailure, settle } from "./screen-test-support";

const disposes = createDisposals();

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

    disposes.push(dispose);

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
            perform: () => Promise.reject(conflictFailure("the name is taken")),
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

    it("gives the leave guard one final clean report when the card dies standing dirty", async () => {
        const read = mountedRead(async () => {});
        await settle();

        const reports: boolean[] = [];
        const [isDirty, setIsDirty] = createSignal(false);
        const dispose = createRoot((teardown) => {
            createEditForm({
                read,
                action: "update-model",
                isDirty,
                canSave: () => true,
                perform: () => Promise.resolve({}),
                onSucceeded: () => ({
                    tone: "ok",
                    title: "Saved.",
                }),
                onDirtyChange: (isFormDirty) => {
                    reports.push(isFormDirty);
                },
                onSaved: () => {},
            });
            return teardown;
        });

        try {
            // The visitor's edit turns the mounted card dirty.
            setIsDirty(true);
            await settle();
            expect(reports).toContain(true);
        } finally {
            // The not-found swap takes the dirty card away with no
            // dirty=false report of its own: the mount's last act is the
            // final clean report, so the screen left behind is never armed.
            dispose();
        }

        expect(reports[reports.length - 1]).toBe(false);
    });

    it("runs no success tail and states nothing when the save answers after the screen went away", async () => {
        let loadCount = 0;
        const read = mountedRead(async () => {
            loadCount += 1;
        });
        await settle();
        const loadsAfterMount = loadCount;

        let isRebaselined = false;
        let isLatched = false;
        let releaseWrite: (() => void) | undefined;
        const box: {
            form?: ReturnType<typeof createEditForm>;
        } = {};
        const dispose = createRoot((teardown) => {
            box.form = createEditForm({
                read,
                action: "update-provider",
                isDirty: () => true,
                canSave: () => true,
                perform: () =>
                    new Promise<void>((resolve) => {
                        releaseWrite = resolve;
                    }),
                onSucceeded: () => {
                    isRebaselined = true;

                    return {
                        tone: "ok",
                        title: "Saved.",
                    };
                },
                onDirtyChange: () => {},
                onSaved: () => {
                    isLatched = true;
                },
            });
            return teardown;
        });

        const form = box.form;

        if (form === undefined) {
            throw new Error("The form did not mount");
        }

        form.submit(new SubmitEvent("submit"));
        await Promise.resolve();

        // The visitor leaves with the save in flight; its late answer runs
        // no re-baseline, latches no hold, states no notice, and reloads
        // nothing.
        dispose();
        releaseWrite?.();
        await settle();

        expect(isRebaselined).toBe(false);
        expect(isLatched).toBe(false);
        expect(loadCount).toBe(loadsAfterMount);
        expect(read.notice()).toBeNull();
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
            perform: () => Promise.reject(goneFailure("no model 11")),
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

    it("states nothing and goes nowhere when the delete answers after the screen went away", async () => {
        const read = mountedRead(async () => {});
        await settle();

        let navigations = 0;
        let releaseDelete: (() => void) | undefined;
        const box: {
            confirmDelete?: () => Promise<boolean>;
        } = {};
        const dispose = createRoot((teardown) => {
            box.confirmDelete = createDeleteAction({
                read,
                action: "delete-model",
                perform: () =>
                    new Promise<void>((resolve) => {
                        releaseDelete = resolve;
                    }),
                onDeleted: () => {
                    navigations += 1;
                },
            });
            return teardown;
        });

        const confirmDelete = box.confirmDelete;

        if (confirmDelete === undefined) {
            throw new Error("The delete action did not mount");
        }

        const deletion = confirmDelete();
        await Promise.resolve();

        // The visitor leaves with the delete in flight; its late answer
        // states nothing, reloads nothing, and navigates nowhere (§6.4).
        dispose();
        releaseDelete?.();
        await settle();

        await expect(deletion).resolves.toBe(false);
        expect(navigations).toBe(0);
        expect(read.notice()).toBeNull();
    });
});
