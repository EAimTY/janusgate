import { describe, expect, test } from "vitest";
import { hasGuardedSubjects, registerLeaveGuardSubject } from "./leave-guard-registry";

/**
 * The leave-guard registry's seat semantics: registration standing
 * unguarded, reports recording only changes, release being idempotent, and
 * the live truth answering this moment's registry — the shell's
 * beforeunload gate reads exactly this. Solid 2 schedules signal writes, so
 * every write below settles through a tick before the registry is read.
 */

/** Solid 2 schedules signal writes; every write below settles through a tick. */
async function tick(): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("registerLeaveGuardSubject", () => {
    test("registers unguarded and reports its way into the guarded truth", async () => {
        const seat = registerLeaveGuardSubject();

        expect(hasGuardedSubjects()).toBe(false);

        seat.reportGuarded(true);
        await tick();
        expect(hasGuardedSubjects()).toBe(true);

        seat.release();
        await tick();
        expect(hasGuardedSubjects()).toBe(false);
    });

    test("reporting the state a seat already records changes nothing", async () => {
        const seat = registerLeaveGuardSubject();

        seat.reportGuarded(false);
        await tick();
        expect(hasGuardedSubjects()).toBe(false);

        seat.reportGuarded(true);
        seat.reportGuarded(true);
        await tick();
        expect(hasGuardedSubjects()).toBe(true);

        seat.release();
    });

    test("release is idempotent, and repeated release changes nothing", async () => {
        const seat = registerLeaveGuardSubject();
        seat.reportGuarded(true);
        await tick();
        expect(hasGuardedSubjects()).toBe(true);

        seat.release();
        seat.release();
        await tick();
        expect(hasGuardedSubjects()).toBe(false);
    });

    test("one guarded seat among many keeps the gate open", async () => {
        const first = registerLeaveGuardSubject();
        const second = registerLeaveGuardSubject();

        first.reportGuarded(true);
        second.reportGuarded(true);
        await tick();
        expect(hasGuardedSubjects()).toBe(true);

        // One form reverted: the other still holds edits.
        first.reportGuarded(false);
        await tick();
        expect(hasGuardedSubjects()).toBe(true);

        second.reportGuarded(false);
        await tick();
        expect(hasGuardedSubjects()).toBe(false);

        first.release();
        second.release();
    });
});
