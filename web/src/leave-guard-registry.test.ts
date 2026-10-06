import { afterEach, describe, expect, it } from "vitest";
import {
    hasGuardedSubjects,
    type LeaveGuardSeat,
    registerLeaveGuardSubject,
} from "./leave-guard-registry";
import { drainWrites } from "./test-support/management-api";

// The registry is module state shared by every test in this process: each
// test gives the seats it took back, so a leftover guarded subject cannot
// leave the gate armed for the next test.
const takenSeats: Array<LeaveGuardSeat> = [];

afterEach(() => {
    for (const seat of takenSeats) {
        seat.release();
    }
    takenSeats.length = 0;
});

function takeSeat(): LeaveGuardSeat {
    const seat = registerLeaveGuardSubject();
    takenSeats.push(seat);
    return seat;
}

describe("registerLeaveGuardSubject", () => {
    it("keeps the gate closed while every registered subject is unguarded", async () => {
        takeSeat();
        takeSeat();
        await drainWrites();

        expect(hasGuardedSubjects()).toBe(false);
    });

    it("opens the gate on a subject's guarded state and closes it when it stands down", async () => {
        const seat = takeSeat();

        seat.reportGuarded(true);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(true);

        seat.reportGuarded(false);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("stays open while any of several subjects stands guarded", async () => {
        const first = takeSeat();
        const second = takeSeat();

        first.reportGuarded(true);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(true);

        // One subject's standing down leaves another's guarding standing.
        second.reportGuarded(true);
        first.reportGuarded(false);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(true);

        second.reportGuarded(false);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("cannot be wedged by a repeated report of the state already recorded", async () => {
        const seat = takeSeat();

        // A count of guarded reports would stick open after a single stand-down;
        // the seat's state is set, not counted.
        seat.reportGuarded(true);
        seat.reportGuarded(true);
        seat.reportGuarded(false);
        await drainWrites();

        expect(hasGuardedSubjects()).toBe(false);
    });
});

describe("LeaveGuardSeat.release", () => {
    it("closes the gate a guarded subject held, and a second call changes nothing", async () => {
        const seat = takeSeat();

        seat.reportGuarded(true);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(true);

        seat.release();
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(false);

        seat.release();
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("leaves the other subjects' guarding standing", async () => {
        const gone = takeSeat();
        const kept = takeSeat();

        gone.reportGuarded(true);
        kept.reportGuarded(true);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(true);

        gone.release();
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(true);

        kept.reportGuarded(false);
        await drainWrites();
        expect(hasGuardedSubjects()).toBe(false);
    });
});
