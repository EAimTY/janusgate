import { afterEach, describe, expect, it } from "vitest";
import { drainWrites } from "/src/screens/screen-test-support";
import {
    type DirtyFormRegistration,
    hasDirtyForms,
    registerDirtyForm,
} from "./dirty-form-registry";

// The registry is module state shared by every test in this process: each
// test gives the seats it took back, so a leftover dirty form cannot leave
// the gate armed for the next test.
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

describe("registerDirtyForm", () => {
    it("keeps the gate closed while every registered form is clean", async () => {
        takeSeat();
        takeSeat();
        await drainWrites();

        expect(hasDirtyForms()).toBe(false);
    });

    it("opens the gate on a form's dirt and closes it when the dirt clears", async () => {
        const seat = takeSeat();

        seat.reportDirty(true);
        await drainWrites();
        expect(hasDirtyForms()).toBe(true);

        seat.reportDirty(false);
        await drainWrites();
        expect(hasDirtyForms()).toBe(false);
    });

    it("stays open while any of several forms stands dirty", async () => {
        const first = takeSeat();
        const second = takeSeat();

        first.reportDirty(true);
        await drainWrites();
        expect(hasDirtyForms()).toBe(true);

        // One form's clearing leaves another's dirt standing.
        second.reportDirty(true);
        first.reportDirty(false);
        await drainWrites();
        expect(hasDirtyForms()).toBe(true);

        second.reportDirty(false);
        await drainWrites();
        expect(hasDirtyForms()).toBe(false);
    });

    it("cannot be wedged by a repeated report of the state already recorded", async () => {
        const seat = takeSeat();

        // A count of dirty reports would stick open after a single clear;
        // the seat's state is set, not counted.
        seat.reportDirty(true);
        seat.reportDirty(true);
        seat.reportDirty(false);
        await drainWrites();

        expect(hasDirtyForms()).toBe(false);
    });
});

describe("DirtyFormRegistration.unregister", () => {
    it("closes the gate a dirty form held, and a second call changes nothing", async () => {
        const seat = takeSeat();

        seat.reportDirty(true);
        await drainWrites();
        expect(hasDirtyForms()).toBe(true);

        seat.unregister();
        await drainWrites();
        expect(hasDirtyForms()).toBe(false);

        seat.unregister();
        await drainWrites();
        expect(hasDirtyForms()).toBe(false);
    });

    it("leaves the other forms' dirt standing", async () => {
        const gone = takeSeat();
        const kept = takeSeat();

        gone.reportDirty(true);
        kept.reportDirty(true);
        await drainWrites();
        expect(hasDirtyForms()).toBe(true);

        gone.unregister();
        await drainWrites();
        expect(hasDirtyForms()).toBe(true);

        kept.reportDirty(false);
        await drainWrites();
        expect(hasDirtyForms()).toBe(false);
    });
});
