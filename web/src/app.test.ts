import { afterEach, describe, expect, it } from "vitest";
import { drainWrites } from "/src/screens/screen-test-support";
import { guardUnsavedFormsOnLeave } from "./app";
import { type DirtyFormRegistration, registerDirtyForm } from "./dirty-form-registry";

/**
 * A plain stand-in for the leave event: only the surface the guard writes.
 * jsdom implements no document lifecycle, so no real `beforeunload` event can
 * be dispatched here — the guard's decision is pinned against this stand-in,
 * and the listener's registration itself is the browser's standard API.
 */
function leaveStandIn(): {
    readonly event: {
        readonly preventDefault: () => void;
        returnValue: string;
    };
    readonly cancelCount: () => number;
} {
    let cancellations = 0;

    return {
        event: {
            preventDefault: (): void => {
                cancellations += 1;
            },
            returnValue: "",
        },
        cancelCount: (): number => cancellations,
    };
}

// Solid settles a signal write on a microtask, so a seat's report becomes
// observable to the guard only after a drain of the queue — exactly the gap
// the real event-time read always sits beyond.

// The registry is module state: every test gives the seats it took back, so
// no test leaves the guard armed for the next one.
const takenSeats: Array<DirtyFormRegistration> = [];

afterEach(() => {
    for (const seat of takenSeats) {
        seat.unregister();
    }
    takenSeats.length = 0;
});

function takeSeat(): DirtyFormRegistration {
    const seat = registerDirtyForm();
    takenSeats.push(seat);
    return seat;
}

describe("guardUnsavedFormsOnLeave", () => {
    it("cancels nothing while the registry is clean", () => {
        const { event, cancelCount } = leaveStandIn();

        guardUnsavedFormsOnLeave(event);

        expect(cancelCount()).toBe(0);
        expect(event.returnValue).toBe("");
    });

    it("marks the leave for the browser's confirmation while a form stands dirty", async () => {
        const { event, cancelCount } = leaveStandIn();
        takeSeat().reportDirty(true);
        await drainWrites();

        guardUnsavedFormsOnLeave(event);

        expect(cancelCount()).toBe(1);
        expect(event.returnValue).toBe("");
    });

    it("reads the registry live — the one handler answers the dirt of the moment", async () => {
        const { event, cancelCount } = leaveStandIn();
        const seat = takeSeat();

        seat.reportDirty(true);
        await drainWrites();
        guardUnsavedFormsOnLeave(event);
        expect(cancelCount()).toBe(1);

        // The dirt clears: the same listener, asked again, cancels nothing —
        // a captured snapshot would keep blocking.
        seat.reportDirty(false);
        await drainWrites();
        guardUnsavedFormsOnLeave(event);
        expect(cancelCount()).toBe(1);

        // And the dirt returns.
        seat.reportDirty(true);
        await drainWrites();
        guardUnsavedFormsOnLeave(event);
        expect(cancelCount()).toBe(2);
    });

    it("stays disarmed once the dirty form's seat is given up", async () => {
        const { event, cancelCount } = leaveStandIn();
        const seat = takeSeat();
        seat.reportDirty(true);

        // The screen went away without clearing its dirt first: the seat's
        // disposal must still leave the guard disarmed.
        seat.unregister();
        await drainWrites();
        guardUnsavedFormsOnLeave(event);

        expect(cancelCount()).toBe(0);
    });
});
