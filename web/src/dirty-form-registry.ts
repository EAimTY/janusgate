import { createSignal } from "solid-js";

/**
 * One form's seat in the dirty-form registry: the handle its owner keeps for
 * as long as the form is mounted. The registry never derives dirt itself — a
 * form's own tracking (the edit screens' settings forms, README §6.3) stays
 * the one dirt source, mirrored here so the shell's leave guard can read it
 * without knowing any screen.
 */
export type DirtyFormRegistration = {
    /** Reports whether this form currently stands dirty. */
    readonly reportDirty: (isDirty: boolean) => void;
    /**
     * Gives the seat up: the form's mount went away, dirty or not.
     * Idempotent — an already-given-up seat changes nothing.
     */
    readonly unregister: () => void;
};

/** The forms currently standing dirty; the leave guard's live truth. */
const [dirtyForms, setDirtyForms] = createSignal<ReadonlySet<DirtyFormRegistration>>(
    new Set<DirtyFormRegistration>(),
);

/**
 * Registers a form with the registry, initially clean. The caller owns the
 * seat's lifetime: when the form's mount goes away, `unregister` runs — under
 * a Solid owner, `onCleanup` beside the registration. Reporting the state a
 * seat already records changes nothing, so no caller ordering can wedge the
 * gate open or closed.
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
 * Whether any registered form currently stands dirty — the shell's
 * `beforeunload` gate (README §6.3). The registry is read live, so a call
 * answers this moment's truth, never a snapshot; called inside a reactive
 * scope, it tracks the registry.
 */
export function hasDirtyForms(): boolean {
    return dirtyForms().size > 0;
}
