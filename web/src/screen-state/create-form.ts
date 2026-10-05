import type { Accessor } from "solid-js";
import type { ActionName } from "../management-api/diagnosis";
import { type ActionBegin, createBeginAction } from "./action-begin";
import { createDisposalGuard } from "./lifetime";
import type { ScreenNotice } from "./notices";

/**
 * The creation action the three new screens share: the single-flight
 * creation a form submit runs. The flight flag, the disposal-guard check,
 * and the settle the answered begin runs are the screens' shared begin
 * action's (`createBeginAction`): an aborted call states nothing; a failed
 * one is diagnosed onto the screen's failure channel under the action's
 * name; a blocked prelude states its own notice there; a succeeded one runs
 * the success tail alone.
 *
 * Disposal is guarded here, in one home: a call answering after the screen
 * went away states nothing and lands nowhere, the way the OAuth flow and
 * the delete action guard by (README §6.4) — the creation calls pass no
 * signal, so the late answer is the one thing that must not act.
 */
export function createCreateAction<TResult>(options: {
    /** The creation's action name, which several failure diagnoses specialize on. */
    readonly action: ActionName;
    /** The failure channel the flight clears and every blocked or failed creation states on. */
    readonly channel: (notice: ScreenNotice | null) => void;
    /** Whether a creation may run now: the form's inline rules beside the flight gate. */
    readonly canRun: Accessor<boolean>;
    /**
     * The creation a submit runs: the screen's own prelude — a refused one
     * answers `"blocked"` with its notice — followed by the gateway's
     * creation call, wrapped in `performManagementCall`. A screen without a
     * prelude answers the `"called"` arm outright.
     */
    readonly perform: () => Promise<ActionBegin<TResult>>;
    /** Where the screen goes once the gateway answered the new row. */
    readonly onCreated: (result: TResult) => void;
}): {
    /** Whether the creation is in flight — the form's fields' and buttons' busy state. */
    readonly isCreating: Accessor<boolean>;
    /** The form's submit handler: the default prevented, then the creation. */
    readonly submit: (event: SubmitEvent) => void;
} {
    // The one disposal guard, set when the consuming screen goes away: a
    // call answering afterwards must state nothing and land nowhere.
    const isDisposed = createDisposalGuard();

    const { isInFlight: isCreating, run } = createBeginAction({
        action: options.action,
        channel: options.channel,
        isDisposed,
        canRun: options.canRun,
        perform: options.perform,
        onSucceeded: options.onCreated,
    });

    return {
        isCreating,
        submit: (event: SubmitEvent): void => {
            event.preventDefault();
            void run();
        },
    };
}
