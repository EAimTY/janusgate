import { createRoot, createSignal } from "solid-js";
import { describe, expect, test, vi } from "vitest";
import type { ManagementCallOutcome } from "/src/management-api/outcome";
import { performManagementCall } from "/src/management-api/outcome";
import { createBeginAction } from "./action-begin";
import { createCreateAction } from "./create-form";
import { createDisposalGuard } from "./lifetime";
import type { ScreenNotice } from "./notices";

/**
 * The begin action the creation form and the OAuth flow share: one flight
 * signal and one settle — a blocked prelude states its notice, an aborted
 * call states nothing, a failed one is diagnosed onto the channel, a
 * succeeded one runs the success tail alone, a begin answering after the
 * screen went away lands nowhere, and a second run while one is in flight is
 * refused.
 */

/** Advances past owned scopes and staged writes. */
async function tick(): Promise<void> {
    for (let round = 0; round < 4; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

function called<TResult>(result: TResult): ActionBeginOf<TResult> {
    return {
        status: "called",
        outcome: {
            status: "ok",
            result,
        },
    };
}

/** The begin union the actions settle, spelled locally for the harness. */
type ActionBeginOf<TResult> =
    | {
          readonly status: "blocked";
          readonly notice: ScreenNotice;
      }
    | {
          readonly status: "called";
          readonly outcome: ManagementCallOutcome<TResult>;
      };

describe("createDisposalGuard", () => {
    test("answers false until the owner's cleanup runs, then true forever", async () => {
        await createRoot(async (dispose) => {
            const isDisposed = createDisposalGuard();

            expect(isDisposed()).toBe(false);
            dispose();
            await tick();

            expect(isDisposed()).toBe(true);
        });
    });
});

describe("createBeginAction", () => {
    function mount<TResult>(options: {
        readonly perform: () => Promise<ActionBeginOf<TResult>>;
        readonly canRun?: () => boolean;
        readonly isDisposed?: () => boolean;
        readonly onSucceeded?: (result: TResult) => void;
    }): {
        begin: ReturnType<typeof createBeginAction<TResult>>;
        channel: () => ScreenNotice | null;
        dispose: () => void;
    } {
        let disposeRoot: (() => void) | null = null;
        const [channel, setChannel] = createSignal<ScreenNotice | null>(null);
        const begin = createRoot((dispose) => {
            disposeRoot = dispose;
            return createBeginAction<TResult>({
                action: "start-oauth",
                channel: setChannel,
                isDisposed: options.isDisposed ?? (() => false),
                canRun: options.canRun ?? (() => true),
                perform: options.perform,
                onSucceeded: options.onSucceeded ?? (() => {}),
            });
        });

        return {
            begin,
            channel: () => channel(),
            dispose: () => disposeRoot?.(),
        };
    }

    test("runs the begin and hands the result to the success tail alone", async () => {
        const onSucceeded = vi.fn();
        const mount_ = mount<number>({
            perform: async () => called(7),
            onSucceeded,
        });
        await tick();

        await mount_.begin.run();
        await tick();

        expect(onSucceeded).toHaveBeenCalledWith(7);
        expect(mount_.channel()).toBeNull();
        mount_.dispose();
    });

    test("refuses a second run while one is in flight", async () => {
        let settle: (() => void) | null = null;
        // The assignment happens inside the perform executor, which the read
        // site's control-flow analysis cannot see; settle through a function,
        // where the variable reads at its declared type.
        const settleRun = () => settle?.();
        let runs = 0;
        const mount_ = mount<number>({
            perform: () => {
                runs += 1;
                return new Promise<ActionBeginOf<number>>((resolve) => {
                    settle = () => resolve(called(runs));
                });
            },
        });
        await tick();

        const first = mount_.begin.run();
        // The flight flag commits before the second run reads it.
        await tick();
        await mount_.begin.run();
        settleRun();
        await first;
        await tick();

        expect(runs).toBe(1);
        mount_.dispose();
    });

    test("refuses a run whose caller gate stands closed", async () => {
        const perform = vi.fn(async () => called(1));
        const mount_ = mount<number>({
            perform,
            canRun: () => false,
        });
        await tick();

        await mount_.begin.run();
        await tick();

        expect(perform).not.toHaveBeenCalled();
        mount_.dispose();
    });

    test("states a blocked prelude's notice on the channel", async () => {
        const perform = vi.fn(async () => ({
            status: "blocked" as const,
            notice: {
                tone: "bad" as const,
                title: "The prelude refused.",
            },
        }));
        const mount_ = mount<number>({
            perform,
        });
        await tick();

        await mount_.begin.run();
        await tick();

        expect(mount_.channel()).toEqual({
            tone: "bad",
            title: "The prelude refused.",
        });
        mount_.dispose();
    });

    test("states nothing for an aborted call", async () => {
        const mount_ = mount<number>({
            perform: async () => ({
                status: "called",
                outcome: {
                    status: "aborted",
                },
            }),
        });
        await tick();

        await mount_.begin.run();
        await tick();

        expect(mount_.channel()).toBeNull();
        mount_.dispose();
    });

    test("diagnoses a failed call onto the channel under the action's name", async () => {
        const mount_ = mount<number>({
            perform: async () => ({
                status: "called",
                outcome: {
                    status: "failed",
                    cause: new Error("HTTP 404"),
                },
            }),
        });
        await tick();

        await mount_.begin.run();
        await tick();

        expect(mount_.channel()?.tone).toBe("bad");
        mount_.dispose();
    });

    test("a begin answering after the screen went away lands nowhere", async () => {
        const onSucceeded = vi.fn();
        let isDisposed = false;
        const mount_ = mount<number>({
            perform: async () => called(7),
            isDisposed: () => isDisposed,
            onSucceeded,
        });
        await tick();

        isDisposed = true;
        await mount_.begin.run();
        await tick();

        expect(onSucceeded).not.toHaveBeenCalled();
        mount_.dispose();
    });
});

describe("createCreateAction", () => {
    test("prevents the submit's default and runs the creation once", async () => {
        const onCreated = vi.fn();
        const event = {
            preventDefault: vi.fn(),
        } as unknown as SubmitEvent;
        await createRoot(async (dispose) => {
            const creation = createCreateAction<number>({
                action: "create-provider",
                channel: () => {},
                canRun: () => true,
                perform: async () => called(42),
                onCreated,
            });
            await tick();

            creation.submit(event);
            await tick();

            expect(event.preventDefault).toHaveBeenCalled();
            expect(onCreated).toHaveBeenCalledWith(42);
            dispose();
        });
    });
});

describe("performManagementCall around the begin", () => {
    test("wraps a rejection as the failed outcome the settle diagnoses", async () => {
        const outcome = await performManagementCall<never>(async () => {
            throw new Error("transport down");
        });

        expect(outcome.status).toBe("failed");
    });
});
