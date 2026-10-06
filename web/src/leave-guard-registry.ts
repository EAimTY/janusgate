import { createSignal } from "solid-js";

/**
 * One guarded subject's seat in the leave-guard registry: the handle its
 * owner keeps for as long as the subject lives — a settings form's mount, or
 * a live OAuth flow. The registry never derives guardedness itself — a
 * form's own dirt tracking (the edit screens' settings forms, README §6.3)
 * stays the one dirt source, and the OAuth flow machine guards its seat for
 * exactly as long as its flow stands live — mirrored here so the shell's
 * leave guard can read both without knowing any screen.
 */
export type LeaveGuardSeat = {
    /** Reports whether this seat's subject currently stands guarded. */
    readonly reportGuarded: (isGuarded: boolean) => void;
    /**
     * Gives the seat up: the subject's mount went away, guarded or not.
     * Idempotent — an already-released seat changes nothing.
     */
    readonly release: () => void;
};

/**
 * The guarded subjects standing at risk right now — the forms dirty, the
 * OAuth flows live; the leave guard's live truth.
 */
const [guardedSubjects, setGuardedSubjects] = createSignal<ReadonlySet<LeaveGuardSeat>>(
    new Set<LeaveGuardSeat>(),
);

/**
 * Registers a guarded subject with the registry, initially standing unguarded.
 * The caller owns the seat's lifetime: when the mount goes away, `release`
 * runs — under a Solid owner, `onCleanup` beside the registration. Reporting
 * the state a seat already records changes nothing, so no caller ordering can
 * wedge the gate open or closed.
 */
export function registerLeaveGuardSubject(): LeaveGuardSeat {
    const seat: LeaveGuardSeat = {
        reportGuarded: (isGuarded: boolean): void => {
            setGuardedSubjects((current) => {
                if (current.has(seat) === isGuarded) {
                    return current;
                }

                const next = new Set(current);
                if (isGuarded) {
                    next.add(seat);
                } else {
                    next.delete(seat);
                }

                return next;
            });
        },
        release: (): void => {
            setGuardedSubjects((current) => {
                if (!current.has(seat)) {
                    return current;
                }

                const next = new Set(current);
                next.delete(seat);

                return next;
            });
        },
    };

    return seat;
}

/**
 * Whether any registered subject currently stands guarded — a form dirty or
 * an OAuth flow live — the shell's `beforeunload` gate (README §6.3). The
 * registry is read live, so a call answers this moment's truth, never a
 * snapshot; called inside a reactive scope, it tracks the registry.
 */
export function hasGuardedSubjects(): boolean {
    return guardedSubjects().size > 0;
}
