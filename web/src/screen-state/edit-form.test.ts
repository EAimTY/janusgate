import { createRoot, createSignal } from "solid-js";
import { describe, expect, test, vi } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { createDeleteAction, createEditForm } from "./edit-form";
import type { ScreenNotice } from "./notices";
import { createScreenRead } from "./read";

/**
 * The settings-form wiring the edit screens share: the save gate (dirt plus
 * inline rules, and never while a save runs), a failed or aborted save
 * stopping at the notice, a succeeded save running the re-baseline, latching
 * the row hold until its own reload lands, and stating the returned notice
 * — which survives its own reload only on screens that keep action notices;
 * and the confirmed delete whose gone answer first reloads the listing so
 * the not-found state takes over.
 */

/** Advances past owned scopes, staged writes, and answered promises. */
async function tick(): Promise<void> {
    for (let round = 0; round < 4; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

const SAVED_NOTICE: ScreenNotice = {
    tone: "ok",
    title: "saved",
};

describe("createEditForm", () => {
    /** One form mounted under one root beside its screen read. */
    function mountForm(options: {
        readonly canSave?: () => boolean;
        readonly perform: () => Promise<unknown>;
        readonly onSucceeded?: (result: unknown) => ScreenNotice;
    }): {
        isSaving: () => boolean;
        submit: (event: SubmitEvent) => void;
        savedCount: () => number;
        read: ReturnType<typeof createScreenRead>;
        dispose: () => void;
    } {
        let disposeRoot: (() => void) | null = null;
        let saved = 0;
        const harness = createRoot((dispose) => {
            disposeRoot = dispose;
            const read = createScreenRead({
                load: async () => {},
                // The edit screens' keep policy: a save's outcome survives the
                // reload the save itself triggers.
                isNoticeKeptOnSuccess: true,
            });
            const [isDirty] = createSignal(false);
            const form = createEditForm<unknown>({
                read,
                action: "update-provider",
                isDirty,
                canSave: options.canSave ?? (() => true),
                perform: options.perform,
                onSucceeded: options.onSucceeded ?? (() => SAVED_NOTICE),
                onDirtyChange: () => {},
                onSaved: () => {
                    saved += 1;
                },
            });

            return {
                read,
                form,
            };
        });

        return {
            isSaving: harness.form.isSaving,
            submit: harness.form.submit,
            savedCount: () => saved,
            read: harness.read,
            dispose: () => disposeRoot?.(),
        };
    }

    const SUBMIT_EVENT = {
        preventDefault: vi.fn(),
    } as unknown as SubmitEvent;

    test("prevents the submit's default, runs the write, states the notice, and reloads", async () => {
        const onSucceeded = vi.fn(() => SAVED_NOTICE);
        const mounted = mountForm({
            perform: async () => ({}),
            onSucceeded,
        });
        await tick();

        mounted.submit(SUBMIT_EVENT);
        await tick();

        expect(SUBMIT_EVENT.preventDefault).toHaveBeenCalled();
        expect(onSucceeded).toHaveBeenCalledWith({});
        expect(mounted.savedCount()).toBe(1);
        // The save's own reload landed and the kept notice survived it.
        expect(mounted.read.notice()).toEqual(SAVED_NOTICE);
        mounted.dispose();
    });

    test("refuses a save while the caller's gate stands closed", async () => {
        const perform = vi.fn(async () => ({}));
        const mounted = mountForm({
            canSave: () => false,
            perform,
        });
        await tick();

        mounted.submit(SUBMIT_EVENT);
        await tick();

        expect(perform).not.toHaveBeenCalled();
        expect(mounted.savedCount()).toBe(0);
        mounted.dispose();
    });

    test("refuses a save while one is already in flight", async () => {
        const settles: Array<() => void> = [];
        const mounted = mountForm({
            perform: () =>
                new Promise<unknown>((resolve) => {
                    settles.push(() => resolve({}));
                }),
        });
        await tick();

        mounted.submit(SUBMIT_EVENT);
        // The flight flag commits before the second submit reads it.
        await tick();
        mounted.submit(SUBMIT_EVENT);
        for (const settle of settles) {
            settle();
        }
        await tick();

        expect(settles).toHaveLength(1);
        expect(mounted.savedCount()).toBe(1);
        mounted.dispose();
    });

    test("a failed save states the diagnosis and leaves the success tail unrun", async () => {
        const onSucceeded = vi.fn(() => SAVED_NOTICE);
        const mounted = mountForm({
            perform: async () => {
                throw new Error("HTTP 409");
            },
            onSucceeded,
        });
        await tick();

        mounted.submit(SUBMIT_EVENT);
        await tick();

        expect(onSucceeded).not.toHaveBeenCalled();
        expect(mounted.savedCount()).toBe(0);
        expect(mounted.read.notice()?.tone).toBe("bad");
        mounted.dispose();
    });

    test("a save answering after the screen went away states nothing", async () => {
        let disposeForm: (() => void) | null = null;
        const onSucceeded = vi.fn(() => SAVED_NOTICE);
        const event = {
            preventDefault: vi.fn(),
        } as unknown as SubmitEvent;
        const saved = await createRoot(async (dispose) => {
            disposeForm = dispose;
            const read = createScreenRead({
                load: async () => {},
            });
            const [isDirty] = createSignal(false);
            const form = createEditForm<unknown>({
                read,
                action: "update-provider",
                isDirty,
                canSave: () => true,
                perform: () => {
                    // The screen goes away while the write is in flight.
                    disposeForm?.();
                    return Promise.resolve({});
                },
                onSucceeded,
                onDirtyChange: () => {},
                onSaved: () => {},
            });
            await tick();

            form.submit(event);
            await tick();

            return {
                notice: read.notice(),
                stated: onSucceeded.mock.calls.length,
            };
        });
        void disposeForm;

        expect(saved.stated).toBe(0);
        expect(saved.notice).toBeNull();
        void event;
    });
});

describe("createDeleteAction", () => {
    test("answers true after a confirmed deletion succeeds", async () => {
        await createRoot(async (dispose) => {
            const root = createScreenRead({
                load: async () => {},
            });
            const onDeleted = vi.fn();
            const confirmed = createDeleteAction({
                read: root,
                action: "delete-provider",
                perform: async () => ({}),
                onDeleted,
            });
            await tick();

            await expect(confirmed()).resolves.toBe(true);
            expect(onDeleted).toHaveBeenCalledTimes(1);
            dispose();
        });
    });

    test("states a failed deletion and answers false, leaving the screen in place", async () => {
        await createRoot(async (dispose) => {
            const root = createScreenRead({
                load: async () => {},
            });
            const onDeleted = vi.fn();
            const confirmed = createDeleteAction({
                read: root,
                action: "delete-provider",
                perform: async () => {
                    throw new Error("HTTP 409");
                },
                onDeleted,
            });
            await tick();

            await expect(confirmed()).resolves.toBe(false);
            expect(onDeleted).not.toHaveBeenCalled();
            expect(root.notice()?.tone).toBe("bad");
            dispose();
        });
    });

    test("a gone answer first reloads the listing so the not-found state takes over", async () => {
        await createRoot(async (dispose) => {
            let loads = 0;
            const read = createScreenRead({
                load: async () => {
                    loads += 1;
                },
            });
            await tick();
            expect(loads).toBe(1);

            const confirmed = createDeleteAction({
                read,
                action: "delete-pool",
                perform: async () => {
                    throw new ManagementApiError("gateway", "no such pool", {
                        errno: ErrorCode.NotFound,
                        httpStatus: 404,
                    });
                },
                onDeleted: vi.fn(),
            });

            await expect(confirmed()).resolves.toBe(false);
            await tick();

            expect(loads).toBe(2);
            dispose();
        });
    });
});
