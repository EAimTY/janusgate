import { type Accessor, createEffect, createSignal, onCleanup } from "solid-js";
import type { ActionName } from "../management-api/diagnosis";
import { performManagementCall } from "../management-api/outcome";
import { createDisposalGuard } from "./lifetime";
import type { ScreenNotice } from "./notices";
import type { ScreenRead } from "./read";

/**
 * The save-and-dirt wiring of one settings form, shared by the five edit
 * screens (`web/docs/architecture.md`): a submit runs the caller's write through the save
 * gate, reports its outcome through the screen's notice — a failed or aborted
 * save stops there, leaving the success tail unrun — and a succeeded save
 * runs the caller's re-baseline, latches the row hold until the save's own
 * reload lands, and states the notice the re-baseline returned. The form's
 * dirt is reported to the row hold for as long as the form stands dirty. The
 * form's fields, its dirt, its save gate, its re-baseline onto the saved
 * state, and its notice are the caller's — this owns only the wiring every
 * settings form spells identically.
 */
export function createEditForm<TResult>(options: {
    /** The screen read whose standing notice states the save's outcome. */
    readonly read: ScreenRead;
    /** The save's action name, which several failure diagnoses specialize on. */
    readonly action: ActionName;
    /** Whether the form's current values differ from its baseline. */
    readonly isDirty: Accessor<boolean>;
    /** Whether a save may run now: dirt, plus every inline rule holding. */
    readonly canSave: Accessor<boolean>;
    /** The write a submit runs, wrapped in `performManagementCall`. */
    readonly perform: () => Promise<TResult>;
    /**
     * Re-baselines the form onto the saved state the write's answer carries
     * and returns the notice the save states. The answer is the form's new
     * truth where the gateway answers `{}` — the caller restates what it
     * sent — or the returned row where the write recomposes one.
     */
    readonly onSucceeded: (result: TResult) => ScreenNotice;
    /** Reports the form's dirt to the row hold (`web/docs/architecture.md`). */
    readonly onDirtyChange: (isDirty: boolean) => void;
    /** Reports a succeeded save: the hold stands until the save's reload lands. */
    readonly onSaved: () => void;
}): {
    /** Whether the save is in flight — the form's fields' and buttons' busy state. */
    readonly isSaving: Accessor<boolean>;
    /** The form's submit handler: the default prevented, then the save. */
    readonly submit: (event: SubmitEvent) => void;
} {
    const [isSaving, setIsSaving] = createSignal(false);

    // The screen holds the form's row while this dirt stands (`web/docs/architecture.md`).
    createEffect(
        () => options.isDirty(),
        (isDirtyValue) => {
            options.onDirtyChange(isDirtyValue);
        },
    );

    // A mount that dies dirty — the not-found swap unmounts the card without
    // a dirty=false report — gives the leave guard one final clean report, so
    // the screen it leaves behind is never armed.
    onCleanup(() => options.onDirtyChange(false));

    // The one disposal guard, set when the consuming screen goes away: a
    // call answering afterwards must state nothing, re-baseline nothing,
    // and reload nothing.
    const isDisposed = createDisposalGuard();

    const save = async (): Promise<void> => {
        if (isSaving() || !options.canSave()) {
            return;
        }

        setIsSaving(true);

        const outcome = await performManagementCall(options.perform);

        setIsSaving(false);

        if (isDisposed()) {
            return;
        }

        if (
            !options.read.reportAction(
                outcome,
                {
                    action: options.action,
                },
                {
                    onGone: options.read.reload,
                },
            )
        ) {
            return;
        }

        // The re-baseline runs first, so the notice reads the post-save form.
        const notice = options.onSucceeded(outcome.result);
        // The hold stands until this save's reload lands: the listing still
        // answers what the save overwrote.
        options.onSaved();
        options.read.showNotice(notice);
        options.read.reload();
    };

    return {
        isSaving,
        submit: (event: SubmitEvent): void => {
            event.preventDefault();
            void save();
        },
    };
}

/**
 * One danger card's delete action: the confirmed call, its outcome reported
 * through the screen's notice — a gone answer first reloading the listing, so
 * the not-found state takes over instead of a ghost form (`web/docs/architecture.md`) — and the
 * boolean the confirmation card reads, answering whether the row is gone. The
 * call, its action name, and where a confirmed deletion goes are the
 * caller's.
 */
export function createDeleteAction(options: {
    /** The screen read whose standing notice states the delete's outcome. */
    readonly read: ScreenRead;
    /** The delete's action name, which several failure diagnoses specialize on. */
    readonly action: ActionName;
    /** The delete call a confirmation runs, wrapped in `performManagementCall`. */
    readonly perform: () => Promise<unknown>;
    /** Where the screen goes once the delete is confirmed and succeeded. */
    readonly onDeleted: () => void;
}): () => Promise<boolean> {
    // The one disposal guard, set when the consuming screen goes away: a
    // call answering afterwards must state nothing, reload nothing, and
    // navigate nowhere (`web/docs/architecture.md`).
    const isDisposed = createDisposalGuard();

    return async (): Promise<boolean> => {
        const outcome = await performManagementCall(options.perform);

        if (isDisposed()) {
            return false;
        }

        if (
            !options.read.reportAction(
                outcome,
                {
                    action: options.action,
                },
                {
                    onGone: options.read.reload,
                },
            )
        ) {
            return false;
        }

        options.onDeleted();

        return true;
    };
}
