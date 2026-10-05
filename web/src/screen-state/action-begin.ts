/**
 * The begin action the screens' two begin steps share — the creation form's
 * submit and the OAuth flow's begin: one begin union and the single-flight
 * wrapper owning the flight flag, the disposal-guard check, and the settle
 * every answered begin runs identically. The callers keep their own gates
 * and success tails; an aborted call states nothing and a begin answering
 * after the screen went away states nothing and lands nowhere (README §6.4).
 */
import { type Accessor, createSignal } from "solid-js";
import type { ActionName } from "../management-api/diagnosis";
import type { ManagementCallOutcome } from "../management-api/outcome";
import { type ScreenNotice, screenFailureNotice } from "./notices";

/**
 * What a begin step produced: the screen's own prelude refused and the
 * channel states why, or the gateway's call ran and its outcome is the
 * action's to report.
 */
export type ActionBegin<TResult> =
    | {
          readonly status: "blocked";
          readonly notice: ScreenNotice;
      }
    | {
          readonly status: "called";
          readonly outcome: ManagementCallOutcome<TResult>;
      };

/**
 * The begin action a form submit and a flow begin run inside: one flight
 * signal, one settle. A begin runs only through the flight gate and the
 * caller's own gate; the flight clears the channel before taking off; the
 * answered begin settles after the flight lifts and only when the screen
 * still stands — a blocked prelude states its notice on the channel, an
 * aborted call states nothing, a failed one is diagnosed onto the channel
 * under the action's name, and a succeeded one runs the success tail alone.
 *
 * The disposal guard is the caller's — created in the caller's reactive
 * owner like every lifecycle registration, and often shared with the
 * caller's other late-answering calls — while the check lives here, so a
 * late answer lands nowhere.
 */
export function createBeginAction<TResult>(options: {
    /** The begin's action name, which several failure diagnoses specialize on. */
    readonly action: ActionName;
    /** The failure channel the flight clears and every blocked or failed begin states on. */
    readonly channel: (notice: ScreenNotice | null) => void;
    /** Whether the owning screen has gone away: a begin answering afterwards stops. */
    readonly isDisposed: Accessor<boolean>;
    /** Whether a begin may run now: the caller's own gate beside the flight gate. */
    readonly canRun: Accessor<boolean>;
    /**
     * The begin a run performs: the screen's own prelude — a refused one
     * answers `"blocked"` with its notice — followed by the gateway's call,
     * wrapped in `performManagementCall`. A caller without a prelude answers
     * the `"called"` arm outright.
     */
    readonly perform: () => Promise<ActionBegin<TResult>>;
    /** What a succeeded begin does with the gateway's answer. */
    readonly onSucceeded: (result: TResult) => void;
}): {
    /** Whether the begin is in flight — the caller's busy state. */
    readonly isInFlight: Accessor<boolean>;
    /** Runs one begin now: gated, single-flight, and settled. */
    readonly run: () => Promise<void>;
} {
    const [isInFlight, setIsInFlight] = createSignal(false);

    const run = async (): Promise<void> => {
        if (isInFlight() || !options.canRun()) {
            return;
        }

        options.channel(null);
        setIsInFlight(true);

        const begun = await options.perform();

        setIsInFlight(false);

        if (options.isDisposed()) {
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

        options.onSucceeded(begun.outcome.result);
    };

    return {
        isInFlight,
        run,
    };
}
