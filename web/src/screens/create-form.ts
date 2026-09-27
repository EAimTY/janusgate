import { type Accessor, createSignal } from "solid-js";
import type { ActionName } from "/src/management-api/diagnosis";
import type { ManagementCallOutcome } from "/src/management-api/outcome";
import { createDisposalGuard, type ScreenNotice, screenFailureNotice } from "./read";

/**
 * What a creation's perform step produced: the screen's own prelude refused
 * and the channel states why, or the gateway's creation call ran and its
 * outcome is the action's to report — the same begin shape the OAuth flow's
 * begin step answers.
 */
export type CreateActionBegin<TResult> =
    | {
          readonly status: "blocked";
          readonly notice: ScreenNotice;
      }
    | {
          readonly status: "called";
          readonly outcome: ManagementCallOutcome<TResult>;
      };

/**
 * The creation action the three new screens share: the single-flight
 * creation a form submit runs. It owns the flight signal every screen
 * redeclared and the wiring around the call — an aborted call states
 * nothing; a failed one is diagnosed onto the screen's failure channel
 * under the action's name; a blocked prelude states its own notice there;
 * a succeeded one runs the success tail alone.
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
    readonly perform: () => Promise<CreateActionBegin<TResult>>;
    /** Where the screen goes once the gateway answered the new row. */
    readonly onCreated: (result: TResult) => void;
}): {
    /** Whether the creation is in flight — the form's fields' and buttons' busy state. */
    readonly isCreating: Accessor<boolean>;
    /** The form's submit handler: the default prevented, then the creation. */
    readonly submit: (event: SubmitEvent) => void;
} {
    const [isCreating, setIsCreating] = createSignal(false);

    // The one disposal guard, set when the consuming screen goes away: a
    // call answering afterwards must state nothing and land nowhere.
    const isDisposed = createDisposalGuard();

    const run = async (): Promise<void> => {
        if (isCreating() || !options.canRun()) {
            return;
        }

        options.channel(null);
        setIsCreating(true);

        const begun = await options.perform();

        setIsCreating(false);

        if (isDisposed()) {
            return;
        }

        if (begun.status === "blocked") {
            options.channel(begun.notice);
            return;
        }

        if (begun.outcome.status === "aborted") {
            return;
        }

        if (begun.outcome.status === "failed") {
            options.channel(
                screenFailureNotice(begun.outcome.cause, {
                    action: options.action,
                }),
            );
            return;
        }

        options.onCreated(begun.outcome.result);
    };

    return {
        isCreating,
        submit: (event: SubmitEvent): void => {
            event.preventDefault();
            void run();
        },
    };
}
