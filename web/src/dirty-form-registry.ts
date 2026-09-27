import { createSignal } from "solid-js";

/**
 * One guarded subject's seat in the leave-guard registry: the handle its
 * owner keeps for as long as the subject lives — a settings form's mount, or
 * a live OAuth flow. The registry never derives dirt itself — a form's own
 * tracking (the edit screens' settings forms, README §6.3) stays the one
 * dirt source, and the OAuth flow machine arms its seat for exactly as long
 * as its flow stands live — mirrored here so the shell's leave guard can
 * read both without knowing any screen.
 */
export type DirtyFormRegistration = {
    /** Reports whether this seat's subject currently stands guarded. */
    readonly reportDirty: (isDirty: boolean) => void;
    /**
     * Gives the seat up: the subject's mount went away, guarded or not.
     * Idempotent — an already-given-up seat changes nothing.
     */
    readonly unregister: () => void;
};

/**
 * The guarded subjects standing at risk right now — the forms dirty, the
 * OAuth flows live; the leave guard's live truth.
 */
const [dirtyForms, setDirtyForms] = createSignal<ReadonlySet<DirtyFormRegistration>>(
    new Set<DirtyFormRegistration>(),
);

/**
 * Registers a guarded subject with the registry, initially standing clean.
 * The caller owns the seat's lifetime: when the mount goes away,
 * `unregister` runs — under a Solid owner, `onCleanup` beside the
 * registration. Reporting the state a seat already records changes nothing,
 * so no caller ordering can wedge the gate open or closed.
 */
export function registerDirtyForm(): DirtyFormRegistration {
    const registration: DirtyFormRegistration = {
        reportDirty: (isDirty: boolean): void => {
            setDirtyForms((current) => {
                if (current.has(registration) === isDirty) {
                    return current;
                }

                const next = new Set(current);
                if (isDirty) {
                    next.add(registration);
                } else {
                    next.delete(registration);
                }

                return next;
            });
        },
        unregister: (): void => {
            setDirtyForms((current) => {
                if (!current.has(registration)) {
                    return current;
                }

                const next = new Set(current);
                next.delete(registration);

                return next;
            });
        },
    };

    return registration;
}

/**
 * Whether any registered subject currently stands guarded — a form dirty or
 * an OAuth flow live — the shell's `beforeunload` gate (README §6.3). The
 * registry is read live, so a call answers this moment's truth, never a
 * snapshot; called inside a reactive scope, it tracks the registry.
 */
export function hasDirtyForms(): boolean {
    return dirtyForms().size > 0;
}
