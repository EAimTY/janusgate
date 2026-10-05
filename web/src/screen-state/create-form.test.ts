import { createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { conflictFailure, createDisposals, settle } from "../test-support/management-api";
import type { ActionBegin } from "./action-begin";
import { createCreateAction } from "./create-form";
import type { ScreenNotice } from "./notices";

/** The row this suite's creations answer with. */
type CreatedRow = {
    readonly id: number;
};

const BLOCKED_NOTICE: ScreenNotice = {
    tone: "bad",
    title: "The definition JSON is not a definition the gateway could store.",
};

/** The `"called"` begin of a creation that answered `result`. */
function calledRow(result: CreatedRow): ActionBegin<CreatedRow> {
    return {
        status: "called",
        outcome: {
            status: "ok",
            result,
        },
    };
}

/** The `"called"` begin of a creation whose gateway call failed. */
function failedRow(cause: unknown): ActionBegin<CreatedRow> {
    return {
        status: "called",
        outcome: {
            status: "failed",
            cause,
        },
    };
}

/** The `"called"` begin of a creation whose gateway call was aborted. */
function abortedRow(): ActionBegin<CreatedRow> {
    return {
        status: "called",
        outcome: {
            status: "aborted",
        },
    };
}

const disposes = createDisposals();

/**
 * Mounts one value inside a reactive root, registered for disposal after the
 * test — the way a mounted screen owns its action, so the disposal guard's
 * cleanup exists. Answers the value and the root's own dispose, for tests
 * that take the screen away mid-exchange.
 */
function mountInRoot<TValue>(mount: () => TValue): {
    readonly value: TValue;
    readonly dispose: () => void;
} {
    const box: {
        value?: TValue;
    } = {};
    const dispose = createRoot((teardown) => {
        box.value = mount();
        return teardown;
    });

    if (box.value === undefined) {
        throw new Error("Nothing mounted");
    }

    disposes.push(dispose);

    return {
        value: box.value,
        dispose,
    };
}

describe("createCreateAction", () => {
    it("runs a creation only through its gate, one at a time", async () => {
        let creationCount = 0;
        let releaseCreation: ((begin: ActionBegin<CreatedRow>) => void) | undefined;
        let isGateOpen = false;

        const { value: action } = mountInRoot(() =>
            createCreateAction({
                action: "create-provider",
                channel: () => {},
                canRun: () => isGateOpen,
                perform: () => {
                    creationCount += 1;
                    return new Promise<ActionBegin<CreatedRow>>((resolve) => {
                        releaseCreation = resolve;
                    });
                },
                onCreated: () => {},
            }),
        );

        // A closed gate runs nothing, and nothing takes flight.
        action.submit(new SubmitEvent("submit"));
        await settle();
        expect(creationCount).toBe(0);
        expect(action.isCreating()).toBe(false);

        // An open gate runs one creation; a submit racing it in flight runs
        // none. The one-microtask wait is the gap between two real events,
        // across which the flight signal has landed.
        isGateOpen = true;
        action.submit(new SubmitEvent("submit"));
        await Promise.resolve();
        action.submit(new SubmitEvent("submit"));
        expect(creationCount).toBe(1);
        expect(action.isCreating()).toBe(true);

        releaseCreation?.(
            calledRow({
                id: 1,
            }),
        );
        await settle();

        expect(action.isCreating()).toBe(false);
        expect(creationCount).toBe(1);
    });

    it("clears the channel, hands the answered row to the success tail, and stops there", async () => {
        const stated: Array<ScreenNotice | null> = [];
        const landed: CreatedRow[] = [];

        const { value: action } = mountInRoot(() =>
            createCreateAction({
                action: "create-model",
                channel: (notice) => {
                    stated.push(notice);
                },
                canRun: () => true,
                perform: async () =>
                    calledRow({
                        id: 9,
                    }),
                onCreated: (created) => {
                    landed.push(created);
                },
            }),
        );

        action.submit(new SubmitEvent("submit"));
        await settle();

        // The flight cleared the channel before taking off, and the answered
        // row is the success tail's whole subject — the tail navigates.
        expect(stated).toEqual([
            null,
        ]);
        expect(landed).toEqual([
            {
                id: 9,
            },
        ]);
    });

    it("states a failed creation's diagnosis on the channel and leaves the success tail unrun", async () => {
        const stated: Array<ScreenNotice | null> = [];
        let landed = 0;

        const { value: action } = mountInRoot(() =>
            createCreateAction({
                action: "create-api-key",
                channel: (notice) => {
                    stated.push(notice);
                },
                canRun: () => true,
                perform: () => Promise.resolve(failedRow(conflictFailure("the name is taken"))),
                onCreated: () => {
                    landed += 1;
                },
            }),
        );

        action.submit(new SubmitEvent("submit"));
        await settle();

        expect(landed).toBe(0);
        expect(stated).toHaveLength(2);
        expect(stated[0]).toBeNull();
        expect(stated[1]?.tone).toBe("bad");
        expect(stated[1]?.detail).toContain("the name is taken");
    });

    it("states an aborted creation's nothing", async () => {
        const stated: Array<ScreenNotice | null> = [];
        let landed = 0;

        const { value: action } = mountInRoot(() =>
            createCreateAction({
                action: "create-model",
                channel: (notice) => {
                    stated.push(notice);
                },
                canRun: () => true,
                perform: async () => abortedRow(),
                onCreated: () => {
                    landed += 1;
                },
            }),
        );

        action.submit(new SubmitEvent("submit"));
        await settle();

        expect(stated).toEqual([
            null,
        ]);
        expect(landed).toBe(0);
    });

    it("states a refused prelude's notice on the channel and runs no success tail", async () => {
        const stated: Array<ScreenNotice | null> = [];
        let landed = 0;

        const { value: action } = mountInRoot(() =>
            createCreateAction({
                action: "create-provider",
                channel: (notice) => {
                    stated.push(notice);
                },
                canRun: () => true,
                perform: async () => ({
                    status: "blocked" as const,
                    notice: BLOCKED_NOTICE,
                }),
                onCreated: () => {
                    landed += 1;
                },
            }),
        );

        action.submit(new SubmitEvent("submit"));
        await settle();

        expect(stated).toEqual([
            null,
            BLOCKED_NOTICE,
        ]);
        expect(landed).toBe(0);
    });

    it("states nothing and lands nowhere when the creation answers after the screen went away", async () => {
        const stated: Array<ScreenNotice | null> = [];
        let landed = 0;
        let releaseCreation: ((begin: ActionBegin<CreatedRow>) => void) | undefined;

        const { value: action, dispose } = mountInRoot(() =>
            createCreateAction({
                action: "create-model",
                channel: (notice) => {
                    stated.push(notice);
                },
                canRun: () => true,
                perform: () =>
                    new Promise<ActionBegin<CreatedRow>>((resolve) => {
                        releaseCreation = resolve;
                    }),
                onCreated: () => {
                    landed += 1;
                },
            }),
        );

        action.submit(new SubmitEvent("submit"));
        await Promise.resolve();
        expect(action.isCreating()).toBe(true);

        // The visitor leaves with the creation in flight; its late answer
        // states nothing and lands nowhere — the channel keeps exactly what
        // the flight cleared.
        const statedBeforeDisposal = stated.length;
        dispose();
        releaseCreation?.(
            calledRow({
                id: 3,
            }),
        );
        await settle();

        expect(landed).toBe(0);
        expect(stated).toHaveLength(statedBeforeDisposal);
    });
});
